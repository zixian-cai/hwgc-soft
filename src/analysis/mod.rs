use crate::*;
use anyhow::Result;
use std::alloc;
use std::collections::VecDeque;
use std::path::Path;

mod work;
use work::*;
mod stats;
use stats::*;
pub(crate) mod depth;

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
    }

    fn run<O: ObjectModel>(&mut self, o: &O) {
        let num_roots = o.roots().len();
        // Gather stats about roots
        for root in o.roots() {
            self.stats.slots += 1;
            if *root != 0 {
                self.stats.non_empty_root_slots += 1;
            } else {
                self.stats.empty_root_slots += 1;
            }
        }

        // Write roots to raw memory for GC workers to use
        let root_pages_layout =
            alloc::Layout::from_size_align(num_roots * size_of::<u64>(), 4096).unwrap();
        let root_pages_raw = unsafe { alloc::alloc(root_pages_layout) };
        unsafe {
            std::ptr::copy(
                o.roots().as_ptr(),
                root_pages_raw as *mut u64,
                o.roots().len(),
            );
        }
        if !self.group_slots {
            for i in 0..o.roots().len() {
                let e = (root_pages_raw as *mut u64).wrapping_add(i);
                let worker = self.get_owner_thread(e as u64);
                self.create_root_edges_work(worker, e, 1);
            }
        } else {
            for i in 0..self.num_threads {
                self.create_root_edges_work(i, root_pages_raw as *mut u64, num_roots as u64);
            }
        }
        let object_sizes = o.object_sizes();
        // If group-slots optimization is not enable, then the work queue
        // depth should be equal to the number of roots
        if !self.group_slots {
            debug_assert_eq!(self.work_queue.len(), o.roots().len());
        }
        while let Some(tagged_work) = self.work_queue.pop_front() {
            self.do_work::<O>(tagged_work, object_sizes);
        }
        debug_assert!(self.work_queue.is_empty());
        // for n in o.objects() {
        //     let header = Header::load(*n);
        //     if header.get_mark_byte() != 1 {
        //         error!("0x{:x} not marked by transitive closure", n);
        //     }
        // }
        unsafe { alloc::dealloc(root_pages_raw, root_pages_layout) };
    }
}

pub fn reified_analysis<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let analysis_args = if let Some(Commands::Analyze(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    assert_eq!(
        args.object_model,
        ObjectModelChoice::Bidirectional,
        "The distributed GC work analysis assumes bidirectional for now"
    );
    let mut analysis = Analysis::from_args(analysis_args);
    for path in &args.paths {
        let p: &Path = path.as_ref();
        // Fake a DaCapo iteration for easier parsing
        println!(
            "===== DaCapo hwgc-soft {:?} starting =====",
            p.file_name().unwrap()
        );
        let start = std::time::Instant::now();
        // reset object model internal states
        object_model.reset();
        let heapdump = HeapDump::from_binpb_zst(path)?;
        // mmap
        heapdump.map_spaces()?;
        // write objects to the heap
        object_model.restore_objects(&heapdump);
        analysis.run(&object_model);
        let duration = start.elapsed();
        println!(
            "===== DaCapo hwgc-soft {:?} PASSED in {} msec =====",
            p.file_name().unwrap(),
            duration.as_millis()
        );
        analysis.stats.print();
        analysis.reset();
        heapdump.unmap_spaces()?;
    }
    Ok(())
}
