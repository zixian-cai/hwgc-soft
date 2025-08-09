use polars::time;

use super::SimulationArchitecture;
use crate::simulate::memory::RankID;
use crate::simulate::tracing::InstantEventScope;
use crate::util::ticks_to_us;
use crate::{simulate::memory::AddressMapping, trace::trace_object, *};
use std::collections::{HashMap, VecDeque};

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
    fn new<O: ObjectModel>(_args: &SimulationArgs, object_model: &O) -> Self {
        // Convert &[u64] into Vec<u64>
        let mut processors: Vec<NMPProcessor<LOG_NUM_THREADS>> = (0..Self::NUM_THREADS)
            .map(|id| NMPProcessor::new(id as usize))
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

    fn events(&self) -> Vec<simulate::tracing::TracingEvent> {
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
}

#[derive(Debug, Clone)]
enum NMPProcessorWork {
    Mark(u64),
    Load(*mut u64),
    Idle,
    ReadInbox,
    SendMessage(NMPMessage),
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Hash, Eq)]
enum NMPProcessorWorkType {
    Mark = 0,
    Load = 1,
    Idle = 2,
    ReadInbox = 3,
    SendMessage = 4,
}

impl NMPProcessorWork {
    fn get_type(&self) -> NMPProcessorWorkType {
        match self {
            NMPProcessorWork::Mark(_) => NMPProcessorWorkType::Mark,
            NMPProcessorWork::Load(_) => NMPProcessorWorkType::Load,
            NMPProcessorWork::Idle => NMPProcessorWorkType::Idle,
            NMPProcessorWork::ReadInbox => NMPProcessorWorkType::ReadInbox,
            NMPProcessorWork::SendMessage(_) => NMPProcessorWorkType::SendMessage,
        }
    }
}

impl<const LOG_NUM_THREADS: u8> NMPProcessor<LOG_NUM_THREADS> {
    fn new(id: usize) -> Self {
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
            cache: SetAssociativeCache::new(64, 8),
            work_count: HashMap::new(),
            idle_ranges: vec![],
            idle_start: None,
            frequency_ghz: 1.6,
            idle_readinbox_ticks: 0,
        }
    }
    fn tick<O: ObjectModel>(&mut self) -> Option<NMPMessage> {
        self.ticks += 1;

        // This is to deal with the latencies of actions that take more than one tick
        if self.stall_ticks > 0 {
            self.stall_ticks -= 1;
            self.busy_ticks += 1;
            trace!(
                "[P{}] stalled for {:?}, {} ticks left",
                self.id,
                self.stalled_work,
                self.stall_ticks
            );
            return None;
        }

        let work = if let Some(w) = self.stalled_work.take() {
            trace!("[P{}] executing previously stalled work: {:?}", self.id, w);
            // Act on the stalled work
            w
        } else {
            if let Some(w) = self.works.pop_front() {
                if self.get_latency(&w) > 1 {
                    // If the work takes more than one tick, stall it
                    self.stall_ticks = self.get_latency(&w) - 1; // -1 because we are already in this tick
                    self.stalled_work = Some(w);
                    trace!(
                        "[P{}] stalling work: {:?}, {} ticks left",
                        self.id,
                        self.stalled_work,
                        self.stall_ticks
                    );
                    return None;
                } else {
                    w
                }
            } else {
                NMPProcessorWork::Idle
            }
        };

        if !matches!(work, NMPProcessorWork::Idle) {
            self.busy_ticks += 1;
        }

        if !(matches!(work, NMPProcessorWork::Idle) || matches!(work, NMPProcessorWork::ReadInbox))
        {
            // This processor is doing productive work now
            if let Some(start) = self.idle_start.take() {
                self.idle_ranges.push((start, self.ticks - 1));
            }
        }

        let mut ret = None;
        self.work_count
            .entry(work.get_type())
            .and_modify(|e| *e += 1)
            .or_insert(1);
        match work {
            NMPProcessorWork::Mark(o) => {
                trace!("[P{}] marking object {}", self.id, o);
                if unsafe { trace_object(o, 1) } {
                    self.cache.write(o);
                    self.marked_objects += 1;
                    O::scan_object(o, |edge, repeat| {
                        for i in 0..repeat {
                            let e = edge.wrapping_add(i as usize);
                            let owner = NMPGC::<LOG_NUM_THREADS>::get_owner_processor(e as u64);
                            if owner == self.id {
                                self.works.push_back(NMPProcessorWork::Load(e));
                            } else {
                                self.works
                                    .push_back(NMPProcessorWork::SendMessage(NMPMessage {
                                        recipient: owner,
                                        work: NMPMessageWork::Load(e),
                                    }));
                            }
                        }
                    });
                }
            }
            NMPProcessorWork::Load(e) => {
                let child = unsafe { *e };
                self.cache.read(e as u64);
                if child != 0 {
                    let owner = NMPGC::<LOG_NUM_THREADS>::get_owner_processor(child);
                    if owner == self.id {
                        self.works.push_back(NMPProcessorWork::Mark(child));
                    } else {
                        let msg = NMPMessage {
                            recipient: owner,
                            work: NMPMessageWork::Mark(child),
                        };
                        self.works.push_back(NMPProcessorWork::SendMessage(msg));
                    }
                }
            }
            NMPProcessorWork::Idle => {
                if !self.inbox.is_empty() {
                    self.idle_readinbox_ticks += 1;
                    self.works.push_back(NMPProcessorWork::ReadInbox);
                } else {
                    // This process is truly idle
                    if self.idle_start.is_none() {
                        self.idle_start = Some(self.ticks);
                    }
                }
            }
            NMPProcessorWork::SendMessage(msg) => {
                trace!(
                    "[P{}] sending message to P{}: {:?}",
                    self.id,
                    msg.recipient,
                    msg.work
                );
                ret = Some(msg);
            }
            NMPProcessorWork::ReadInbox => {
                if let Some(msg) = self.inbox.pop() {
                    trace!("[P{}] reading inbox message: {:?}", self.id, msg);
                    match msg.work {
                        NMPMessageWork::Load(e) => {
                            self.works.push_back(NMPProcessorWork::Load(e));
                        }
                        NMPMessageWork::Mark(o) => {
                            self.works.push_back(NMPProcessorWork::Mark(o));
                        }
                    }
                }
            }
        }
        trace!(
            "[P{}] work count: {:?}, inbox count: {:?}, stalled_work: {:?}, marked_objects: {:?}",
            self.id,
            self.works.len(),
            self.inbox.len(),
            self.stalled_work,
            self.marked_objects
        );
        ret
    }

    fn get_latency(&self, work: &NMPProcessorWork) -> usize {
        match work {
            NMPProcessorWork::Mark(o) => self.cache.write_latency(*o),
            NMPProcessorWork::Idle => 1,
            NMPProcessorWork::Load(e) => self.cache.read_latency(*e as u64),
            NMPProcessorWork::ReadInbox => 2,
            NMPProcessorWork::SendMessage(_) => 10,
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

#[derive(Debug, Clone)]
/// Each processor generates at most one message per tick
struct NMPMessage {
    recipient: usize,
    work: NMPMessageWork,
}

#[derive(Debug, Clone)]
enum NMPMessageWork {
    Mark(u64),
    Load(*mut u64),
}
