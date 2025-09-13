use crate::*;
use anyhow::{Ok, Result};
use std::io::Write;

pub fn export<O: ObjectModel>(mut _object_model: O, args: Args) -> Result<()> {
    let export_args = if let Some(Commands::Export(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    assert_eq!(args.paths.len(), 1, "Can only export one heap dump at a time");
    let heapdump = HeapDump::from_path(&args.paths[0])?;
    // Open the output file for writing
    let mut output_file = std::fs::File::create(&export_args.output_path)?;
    writeln!(output_file, "source,target")?;
    for o in &heapdump.objects {
        for e in &o.edges {
            if e.objref != 0 {
                writeln!(output_file, "{},{}", o.start, e.objref)?;
            }
        }
    }
    Ok(())
}