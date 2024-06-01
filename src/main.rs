#[macro_use]
extern crate log;
use anyhow::anyhow;
use anyhow::Result;

use clap::Parser;
use hwgc_soft::NoOpMemoryInterface;
use hwgc_soft::*;
use std::time::Instant;

fn reified_main<O: ObjectModel>(object_model: O, args: Args) -> Result<()> {
    if let Some(ref cmd) = args.command {
        match cmd {
            Commands::Trace(_) => reified_main_host_memory(object_model, args),
            Commands::Analyze(_) => reified_main_host_memory(object_model, args),
            Commands::Depth(_) => reified_main_host_memory(object_model, args),
            // Since memdump operates on target address space
            // It will handle the memory interface and tib allocation arena
            Commands::Memdump(_) => dump_mem(object_model, args),
        }
    } else {
        Ok(())
    }
}

fn reified_main_host_memory<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    // 8 MiB of memory for dynamically allocating TIB stuff
    // I previously measured that benchmarks allocate about 5000-6000 Klass objects using 4~6M
    // Even with alignment encoding, it only uses about 0.5M more.
    //
    // TODO make it a command line argument
    let tib_arena_size: usize = 8 * 1024 * 1024;
    let tib_arena_backing = crate::util::mmap_anon(tib_arena_size).unwrap();
    let mut tib_arena = BumpAllocationArena::new(
        tib_arena_backing as *mut u8,
        tib_arena_backing as *mut u8,
        tib_arena_size,
        16,
    );

    for path in &args.paths {
        let start = Instant::now();
        let heapdump = HeapDump::from_binpb_zst(path)?;
        // TODO: Should use target memory address when restoring tib
        // for memdump
        let tibs_cached = object_model.restore_tibs::<NoOpMemoryInterface>(
            &heapdump,
            &NoOpMemoryInterface::new(),
            &mut tib_arena,
        );
        let elapsed = start.elapsed();
        info!(
            "{} extra TIBs cached from processing {} in {} ms",
            tibs_cached,
            path,
            elapsed.as_millis()
        );
    }

    if let Some(ref cmd) = args.command {
        match cmd {
            Commands::Trace(_) => reified_trace(object_model, tib_arena, args),
            Commands::Analyze(_) => reified_analysis(object_model, tib_arena, args),
            Commands::Depth(_) => object_depth(object_model, tib_arena, args),
            Commands::Memdump(_) => Err(anyhow!("Should be handled elsewhere")),
        }
    } else {
        Ok(())
    }
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
