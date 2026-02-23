use super::SimulationArchitecture;
use crate::simulate::memory::RankID;
use crate::simulate::memory::{AddressMapping, DDR4RankOption};
use crate::util::ticks_to_us;
use crate::{ObjectModel, SimulationArgs};
use std::collections::{HashMap, VecDeque};

mod network;
mod topology;
mod work;
use network::Network;
use topology::Topology;
use work::{NMPMessage, NMPProcessorWork, NMPProcessorWorkType};

use super::memory::SetAssociativeCache;
use super::tracing::TracingEvent;

#[allow(clippy::upper_case_acronyms)]
pub(crate) struct NMPGC<const LOG_NUM_THREADS: u8> {
    processors: Vec<NMPProcessor<LOG_NUM_THREADS>>,
    ticks: usize,
    frequency_ghz: f64,
    topology: Box<dyn Topology>,
    network: Network,
}

impl<const LOG_NUM_THREADS: u8> NMPGC<LOG_NUM_THREADS> {
    const NUM_THREADS: u64 = 1u64 << LOG_NUM_THREADS;
    fn format_thousands(mut n: usize) -> String {
        if n == 0 {
            return "0".to_string();
        }
        let mut s = String::new();
        while n > 0 {
            let rem = n % 1000;
            n /= 1000;
            if n > 0 {
                s.insert_str(0, &format!(",{:03}", rem));
            } else {
                s.insert_str(0, &format!("{}", rem));
            }
        }
        s
    }

    fn get_owner_processor(o: u64) -> usize {
        let mapping = AddressMapping(o);
        mapping.get_owner_id()
    }
}

