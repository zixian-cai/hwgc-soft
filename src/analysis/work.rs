use std::collections::HashMap;

use object_model::BidirectionalTib;

use crate::BidirectionalObjectModel;
use crate::{heapdump::Space, object_model::Header, *};

#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub(super) enum Work {
    MarkObject(u64),
    LoadTIB(u64),
    ScanObject {
        tib_ptr: *mut BidirectionalTib,
        o: u64,
    },
    ScanRefarray(u64),
    Edges {
        start: *mut u64,
        count: u64,
    },
}

#[derive(Debug)]
pub(super) struct TaggedWork {
    creator: Worker,
    worker: Worker,
    work: Work,
}

#[derive(PartialEq, Eq, Debug)]
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
            let Worker::Numbered(y) = work.worker else {
                unreachable!()
            };
            if x != y {
                self.stats
                    .external_messages
                    .entry((y, std::mem::discriminant(&work.work)))
                    .and_modify(|e| *e += 1)
                    .or_insert(1);
            } else {
                self.stats
                    .internal_messages
                    .entry((y, std::mem::discriminant(&work.work)))
                    .and_modify(|e| *e += 1)
                    .or_insert(1);
            }
        } else {
            let Worker::Numbered(y) = work.worker else {
                unreachable!()
            };
            self.stats
                .external_messages
                .entry((y, std::mem::discriminant(&work.work)))
                .and_modify(|e| *e += 1)
                .or_insert(1);
        }
        self.work_queue.push_back(work);
    }

    pub(super) fn create_root_edges_work(&mut self, worker: usize, start: *mut u64, count: u64) {
        let work = TaggedWork {
            creator: Worker::Environment,
            worker: Worker::Numbered(worker),
            work: Work::Edges { start, count },
        };
        self.create_work(work);
    }

    fn create_edges_work(&mut self, creator: usize, worker: usize, start: *mut u64, count: u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::Edges { start, count },
        };
        self.create_work(work);
    }

    fn create_load_tib_work(&mut self, creator: usize, worker: usize, o: u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::LoadTIB(o),
        };
        self.create_work(work);
    }

    fn create_mark_object_work(&mut self, creator: usize, worker: usize, o: u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::MarkObject(o),
        };
        self.create_work(work);
    }

    fn create_scan_object_work(
        &mut self,
        creator: usize,
        worker: usize,
        tib_ptr: *mut BidirectionalTib,
        o: u64,
    ) {
        // Only used when #refs is not encoded in the header
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ScanObject { tib_ptr, o },
        };
        self.create_work(work);
    }

    fn create_scan_refarray_work(&mut self, creator: usize, worker: usize, o: u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ScanRefarray(o),
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
    pub(super) fn do_work(&mut self, work: TaggedWork, object_sizes: &HashMap<u64, u64>) {
        // use usize::MAX to represent the environment so that the worker
        // knows that the work comes from an external message
        let creator = match work.creator {
            Worker::Numbered(x) => x,
            Worker::Environment => usize::MAX,
        };
        let Worker::Numbered(worker) = work.worker else {
            unreachable!()
        };
        let inner_work = work.work;
        match inner_work {
            Work::MarkObject(o) => {
                self.do_mark_object(o, object_sizes);
            }
            Work::LoadTIB(o) => self.do_load_tib(o),
            Work::ScanObject { tib_ptr, o } => self.do_scan_object(tib_ptr, o),
            Work::ScanRefarray(o) => self.do_scan_refarray(o),
            Work::Edges { start, count } => self.do_edges(creator, worker, start, count),
        }
    }

    fn do_los_object_stats(&mut self, o: u64, object_size: u64) {
        if let Space::Los = HeapDump::get_space_type(o) {
            self.stats.los_object_size += object_size;
            self.stats.los_objects += 1;
            let is_objarray = unsafe { BidirectionalObjectModel::<true>::is_objarray(o) };
            if is_objarray {
                self.stats.los_objarrays += 1;
                self.stats.los_objarray_size += object_size
            }
        }
    }

    fn do_objarray_slot_stats(&mut self, o: u64) {
        let is_objarray = unsafe { BidirectionalObjectModel::<true>::is_objarray(o) };
        if is_objarray {
            BidirectionalObjectModel::<true>::scan_object(o, |e, repeat| {
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

    fn do_mark_object(&mut self, o: u64, object_sizes: &HashMap<u64, u64>) {
        debug_assert_ne!(o, 0);
        let mut header = Header::load(o);
        let mark_byte = header.get_mark_byte();
        if mark_byte == 1 {
            return;
        }
        // Always safe to read, but might be meaningless
        let status_byte = header.get_byte(BidirectionalTib::STATUS_BYTE_OFFSET);
        let num_refs = header.get_byte(BidirectionalTib::NUMREFS_BYTE_OFFSET);
        self.stats.marked_objects += 1;
        let object_size = object_sizes.get(&o).unwrap();
        self.stats.total_object_size += object_size;
        // mark the object
        header.set_mark_byte(1);
        header.store(o);
        let object_owner = self.get_owner_thread(o);
        match status_byte {
            0 => {}
            1 => {
                self.send_edges(
                    object_owner,
                    (o as *mut u64).wrapping_add(2),
                    num_refs as u64,
                );
            }
            2 => {
                let array_length_owner =
                    self.get_owner_thread((o as *mut u64).wrapping_add(2) as u64);
                self.create_scan_refarray_work(object_owner, array_length_owner, o);
            }
            u8::MAX => {
                let tib_owner = self.get_owner_thread((o as *mut u64).wrapping_add(1) as u64);
                self.create_load_tib_work(object_owner, tib_owner, o);
            }
            _ => {
                unreachable!()
            }
        };
        // We might not be able to access the entire object, but we can cheat
        // for the purpose of collecting stats
        self.do_los_object_stats(o, *object_size);
        self.do_objarray_slot_stats(o);
    }

    fn do_load_tib(&mut self, o: u64) {
        let tib_slot = (o as *mut u64).wrapping_add(1);
        let tib_slot_owner = self.get_owner_thread(tib_slot as u64);
        let tib_ptr = unsafe { *tib_slot } as *mut BidirectionalTib;
        let tib_ptr_owner = self.get_owner_thread(tib_ptr as u64);
        self.create_scan_object_work(tib_slot_owner, tib_ptr_owner, tib_ptr, o);
    }

    fn do_scan_object(&mut self, tib_ptr: *mut BidirectionalTib, o: u64) {
        let tib_owner = self.get_owner_thread(tib_ptr as u64);
        let tib = unsafe { &*tib_ptr };
        let num_refs = tib.num_refs;
        self.send_edges(tib_owner, (o as *mut u64).wrapping_add(2), num_refs as u64);
    }

    fn do_scan_refarray(&mut self, o: u64) {
        let array_length_ptr = (o as *mut u64).wrapping_add(2);
        let array_length_owner = self.get_owner_thread(array_length_ptr as u64);
        let array_length = unsafe { *array_length_ptr };
        self.send_edges(
            array_length_owner,
            (o as *mut u64).wrapping_add(3),
            array_length,
        );
    }

    fn load_edge(&mut self, creator: usize, worker: usize, edge: *mut u64) {
        let is_root_edge = creator == usize::MAX;
        let from_internal_message = creator == worker;
        self.stats.slots += 1;
        let child = unsafe { *edge };
        if child != 0 {
            let child_owner = self.get_owner_thread(child);
            let is_child_visible = child_owner == worker;
            self.create_mark_object_work(worker, child_owner, child);
            if is_root_edge {
                self.stats.non_empty_root_slots += 1;
                return;
            }
            if from_internal_message {
                if is_child_visible {
                    self.stats.visible_non_empty_slots_visible_child += 1;
                } else {
                    self.stats.visible_non_empty_slots_invisible_child += 1;
                }
            } else if is_child_visible {
                self.stats.invisible_non_empty_slots_visible_child += 1;
            } else {
                self.stats.invisible_non_empty_slots_invisible_child += 1;
            }
        } else if is_root_edge {
            self.stats.empty_root_slots += 1;
        } else if from_internal_message {
            self.stats.visible_empty_slots += 1;
        } else {
            self.stats.invisible_empty_slots += 1;
        }
    }

    fn do_edges(&mut self, creator: usize, worker: usize, start: *mut u64, count: u64) {
        // trace!("PE worker {} start 0x{:x} count {}", worker, start as u64, count);
        let end = start.wrapping_add(count as usize);
        if !self.rle {
            // When run-length encoding is disabled, we should only have one edge
            debug_assert_eq!(count, 1);
        }
        if count == 1 {
            // If this group only has one edge, we must own it
            debug_assert_eq!(worker, self.get_owner_thread(start as u64));
        }
        // Figure out the edges we are responsible for
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
                self.load_edge(creator, worker, edge);
                edge = edge.wrapping_add(1);
            }
            // Go to the next stride of the same thread
            stride_start = (stride_start as usize + self.next_stride_delta) as *mut u64;
            stride_end = stride_start.wrapping_add(pointers_in_stride);
        }
    }

    fn send_edges(&mut self, sender: usize, start: *mut u64, count: u64) {
        if count == 0 {
            // Sometimes a group of 0 edge is reported
            // because of 0 sized objarray for bidirectional/openjdk
            // or 0 ref object for bidirectional fallback
            return;
        }
        if count == 1 {
            let edge_owner = self.get_owner_thread(start as u64);
            if edge_owner == sender && self.eager_load {
                self.load_edge(sender, sender, start);
            } else {
                self.create_edges_work(sender, edge_owner, start, count);
            }
            return;
        }
        if !self.rle {
            for i in 0..count {
                let edge = start.wrapping_add(i as usize);
                let edge_owner = self.get_owner_thread(edge as u64);
                self.create_edges_work(sender, edge_owner, edge, 1);
            }
            return;
        }

        // This group has more than one edges
        // A more heavyweight process
        let edge_owner = self.get_owner_thread(start as u64);
        let stride_end = self.get_stride_end(start);
        let ptrs_fit_in_1st_stride =
            (stride_end as usize - start as usize) >> self.log_pointer_size;
        // if repeat > 16 {
        //     dbg!(edge_owner);
        //     dbg!(repeat);
        //     dbg!(ptrs_fit_in_1st_stride);
        // }
        // We need to send something to the edge owner regardless
        if edge_owner == sender && self.eager_load {
            self.do_edges(sender, edge_owner, start, count);
        } else {
            self.create_edges_work(sender, edge_owner, start, count);
        }
        let ptr_in_stide = self.get_pointers_in_stride() as u64;
        if count > ptrs_fit_in_1st_stride as u64 {
            // We need to send out more messages
            let leftover = count - ptrs_fit_in_1st_stride as u64;
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
                let worker = i % self.num_threads;
                // println!("{}->{} {:?}*{}", object_owner, edge_owner, edge, repeat);
                self.create_edges_work(sender, worker, start, count);
            }
        }
    }
}
