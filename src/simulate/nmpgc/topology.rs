use super::super::memory::RankID;
use std::fmt::Debug;

pub(super) trait Topology: Debug {
    /// Total end-to-end latency between two ranks (includes DIMM-to-rank on both ends).
    #[allow(dead_code)]
    fn get_latency(&self, from: u8, to: u8) -> usize;

    /// Returns the ordered sequence of directed DIMM-to-DIMM links a message must traverse.
    /// Each element is `(from_dimm, to_dimm)`.
    fn get_route(&self, from_dimm: u8, to_dimm: u8) -> Vec<(u8, u8)>;

    /// Latency in cycles for a message to traverse one link (one hop).
    fn get_per_hop_latency(&self) -> usize;

    /// Latency from the DIMM link controller to a rank on that DIMM.
    fn get_dimm_to_rank_latency(&self) -> usize;

    /// All unique undirected links in the topology, each represented as `(a, b)` with `a < b`.
    fn get_links(&self) -> Vec<(u8, u8)>;

    /// Number of DIMMs in the topology.
    #[allow(dead_code)]
    fn get_num_dimms(&self) -> u8;

    /// Prints a human-readable connection diagram showing DIMMs, their
    /// ranks, and how they are connected.
    fn print_diagram(&self);

    /// Returns a sort key for a directed link so that link stats can be
    /// printed in physical arrangement order. The key is `(group, is_reverse)`
    /// where `group` identifies adjacent physical links and `is_reverse`
    /// orders the forward direction before reverse.
    fn link_sort_key(&self, from_dimm: u8, to_dimm: u8) -> (usize, bool);
}

/// Builds a label for a DIMM showing its ID and the processor/rank IDs on it.
fn dimm_label(dimm_id: u8) -> String {
    let mut ranks = Vec::new();
    for rank_bit in 0..2u8 {
        let mut rid = RankID(0);
        rid.set_channel(dimm_id & 1);
        rid.set_dimm((dimm_id >> 1) & 1);
        rid.set_rank(rank_bit);
        ranks.push(format!("P{}", rid.0));
    }
    format!("DIMM{}({})", dimm_id, ranks.join(","))
}

// ─── Line Topology ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct LineTopology {
    /// Latency between DIMMs (kept for backwards-compatible `get_latency`)
    #[allow(dead_code)]
    dimm_latency_matrix: [[usize; 4]; 4],
    /// DIMM ordering along the line: position_of[dimm_id] gives its index.
    pub(super) position_of: [usize; 4],
    /// Inverse of `position_of`: dimm_at[position] gives the DIMM id.
    dimm_at: [u8; 4],
}

impl LineTopology {
    const DIMM_TO_RANK_LATENCY: usize = 2;
    const PER_HOP_LATENCY: usize = 4;

    pub(super) fn new() -> Self {
        // 0: channel 0, dimm 0,  1: channel 1, dimm 0,
        // 2: channel 0, dimm 1,  3: channel 1, dimm 1
        // Line order: 0 <-> 2 <-> 1 <-> 3
        let dimm_latency_matrix = [[0, 8, 4, 12], [8, 0, 4, 4], [4, 4, 0, 8], [12, 4, 8, 0]];
        let dimm_at = [0, 2, 1, 3];
        let mut position_of = [0usize; 4];
        for (pos, &dimm) in dimm_at.iter().enumerate() {
            position_of[dimm as usize] = pos;
        }
        LineTopology {
            dimm_latency_matrix,
            position_of,
            dimm_at,
        }
    }
}

impl Debug for LineTopology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LineTopology")
    }
}

impl Topology for LineTopology {
    fn get_latency(&self, from: u8, to: u8) -> usize {
        debug_assert_ne!(from, to);
        let from_id = RankID(from);
        let to_id = RankID(to);
        let mut from_dimm = RankID(from_id.0);
        from_dimm.set_rank(0);
        let mut to_dimm = RankID(to_id.0);
        to_dimm.set_rank(0);

        if from_dimm == to_dimm {
            return Self::DIMM_TO_RANK_LATENCY;
        }

        let between_dimm_latency =
            self.dimm_latency_matrix[from_dimm.0 as usize][to_dimm.0 as usize];
        between_dimm_latency + Self::DIMM_TO_RANK_LATENCY * 2
    }

    fn get_route(&self, from_dimm: u8, to_dimm: u8) -> Vec<(u8, u8)> {
        debug_assert_ne!(from_dimm, to_dimm);
        let from_pos = self.position_of[from_dimm as usize];
        let to_pos = self.position_of[to_dimm as usize];

        let mut route = Vec::new();
        if from_pos < to_pos {
            for i in from_pos..to_pos {
                route.push((self.dimm_at[i], self.dimm_at[i + 1]));
            }
        } else {
            for i in (to_pos..from_pos).rev() {
                route.push((self.dimm_at[i + 1], self.dimm_at[i]));
            }
        }
        route
    }

