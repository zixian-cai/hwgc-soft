use super::super::memory::{DimmId, RankId};
use std::fmt::Debug;

pub(super) trait Topology: Debug {
    /// Returns the ordered sequence of directed DIMM-to-DIMM links a message must traverse.
    /// Each element is `(from_dimm, to_dimm)`.
    fn get_route(&self, from_dimm: DimmId, to_dimm: DimmId) -> Vec<(DimmId, DimmId)>;
    /// All unique undirected links in the topology, each represented as `(a, b)` with `a < b`.
    fn get_links(&self) -> Vec<(DimmId, DimmId)>;

    /// Number of DIMMs in the topology.
    #[allow(dead_code)]
    fn get_num_dimms(&self) -> u8;

    /// Prints a human-readable connection diagram showing DIMMs, their
    /// ranks, and how they are connected.
    fn print_diagram(&self) {
        let n = self.get_num_dimms();
        let links = self.get_links();
        let mut adj = vec![Vec::new(); n as usize];
        for &(u, v) in &links {
            adj[u.0 as usize].push(v.0);
            adj[v.0 as usize].push(u.0);
        }

        println!("Topology ({:?}):", self);
        for u in 0..n {
            let mut neighbors = adj[u as usize].clone();
            neighbors.sort();
            let neighbor_labels: Vec<String> =
                neighbors.iter().map(|&v| format!("DIMM{}", v)).collect();
            println!(
                "  {} <-> [{}]",
                dimm_label(DimmId(u)),
                neighbor_labels.join(", ")
            );
        }
    }

    /// Returns a sort key for a directed link so that link stats can be
    /// printed in physical arrangement order. The key is `(group, is_reverse)`
    /// where `group` identifies adjacent physical links and `is_reverse`
    /// orders the forward direction before reverse.
    fn link_sort_key(&self, from_dimm: DimmId, to_dimm: DimmId) -> (usize, bool);
}

/// Builds a label for a DIMM showing its ID, physical location, and the processor/rank IDs on it.
fn dimm_label(dimm_id: DimmId) -> String {
    let mut ranks = Vec::new();
    for rank_bit in 0..2u8 {
        let mut rid = RankId(0);
        rid.set_channel(dimm_id.channel());
        rid.set_dimm(dimm_id.dimm());
        rid.set_rank(rank_bit);
        ranks.push(format!("P{}", rid.0));
    }
    format!("DIMM{} ({}) [{}]", dimm_id.0, dimm_id, ranks.join(","))
}

// ─── Line Topology ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct LineTopology {
    /// DIMM ordering along the line: position_of[dimm_id] gives its index.
    pub(super) position_of: [usize; 4],
    /// Inverse of `position_of`: dimm_at[position] gives the DIMM id.
    dimm_at: [DimmId; 4],
}

