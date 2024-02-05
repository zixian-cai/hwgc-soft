use std::collections::{HashMap, VecDeque};

use crate::{object_model::Header, *};
use anyhow::Result;

#[allow(clippy::enum_variant_names)]
enum Work {
    ProcessEdges { start: *mut u64, count: u64 },
    ProcessEdge(*mut u64),
    ProcessNode(u64),
}

struct TaggedWork {
    creator: Worker,
    worker: Worker,
    work: Work,
}

#[derive(PartialEq, Eq)]
enum Worker {
    Numbered(usize),
    Environment,
}

struct Analysis {
    owner_shift: usize,
    log_num_threads: usize,
    num_threads: usize,
    work_queue: VecDeque<TaggedWork>,
    stats: AnalysisStats,
    group_slots: bool,
    log_pointer_size: usize,
    #[allow(dead_code)]
    stride_length: usize,
    /// How far to go to get to the next stride of the same thread
    next_stride_delta: usize,
}

/// Statistics about communication in a distributed near-memory GC
///
/// All slots (edges) can be categorized as follows:
/// 1. Empty slots
/// 2. Non-empty slots
///
/// 1.a Empty slot, visible: a worker scanning the object can also load from the
/// slot, and then discovers that the slot holds null. No further
/// 1.b Empty slot, invisible: a worker scanning the object has to delegate
/// someone else to load the slot (using the ProcessEdge message, or the
/// ProcessEdges message), which subsequently turns out to be null.
/// 2.a Non-empty slot, visible, visible child: a worker scanning the object
/// can also load from the slot, and then discovers a child object, which is
/// also visible. No message was sent in the process. This is rare (~1/N chance).
/// 2.b Non-empty slot, visible, invisible child: a worker scanning the object
/// can also load from the slot, and then discovers a child object, which is
/// invisible. A ProcessNode message is sent.
/// 2.c Non-empty slot, invisible, visible child: a worker scanning the object
/// has to delegate someone else to load the slot (using the ProcessEdge
/// message, or the ProcessEdges message), which is common. The child object
/// happens to be visible to the delegate, which is rare.
/// 2.d Non-empty slot, invisible, invisible child: a worker scanning the object
/// has to delegate someone else to load the slot (using the ProcessEdge
/// message, or the ProcessEdges message), which is common. The delegate
/// discovers a child object, which is invisible. A ProcessNode message is sent.
///
/// Another classification is:
/// 1. Visible slots:
/// 1.a Visible, empty slot
/// 1.b Visible, non-empty slot, visible child
/// 1.c Visible, non-empty slot, invisible child: a ProcessNode message is sent.
/// 2. Invisible slots: need to send ProcessEdge/ProcessEdges messages
/// 2.a Invisible slot, empty slot
/// 2.b Invisible slot, non-empty slot, visible child
/// 2.c Invisible slot, non-empty slot, invisible child: a ProcessNode message is
/// sent.
#[derive(Default)]
struct AnalysisStats {
    /// Total amount of work
    ///
    /// This is equal to the the number of non-empty slots + invisible slots
    /// when the group_slots optimization is disabled.
    /// This is because each non-empty slots has a referent that needs to be called
    /// process_node on using the ProcessNode packet
    /// (a message may or may not be sent, depending on whether the child is
    /// visible to the slot loader).
    /// And each invisible slot results in a ProcessEdge packet sent to
    /// another worker.
    total_work: u64,
    /// Distribuion of work among each worker
    work_dist: HashMap<usize, u64>,
    /// Total objects marked
    marked_objects: u64,
    /// Total number of inter-worker messages sent
    total_msgs: u64,
    msg_process_node: u64,
    msg_process_edge: u64,
    msg_process_edges: u64,
    /// Total number of slots
    slots: u64,
    empty_root_slots: u64,
    non_empty_root_slots: u64,
    visible_empty_slots: u64,
    visible_non_empty_slots_visible_child: u64,
    visible_non_empty_slots_invisible_child: u64,
    invisible_empty_slots: u64,
    invisible_non_empty_slots_visible_child: u64,
    invisible_non_empty_slots_invisible_child: u64,
}

impl Analysis {
    fn from_args(args: AnalysisArgs) -> Self {
        Analysis {
            owner_shift: args.owner_shift,
            log_num_threads: args.log_num_threads,
            num_threads: 1 << args.log_num_threads,
            work_queue: VecDeque::new(),
            stats: Default::default(),
            group_slots: args.group_slots,
            log_pointer_size: 3,
            stride_length: 1 << args.owner_shift,
            next_stride_delta: 1 << (args.owner_shift + args.log_num_threads),
        }
    }

    fn get_owner_thread(&self, o: u64) -> usize {
        let mask = ((self.num_threads - 1) << self.owner_shift) as u64;
        ((o & mask) >> self.owner_shift) as usize
    }

    fn reset(&mut self) {
        self.work_queue.clear();
        self.stats = Default::default();
    }

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