impl<const LOG_NUM_THREADS: u8> SimulationArchitecture for NMPGC<LOG_NUM_THREADS> {
    fn new<O: ObjectModel>(args: &SimulationArgs, object_model: &O) -> Self {
        let rank_option = if args.use_dramsim3 {
            DDR4RankOption::DRAMsim3 {
                config_file: args.dramsim3_config.clone(),
                output_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            }
        } else {
            DDR4RankOption::Naive
        };

        let topology: Box<dyn Topology> = match args.topology {
            crate::cli::TopologyChoice::Line => Box::new(topology::LineTopology::new()),
            crate::cli::TopologyChoice::Ring => Box::new(topology::RingTopology::new()),
        };
        let network = Network::new(&*topology);
        let dimm_to_rank_latency = network::DIMM_TO_RANK_LATENCY;

        // Convert &[u64] into Vec<u64>
        let mut processors: Vec<NMPProcessor<LOG_NUM_THREADS>> = (0..Self::NUM_THREADS)
            .map(|id| NMPProcessor::new(id as usize, rank_option.clone(), dimm_to_rank_latency))
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
            topology,
            network,
        }
    }

    fn tick<O: ObjectModel>(&mut self) -> bool {
        self.ticks += 1;
        let mut messages = Vec::new();

        for p in &mut self.processors {
            let msg = p.tick::<O>();
            if let Some(m) = msg {
                messages.push((p.id, m));
            }
        }

        // Inject outgoing messages into the network fabric.
        for (sender_id, msg) in messages {
            let sender_rank = RankID(sender_id as u8);
            let recipient_rank = RankID(msg.recipient as u8);
            let mut sender_dimm = RankID(sender_rank.0);
            sender_dimm.set_rank(0);
            let mut recipient_dimm = RankID(recipient_rank.0);
            recipient_dimm.set_rank(0);

            if sender_dimm == recipient_dimm {
                // Same DIMM: deliver directly (no network traversal needed).
                self.processors[msg.recipient].inbox.push(msg);
            } else {
                let route = self.topology.get_route(sender_dimm.0, recipient_dimm.0);
                self.network.inject(msg, route);
            }
        }

        // Tick the network: advance in-flight messages.
        let delivered = self.network.tick();
        for msg in delivered {
            self.processors[msg.recipient].inbox.push(msg);
        }

        // Check if all processors are done AND no messages in flight.
        // FIXME: this assumes magical global knowledge, but
        // this actually requires a distributed termination detection algorithm
        let all_done = self.processors.iter().all(|p| p.locally_done()) && self.network.is_empty();
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
                if !matches!(
                    work_type,
                    NMPProcessorWorkType::Idle | NMPProcessorWorkType::Stall
                ) {
                    // Count what would logically be consindered as instructions
                    // Excluding stalls and idle
                    non_idle_work_count += count;
                }
            }
            println!("hart {} in hart group {} finished tracing {} objects in {} cycles, {} instructions",
                processor.id, processor.id, processor.marked_objects, self.ticks, non_idle_work_count
            );
        }

        // Network bandwidth stats (8 B per message, i.e. a 64-bit address)
        const MESSAGE_SIZE_BYTES: f64 = 8.0;
        let total_time_s = self.ticks as f64 / (self.frequency_ghz * 1e9);
        for link in self.network.bandwidth_stats() {
            let key_prefix = format!("link_{}_to_{}", link.from_dimm, link.to_dimm);
            stats.insert(
                format!("{}.messages_forwarded", key_prefix),
                link.messages_forwarded as f64,
            );
            stats.insert(
                format!("{}.peak_messages_per_tick", key_prefix),
                link.peak_messages_per_tick as f64,
            );
            // Peak throughput demand in GB/s: peak_messages_per_tick * 64B * freq_ghz
            let peak_gbps =
                link.peak_messages_per_tick as f64 * MESSAGE_SIZE_BYTES * self.frequency_ghz;
            stats.insert(format!("{}.peak_throughput_gbps", key_prefix), peak_gbps);
            // Average throughput in GB/s
            if total_time_s > 0.0 {
                let avg_gbps =
                    link.messages_forwarded as f64 * MESSAGE_SIZE_BYTES / total_time_s / 1e9;
                stats.insert(format!("{}.avg_throughput_gbps", key_prefix), avg_gbps);
            }
            info!(
                "[Network] link DIMM{} -> DIMM{}: {} messages forwarded, peak {}/tick ({:.3} GB/s)",
                link.from_dimm,
                link.to_dimm,
                Self::format_thousands(link.messages_forwarded),
                link.peak_messages_per_tick,
                peak_gbps,
            );
        }

        // Compute aggregate stats
        let utilization = total_busy_ticks as f64 / (self.ticks * self.processors.len()) as f64;
        let read_hit_rate = total_read_hits as f64 / (total_read_hits + total_read_misses) as f64;
        let write_hit_rate =
            total_write_hits as f64 / (total_write_hits + total_write_misses) as f64;
        let time_ms = self.ticks as f64 / (self.frequency_ghz * 1e6);

        // Human-readable summary
        println!("######################### Human-Readable Summary ##########################");
        println!("Timing & Utilization:");
        println!(
            "  Ticks:              {}",
            Self::format_thousands(self.ticks)
        );
        println!("  Time:               {:.3} ms", time_ms);
        println!(
            "  Total marked objs:  {}",
            Self::format_thousands(total_marked_objects)
        );
        println!(
            "  Total busy ticks:   {}",
            Self::format_thousands(total_busy_ticks)
        );
        println!("  Utilization:        {:.3}", utilization);
        println!();
        println!("Cache (aggregate):");
        println!(
            "  Read hits:    {:>10}    Read misses:  {:>10}    Hit rate: {:.3}",
            Self::format_thousands(total_read_hits),
            Self::format_thousands(total_read_misses),
            read_hit_rate
        );
        println!(
            "  Write hits:   {:>10}    Write misses: {:>10}    Hit rate: {:.3}",
            Self::format_thousands(total_write_hits),
            Self::format_thousands(total_write_misses),
            write_hit_rate
        );
        println!();
        println!("Per-Processor:");
        println!(
            "  {:<4} {:>10} {:>10} {:>8} {:>10} {:>10} {:>10} {:>10}",
            "P", "Marked", "Busy", "Util", "RdHit", "RdMiss", "WrHit", "WrMiss"
        );
        for p in &self.processors {
            println!(
                "  {:<4} {:>10} {:>10} {:>8.3} {:>10} {:>10} {:>10} {:>10}",
                p.id,
                Self::format_thousands(p.marked_objects),
                Self::format_thousands(p.busy_ticks),
                p.busy_ticks as f64 / self.ticks as f64,
                Self::format_thousands(p.cache.stats.read_hits),
                Self::format_thousands(p.cache.stats.read_misses),
                Self::format_thousands(p.cache.stats.write_hits),
                Self::format_thousands(p.cache.stats.write_misses)
            );
        }
        println!();
        self.topology.print_diagram();
        println!();
        println!("Network Links:");
        println!(
            "  {:<16} {:>10} {:>10} {:>12} {:>12}",
            "Link", "Msgs Fwd", "Peak/Tick", "Peak GB/s", "Avg GB/s"
        );
        // Sort link stats by physical connection order.
        let mut link_stats = self.network.bandwidth_stats();
        link_stats.sort_by_key(|s| self.topology.link_sort_key(s.from_dimm, s.to_dimm));
        for link in &link_stats {
            let peak_gbps =
                link.peak_messages_per_tick as f64 * MESSAGE_SIZE_BYTES * self.frequency_ghz;
            let avg_gbps = if total_time_s > 0.0 {
                link.messages_forwarded as f64 * MESSAGE_SIZE_BYTES / total_time_s / 1e9
            } else {
                0.0
            };
            println!(
                "  DIMM{} -> DIMM{}    {:>10} {:>10} {:>12.3} {:>12.3}",
                link.from_dimm,
                link.to_dimm,
                Self::format_thousands(link.messages_forwarded),
                link.peak_messages_per_tick,
                peak_gbps,
                avg_gbps
            );
        }
        println!("######################### End Human-Readable Summary ######################");

        stats.insert("ticks".into(), self.ticks as f64);
        stats.insert("marked_objects.sum".into(), total_marked_objects as f64);
        stats.insert("busy_ticks.sum".into(), total_busy_ticks as f64);
        stats.insert("utilization".into(), utilization);
        stats.insert("read_hits.sum".into(), total_read_hits as f64);
        stats.insert("read_misses.sum".into(), total_read_misses as f64);
        stats.insert("write_hits.sum".into(), total_write_hits as f64);
        stats.insert("write_misses.sum".into(), total_write_misses as f64);
        stats.insert("read_hit_rate".into(), read_hit_rate);
        stats.insert("write_hit_rate".into(), write_hit_rate);
        // in ms
        stats.insert("time".into(), time_ms);

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
    pub(super) cache: SetAssociativeCache,
    work_count: HashMap<NMPProcessorWorkType, usize>,
    idle_ranges: Vec<(usize, usize)>,
    idle_start: Option<usize>,
    frequency_ghz: f64, // Only valid for DDR4-3200
    /// Local overhead for handing a message to the DIMM link controller.
    dimm_to_rank_latency: usize,
    edge_chunks: Vec<(u64, u64)>,
    edge_chunk_cursor: (usize, u64),
}

impl<const LOG_NUM_THREADS: u8> NMPProcessor<LOG_NUM_THREADS> {
    fn new(id: usize, rank_option: DDR4RankOption, dimm_to_rank_latency: usize) -> Self {
        NMPProcessor {
            id,
            busy_ticks: 0,
            marked_objects: 0,
            inbox: vec![],
            works: VecDeque::new(),
            ticks: 0,
            // 32 KB
            cache: SetAssociativeCache::new(64, 8, rank_option),
            work_count: HashMap::new(),
            idle_ranges: vec![],
            idle_start: None,
            frequency_ghz: 1.6,
            idle_readinbox_ticks: 0,
            dimm_to_rank_latency,
            edge_chunks: vec![],
            edge_chunk_cursor: (0, 0),
        }
    }

    fn locally_done(&self) -> bool {
        self.works.is_empty() && self.inbox.is_empty()
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
