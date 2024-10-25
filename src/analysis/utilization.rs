use crate::trace::trace_object;
use crate::*;
use anyhow::Result;
use std::collections::VecDeque;

pub fn ideal_utilization<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let threads: usize = std::env::var("THREADS").expect("THREADS not set").parse()?;
    println!("THREADS: {}", threads);
    let _object_depth_args = if let Some(Commands::Utilization(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    let mut ticks: Vec<(usize, usize)> = vec![];
    let mut pauses = 0;
    for (_i, path) in args.paths.iter().enumerate() {
        pauses += 1;
        let mut total_ticks = 0;
        let mut busy_ticks = 0;
        let heapdump = HeapDump::from_binpb_zst(path)?;
        object_model.reset();
        heapdump.map_spaces()?;
        object_model.restore_objects(&heapdump);
        let mut mark_queue: VecDeque<u64> = VecDeque::new();
        for root in object_model.roots() {
            let o = *root;
            mark_queue.push_back(o);
            debug_assert_ne!(o, 0);
        }
        while !mark_queue.is_empty() {
            let mut batch = vec![];
            // Pop `P` objects
            while batch.len() < threads {
                let Some(o) = mark_queue.pop_front() else {
                    break;
                };
                batch.push(o);
            }
            batch.reverse();
            assert!(batch.len() <= threads);
            total_ticks += threads;
            busy_ticks += batch.len();
            // Trace `P` objects
            while let Some(o) = batch.pop() {
                if unsafe { trace_object(o, 1) } {
                    O::scan_object(o, |edge, repeat| {
                        for i in 0..repeat {
                            let e = edge.wrapping_add(i as usize);
                            let child = unsafe { *e };
                            if child != 0 {
                                mark_queue.push_back(child);
                            }
                        }
                    });
                }
            }
        }
        heapdump.unmap_spaces()?;
        ticks.push((total_ticks, busy_ticks));
    }
    let utilizations = ticks
        .iter()
        .map(|(t, b)| *b as f64 / *t as f64)
        .collect::<Vec<f64>>();
    let mean = utilizations.iter().sum::<f64>() / utilizations.len() as f64;
    let min = utilizations
        .iter()
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let max = utilizations
        .iter()
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let geomean = utilizations
        .iter()
        .product::<f64>()
        .powf(1.0 / utilizations.len() as f64);
    for i in 0..4 {
        let x = i + 1;
        println!("===== DaCapo 23.11-chopin xxx starting warmup {x} =====");
        println!("===== DaCapo 23.11-chopin xxx completed warmup {x} in 5399 msec =====");
    }
    println!("===== DaCapo 23.11-chopin xxx starting =====");
    println!("===== DaCapo 23.11-chopin xxx PASSED in 5654 msec =====");
    println!("============================ Tabulate Statistics ============================");
    println!(
        "pauses\ttrace.util.mean\ttrace.util.min\ttrace.util.max\ttrace.util.geomean\tthreads"
    );
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        pauses, mean, min, max, geomean, threads
    );
    println!("-------------------------- End Tabulate Statistics --------------------------");

    Ok(())
}
