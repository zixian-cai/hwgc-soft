use std::collections::HashMap;

use crate::{heapdump::Space, object_model::Header, *};

#[allow(clippy::enum_variant_names)]
enum Work {
    ProcessEdges { start: *mut u64, count: u64 },
    ProcessEdge(*mut u64),
    ProcessNode(u64),
}

pub(super) struct TaggedWork {
    creator: Worker,
    worker: Worker,
    work: Work,
}

#[derive(PartialEq, Eq)]
enum Worker {
    Numbered(usize),
    Environment,
}

// Create work
impl super::Analysis {
    fn create_work(&mut self, work: TaggedWork) {
        if let Worker::Numbered(x) = work.worker {
            *self.stats.work_dist.entry(x).or_default() += 1;
        }
        self.stats.total_work += 1;
        if let Worker::Numbered(x) = work.creator {
            if let Worker::Numbered(y) = work.worker {
                if x != y {
                    self.stats.total_msgs += 1;
                    match work.work {
                        Work::ProcessEdges { .. } => self.stats.msg_process_edges += 1,
                        Work::ProcessEdge(_) => self.stats.msg_process_edge += 1,
                        Work::ProcessNode(_) => self.stats.msg_process_node += 1,
                    }
                }
            }
        }
        self.work_queue.push_back(work);
    }

    pub(super) fn create_root_work(&mut self, root: u64) {
        let tagged_work = TaggedWork {
            creator: Worker::Environment,
            worker: Worker::Numbered(self.get_owner_thread(root)),
            work: Work::ProcessNode(root),
        };
        self.create_work(tagged_work);
    }

    fn create_process_edge_work(&mut self, creator: usize, worker: usize, e: *mut u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ProcessEdge(e),
        };
        self.create_work(work);
    }

    fn create_process_edges_work(
        &mut self,
        creator: usize,
        worker: usize,
        start: *mut u64,
        count: u64,
    ) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ProcessEdges { start, count },
        };
        self.create_work(work);
    }

    fn create_process_node_work(&mut self, creator: usize, worker: usize, o: u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ProcessNode(o),
        };
        self.create_work(work);
    }
}

// Stride helper methods
impl super::Analysis {
    fn get_stride_start(&self, p: *mut u64) -> *mut u64 {
        (((p as usize) >> self.owner_shift) << self.owner_shift) as *mut u64
    }

    fn get_stride_end(&self, p: *mut u64) -> *mut u64 {
        self.get_stride_start(p)
            .wrapping_add(self.get_pointers_in_stride())
    }

    fn get_pointers_in_stride(&self) -> usize {
        1usize << (self.owner_shift - self.log_pointer_size)
    }
}

// Do work
impl super::Analysis {
    pub(super) fn do_work<O: ObjectModel>(
        &mut self,
        work: TaggedWork,
        object_sizes: &HashMap<u64, u64>,
    ) {
        let inner_work = work.work;
        match inner_work {
            Work::ProcessEdges { start, count } => {
                if let Worker::Numbered(w) = work.worker {
                    if let Worker::Numbered(c) = work.creator {
                        self.do_process_edges(c, w, start, count);
                    }
                }
            }
            Work::ProcessEdge(e) => {
                if let Worker::Numbered(w) = work.worker {
                    if let Worker::Numbered(c) = work.creator {
                        self.do_process_edge(c, w, e);
                    }
                }
            }
            Work::ProcessNode(o) => {
                if self.group_slots {
                    self.do_process_node_grouped::<O>(o, object_sizes)
                } else {
                    self.do_process_node::<O>(o, object_sizes)
                }
            }
        }
    }

    fn do_los_object_stats(&mut self, o: u64, object_size: u64) {
        if let Space::Los = HeapDump::get_space_type(o) {
            self.stats.los_object_size += object_size;
            self.stats.los_objects += 1;
        }
    }

