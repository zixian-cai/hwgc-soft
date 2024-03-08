use clap::ValueEnum;

use crate::object_model::Header;
use crate::trace::shape_cache::ShapeLruCache;

use std::time::{Duration, Instant};

use crate::*;
use anyhow::Result;
#[cfg(feature = "zsim")]
use zsim_hooks::*;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum TracingLoopChoice {
    EdgeSlot,
    EdgeObjref,
    NodeObjref,
    DistributedNodeObjref,
    ShapeCache,
    WP,
    WP2,
    WPEdgeSlot,
    WPEdgeSlotDual,
}

#[derive(Debug, Default)]
pub struct TracingStats {
    pub marked_objects: u64,
    pub slots: u64,
    pub non_empty_slots: u64,
    pub sends: u64,
    pub shape_cache_stats: ShapeCacheStats,
}

impl TracingStats {
    fn add(&mut self, other: &TracingStats) {
        self.marked_objects += other.marked_objects;
        self.slots += other.slots;
        self.non_empty_slots += other.non_empty_slots;
        self.sends += other.sends;
        self.shape_cache_stats.add(&other.shape_cache_stats);
    }
}

#[derive(Debug)]
pub struct TimedTracingStats {
    pub stats: TracingStats,
    pub time: Duration,
}

pub(crate) unsafe fn trace_object(o: u64, mark_sense: u8) -> bool {
    // mark sense is 1 intially, and flip every epoch
    // println!("Trace object: 0x{:x}", o as u64);
    debug_assert_ne!(o, 0);
    let mut header = Header::load(o);
    // Return false if already marked
    let mark_byte = header.get_mark_byte();
    if mark_byte == mark_sense {
        false
    } else {
        header.set_mark_byte(mark_sense);
        header.store(o);
        true
    }
}

pub fn trace_object_atomic(o: u64, mark_sense: u8) -> bool {
    // mark sense is 1 intially, and flip every epoch
    // println!("Trace object: 0x{:x}", o as u64);
    debug_assert_ne!(o, 0);
    Header::attempt_mark_byte(o, mark_sense)
}

mod distributed_node_objref;
mod edge_objref;
mod edge_slot;
mod node_objref;
mod sanity;
mod shape_cache;
mod wp;
mod wp2;
mod wp_edge_slot;
mod wp_edge_slot_dual;

use self::util::tracer::Tracer;
use sanity::sanity_trace;

use self::shape_cache::ShapeCacheStats;

fn create_tracer<O: ObjectModel>(args: &TraceArgs) -> Option<Box<dyn Tracer<O>>> {
    // Only WPEdgeSlot supports the tracer interface for now.
    match args.tracing_loop {
        TracingLoopChoice::WPEdgeSlot => Some(wp_edge_slot::create_tracer::<O>(args)),
        TracingLoopChoice::WPEdgeSlotDual => Some(wp_edge_slot_dual::create_tracer::<O>(args)),
        _ => None,
    }
}

fn transitive_closure<O: ObjectModel>(
    args: TraceArgs,
    mark_sense: u8,
    object_model: &mut O,
    shape_cache: &mut ShapeLruCache<O>,
    tracer: Option<&Box<dyn Tracer<O>>>,
) -> TimedTracingStats {
    let start: Instant = Instant::now();
    let l = args.tracing_loop;
    let stats = unsafe {
        match l {
            TracingLoopChoice::EdgeObjref => {
                edge_objref::transitive_closure_edge_objref(mark_sense, object_model)
            }
            TracingLoopChoice::EdgeSlot => {
                edge_slot::transitive_closure_edge_slot(mark_sense, object_model)
            }
            TracingLoopChoice::NodeObjref => {
                node_objref::transitive_closure_node_objref(mark_sense, object_model)
            }
            TracingLoopChoice::DistributedNodeObjref => {
                distributed_node_objref::transitive_closure_distributed_node_objref(
                    mark_sense,
                    object_model,
                )
            }
            TracingLoopChoice::WP => wp::transitive_closure(mark_sense, object_model),
            TracingLoopChoice::WP2 => wp2::transitive_closure(mark_sense, object_model),
            TracingLoopChoice::ShapeCache => shape_cache::transitive_closure_shape_cache(
                args,
                mark_sense,
                object_model,
                shape_cache,
            ),
            TracingLoopChoice::WPEdgeSlot | TracingLoopChoice::WPEdgeSlotDual => {
                if let Some(tracer) = tracer {
                    tracer.trace(mark_sense, object_model)
                } else {
                    unreachable!()
                }
            }
        }
    };
    let elapsed = start.elapsed();
    TimedTracingStats {
        stats,
        time: elapsed,
    }
}

