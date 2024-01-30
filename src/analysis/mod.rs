use std::collections::{HashMap, VecDeque};

use crate::{object_model::Header, *};
use anyhow::Result;

#[allow(clippy::enum_variant_names)]
enum Work {
    ProcessEdges { from: *mut u64, to: *mut u64 },
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
    num_threads: usize,
    work_queue: VecDeque<TaggedWork>,
    stats: AnalysisStats,
}

#[derive(Default)]
struct AnalysisStats {
    // Total amount of work
    // Which is equal to the the number of non-empty slots + invisible slots
    // Because each non-empty slots has a referent that needs to be called
    // process_node on using the ProcessNode packet
    // And each invisible slot results in a ProcessEdge packet sent to
    // another worker
    total_work: u64,
    total_msgs: u64,
    marked_objects: u64,
    /// The number of messages sent due to that the object being scanned is
    /// not entirely visible to the worker
    msg_invisible_slot: u64,
    /// The number of messages sent due to delegating the scan of a child object
    /// to another worker, when the slot (where the child ojbects are found) and
    /// the parent object are owned by the same worker
    msg_child_obj_not_owned_during_process_node: u64,
    /// Someone delegated a slot/slots for us to load, and we discovered child
    /// objects that are not owned by us
    msg_child_obj_not_owned_during_process_edge: u64,
    work_dist: HashMap<usize, u64>,
    non_empty_slots: u64,
    slots: u64,
}

impl Analysis {
    fn from_args(args: AnalysisArgs) -> Self {
        Analysis {
            owner_shift: args.owner_shift,
            num_threads: 1 << args.log_num_threads,
            work_queue: VecDeque::new(),
            stats: Default::default(),
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
            Work::ProcessEdges { from, to } => self.do_process_edges(from, to),
            Work::ProcessEdge(e) => self.do_process_edge(e),
            Work::ProcessNode(o) => self.do_process_node::<O>(o),
        }
    }

    fn create_process_edge_work(&mut self, creator: usize, worker: usize, e: *mut u64) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ProcessEdge(e),
        };
        if work.creator != work.worker {
            self.stats.total_msgs += 1;
            self.stats.msg_invisible_slot += 1;
        }
        self.create_work(work);
    }

    fn create_process_node_work(
        &mut self,
        creator: usize,
        worker: usize,
        o: u64,
        process_node: bool,
    ) {
        let work = TaggedWork {
            creator: Worker::Numbered(creator),
            worker: Worker::Numbered(worker),
            work: Work::ProcessNode(o),
        };
        if work.creator != work.worker {
            self.stats.total_msgs += 1;
            if process_node {
                self.stats.msg_child_obj_not_owned_during_process_node += 1;
            } else {
                self.stats.msg_child_obj_not_owned_during_process_edge += 1;
            }
        }
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
        // Some slots might not be visible to us
        O::scan_object(o, |edge| {
            let edge_owner = self.get_owner_thread(edge as u64);
            if edge_owner == object_owner {
                self.stats.slots += 1;
                let child = unsafe { *edge };
                if child == 0 {
                    return;
                }
                self.stats.non_empty_slots += 1;
                let child_owner = self.get_owner_thread(child);
                self.create_process_node_work(object_owner, child_owner, child, true);
            } else {
                self.create_process_edge_work(object_owner, edge_owner, edge);
            }
        })
    }

    fn do_process_edge(&mut self, e: *mut u64) {
        let edge_owner = self.get_owner_thread(e as u64);
        let child = unsafe { *e };
        self.stats.slots += 1;
        if child == 0 {
            return;
        }
        self.stats.non_empty_slots += 1;
        let child_owner = self.get_owner_thread(child);
        self.create_process_node_work(edge_owner, child_owner, child, false);
    }

    fn do_process_edges(&mut self, _from: *mut u64, _to: *mut u64) {
        todo!()
    }

    fn run<O: ObjectModel>(&mut self, o: &O) {
        for root in o.roots() {
            self.stats.slots += 1;
            if *root != 0 {
                self.stats.non_empty_slots += 1;
            }
            self.create_root_work(*root);
        }
        while let Some(tagged_work) = self.work_queue.pop_front() {
            self.do_work::<O>(tagged_work);
        }
        let mut dist: Vec<(usize, u64)> = self
            .stats
            .work_dist
            .iter()
            .map(|(worker, work_cnt)| (*worker, *work_cnt))
            .collect();
        dist.sort_by_key(|(worker, _)| *worker);
        println!("============================ Tabulate Statistics ============================");
        print!("works\tmessages\tobjects\tslots\tnon_empty_slots\tmsg.invisible_slot\tmsg.remote_child_local_edge\tmsg.remote_child_remote_edge");
        for (x, _) in &dist {
            print!("\tworks.{}", x);
        }
        println!();
        print!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.stats.total_work,
            self.stats.total_msgs,
            self.stats.marked_objects,
            self.stats.slots,
            self.stats.non_empty_slots,
            self.stats.msg_invisible_slot,
            self.stats.msg_child_obj_not_owned_during_process_node,
            self.stats.msg_child_obj_not_owned_during_process_edge
        );
        for (_, work_cnt) in &dist {
            print!("\t{}", work_cnt);
        }
        println!();
        println!("-------------------------- End Tabulate Statistics --------------------------");
        assert_eq!(
            self.stats.total_work,
            self.stats.non_empty_slots + self.stats.msg_invisible_slot
        );
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