    fn do_objarray_stats<O: ObjectModel>(&mut self, o: u64) {
        let is_objarray = unsafe { O::is_objarray(o) };
        if is_objarray {
            O::scan_object(o, |e, repeat| {
                for i in 0..repeat {
                    let edge = e.wrapping_add(i as usize);
                    self.stats.objarray_slots += 1;
                    let child = unsafe { *edge };
                    if child == 0 {
                        self.stats.objarray_empty_slots += 1;
                    }
                }
            });
        }
    }

    fn do_process_node<O: ObjectModel>(&mut self, o: u64, object_sizes: &HashMap<u64, u64>) {
        debug_assert_ne!(o, 0);
        let mut header = Header::load(o);
        let mark_byte = header.get_mark_byte();
        if mark_byte == 1 {
            return;
        }
        self.stats.marked_objects += 1;
        let object_size = object_sizes.get(&o).unwrap();
        self.stats.total_object_size += object_size;
        self.do_los_object_stats(o, *object_size);
        // mark the object
        header.set_mark_byte(1);
        header.store(o);
        // now we need to scan it
        let object_owner = self.get_owner_thread(o);
        O::scan_object(o, |e, repeat| {
            for i in 0..repeat {
                let edge = e.wrapping_add(i as usize);
                let edge_owner = self.get_owner_thread(edge as u64);
                if edge_owner == object_owner {
                    let child = unsafe { *edge };
                    self.do_visible_slot(object_owner, child);
                } else {
                    self.create_process_edge_work(object_owner, edge_owner, edge);
                }
            }
        });
        self.do_objarray_stats::<O>(o);
    }

    fn do_visible_slot(&mut self, worker: usize, child: u64) {
        self.stats.slots += 1;
        if child == 0 {
            self.stats.visible_empty_slots += 1;
        } else {
            let child_owner = self.get_owner_thread(child);
            self.create_process_node_work(worker, child_owner, child);
            if child_owner == worker {
                self.stats.visible_non_empty_slots_visible_child += 1;
            } else {
                self.stats.visible_non_empty_slots_invisible_child += 1;
            }
        }
    }

    fn do_process_node_grouped<O: ObjectModel>(
        &mut self,
        o: u64,
        object_sizes: &HashMap<u64, u64>,
    ) {
        debug_assert_ne!(o, 0);
        let mut header = Header::load(o);
        let mark_byte = header.get_mark_byte();
        if mark_byte == 1 {
            return;
        }
        self.stats.marked_objects += 1;
        let object_size = object_sizes.get(&o).unwrap();
        self.stats.total_object_size += object_size;
        self.do_los_object_stats(o, *object_size);
        // mark the object
        header.set_mark_byte(1);
        header.store(o);
        // now we need to scan it
        let object_owner = self.get_owner_thread(o);
        // For each group of edges, we broadcast to all threads
        O::scan_object(o, |edge, repeat| {
            if repeat == 1 {
                // A lightweight process
                let edge_owner = self.get_owner_thread(edge as u64);
                if edge_owner == object_owner {
                    let child = unsafe { *edge };
                    self.do_visible_slot(object_owner, child);
                } else {
                    self.create_process_edge_work(object_owner, edge_owner, edge);
                }
                return;
            }
            // A more heavyweight process
            let edge_owner = self.get_owner_thread(edge as u64);
            let stride_end = self.get_stride_end(edge);
            // We need to send something to the edge owner regardless
            self.create_process_edges_work(object_owner, edge_owner, edge, repeat);
            let ptrs_fit_in_1st_stride =
                (stride_end as usize - edge as usize) >> self.log_pointer_size;
            // if repeat > 16 {
            //     dbg!(edge_owner);
            //     dbg!(repeat);
            //     dbg!(ptrs_fit_in_1st_stride);
            // }
            let ptr_in_stide = self.get_pointers_in_stride() as u64;
            if repeat > ptrs_fit_in_1st_stride as u64 {
                // We need to send out more messages
                let leftover = repeat - ptrs_fit_in_1st_stride as u64;
                // divide and round up
                let leftover_strides = (leftover + (ptr_in_stide - 1)) / ptr_in_stide;
                // dbg!(leftover_strides);
                debug_assert!(leftover_strides >= 1);
                for i in edge_owner + 1
                    ..std::cmp::min(
                        edge_owner + self.num_threads,
                        edge_owner + leftover_strides as usize + 1,
                    )
                {
                    // if repeat > 16 {
                    //     dbg!(i % self.num_threads);
                    // }
                    self.create_process_edges_work(
                        object_owner,
                        i % self.num_threads,
                        edge,
                        repeat,
                    );
                }
            }
        });
        self.do_objarray_stats::<O>(o);
    }

