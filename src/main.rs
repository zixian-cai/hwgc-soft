#[macro_use]
extern crate log;
use anyhow::Result;

use clap::Parser;
use hwgc_soft::*;
use std::time::Instant;

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

    if let Some(ref cmd) = args.command {
        match cmd {
            Commands::Trace(_) => {
                reified_trace(object_model, args, true)?;
                Ok(())
            }
            Commands::Analyze(_) => reified_analysis(object_model, args),
            Commands::Depth(_) => object_depth(object_model, args),
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
