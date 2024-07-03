use clap::Parser;
use harness::Bencher;

use crate::{ObjectModelChoice, OpenJDKObjectModel, TraceArgs};

use super::TracingStats;

#[derive(Debug)]
struct BenchStats {
    pub pauses: usize,
    /// Time in milliseconds
    pub time: f32,
    pub tracing_stats: TracingStats,
}

pub trait BenchContext: Send + Sync {
    fn run(&mut self);
    fn collect_results(&self, b: &Bencher);
}

struct BenchContextImpl {
    paths: Vec<String>,
    stats: Option<BenchStats>,
}

unsafe impl Send for BenchContextImpl {}
unsafe impl Sync for BenchContextImpl {}

impl BenchContext for BenchContextImpl {
    fn run(&mut self) {
        assert!(self.stats.is_none());

        let tracing_loop = std::env::var("TRACING_LOOP").unwrap_or("WPEdgeSlot".to_string());
        let trace_args = TraceArgs::parse_from(["bench", "--tracing-loop", &tracing_loop]);
        let args = crate::Args {
            paths: self.paths.clone(),
            object_model: ObjectModelChoice::OpenJDK,
            command: Some(crate::Commands::Trace(trace_args)),
        };
        let object_model = OpenJDKObjectModel::<false>::new();
        let (pauses, time, stats) = super::reified_trace(object_model, args, false).unwrap();
        let stats = BenchStats {
            pauses,
            time,
            tracing_stats: stats,
        };
        self.stats = Some(stats);
    }

    fn collect_results(&self, b: &Bencher) {
        let stats = self.stats.as_ref().unwrap();
        b.add_stat("time", stats.time);
        b.add_stat("pauses", stats.pauses);
        b.add_stat("marked_objects", stats.tracing_stats.marked_objects);
        b.add_stat("slots", stats.tracing_stats.slots);
        b.add_stat("non_empty_slots", stats.tracing_stats.non_empty_slots);
    }
}

pub fn create_bench_context(name: &'static str) -> Box<dyn BenchContext> {
    let dir = std::path::PathBuf::from(format!("./sampled/{name}"));
    assert!(dir.is_dir());
    assert!(dir.exists());
    // glob
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() && path.extension().unwrap_or_default() == "zst" {
            paths.push(path.to_str().unwrap().to_string());
        }
    }
    assert!(paths.len() > 0, "No heapdumps found in {}", dir.display());
    println!("Loaded {} heapdumps.", paths.len());
    Box::new(BenchContextImpl { paths, stats: None })
}

#[macro_export]
macro_rules! define_benchmark {
    ($name:ident) => {
        #[harness::bench]
        fn $name(b: &harness::Bencher) {
            let mut context = $crate::bench::create_bench_context(stringify!($name));
            b.time(|| {
                context.run();
            });
            context.collect_results(b);
        }
    };
}
