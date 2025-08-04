use crate::*;
use anyhow::Result;
use std::{collections::HashMap, path::Path};

mod ideal_trace_utilization;
use ideal_trace_utilization::IdealTraceUtilization;
mod nmpgc;
use nmpgc::NMPGC;
mod cache;

trait SimulationArchitecture {
    fn tick<O: ObjectModel>(&mut self) -> bool;
    fn new<O: ObjectModel>(args: &SimulationArgs, object_model: &O) -> Self;
    fn stats(&self) -> HashMap<String, f64>;
}

struct Simulation<A: SimulationArchitecture> {
    architecture: A,
}

impl<A: SimulationArchitecture> Simulation<A> {
    fn new<O: ObjectModel>(args: &SimulationArgs, object_model: &O) -> Self {
        Simulation {
            architecture: A::new(args, object_model),
        }
    }

    fn run<O: ObjectModel>(&mut self) {
        loop {
            let stop = self.architecture.tick::<O>();
            if stop {
                break;
            }
        }
    }

    fn stats(&self) -> HashMap<String, f64> {
        self.architecture.stats()
    }
}

pub fn reified_simulation<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let simulation_args = if let Some(Commands::Simulate(sim_args)) = args.command {
        sim_args
    } else {
        panic!("Incorrect dispatch");
    };
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
        let stats = match simulation_args.architecture {
            SimulationArchitectureChoice::IdealTraceUtilization => {
                let mut simuation: Simulation<IdealTraceUtilization> =
                    Simulation::new(&simulation_args, &object_model);
                simuation.run::<O>();
                simuation.stats()
            }
            SimulationArchitectureChoice::NMPGC => match simulation_args.processors {
                1 => {
                    let mut simuation: Simulation<NMPGC<0>> =
                        Simulation::new(&simulation_args, &object_model);
                    simuation.run::<O>();
                    simuation.stats()
                }
                2 => {
                    let mut simuation: Simulation<NMPGC<1>> =
                        Simulation::new(&simulation_args, &object_model);
                    simuation.run::<O>();
                    simuation.stats()
                }
                4 => {
                    let mut simuation: Simulation<NMPGC<2>> =
                        Simulation::new(&simulation_args, &object_model);
                    simuation.run::<O>();
                    simuation.stats()
                }
                _ => {
                    panic!(
                        "Unsupported number of processors for NMPGC: {}",
                        simulation_args.processors
                    );
                }
            },
        };
        let duration = start.elapsed();
        println!(
            "===== DaCapo hwgc-soft {:?} PASSED in {} msec =====",
            p.file_name().unwrap(),
            duration.as_millis()
        );
        println!("============================ Tabulate Statistics ============================");
        let stats_pairs: Vec<(String, f64)> = stats.into_iter().collect();
        for (i, (key, _)) in stats_pairs.iter().enumerate() {
            if i > 0 {
                print!("\t");
            }
            print!("{}", key);
        }
        println!();
        for (i, (_, value)) in stats_pairs.iter().enumerate() {
            if i > 0 {
                print!("\t");
            }
            print!("{:.3}", value);
        }
        println!();
        println!("-------------------------- End Tabulate Statistics --------------------------");
        heapdump.unmap_spaces()?;
    }
    Ok(())
}
