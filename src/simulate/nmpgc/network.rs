use super::super::memory::DimmId;
use super::topology::Topology;
use super::work::NMPMessage;
use std::collections::HashMap;

/// A message in transit through the network.
#[derive(Debug)]
struct InFlightMessage {
    message: NMPMessage,
    /// Full route of directed links to traverse.
    route: Vec<(DimmId, DimmId)>,
    /// Index of the current hop in `route`.
    current_hop: usize,
    /// Cycles remaining on the current hop.
    remaining_hop_latency: usize,
}

/// Per-directed-link statistics.
#[derive(Debug, Default, Clone)]
struct DirectedLinkStats {
    /// Total messages that have traversed this directed link.
    messages_forwarded: usize,
}

/// The network fabric that models hop-by-hop message forwarding with
/// per-link bandwidth tracking.
pub(super) const PER_HOP_LATENCY: usize = 4;
pub(super) const DIMM_TO_RANK_LATENCY: usize = 2;

#[derive(Debug)]
pub(super) struct Network {
    in_flight: Vec<InFlightMessage>,
    /// Keyed by directed link `(from_dimm, to_dimm)`.
    link_stats: HashMap<(DimmId, DimmId), DirectedLinkStats>,

    /// Per-tick flit count per directed link, used to find peak demand.
    /// Keyed by `(from_dimm, to_dimm)`, value is the count for the current tick.
    current_tick_flits: HashMap<(DimmId, DimmId), usize>,
    /// The maximum single-tick flit count observed on any directed link.
    peak_tick_flits: HashMap<(DimmId, DimmId), usize>,
}

/// Summary of bandwidth statistics for a single directed link.
#[derive(Debug, Clone)]
pub(super) struct LinkBandwidthStats {
    pub(super) from_dimm: DimmId,
    pub(super) to_dimm: DimmId,
    pub(super) messages_forwarded: usize,
    /// Peak flits (message fragments) in a single tick on this directed link.
    pub(super) peak_flits_per_tick: usize,
}

impl Network {
    pub(super) fn new(topology: &dyn Topology) -> Self {
        let mut link_stats = HashMap::new();
        let mut current_tick_flits = HashMap::new();
        let mut peak_tick_flits = HashMap::new();

        // Register both directions for each undirected link.
        for (a, b) in topology.get_links() {
            link_stats.insert((a, b), DirectedLinkStats::default());
            link_stats.insert((b, a), DirectedLinkStats::default());
            current_tick_flits.insert((a, b), 0);
            current_tick_flits.insert((b, a), 0);
            peak_tick_flits.insert((a, b), 0);
            peak_tick_flits.insert((b, a), 0);
        }

        Network {
            in_flight: Vec::new(),
            link_stats,

            current_tick_flits,
            peak_tick_flits,
        }
    }

    /// Inject a new message into the network. The route must be non-empty.
    pub(super) fn inject(&mut self, msg: NMPMessage, route: Vec<(DimmId, DimmId)>) {
        debug_assert!(!route.is_empty());
        // Record the first link traversal immediately.
        self.record_link_traversal(route[0]);
        self.in_flight.push(InFlightMessage {
            message: msg,
            route,
            current_hop: 0,
            remaining_hop_latency: PER_HOP_LATENCY,
        });
    }

    fn record_link_traversal(&mut self, link: (DimmId, DimmId)) {
        self.link_stats
            .get_mut(&link)
            .expect("link not registered in topology")
            .messages_forwarded += 1;
    }