fn verify_mark<O: ObjectModel>(mark_sense: u8, object_model: &mut O) {
    for o in object_model.objects() {
        let header = Header::load(*o);
        if header.get_mark_byte() != mark_sense {
            error!("0x{:x} not marked by transitive closure", o);
        }
    }
}

pub fn reified_trace<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let trace_args = if let Some(Commands::Trace(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };

    if trace_args.tracing_loop == TracingLoopChoice::ShapeCache && trace_args.iterations != 1 {
        panic!("Only one iteration per heapdump is supported when doing shape cache analysis for avoiding warming up the shape cache");
    }
    let mut time = 0;
    let mut pauses = 0;
    let mut total_stats: TracingStats = Default::default();

    let mut shape_cache: ShapeLruCache<O> = ShapeLruCache::new(trace_args.shape_cache_size);

    for path in &args.paths {
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
        let mut mark_sense: u8 = 0;
        #[cfg(feature = "m5")]
        unsafe {
            m5::m5_reset_stats(0, 0);
        }
        #[cfg(feature = "zsim")]
        zsim_roi_begin();
        let iterations = trace_args.iterations;
        let tracer = create_tracer::<O>(&trace_args);
        if let Some(tracer) = tracer.as_ref() {
            tracer.startup();
        }
        for i in 0..iterations {
            mark_sense = (i % 2 == 0) as u8;
            let timed_stats = transitive_closure(
                trace_args,
                mark_sense,
                &mut object_model,
                &mut shape_cache,
                tracer.as_ref(),
            );
            let millis = timed_stats.time.as_micros() as f64 / 1000f64;
            let stats = timed_stats.stats;
            info!(
                "Finished marking {} objects, and processing {} slots ({} non-empty) in {:.3} ms",
                stats.marked_objects, stats.slots, stats.non_empty_slots, millis
            );
            info!(
                "That is, {:.1} objects/ms, and {:.1} slots/ms ({:.1} non-empty/ms)",
                stats.marked_objects as f64 / millis,
                stats.slots as f64 / millis,
                stats.non_empty_slots as f64 / millis
            );
            if stats.non_empty_slots != 0 {
                info!(
                    "Total communication: {}, {:.1}% of non-empty slots",
                    stats.sends,
                    stats.sends as f64 / stats.non_empty_slots as f64 * 100f64
                );
            }
            if cfg!(feature = "detailed_stats") {
                debug_assert_eq!(stats.marked_objects as usize, heapdump.objects.len());
            }
            if i == iterations - 1 {
                pauses += 1;
                time += timed_stats.time.as_micros();
                // println!("{:?}", stats);
                total_stats.add(&stats);
            }
            info!(
                "Final iteration {} ms",
                timed_stats.time.as_micros() as f64 / 1000f64
            );
        }
        #[cfg(feature = "m5")]
        unsafe {
            m5::m5_dump_reset_stats(0, 0);
        }
        #[cfg(feature = "zsim")]
        zsim_roi_end();
        verify_mark(mark_sense, &mut object_model);
        heapdump.unmap_spaces()?;
        if let Some(tracer) = tracer.as_ref() {
            tracer.teardown();
        }
    }

    println!("============================ Tabulate Statistics ============================");
    println!(
        "pauses\ttime\tobjects\tslots\tnon_empty_slots\tsends\t{}",
        total_stats.shape_cache_stats.get_stats_header()
    );
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        pauses,
        time,
        total_stats.marked_objects,
        total_stats.slots,
        total_stats.non_empty_slots,
        total_stats.sends,
        total_stats.shape_cache_stats.get_stats_value()
    );
    println!("-------------------------- End Tabulate Statistics --------------------------");
    Ok(())
}

