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
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    path: String,

    #[arg(short, long, default_value_t = 5)]
    iterations: usize,

    #[arg(value_enum)]
    object_model: ObjectModelChoice,
}

fn reified_main<O: ObjectModel>(mut object_model: O, heapdump: HeapDump, iterations: usize) {
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
            transitive_closure(mark_sense, &mut object_model);
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
            reified_main(OpenJDKObjectModel::new(), heapdump, args.iterations);
        }
        ObjectModelChoice::Bidirectional => {
            reified_main(BidirectionalObjectModel::new(), heapdump, args.iterations);
        }
    }
    Ok(())
}