    fn get_per_hop_latency(&self) -> usize {
        Self::PER_HOP_LATENCY
    }

    fn get_dimm_to_rank_latency(&self) -> usize {
        Self::DIMM_TO_RANK_LATENCY
    }

    fn get_links(&self) -> Vec<(u8, u8)> {
        let mut links = Vec::new();
        for i in 0..self.dimm_at.len() - 1 {
            let a = self.dimm_at[i];
            let b = self.dimm_at[i + 1];
            links.push((a.min(b), a.max(b)));
        }
        links
    }

    fn get_num_dimms(&self) -> u8 {
        self.dimm_at.len() as u8
    }

    fn print_diagram(&self) {
        println!("Topology (Line):");
        let labels: Vec<String> = self.dimm_at.iter().map(|&d| dimm_label(d)).collect();
        println!("  {}", labels.join(" <-> "));
    }

    fn link_sort_key(&self, from_dimm: u8, to_dimm: u8) -> (usize, bool) {
        let from_pos = self.position_of[from_dimm as usize];
        let to_pos = self.position_of[to_dimm as usize];
        let min_pos = from_pos.min(to_pos);
        let is_reverse = from_pos > to_pos;
        (min_pos, is_reverse)
    }
}

// ─── Ring Topology ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct RingTopology {
    /// DIMM ordering around the ring.
    dimm_at: [u8; 4],
    /// Inverse: position_of[dimm_id] gives its index in the ring.
    pub(super) position_of: [usize; 4],
}

impl RingTopology {
    const DIMM_TO_RANK_LATENCY: usize = 2;
    const PER_HOP_LATENCY: usize = 4;
    const N: usize = 4;

    pub(super) fn new() -> Self {
        // Same DIMM ordering as LineTopology, but with a wrap-around link.
        // Ring: 0 <-> 2 <-> 1 <-> 3 <-> 0
        let dimm_at = [0, 2, 1, 3];
        let mut position_of = [0usize; 4];
        for (pos, &dimm) in dimm_at.iter().enumerate() {
            position_of[dimm as usize] = pos;
        }
        RingTopology {
            dimm_at,
            position_of,
        }
    }
}

impl Debug for RingTopology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RingTopology")
    }
}

impl Topology for RingTopology {
    fn get_latency(&self, from: u8, to: u8) -> usize {
        debug_assert_ne!(from, to);
        let mut from_dimm = RankID(from);
        from_dimm.set_rank(0);
        let mut to_dimm = RankID(to);
        to_dimm.set_rank(0);

        if from_dimm == to_dimm {
            return Self::DIMM_TO_RANK_LATENCY;
        }

        let hops = self.get_route(from_dimm.0, to_dimm.0).len();
        hops * Self::PER_HOP_LATENCY + 2 * Self::DIMM_TO_RANK_LATENCY
    }

    fn get_route(&self, from_dimm: u8, to_dimm: u8) -> Vec<(u8, u8)> {
        debug_assert_ne!(from_dimm, to_dimm);
        let from_pos = self.position_of[from_dimm as usize];
        let to_pos = self.position_of[to_dimm as usize];
        let n = Self::N;

        // Clockwise distance (from_pos -> to_pos going forward)
        let cw_dist = (to_pos + n - from_pos) % n;
        // Counter-clockwise distance
        let ccw_dist = (from_pos + n - to_pos) % n;

        let mut route = Vec::new();
        if cw_dist <= ccw_dist {
            // Go clockwise
            for step in 0..cw_dist {
                let cur = (from_pos + step) % n;
                let next = (from_pos + step + 1) % n;
                route.push((self.dimm_at[cur], self.dimm_at[next]));
            }
        } else {
            // Go counter-clockwise
            for step in 0..ccw_dist {
                let cur = (from_pos + n - step) % n;
                let next = (from_pos + n - step - 1) % n;
                route.push((self.dimm_at[cur], self.dimm_at[next]));
            }
        }
        route
    }

    fn get_per_hop_latency(&self) -> usize {
        Self::PER_HOP_LATENCY
    }

    fn get_dimm_to_rank_latency(&self) -> usize {
        Self::DIMM_TO_RANK_LATENCY
    }

    fn get_links(&self) -> Vec<(u8, u8)> {
        let n = Self::N;
        let mut links = Vec::new();
        for i in 0..n {
            let a = self.dimm_at[i];
            let b = self.dimm_at[(i + 1) % n];
            links.push((a.min(b), a.max(b)));
        }
        links
    }