    /// Advance all in-flight messages by one cycle.
    /// Returns messages that have arrived at their destination DIMM.
    /// The caller is responsible for adding the DIMM-to-rank latency
    /// stall on the receiving end.
    pub(super) fn tick(&mut self) -> Vec<NMPMessage> {
        // Calculate flits traversing each link in this tick.
        for count in self.current_tick_flits.values_mut() {
            *count = 0;
        }
        for msg in &self.in_flight {
            let link = msg.route[msg.current_hop];
            *self.current_tick_flits.get_mut(&link).unwrap() += 1;
        }

        // Flush per-tick counts: update peaks.
        for (link, count) in &self.current_tick_flits {
            let peak = self.peak_tick_flits.get_mut(link).unwrap();
            if *count > *peak {
                *peak = *count;
            }
        }

        let mut delivered = Vec::new();
        let mut i = 0;
        while i < self.in_flight.len() {
            self.in_flight[i].remaining_hop_latency -= 1;
            if self.in_flight[i].remaining_hop_latency == 0 {
                // Current hop complete — advance cursor.
                self.in_flight[i].current_hop += 1;
                if self.in_flight[i].current_hop >= self.in_flight[i].route.len() {
                    // Message has arrived at the destination DIMM.
                    let msg = self.in_flight.swap_remove(i);
                    delivered.push(msg.message);
                    // Don't increment i; swap_remove moved the last element here.
                } else {
                    // Move to the next hop.
                    let next_link = self.in_flight[i].route[self.in_flight[i].current_hop];
                    self.record_link_traversal(next_link);
                    self.in_flight[i].remaining_hop_latency = PER_HOP_LATENCY;
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        delivered
    }

    /// Returns true if there are no messages in flight.
    pub(super) fn is_empty(&self) -> bool {
        self.in_flight.is_empty()
    }

    /// Returns per-directed-link bandwidth statistics.
    pub(super) fn bandwidth_stats(&self) -> Vec<LinkBandwidthStats> {
        let mut stats: Vec<_> = self
            .link_stats
            .iter()
            .map(|(&(from, to), link)| LinkBandwidthStats {
                from_dimm: from,
                to_dimm: to,
                messages_forwarded: link.messages_forwarded,
                peak_flits_per_tick: *self.peak_tick_flits.get(&(from, to)).unwrap_or(&0),
            })
            .collect();
        stats.sort_by_key(|s| (s.from_dimm, s.to_dimm));
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::super::topology::LineTopology;
    use super::super::topology::Topology;
    use super::super::work::NMPMessage;
    use super::*;

    fn make_msg(recipient: usize) -> NMPMessage {
        NMPMessage::new_mark(recipient, 0x1000)
    }

    #[test]
    fn test_network_single_hop_delivery() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // DIMM 0 -> DIMM 2: single hop
        let route = topo.get_route(DimmId(0), DimmId(2));
        assert_eq!(route.len(), 1);

        net.inject(make_msg(2), route);
        assert!(!net.is_empty());

        // Tick for per_hop_latency cycles
        let hop = PER_HOP_LATENCY;
        for tick in 0..hop {
            let delivered = net.tick();
            if tick < hop - 1 {
                assert!(
                    delivered.is_empty(),
                    "should not deliver before hop latency"
                );
            } else {
                assert_eq!(delivered.len(), 1);
                assert_eq!(delivered[0].recipient, 2);
            }
        }
        assert!(net.is_empty());
    }

    #[test]
    fn test_network_multi_hop_delivery() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // DIMM 0 -> DIMM 3: 3 hops (0->2->1->3)
        let route = topo.get_route(DimmId(0), DimmId(3));
        assert_eq!(route.len(), 3);

        net.inject(make_msg(3), route);

        let hop = PER_HOP_LATENCY;
        let total_ticks = 3 * hop;
        let mut delivered_count = 0;
        for _ in 0..total_ticks {
            let delivered = net.tick();
            delivered_count += delivered.len();
        }
        assert_eq!(delivered_count, 1);
        assert!(net.is_empty());
    }

    #[test]
    fn test_network_link_stats() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // Send from DIMM 0 -> DIMM 3 (3 hops: 0->2, 2->1, 1->3)
        let route = topo.get_route(DimmId(0), DimmId(3));
        net.inject(make_msg(3), route);

        let hop = PER_HOP_LATENCY;
        for _ in 0..(3 * hop) {
            net.tick();
        }

        let stats = net.bandwidth_stats();
        // Each of the 3 directed links should have 1 message forwarded
        let fwd_02 = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(0) && s.to_dimm == DimmId(2))
            .unwrap();
        assert_eq!(fwd_02.messages_forwarded, 1);
        let fwd_21 = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(2) && s.to_dimm == DimmId(1))
            .unwrap();
        assert_eq!(fwd_21.messages_forwarded, 1);
        let fwd_13 = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(1) && s.to_dimm == DimmId(3))
            .unwrap();
        assert_eq!(fwd_13.messages_forwarded, 1);
        // Reverse directions should have 0
        let fwd_20 = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(2) && s.to_dimm == DimmId(0))
            .unwrap();
        assert_eq!(fwd_20.messages_forwarded, 0);
    }

    #[test]
    fn test_network_peak_bandwidth() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // Inject 3 messages on the same single-hop link in the same tick.
        for _ in 0..3 {
            let route = topo.get_route(DimmId(0), DimmId(2));
            net.inject(make_msg(2), route);
        }

        let hop = PER_HOP_LATENCY;
        for _ in 0..hop {
            net.tick();
        }

        let stats = net.bandwidth_stats();
        let link = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(0) && s.to_dimm == DimmId(2))
            .unwrap();
        assert_eq!(link.messages_forwarded, 3);
        // All 3 were injected in the same tick and will traverse together for 4 ticks.
        // So the peak flits per tick should be 3.
        assert_eq!(link.peak_flits_per_tick, 3);
    }

    #[test]
    fn test_network_empty_tick() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);
        assert!(net.is_empty());
        let delivered = net.tick();
        assert!(delivered.is_empty());
        assert!(net.is_empty());
    }

    #[test]
    fn test_network_concurrent_overlapping_traffic() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // Two messages crossing on link (2,1)/(1,2):
        // Message A: DIMM 0 -> DIMM 3 (route: 0->2, 2->1, 1->3)
        // Message B: DIMM 3 -> DIMM 0 (route: 3->1, 1->2, 2->0)
        let route_a = topo.get_route(DimmId(0), DimmId(3));
        let route_b = topo.get_route(DimmId(3), DimmId(0));
        net.inject(make_msg(3), route_a);
        net.inject(make_msg(0), route_b);

        let hop = PER_HOP_LATENCY;
        // Both messages are 3 hops, need 3 * hop ticks
        let mut delivered = Vec::new();
        for _ in 0..(3 * hop) {
            delivered.extend(net.tick());
        }
        assert_eq!(delivered.len(), 2);
        assert!(net.is_empty());

        let stats = net.bandwidth_stats();
        // Link (2,1): message A traverses it on hop 2
        let link_21 = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(2) && s.to_dimm == DimmId(1))
            .unwrap();
        assert_eq!(link_21.messages_forwarded, 1);
        // Link (1,2): message B traverses it on hop 2
        let link_12 = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(1) && s.to_dimm == DimmId(2))
            .unwrap();
        assert_eq!(link_12.messages_forwarded, 1);
    }

    #[test]
    fn test_network_pipelined_flits() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // Inject first message at tick 0
        let route1 = topo.get_route(DimmId(0), DimmId(2));
        net.inject(make_msg(2), route1);

        // Tick once
        net.tick();

        // Inject second message at tick 1
        let route2 = topo.get_route(DimmId(0), DimmId(2));
        net.inject(make_msg(2), route2);

        // Tick for the remaining time
        while !net.is_empty() {
            net.tick();
        }

        let stats = net.bandwidth_stats();
        let link = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(0) && s.to_dimm == DimmId(2))
            .unwrap();
        assert_eq!(link.messages_forwarded, 2);
        // The peak flits per tick should be 2, because msg1 and msg2 overlap
        // for `PER_HOP_LATENCY - 1` ticks
        assert_eq!(link.peak_flits_per_tick, 2);
    }

    #[test]
    fn test_network_separated_flits() {
        let topo = LineTopology::new();
        let mut net = Network::new(&topo);

        // Inject first message at tick 0
        let route1 = topo.get_route(DimmId(0), DimmId(2));
        net.inject(make_msg(2), route1);

        // Tick enough times for the first message to completely clear the link
        let hop = PER_HOP_LATENCY;
        for _ in 0..hop {
            net.tick();
        }

        assert!(net.is_empty());

        // Inject second message
        let route2 = topo.get_route(DimmId(0), DimmId(2));
        net.inject(make_msg(2), route2);

        while !net.is_empty() {
            net.tick();
        }

        let stats = net.bandwidth_stats();
        let link = stats
            .iter()
            .find(|s| s.from_dimm == DimmId(0) && s.to_dimm == DimmId(2))
            .unwrap();
        assert_eq!(link.messages_forwarded, 2);
        // Since they do not overlap in time, the peak flits per tick should just be 1.
        assert_eq!(link.peak_flits_per_tick, 1);
    }
}
