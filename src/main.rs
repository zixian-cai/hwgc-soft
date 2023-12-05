#[macro_use]
extern crate log;

use std::time::Instant;

use anyhow::Result;

use hwgc_soft::*;

#[cfg(feature = "zsim")]
use zsim_hooks::*;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
enum ObjectModelChoice {
    Openjdk,
    Bidirectional,
    BidirectionalFallback,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    path: String,

    #[arg(short, long, default_value_t = 5)]
    iterations: usize,

    #[arg(value_enum)]
    object_model: ObjectModelChoice,

    #[arg(short, long, default_value_t = false)]
    edge_enqueuing: bool,
}

fn reified_main<O: ObjectModel>(
    mut object_model: O,
    heapdump: HeapDump,
    iterations: usize,
    node_enqueuing: bool,
) {
    let start = Instant::now();
    object_model.restore_objects(&heapdump);
    let elapsed = start.elapsed();
    info!(
        "Finish deserializing the heapdump, {} objects in {} ms",
        heapdump.objects.len(),
        elapsed.as_micros() as f64 / 1000f64
    );
    info!(
        "Sanity trace reporting {} reachable objects",
        sanity_trace(&heapdump)
    );
    let mut mark_sense: u8 = 0;
    #[cfg(feature = "m5")]
    unsafe {
        m5::m5_reset_stats(0, 0);
    }
    #[cfg(feature = "zsim")]
    zsim_roi_begin();
    unsafe {
        for i in 0..iterations {
            mark_sense = (i % 2 == 0) as u8;
            let start: Instant = Instant::now();
            let marked_object = if node_enqueuing {
                transitive_closure_node(mark_sense, &mut object_model)
            } else {
                transitive_closure_edge(mark_sense, &mut object_model)
            };
            let elapsed = start.elapsed();
            info!(
                "Finished marking {} objects in {} ms",
                marked_object,
                elapsed.as_micros() as f64 / 1000f64
            );
        }
    }
    #[cfg(feature = "m5")]
    unsafe {
        m5::m5_dump_reset_stats(0, 0);
    }
    #[cfg(feature = "zsim")]
    zsim_roi_end();
    verify_mark(mark_sense, &mut object_model);
}

pub fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let heapdump = HeapDump::from_binpb_zst(args.path)?;
    heapdump.map_spaces()?;
    match args.object_model {
        ObjectModelChoice::Openjdk => {
            reified_main(
                OpenJDKObjectModel::new(),
                heapdump,
                args.iterations,
                !args.edge_enqueuing,
            );
        }
        ObjectModelChoice::Bidirectional => {
            reified_main(
                BidirectionalObjectModel::<true>::new(),
                heapdump,
                args.iterations,
                !args.edge_enqueuing,
            );
        }
        ObjectModelChoice::BidirectionalFallback => {
            reified_main(
                BidirectionalObjectModel::<false>::new(),
                heapdump,
                args.iterations,
                !args.edge_enqueuing,
            );
        }
    }
    Ok(())
}
