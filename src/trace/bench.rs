use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use clap::Parser;
use harness::Bencher;

use crate::{
    BidirectionalObjectModel, HeapDump, ObjectModel, ObjectModelChoice, OpenJDKObjectModel,
    TraceArgs,
};

use super::sanity::sanity_trace;
use crate::util::tracer::Tracer;

pub use super::TracingStats;

use super::verify_mark;

pub trait BenchContext: Send + Sync {
    fn iter(&self) -> TracingStats;
    fn finalize(&self, b: &Bencher, stats: TracingStats);
}

struct BenchContextImpl<O: ObjectModel> {
    object_model: O,
    args: TraceArgs,
    tracer: Box<dyn Tracer<O>>,
    heapdump: HeapDump,
    iter: AtomicUsize,
}

unsafe impl<O: ObjectModel> Send for BenchContextImpl<O> {}
unsafe impl<O: ObjectModel> Sync for BenchContextImpl<O> {}

impl<O: ObjectModel> Drop for BenchContextImpl<O> {
    fn drop(&mut self) {
        release(
            &mut self.object_model,
            &*self.tracer,
            self.iter.load(Ordering::SeqCst),
            &self.heapdump,
        )
        .unwrap();
    }
}

impl<O: ObjectModel> BenchContext for BenchContextImpl<O> {
    fn iter(&self) -> TracingStats {
        let curr_iter = self.iter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        iter(
            &self.object_model,
            &self.args,
            &*self.tracer,
            curr_iter,
            &self.heapdump,
        )
        .unwrap()
    }

    fn finalize(&self, b: &Bencher, stats: TracingStats) {
        b.add_stat("marked_objects", stats.marked_objects as u64);
        b.add_stat("slots", stats.slots as u64);
        b.add_stat("non_empty_slots", stats.non_empty_slots as u64);
        b.add_stat("copied_objects", stats.copied_objects as u64);
        b.add_stat("packets", stats.packets as u64);
        b.add_stat("total_run_time_us", stats.total_run_time_us as u64);
        let walltime_us = b.get_walltime().unwrap().as_micros() as f32;
        b.add_stat(
            "utilization",
            stats.total_run_time_us as f32 / (32f32 * walltime_us),
        );
    }
}

fn prepare_with_obj_model<O: ObjectModel>(
    mut object_model: O,
    args: TraceArgs,
    path: &str,
) -> anyhow::Result<Box<dyn BenchContext>> {
    // reset object model internal states
    object_model.reset();
    let heapdump = HeapDump::from_binpb_zst(path)?;
    // mmap
    heapdump.map_spaces()?;
    // write objects to the heap
    {
        let start = Instant::now();
        object_model.restore_objects(&heapdump);
        let elapsed = start.elapsed();
        info!(
            "Finish deserializing the heapdump, {} objects in {} ms",
            heapdump.objects.len(),
            elapsed.as_micros() as f64 / 1000f64
        );
    }
    // sanity check
    {
        if cfg!(debug_assertions) {
            let sanity_traced_objects = sanity_trace(&heapdump);
            info!(
                "Sanity trace reporting {} reachable objects",
                sanity_traced_objects
            );
            assert_eq!(sanity_traced_objects, heapdump.objects.len());
        }
    }
    // main tracing loop
    #[cfg(feature = "m5")]
    unsafe {
        m5::m5_reset_stats(0, 0);
    }
    #[cfg(feature = "zsim")]
    zsim_roi_begin();
    let tracer = super::create_tracer(&args).unwrap();
    tracer.startup();
    Ok(Box::new(BenchContextImpl {
        object_model,
        args,
        tracer,
        heapdump,
        iter: AtomicUsize::new(0),
    }))
}

pub fn prepare(
    object_model: ObjectModelChoice,
    tracing_loop: &str,
    path: &str,
) -> anyhow::Result<Box<dyn BenchContext>> {
    let args =
        TraceArgs::parse_from(["bench", "--tracing-loop", tracing_loop, "--iterations", "1"]);
    match object_model {
        ObjectModelChoice::OpenJDK => {
            prepare_with_obj_model(OpenJDKObjectModel::<false>::new(), args, path)
        }
        ObjectModelChoice::OpenJDKAE => {
            prepare_with_obj_model(OpenJDKObjectModel::<true>::new(), args, path)
        }
        ObjectModelChoice::Bidirectional => {
            prepare_with_obj_model(BidirectionalObjectModel::<true>::new(), args, path)
        }
        ObjectModelChoice::BidirectionalFallback => {
            prepare_with_obj_model(BidirectionalObjectModel::<false>::new(), args, path)
        }
    }
}

fn release<O: ObjectModel>(
    object_model: &mut O,
    tracer: &dyn Tracer<O>,
    iterations: usize,
    heapdump: &HeapDump,
) -> anyhow::Result<()> {
    #[cfg(feature = "m5")]
    unsafe {
        m5::m5_dump_reset_stats(0, 0);
    }
    #[cfg(feature = "zsim")]
    zsim_roi_end();
    let mark_sense = ((iterations - 1) % 2 == 0) as u8;
    verify_mark(mark_sense, object_model);
    heapdump.unmap_spaces()?;
    tracer.teardown();
    Ok(())
}

pub fn iter<O: ObjectModel>(
    object_model: &O,
    _trace_args: &TraceArgs,
    tracer: &dyn Tracer<O>,
    iter: usize,
    _heapdump: &HeapDump,
) -> anyhow::Result<TracingStats> {
    let mark_sense = (iter % 2 == 0) as u8;
    let stats = tracer.trace(mark_sense, object_model);
    Ok(stats)
}
