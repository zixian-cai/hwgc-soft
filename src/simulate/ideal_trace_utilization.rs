use super::SimulationArchitecture;
use crate::{trace::trace_object, *};
use polars::prelude::*;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
};

pub(crate) struct IdealTraceUtilization {
    processors: Vec<ITUProcessor>,
    tracing_queue: VecDeque<u64>,
    ticks: usize,
    frontier_sizes: Vec<u64>, // Polars column can't be usize
    frontier_ticks: Vec<u64>,
}

impl SimulationArchitecture for IdealTraceUtilization {
    fn new<O: ObjectModel>(args: &SimulationArgs, object_model: &O) -> Self {
        // Convert &[u64] into Vec<u64>
        let mut queue: VecDeque<u64> = VecDeque::new();
        for root in object_model.roots() {
            let o = *root;
            queue.push_back(o);
            debug_assert_ne!(o, 0);
        }
        IdealTraceUtilization {
            processors: vec![ITUProcessor::new(); args.processors],
            tracing_queue: queue,
            ticks: 0,
            frontier_sizes: vec![],
            frontier_ticks: vec![],
        }
    }

    fn tick<O: ObjectModel>(&mut self) -> bool {
        // The number of objects in the traversal frontier at the end of the tick
        // At the end of tick 0, the frontier is the roots
        if self.ticks % 100 == 0 {
            self.frontier_sizes.push(self.tracing_queue.len() as u64);
            self.frontier_ticks.push(self.ticks as u64);
        }
        self.ticks += 1;
        let mut append_to_queue = Vec::new();
        for processor in &mut self.processors {
            append_to_queue.extend(processor.tick::<O>(self.tracing_queue.pop_front()));
        }
        self.tracing_queue.extend(append_to_queue);
        let terminate = self.tracing_queue.is_empty();
        if terminate {
            // Before we terminate, dump the frontier stats
            self.frontier_sizes.push(self.tracing_queue.len() as u64); // 0 in this case
            self.frontier_ticks.push(self.ticks as u64);
            let mut df = df! {
                "frontier_size" => &self.frontier_sizes,
                "tick" => &self.frontier_ticks
            }
            .unwrap();
            let file = File::create("ideal_trace_utilization_frontier.parquet").unwrap();
            let writer = ParquetWriter::new(file);
            writer.finish(&mut df).unwrap();
        }
        terminate
    }

    fn stats(&self) -> HashMap<String, f64> {
        let mut stats = HashMap::new();
        let mut total_marked_objects = 0;
        let mut total_busy_ticks = 0;

        for processor in &self.processors {
            total_marked_objects += processor.marked_objects;
            total_busy_ticks += processor.busy_ticks;
        }
        stats.insert("ticks".into(), self.ticks as f64);
        stats.insert("marked_objects.sum".into(), total_marked_objects as f64);
        stats.insert("busy_ticks.sum".into(), total_busy_ticks as f64);
        stats.insert(
            "utilization".into(),
            total_busy_ticks as f64 / (self.ticks * self.processors.len()) as f64,
        );
        stats
    }
}

#[derive(Debug, Default, Clone)]
struct ITUProcessor {
    busy_ticks: usize,
    marked_objects: usize,
}

impl ITUProcessor {
    fn new() -> Self {
        ITUProcessor {
            busy_ticks: 0,
            marked_objects: 0,
        }
    }
    fn tick<O: ObjectModel>(&mut self, o: Option<u64>) -> Vec<u64> {
        if o.is_none() {
            return vec![];
        }
        let o = o.unwrap();
        self.busy_ticks += 1;
        let mut children: Vec<u64> = vec![];
        if unsafe { trace_object(o, 1) } {
            self.marked_objects += 1;
            O::scan_object(o, |edge, repeat| {
                for i in 0..repeat {
                    let e = edge.wrapping_add(i as usize);
                    let child = unsafe { *e };
                    if child != 0 {
                        children.push(child);
                    }
                }
            });
        }

        children
    }
}
