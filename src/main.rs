#[macro_use]
extern crate log;

use std::time::{Duration, Instant};

use anyhow::Result;

use hwgc_soft::*;

#[cfg(feature = "zsim")]
use zsim_hooks::*;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all="verbatim")]
enum ObjectModelChoice {
    OpenJDK,
    OpenJDKAE,
    Bidirectional,
    BidirectionalFallback,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(required = true)]
    paths: Vec<String>,

    #[arg(short, long, default_value_t = 5)]
    iterations: usize,

    #[arg(short, long, value_enum)]
    object_model: ObjectModelChoice,

    #[arg(short, long, value_enum)]
    tracing_loop: TracingLoopChoice,
}

fn reified_main<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    for path in &args.paths {
        let start = Instant::now();
        let heapdump = HeapDump::from_binpb_zst(path)?;
        let tibs_cached = object_model.restore_tibs(&heapdump);
        let elapsed = start.elapsed();
        info!(
            "{} extra TIBs cached from processing {} in {} ms",
            tibs_cached,
            path,
            elapsed.as_millis()
        );
    }

    let mut time = 0;
    let mut pauses = 0;

    for path in &args.paths {
        // reset object model internal states
        object_model.reset();
        let heapdump = HeapDump::from_binpb_zst(path)?;
        // mmap
        heapdump.map_spaces()?;
        // write objects to the heap
        let start = Instant::now();
        object_model.restore_objects(&heapdump);
        let elapsed = start.elapsed();
        info!(
            "Finish deserializing the heapdump, {} objects in {} ms",
            heapdump.objects.len(),
            elapsed.as_micros() as f64 / 1000f64
        );
        if cfg!(debug_assertions) {
            let sanity_traced_objects = sanity_trace(&heapdump);
            info!(
                "Sanity trace reporting {} reachable objects",
                sanity_traced_objects
            );
            assert_eq!(sanity_traced_objects, heapdump.objects.len());
        }
        let mut mark_sense: u8 = 0;
        #[cfg(feature = "m5")]
        unsafe {
            m5::m5_reset_stats(0, 0);
        }
        #[cfg(feature = "zsim")]
        zsim_roi_begin();
        let mut elapsed = Duration::ZERO;
        for i in 0..args.iterations {
            mark_sense = (i % 2 == 0) as u8;
            let start: Instant = Instant::now();
            let marked_objects =
                transitive_closure(args.tracing_loop, mark_sense, &mut object_model);
            elapsed = start.elapsed();
            debug!(
                "Finished marking {} objects in {} ms",
                marked_objects,
                elapsed.as_micros() as f64 / 1000f64
            );
            debug_assert_eq!(marked_objects as usize, heapdump.objects.len());
        }
        pauses += 1;
        time += elapsed.as_micros();
        info!(
            "Final iteration {} ms",
            elapsed.as_micros() as f64 / 1000f64
        );
        #[cfg(feature = "m5")]
        unsafe {
            m5::m5_dump_reset_stats(0, 0);
        }
        #[cfg(feature = "zsim")]
        zsim_roi_end();
        verify_mark(mark_sense, &mut object_model);
        heapdump.unmap_spaces()?;
    }

    println!("============================ Tabulate Statistics ============================");
    println!("pauses\ttime");
    println!("{}\t{}", pauses, time);
    println!("-------------------------- End Tabulate Statistics --------------------------");
    Ok(())
}

fn get_git_info() -> String {
    match (built_info::GIT_COMMIT_HASH, built_info::GIT_DIRTY) {
        (Some(hash), Some(dirty)) => format!(
            "{}{}",
            hash.split_at(7).0,
            if dirty { "-dirty" } else { "" }
        ),
        (Some(hash), None) => format!("{}{}", hash.split_at(7).0, "-?"),
        _ => "unknown-git-version".to_string(),
    }
}

pub fn main() -> Result<()> {
    env_logger::init();
    println!("hwgc_soft {}", get_git_info());
    let args = Args::parse();
    match args.object_model {
        ObjectModelChoice::OpenJDK => reified_main(OpenJDKObjectModel::<false>::new(), args),
        ObjectModelChoice::OpenJDKAE => reified_main(OpenJDKObjectModel::<true>::new(), args),
        ObjectModelChoice::Bidirectional => {
            reified_main(BidirectionalObjectModel::<true>::new(), args)
        }
        ObjectModelChoice::BidirectionalFallback => {
            reified_main(BidirectionalObjectModel::<false>::new(), args)
        }
    }
}
