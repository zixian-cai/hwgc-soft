use crate::*;
use anyhow::Result;
use std::path::Path;

struct Simulation {
    num_processes: usize,
}

impl Simulation {
    fn from_args(args: SimulationArgs) -> Self {
        Simulation {
            num_processes: args.processes,
        }
    }

    fn run<O: ObjectModel>(&mut self, object_model: &O) {}

    fn print(&self) {}

    fn reset(&mut self) {}
}

pub fn reified_simulation<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let simulation_args = if let Some(Commands::Simulate(sim_args)) = args.command {
        sim_args
    } else {
        panic!("Incorrect dispatch");
    };
    let mut simuation = Simulation::from_args(simulation_args);
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
        simuation.run(&object_model);
        let duration = start.elapsed();
        println!(
            "===== DaCapo hwgc-soft {:?} PASSED in {} msec =====",
            p.file_name().unwrap(),
            duration.as_millis()
        );
        simuation.print();
        simuation.reset();
        heapdump.unmap_spaces()?;
    }
    Ok(())
}