    fn get_num_dimms(&self) -> u8 {
        Self::N as u8
    }

    fn print_diagram(&self) {
        println!("Topology (Ring):");
        let labels: Vec<String> = self.dimm_at.iter().map(|&d| dimm_label(d)).collect();
        // Show ring with wrap-around
        println!(
            "  {} <-> {}",
            labels.join(" <-> "),
            dimm_label(self.dimm_at[0])
        );
    }

    fn link_sort_key(&self, from_dimm: u8, to_dimm: u8) -> (usize, bool) {
        let from_pos = self.position_of[from_dimm as usize];
        let to_pos = self.position_of[to_dimm as usize];
        let n = Self::N;
        // Check if this is a wrap-around link (positions 0 and N-1)
        let is_wrap = (from_pos == 0 && to_pos == n - 1) || (from_pos == n - 1 && to_pos == 0);
        let min_pos = if is_wrap {
            n - 1 // Sort wrap-around link last
        } else {
            from_pos.min(to_pos)
        };
        let is_reverse = if is_wrap {
            from_pos == 0 // 0->3 is the reverse direction for the wrap link
        } else {
            from_pos > to_pos
        };
        (min_pos, is_reverse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Line Topology Tests ────────────────────────────────────────────

    #[test]
    fn test_line_topology_latency_same_dimm() {
        let topology = LineTopology::new();
        let from = RankID(1);
        let mut to = RankID(1);
        to.set_rank(1);
        assert_ne!(from, to);
        assert_eq!(
            topology.get_latency(from.0, to.0),
            LineTopology::DIMM_TO_RANK_LATENCY
        );
    }

    #[test]
    fn test_line_topology_latency_different_dimms() {
        let topology = LineTopology::new();
        // 0 -> 2 -> 1
        let latency = topology.get_latency(0, 1);
        assert_eq!(latency, 8 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
    }

    #[test]
    fn test_line_topology_latency_reverse_path() {
        let topology = LineTopology::new();
        // 1 -> 2 -> 0
        let latency = topology.get_latency(1, 0);
        assert_eq!(latency, 8 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
    }

    #[test]
    #[should_panic]
    fn test_line_topology_latency_same_rank() {
        let topology = LineTopology::new();
        topology.get_latency(0, 0); // Should panic due to debug_assert_ne!
    }

    #[test]
    fn test_line_topology_single_hop() {
        let topology = LineTopology::new();
        // 0 -> 2
        let latency = topology.get_latency(0, 2);
        assert_eq!(latency, 4 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
    }

    #[test]
    fn test_line_topology_three_hops() {
        let topology = LineTopology::new();
        // 0 -> 2 -> 1 -> 3
        let latency = topology.get_latency(0, 3);
        assert_eq!(latency, 12 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
    }

    #[test]
    fn test_line_topology_route_adjacent() {
        let topology = LineTopology::new();
        // DIMM 0 -> DIMM 2 (adjacent in line order)
        let route = topology.get_route(0, 2);
        assert_eq!(route, vec![(0, 2)]);
    }

    #[test]
    fn test_line_topology_route_two_hops() {
        let topology = LineTopology::new();
        // DIMM 0 -> DIMM 1: line is 0-2-1-3, so route is 0->2, 2->1
        let route = topology.get_route(0, 1);
        assert_eq!(route, vec![(0, 2), (2, 1)]);
    }

    #[test]
    fn test_line_topology_route_three_hops() {
        let topology = LineTopology::new();
        // DIMM 0 -> DIMM 3: 0->2->1->3
        let route = topology.get_route(0, 3);
        assert_eq!(route, vec![(0, 2), (2, 1), (1, 3)]);
    }

    #[test]
    fn test_line_topology_route_reverse() {
        let topology = LineTopology::new();
        // DIMM 3 -> DIMM 0: 3->1->2->0
        let route = topology.get_route(3, 0);
        assert_eq!(route, vec![(3, 1), (1, 2), (2, 0)]);
    }

    #[test]
    fn test_line_topology_links() {
        let topology = LineTopology::new();
        let mut links = topology.get_links();
        links.sort();
        // Line: 0-2-1-3 → links (0,2), (1,2), (1,3)
        assert_eq!(links, vec![(0, 2), (1, 2), (1, 3)]);
    }

    #[test]
    fn test_line_topology_route_consistency() {
        let topology = LineTopology::new();
        // Verify that route hop count × per-hop latency equals the DIMM-to-DIMM
        // portion of get_latency (i.e. get_latency minus the two DIMM-to-rank ends).
        for from in 0u8..4 {
            for to in 0u8..4 {
                if from == to {
                    continue;
                }
                let route = topology.get_route(from, to);
                let route_latency = route.len() * topology.get_per_hop_latency();
                let total_latency = route_latency + 2 * topology.get_dimm_to_rank_latency();

                let matrix_latency = topology.dimm_latency_matrix[from as usize][to as usize]
                    + 2 * LineTopology::DIMM_TO_RANK_LATENCY;

                assert_eq!(
                    total_latency, matrix_latency,
                    "Mismatch for route {} -> {}: route gives {}, matrix gives {}",
                    from, to, total_latency, matrix_latency
                );
            }
        }
    }

    // ─── Ring Topology Tests ────────────────────────────────────────────

    #[test]
    fn test_ring_topology_links() {
        let topology = RingTopology::new();
        let mut links = topology.get_links();
        links.sort();
        // Ring: 0-2-1-3-0 → 4 links: (0,2), (1,2), (1,3), (0,3)
        assert_eq!(links, vec![(0, 2), (0, 3), (1, 2), (1, 3)]);
    }

    #[test]
    fn test_ring_topology_route_adjacent() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 2: adjacent in ring (position 0 -> 1), 1 hop
        let route = topology.get_route(0, 2);
        assert_eq!(route, vec![(0, 2)]);
    }

    #[test]
    fn test_ring_topology_route_opposite() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 1: positions 0 and 2, equidistant (2 hops each way).
        // Clockwise: 0->2->1 (2 hops), CCW: 0->3->1 (2 hops).
        // With tie-break favoring clockwise, route is 0->2, 2->1.
        let route = topology.get_route(0, 1);
        assert_eq!(route.len(), 2);
        assert_eq!(route, vec![(0, 2), (2, 1)]);
    }

    #[test]
    fn test_ring_topology_shortest_path() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 3: positions 0 and 3.
        // Clockwise: 0->2->1->3 (3 hops), CCW: 0->3 (1 hop).
        // Should take the shorter counter-clockwise route.
        let route = topology.get_route(0, 3);
        assert_eq!(route, vec![(0, 3)]);
    }

    #[test]
    fn test_ring_topology_shortest_path_reverse() {
        let topology = RingTopology::new();
        // DIMM 3 -> DIMM 0: positions 3 and 0.
        // Clockwise: 3->0 (1 hop), CCW: 3->1->2->0 (3 hops).
        let route = topology.get_route(3, 0);
        assert_eq!(route, vec![(3, 0)]);
    }

    #[test]
    fn test_ring_topology_latency_adjacent() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 2: 1 hop
        let latency = topology.get_latency(0, 2);
        assert_eq!(
            latency,
            RingTopology::PER_HOP_LATENCY + 2 * RingTopology::DIMM_TO_RANK_LATENCY
        );
    }

    #[test]
    fn test_ring_topology_latency_wrap_around() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 3: 1 hop (via wrap-around)
        let latency = topology.get_latency(0, 3);
        assert_eq!(
            latency,
            RingTopology::PER_HOP_LATENCY + 2 * RingTopology::DIMM_TO_RANK_LATENCY
        );
    }

    #[test]
    fn test_ring_topology_max_hops_is_two() {
        let topology = RingTopology::new();
        // With 4 DIMMs in a ring, the maximum shortest path is 2 hops.
        for from in 0u8..4 {
            for to in 0u8..4 {
                if from == to {
                    continue;
                }
                let route = topology.get_route(from, to);
                assert!(
                    route.len() <= 2,
                    "Route {} -> {} has {} hops, expected <= 2",
                    from,
                    to,
                    route.len()
                );
            }
        }
    }

    #[test]
    fn test_ring_topology_route_symmetry() {
        let topology = RingTopology::new();
        // All routes should have the same hop count in both directions.
        for from in 0u8..4 {
            for to in 0u8..4 {
                if from == to {
                    continue;
                }
                let fwd = topology.get_route(from, to);
                let rev = topology.get_route(to, from);
                assert_eq!(
                    fwd.len(),
                    rev.len(),
                    "Asymmetric route lengths for {} <-> {}: {} vs {}",
                    from,
                    to,
                    fwd.len(),
                    rev.len()
                );
            }
        }
    }

    #[test]
    fn test_ring_topology_same_dimm_latency() {
        let topology = RingTopology::new();
        let from = RankID(1);
        let mut to = RankID(1);
        to.set_rank(1);
        assert_eq!(
            topology.get_latency(from.0, to.0),
            RingTopology::DIMM_TO_RANK_LATENCY
        );
    }
}
