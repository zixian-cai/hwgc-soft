use super::SimulationArchitecture;
use crate::simulate::memory::RankID;
use crate::simulate::nmpgc::topology::Topology;
use crate::util::ticks_to_us;
use crate::simulate::memory::{AddressMapping, DDR4RankOption};
use crate::{ObjectModel, SimulationArgs};
use std::collections::{HashMap, VecDeque};

mod topology;
mod work;
use work::{NMPMessage, NMPProcessorWork, NMPProcessorWorkType};

use super::memory::{DataCache, SetAssociativeCache};
use super::tracing::TracingEvent;

pub(crate) struct NMPGC<const LOG_NUM_THREADS: u8> {
    processors: Vec<NMPProcessor<LOG_NUM_THREADS>>,
    ticks: usize,
    frequency_ghz: f64,
}

impl<const LOG_NUM_THREADS: u8> NMPGC<LOG_NUM_THREADS> {
    const NUM_THREADS: u64 = 1u64 << LOG_NUM_THREADS;
    fn get_owner_processor(o: u64) -> usize {
        let mapping = AddressMapping(o);
        mapping.get_owner_id()
    }
}

impl<const LOG_NUM_THREADS: u8> SimulationArchitecture for NMPGC<LOG_NUM_THREADS> {
    fn new<O: ObjectModel>(args: &SimulationArgs, object_model: &O) -> Self {
        let rank_option = if args.use_dramsim3 {
            DDR4RankOption::DRAMsim3 {
                config_file: args
                    .dramsim3_config
                    .clone()
                    .unwrap_or_else(|| "configs/DDR4_8Gb_x8_3200.ini".to_string()),
                output_dir: ".".to_string(),
            }
        } else {
            DDR4RankOption::Naive
        };

        // Convert &[u64] into Vec<u64>
        let mut processors: Vec<NMPProcessor<LOG_NUM_THREADS>> = (0..Self::NUM_THREADS)
            .map(|id| NMPProcessor::new(id as usize, rank_option.clone()))
            .collect();
        for root in object_model.roots() {
            let o = *root;
            debug_assert_ne!(o, 0);
            let owner = Self::get_owner_processor(o);
            processors[owner].works.push_back(NMPProcessorWork::Mark(o));
        }
        NMPGC {
            processors,
            ticks: 0,
            // Only valid for DDR4-3200
            frequency_ghz: 1.6,
        }
    }

    fn tick<O: ObjectModel>(&mut self) -> bool {
        self.ticks += 1;
        let mut messages = Vec::new();

        for p in &mut self.processors {
            let msg = p.tick::<O>();
            if let Some(m) = msg {
                messages.push(m);
            }
        }
        // Propagate messages
        for m in messages {
            self.processors[m.recipient].inbox.push(m);
        }
        // Check if all processors are done
        // FIXME: this assumes magical global knowledge, but
        // this actually requires a distributed termination detection algorithm
        let all_done = self.processors.iter().all(|p| p.locally_done());
        all_done
    }

    fn stats(&self) -> HashMap<String, f64> {
        let mut stats = HashMap::new();
        let mut total_marked_objects = 0;
        let mut total_busy_ticks = 0;
        let mut total_read_hits = 0;
        let mut total_read_misses = 0;
        let mut total_write_hits = 0;
        let mut total_write_misses = 0;

        for processor in &self.processors {
            info!("[P{}] marked objects: {}, busy ticks: {}, utilization: {:.3}, read hits: {}, read misses: {}, write hits: {}, write misses: {}, idle -> read inbox: {}",
                processor.id, processor.marked_objects, processor.busy_ticks,
                processor.busy_ticks as f64 / self.ticks as f64,
                processor.cache.stats.read_hits, processor.cache.stats.read_misses,
                processor.cache.stats.write_hits, processor.cache.stats.write_misses,
            processor.idle_readinbox_ticks);
            info!("[P{}] work count: {:?}", processor.id, processor.work_count);
            total_marked_objects += processor.marked_objects;
            total_busy_ticks += processor.busy_ticks;
            total_read_hits += processor.cache.stats.read_hits;
            total_read_misses += processor.cache.stats.read_misses;
            total_write_hits += processor.cache.stats.write_hits;
            total_write_misses += processor.cache.stats.write_misses;
        }
        // This is to output in a format similar to FireSim simulation
        for processor in &self.processors {
            let mut non_idle_work_count = 0;
            for (work_type, count) in &processor.work_count {
                if !matches!(work_type, NMPProcessorWorkType::Idle) {
                    non_idle_work_count += count;
                }
            }
            println!("hart {} in hart group {} finished tracing {} objects in {} cycles, {} instructions",
                processor.id, processor.id, processor.marked_objects, self.ticks, non_idle_work_count
            );
        }
        stats.insert("ticks".into(), self.ticks as f64);
        stats.insert("marked_objects.sum".into(), total_marked_objects as f64);
        stats.insert("busy_ticks.sum".into(), total_busy_ticks as f64);
        stats.insert(
            "utilization".into(),
            total_busy_ticks as f64 / (self.ticks * self.processors.len()) as f64,
        );
        stats.insert("read_hits.sum".into(), total_read_hits as f64);
        stats.insert("read_misses.sum".into(), total_read_misses as f64);
        stats.insert("write_hits.sum".into(), total_write_hits as f64);
        stats.insert("write_misses.sum".into(), total_write_misses as f64);
        stats.insert(
            "read_hit_rate".into(),
            total_read_hits as f64 / (total_read_hits + total_read_misses) as f64,
        );
        stats.insert(
            "write_hit_rate".into(),
            total_write_hits as f64 / (total_write_hits + total_write_misses) as f64,
        );
        // in ms
        stats.insert(
            "time".into(),
            self.ticks as f64 / (self.frequency_ghz * 1e6),
        );

        stats
    }