    fn do_process_edge(&mut self, creator: usize, worker: usize, e: *mut u64) {
        let is_visible_slot = creator == worker;
        // if the slot is visible, it should already be done during object scanning
        debug_assert!(!is_visible_slot);
        let edge_owner = self.get_owner_thread(e as u64);
        debug_assert_eq!(edge_owner, worker);
        let child = unsafe { *e };
        self.stats.slots += 1;
        if child != 0 {
            let child_owner = self.get_owner_thread(child);
            let is_child_visile = child_owner == edge_owner;
            self.create_process_node_work(edge_owner, child_owner, child);
            if is_child_visile {
                self.stats.invisible_non_empty_slots_visible_child += 1;
            } else {
                self.stats.invisible_non_empty_slots_invisible_child += 1;
            }
        } else {
            self.stats.invisible_empty_slots += 1;
        }
    }

    fn do_process_edges(&mut self, creator: usize, worker: usize, start: *mut u64, count: u64) {
        let are_visible_slots = creator == worker;
        // trace!("PE worker {} start 0x{:x} count {}", worker, start as u64, count);
        let end = start.wrapping_add(count as usize);
        // Suppose owner shift is 3, i.e., each thread can only see individual words
        // Suppose we have 2 threads, and we are thread 0
        // Suppose we start with 01000 and end with 11000 (count = 3)
        // We clear lower bits, so we have 0
        let stride_start = (start as usize) >> (self.owner_shift + self.log_num_threads);
        // We set the thread id, so 00;
        let stride_start = (stride_start << self.log_num_threads) | worker;
        // Then we get the start of the first stride, so 00000
        let mut stride_start = (stride_start << self.owner_shift) as *mut u64;
        let pointers_in_stride = self.get_pointers_in_stride();
        let mut stride_end = stride_start.wrapping_add(pointers_in_stride);
        loop {
            // trace!("Stride worker {} start 0x{:x}", worker, stride_start as u64);
            if stride_start >= end {
                break;
            }
            // Stride start should be >= start, except when start is owned by start 0
            // then we pick the max of them
            let mut edge = std::cmp::max(start, stride_start);
            while edge < stride_end {
                // trace!("Edge worker {} 0x{:x}", worker, edge as u64);
                debug_assert_eq!(self.get_owner_thread(edge as u64), worker);
                if edge >= end {
                    break;
                }
                debug_assert!(edge >= start && edge < end);
                self.stats.slots += 1;
                let child = unsafe { *edge };
                if child != 0 {
                    let child_owner = self.get_owner_thread(child);
                    let is_child_visile = child_owner == worker;
                    self.create_process_node_work(worker, child_owner, child);
                    if are_visible_slots {
                        if is_child_visile {
                            self.stats.visible_non_empty_slots_visible_child += 1;
                        } else {
                            self.stats.visible_non_empty_slots_invisible_child += 1;
                        }
                    } else if is_child_visile {
                        self.stats.invisible_non_empty_slots_visible_child += 1;
                    } else {
                        self.stats.invisible_non_empty_slots_invisible_child += 1;
                    }
                } else if are_visible_slots {
                    self.stats.visible_empty_slots += 1;
                } else {
                    self.stats.invisible_empty_slots += 1;
                }
                edge = edge.wrapping_add(1);
            }
            // Go to the next stride of the same thread
            stride_start = (stride_start as usize + self.next_stride_delta) as *mut u64;
            stride_end = stride_start.wrapping_add(pointers_in_stride);
        }
    }
}