impl LineTopology {
    pub(super) fn new() -> Self {
        // 0: channel 0, dimm 0,  1: channel 1, dimm 0,
        // 2: channel 0, dimm 1,  3: channel 1, dimm 1
        // Line order: 0 <-> 2 <-> 1 <-> 3
        let dimm_at = [DimmId(0), DimmId(2), DimmId(1), DimmId(3)];
        let mut position_of = [0usize; 4];
        for (pos, &dimm) in dimm_at.iter().enumerate() {
            position_of[dimm.0 as usize] = pos;
        }
        LineTopology {
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
    fn get_route(&self, from_dimm: DimmId, to_dimm: DimmId) -> Vec<(DimmId, DimmId)> {
        debug_assert_ne!(from_dimm, to_dimm);
        let from_pos = self.position_of[from_dimm.0 as usize];
        let to_pos = self.position_of[to_dimm.0 as usize];

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

    fn get_links(&self) -> Vec<(DimmId, DimmId)> {
        let mut links = Vec::new();
        for i in 0..self.dimm_at.len() - 1 {
            let a = self.dimm_at[i];
            let b = self.dimm_at[i + 1];
            links.push((DimmId(a.0.min(b.0)), DimmId(a.0.max(b.0))));
        }
        links
    }

    fn get_num_dimms(&self) -> u8 {
        self.dimm_at.len() as u8
    }

    fn link_sort_key(&self, from_dimm: DimmId, to_dimm: DimmId) -> (usize, bool) {
        let from_pos = self.position_of[from_dimm.0 as usize];
        let to_pos = self.position_of[to_dimm.0 as usize];
        let min_pos = from_pos.min(to_pos);
        let is_reverse = from_pos > to_pos;
        (min_pos, is_reverse)
    }
}

// ─── Ring Topology ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct RingTopology {
    /// DIMM ordering around the ring.
    dimm_at: [DimmId; 4],
    /// Inverse: position_of[dimm_id] gives its index in the ring.
    pub(super) position_of: [usize; 4],
}

impl RingTopology {
    const N: usize = 4;

    pub(super) fn new() -> Self {
        // Same DIMM ordering as LineTopology, but with a wrap-around link.
        // Ring: 0 <-> 2 <-> 1 <-> 3 <-> 0
        let dimm_at = [DimmId(0), DimmId(2), DimmId(1), DimmId(3)];
        let mut position_of = [0usize; 4];
        for (pos, &dimm) in dimm_at.iter().enumerate() {
            position_of[dimm.0 as usize] = pos;
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
    fn get_route(&self, from_dimm: DimmId, to_dimm: DimmId) -> Vec<(DimmId, DimmId)> {
        debug_assert_ne!(from_dimm, to_dimm);
        let from_pos = self.position_of[from_dimm.0 as usize];
        let to_pos = self.position_of[to_dimm.0 as usize];
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

    fn get_links(&self) -> Vec<(DimmId, DimmId)> {
        let n = Self::N;
        let mut links = Vec::new();
        for i in 0..n {
            let a = self.dimm_at[i];
            let b = self.dimm_at[(i + 1) % n];
            links.push((DimmId(a.0.min(b.0)), DimmId(a.0.max(b.0))));
        }
        links
    }

    fn get_num_dimms(&self) -> u8 {
        Self::N as u8
    }

    fn link_sort_key(&self, from_dimm: DimmId, to_dimm: DimmId) -> (usize, bool) {
        let from_pos = self.position_of[from_dimm.0 as usize];
        let to_pos = self.position_of[to_dimm.0 as usize];
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

// ─── Fully Connected Topology ───────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct FullyConnectedTopology {
    num_dimms: usize,
}

impl FullyConnectedTopology {
    pub(super) fn new(num_dimms: usize) -> Self {
        FullyConnectedTopology { num_dimms }
    }
}

impl Debug for FullyConnectedTopology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FullyConnectedTopology")
    }
}

impl Topology for FullyConnectedTopology {
    fn get_route(&self, from_dimm: DimmId, to_dimm: DimmId) -> Vec<(DimmId, DimmId)> {
        debug_assert_ne!(from_dimm, to_dimm);
        vec![(from_dimm, to_dimm)]
    }

    fn get_links(&self) -> Vec<(DimmId, DimmId)> {
        let mut links = Vec::new();
        for i in 0..self.num_dimms as u8 {
            for j in (i + 1)..self.num_dimms as u8 {
                links.push((DimmId(i), DimmId(j)));
            }
        }
        links
    }

    fn get_num_dimms(&self) -> u8 {
        self.num_dimms as u8
    }

    fn link_sort_key(&self, from_dimm: DimmId, to_dimm: DimmId) -> (usize, bool) {
        let min_dimm = from_dimm.0.min(to_dimm.0);
        let max_dimm = from_dimm.0.max(to_dimm.0);
        let group_id = (min_dimm as usize) * self.num_dimms + (max_dimm as usize);
        let is_reverse = from_dimm.0 > to_dimm.0;
        (group_id, is_reverse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Line Topology Tests ────────────────────────────────────────────

    #[test]
    fn test_line_topology_route_adjacent() {
        let topology = LineTopology::new();
        // DIMM 0 -> DIMM 2 (adjacent in line order)
        let route = topology.get_route(DimmId(0), DimmId(2));
        assert_eq!(route, vec![(DimmId(0), DimmId(2))]);
    }

    #[test]
    fn test_line_topology_route_two_hops() {
        let topology = LineTopology::new();
        // DIMM 0 -> DIMM 1: line is 0-2-1-3, so route is 0->2, 2->1
        let route = topology.get_route(DimmId(0), DimmId(1));
        assert_eq!(route, vec![(DimmId(0), DimmId(2)), (DimmId(2), DimmId(1))]);
    }

    #[test]
    fn test_line_topology_route_three_hops() {
        let topology = LineTopology::new();
        // DIMM 0 -> DIMM 3: 0->2->1->3
        let route = topology.get_route(DimmId(0), DimmId(3));
        assert_eq!(
            route,
            vec![
                (DimmId(0), DimmId(2)),
                (DimmId(2), DimmId(1)),
                (DimmId(1), DimmId(3))
            ]
        );
    }

    #[test]
    fn test_line_topology_route_reverse() {
        let topology = LineTopology::new();
        // DIMM 3 -> DIMM 0: 3->1->2->0
        let route = topology.get_route(DimmId(3), DimmId(0));
        assert_eq!(
            route,
            vec![
                (DimmId(3), DimmId(1)),
                (DimmId(1), DimmId(2)),
                (DimmId(2), DimmId(0))
            ]
        );
    }

    #[test]
    fn test_line_topology_links() {
        let topology = LineTopology::new();
        let mut links = topology.get_links();
        links.sort();
        // Line: 0-2-1-3 → links (0,2), (1,2), (1,3)
        assert_eq!(
            links,
            vec![
                (DimmId(0), DimmId(2)),
                (DimmId(1), DimmId(2)),
                (DimmId(1), DimmId(3))
            ]
        );
    }

    // ─── Ring Topology Tests ────────────────────────────────────────────

    #[test]
    fn test_ring_topology_links() {
        let topology = RingTopology::new();
        let mut links = topology.get_links();
        links.sort();
        // Ring: 0-2-1-3-0 → 4 links: (0,2), (1,2), (1,3), (0,3)
        assert_eq!(
            links,
            vec![
                (DimmId(0), DimmId(2)),
                (DimmId(0), DimmId(3)),
                (DimmId(1), DimmId(2)),
                (DimmId(1), DimmId(3))
            ]
        );
    }

    #[test]
    fn test_ring_topology_route_adjacent() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 2: adjacent in ring (position 0 -> 1), 1 hop
        let route = topology.get_route(DimmId(0), DimmId(2));
        assert_eq!(route, vec![(DimmId(0), DimmId(2))]);
    }

    #[test]
    fn test_ring_topology_route_opposite() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 1: positions 0 and 2, equidistant (2 hops each way).
        // Clockwise: 0->2->1 (2 hops), CCW: 0->3->1 (2 hops).
        // With tie-break favoring clockwise, route is 0->2, 2->1.
        let route = topology.get_route(DimmId(0), DimmId(1));
        assert_eq!(route.len(), 2);
        assert_eq!(route, vec![(DimmId(0), DimmId(2)), (DimmId(2), DimmId(1))]);
    }

    #[test]
    fn test_ring_topology_shortest_path() {
        let topology = RingTopology::new();
        // DIMM 0 -> DIMM 3: positions 0 and 3.
        // Clockwise: 0->2->1->3 (3 hops), CCW: 0->3 (1 hop).
        // Should take the shorter counter-clockwise route.
        let route = topology.get_route(DimmId(0), DimmId(3));
        assert_eq!(route, vec![(DimmId(0), DimmId(3))]);
    }

    #[test]
    fn test_ring_topology_shortest_path_reverse() {
        let topology = RingTopology::new();
        // DIMM 3 -> DIMM 0: positions 3 and 0.
        // Clockwise: 3->0 (1 hop), CCW: 3->1->2->0 (3 hops).
        let route = topology.get_route(DimmId(3), DimmId(0));
        assert_eq!(route, vec![(DimmId(3), DimmId(0))]);
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
                let route = topology.get_route(DimmId(from), DimmId(to));
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
                let fwd = topology.get_route(DimmId(from), DimmId(to));
                let rev = topology.get_route(DimmId(to), DimmId(from));
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

    // ─── Fully Connected Topology Tests ─────────────────────────────────

    #[test]
    fn test_fc_topology_links() {
        let topology = FullyConnectedTopology::new(4);
        let mut links = topology.get_links();
        links.sort();
        // FullyConnected with 4 DIMMs should have 4 * 3 / 2 = 6 links:
        assert_eq!(
            links,
            vec![
                (DimmId(0), DimmId(1)),
                (DimmId(0), DimmId(2)),
                (DimmId(0), DimmId(3)),
                (DimmId(1), DimmId(2)),
                (DimmId(1), DimmId(3)),
                (DimmId(2), DimmId(3))
            ]
        );
    }

    #[test]
    fn test_fc_topology_route() {
        let topology = FullyConnectedTopology::new(4);
        // All routes should be exactly 1 hop
        for from in 0u8..4 {
            for to in 0u8..4 {
                if from == to {
                    continue;
                }
                let route = topology.get_route(DimmId(from), DimmId(to));
                assert_eq!(route, vec![(DimmId(from), DimmId(to))]);
            }
        }
    }
}