    fn create_root_work(&mut self, root: u64) {
        let tagged_work = TaggedWork {
            creator: Worker::Environment,
            worker: Worker::Numbered(self.get_owner_thread(root)),
            work: Work::ProcessNode(root),
        };
        self.create_work(tagged_work);
    }

    fn do_work<O: ObjectModel>(&mut self, work: TaggedWork) {
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
                    self.do_process_node_grouped::<O>(o)
                } else {
                    self.do_process_node::<O>(o)
                }
            }
        }
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

    fn do_process_node<O: ObjectModel>(&mut self, o: u64) {
        debug_assert_ne!(o, 0);
        let mut header = Header::load(o);
        let mark_byte = header.get_mark_byte();
        if mark_byte == 1 {
            return;
        }
        self.stats.marked_objects += 1;
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
        })
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

    fn do_process_node_grouped<O: ObjectModel>(&mut self, o: u64) {
        debug_assert_ne!(o, 0);
        let mut header = Header::load(o);
        let mark_byte = header.get_mark_byte();
        if mark_byte == 1 {
            return;
        }
        self.stats.marked_objects += 1;
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

    fn run<O: ObjectModel>(&mut self, o: &O) {
        for root in o.roots() {
            self.stats.slots += 1;
            if *root != 0 {
                self.stats.non_empty_root_slots += 1;
                self.create_root_work(*root);
            } else {
                self.stats.empty_root_slots += 1;
            }
        }
        // I don't think the OpenJDK heapdump gives any empty roots
        debug_assert_eq!(self.work_queue.len(), o.roots().len());
        while let Some(tagged_work) = self.work_queue.pop_front() {
            self.do_work::<O>(tagged_work);
        }
        debug_assert!(self.work_queue.is_empty());
        let mut dist: Vec<(usize, u64)> = self
            .stats
            .work_dist
            .iter()
            .map(|(worker, work_cnt)| (*worker, *work_cnt))
            .collect();
        dist.sort_by_key(|(worker, _)| *worker);
        println!("============================ Tabulate Statistics ============================");
        print!(
            "obj\t\
            msg\tmsg.pn\tmsg.pe\tmsg.pes\t\
            slots\tslots.vis.empty\tslots.vis.child.vis\tslots.vis.child.invis\t\
            slots.invis.empty\tslots.invis.child.vis\tslots.invis.child.invis\t\
            slots.root.empty\tslots.root.non_empty\t\
            work"
        );
        for (x, _) in &dist {
            print!("\twork.{}", x);
        }
        println!();
        print!(
            "{}\t\
            {}\t{}\t{}\t{}\t\
            {}\t{}\t{}\t{}\t\
            {}\t{}\t{}\t\
            {}\t{}\t\
            {}",
            self.stats.marked_objects,
            self.stats.total_msgs,
            self.stats.msg_process_node,
            self.stats.msg_process_edge,
            self.stats.msg_process_edges,
            self.stats.slots,
            self.stats.visible_empty_slots,
            self.stats.visible_non_empty_slots_visible_child,
            self.stats.visible_non_empty_slots_invisible_child,
            self.stats.invisible_empty_slots,
            self.stats.invisible_non_empty_slots_visible_child,
            self.stats.invisible_non_empty_slots_invisible_child,
            self.stats.empty_root_slots,
            self.stats.non_empty_root_slots,
            self.stats.total_work
        );
        for (_, work_cnt) in &dist {
            print!("\t{}", work_cnt);
        }
        println!();
        println!("-------------------------- End Tabulate Statistics --------------------------");
        debug_assert_eq!(
            self.stats.slots,
            self.stats.visible_empty_slots
                + self.stats.visible_non_empty_slots_visible_child
                + self.stats.visible_non_empty_slots_invisible_child
                + self.stats.invisible_empty_slots
                + self.stats.invisible_non_empty_slots_visible_child
                + self.stats.invisible_non_empty_slots_invisible_child
                + self.stats.non_empty_root_slots
                + self.stats.empty_root_slots
        );
        debug_assert_eq!(
            self.stats.total_msgs,
            self.stats.msg_process_edge
                + self.stats.msg_process_edges
                + self.stats.msg_process_node
        );
        debug_assert_eq!(self.stats.total_work, self.stats.work_dist.values().sum());
        // if !self.group_slots {
        //     assert_eq!(
        //         self.stats.total_work,
        //         self.stats.non_empty_slots + self.stats.msg_invisible_slot
        //     );
        // }
        // for n in o.objects() {
        //     let header = Header::load(*n);
        //     if header.get_mark_byte() != 1 {
        //         error!("0x{:x} not marked by transitive closure", n);
        //     }
        // }
    }
}

pub fn reified_analysis<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let analysis_args = if let Some(Commands::Analyze(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    let mut analysis = Analysis::from_args(analysis_args);
    for path in &args.paths {
        // reset object model internal states
        object_model.reset();
        let heapdump = HeapDump::from_binpb_zst(path)?;
        // mmap
        heapdump.map_spaces()?;
        // write objects to the heap
        object_model.restore_objects(&heapdump);
        analysis.run(&object_model);
        analysis.reset();
    }
    Ok(())
}