    fn events(&self) -> Vec<TracingEvent> {
        self.processors.iter().flat_map(|p| p.events()).collect()
    }
}

#[derive(Debug, Clone)]
struct NMPProcessor<const LOG_NUM_THREADS: u8> {
    id: usize,
    ticks: usize, // This is synchronized with the global ticks
    busy_ticks: usize,
    idle_readinbox_ticks: usize,
    marked_objects: usize,
    inbox: Vec<NMPMessage>,
    works: VecDeque<NMPProcessorWork>,
    stalled_work: Option<NMPProcessorWork>,
    stall_ticks: usize,
    pub(super) cache: SetAssociativeCache,
    work_count: HashMap<NMPProcessorWorkType, usize>,
    idle_ranges: Vec<(usize, usize)>,
    idle_start: Option<usize>,
    frequency_ghz: f64, // Only valid for DDR4-3200
    topology: topology::LineTopology,
    edge_chunks: Vec<(u64, u64)>,
    edge_chunk_cursor: (usize, u64),
}

impl<const LOG_NUM_THREADS: u8> NMPProcessor<LOG_NUM_THREADS> {
    fn new(id: usize, rank_option: DDR4RankOption) -> Self {
        NMPProcessor {
            id,
            busy_ticks: 0,
            marked_objects: 0,
            inbox: vec![],
            works: VecDeque::new(),
            stalled_work: None,
            stall_ticks: 0,
            ticks: 0,
            // 32 KB
            cache: SetAssociativeCache::new(64, 8, rank_option),
            work_count: HashMap::new(),
            idle_ranges: vec![],
            idle_start: None,
            frequency_ghz: 1.6,
            idle_readinbox_ticks: 0,
            topology: topology::LineTopology::new(),
            edge_chunks: vec![],
            edge_chunk_cursor: (0, 0),
        }
    }

    fn get_latency(&self, work: &NMPProcessorWork) -> usize {
        match work {
            NMPProcessorWork::Mark(o) => self.cache.write_latency(*o),
            NMPProcessorWork::Idle => 1,
            NMPProcessorWork::Load(e) => self.cache.read_latency(*e as u64),
            NMPProcessorWork::ReadInbox => 2,
            NMPProcessorWork::SendMessage(m) => {
                self.topology.get_latency(self.id as u8, m.recipient as u8)
            }
            NMPProcessorWork::ContinueScan => 1,
        }
    }

    fn locally_done(&self) -> bool {
        self.works.is_empty() && self.stalled_work.is_none() && self.inbox.is_empty()
    }

    fn to_thread_name_event(&self) -> TracingEvent {
        TracingEvent::new_threadname_event(0, self.id as u32, RankID(self.id as u8).to_string())
    }

    fn events(&self) -> Vec<TracingEvent> {
        let mut events = Vec::new();
        events.push(self.to_thread_name_event());
        let mut timestamp_cursor: usize = 0;
        let mut idle_ranges = self.idle_ranges.clone();
        if let Some(start) = self.idle_start {
            idle_ranges.push((start, self.ticks));
        }
        for (begin, end) in &idle_ranges {
            if *begin > timestamp_cursor {
                events.push(TracingEvent::new_duration_event(
                    0,
                    self.id as u32,
                    "busy".to_string(),
                    ticks_to_us(timestamp_cursor as u64, self.frequency_ghz),
                    HashMap::default(),
                    true,
                    None,
                ));
                events.push(TracingEvent::new_duration_event(
                    0,
                    self.id as u32,
                    "busy".to_string(),
                    ticks_to_us(((*begin) - 1) as u64, self.frequency_ghz),
                    HashMap::default(),
                    false,
                    None,
                ));
            }
            events.push(TracingEvent::new_duration_event(
                0,
                self.id as u32,
                "idle".to_string(),
                ticks_to_us(*begin as u64, self.frequency_ghz),
                HashMap::default(),
                true,
                None,
            ));
            events.push(TracingEvent::new_duration_event(
                0,
                self.id as u32,
                "idle".to_string(),
                ticks_to_us(*end as u64, self.frequency_ghz),
                HashMap::default(),
                false,
                None,
            ));
            timestamp_cursor = *end + 1;
        }

        // If the last idle range does not cover the end of the ticks, we add a busy event
        if timestamp_cursor < self.ticks {
            events.push(TracingEvent::new_duration_event(
                0,
                self.id as u32,
                "busy".to_string(),
                ticks_to_us(timestamp_cursor as u64, self.frequency_ghz),
                HashMap::default(),
                true,
                None,
            ));
            events.push(TracingEvent::new_duration_event(
                0,
                self.id as u32,
                "busy".to_string(),
                ticks_to_us(self.ticks as u64, self.frequency_ghz),
                HashMap::default(),
                false,
                None,
            ));
        }

        // These cause json_parser_error in Perfetto
        // events.push(TracingEvent::new_instant_event(
        //     0,
        //     self.id as u32,
        //     "Start".to_string(),
        //     0.0,
        //     HashMap::default(),
        //     InstantEventScope::Thread,
        // ));
        // events.push(TracingEvent::new_instant_event(
        //     0,
        //     self.id as u32,
        //     "Stop".to_string(),
        //     ticks_to_us(self.ticks as u64, self.frequency_ghz),
        //     HashMap::default(),
        //     InstantEventScope::Thread,
        // ));
        events
    }
}