// pub fn bench_prepare<O: ObjectModel>(object_model: &mut O, args: &Args) -> Result<HeapDump> {
//     let trace_args = if let Some(Commands::Trace(a)) = args.command {
//         a
//     } else {
//         panic!("Incorrect dispatch");
//     };
//     assert!(args.paths.len() == 1);
//     let path = &args.paths[0];
//     // reset object model internal states
//     object_model.reset();
//     let heapdump = HeapDump::from_binpb_zst(path)?;
//     // mmap
//     heapdump.map_spaces()?;
//     // write objects to the heap
//     {
//         let start = Instant::now();
//         object_model.restore_objects(&heapdump);
//         let elapsed = start.elapsed();
//         info!(
//             "Finish deserializing the heapdump, {} objects in {} ms",
//             heapdump.objects.len(),
//             elapsed.as_micros() as f64 / 1000f64
//         );
//     }
//     // sanity check
//     {
//         if cfg!(debug_assertions) {
//             let sanity_traced_objects = sanity_trace(&heapdump);
//             info!(
//                 "Sanity trace reporting {} reachable objects",
//                 sanity_traced_objects
//             );
//             assert_eq!(sanity_traced_objects, heapdump.objects.len());
//         }
//     }
//     // main tracing loop
//     #[cfg(feature = "m5")]
//     unsafe {
//         m5::m5_reset_stats(0, 0);
//     }
//     #[cfg(feature = "zsim")]
//     zsim_roi_begin();
//     prologue::<O>(trace_args.tracing_loop);
//     Ok(heapdump)
// }

// pub fn bench_release<O: ObjectModel>(
//     object_model: &mut O,
//     args: &Args,
//     iterations: usize,
//     heapdump: &HeapDump,
// ) -> Result<()> {
//     let trace_args = if let Some(Commands::Trace(a)) = args.command {
//         a
//     } else {
//         panic!("Incorrect dispatch");
//     };
//     #[cfg(feature = "m5")]
//     unsafe {
//         m5::m5_dump_reset_stats(0, 0);
//     }
//     #[cfg(feature = "zsim")]
//     zsim_roi_end();
//     let mark_sense = ((iterations - 1) % 2 == 0) as u8;
//     verify_mark(mark_sense, object_model);
//     heapdump.unmap_spaces()?;
//     epilogue::<O>(trace_args.tracing_loop);
//     Ok(())
// }

// pub fn bench_iter<O: ObjectModel>(
//     object_model: &mut O,
//     args: &Args,
//     iter: usize,
//     _heapdump: &HeapDump,
// ) -> Result<()> {
//     let trace_args = if let Some(Commands::Trace(a)) = args.command {
//         a
//     } else {
//         panic!("Incorrect dispatch");
//     };
//     let mark_sense = (iter % 2 == 0) as u8;
//     let _stats = unsafe {
//         match trace_args.tracing_loop {
//             TracingLoopChoice::EdgeObjref => {
//                 edge_objref::transitive_closure_edge_objref(mark_sense, object_model)
//             }
//             TracingLoopChoice::EdgeSlot => {
//                 edge_slot::transitive_closure_edge_slot(mark_sense, object_model)
//             }
//             TracingLoopChoice::NodeObjref => {
//                 node_objref::transitive_closure_node_objref(mark_sense, object_model)
//             }
//             TracingLoopChoice::DistributedNodeObjref => {
//                 distributed_node_objref::transitive_closure_distributed_node_objref(
//                     mark_sense,
//                     object_model,
//                 )
//             }
//             TracingLoopChoice::WP => wp::transitive_closure(mark_sense, object_model),
//             TracingLoopChoice::WP2 => wp2::transitive_closure(mark_sense, object_model),
//             TracingLoopChoice::WPMMTk => wp_mmtk::transitive_closure(mark_sense, object_model),
//         }
//     };
//     Ok(())
// }

pub fn run_bench(_b: &mut test::Bencher, _trace: TracingLoopChoice, _path: &str) {
    unimplemented!()
    //     let args = Args {
    //         paths: vec![path.to_string()],
    //         object_model: ObjectModelChoice::OpenJDK,
    //         command: Some(Commands::Trace(TraceArgs {
    //             tracing_loop: trace,
    //             iterations: 5,
    //         })),
    //     };
    //     let mut object_model = OpenJDKObjectModel::<false>::new();
    //     let heapdump = bench_prepare(&mut object_model, &args).unwrap();

    //     let mut iter = 0;

    //     bench_iter(&mut object_model, &args, iter, &heapdump).unwrap();
    //     iter += 1;

    //     b.iter(|| {
    //         bench_iter(&mut object_model, &args, iter, &heapdump).unwrap();
    //         iter += 1;
    //     });
    //     bench_release(&mut object_model, &args, iter, &heapdump).unwrap();
}
