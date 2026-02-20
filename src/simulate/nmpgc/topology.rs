use super::super::memory::RankID;
use std::fmt::Debug;

pub(super) trait Topology {
    fn get_latency(&self, from: u8, to: u8) -> usize;
}

struct FullyConnectedUniformTopology {
    latency: usize,
}

impl Topology for FullyConnectedUniformTopology {
    fn get_latency(&self, _from: u8, _to: u8) -> usize {
        self.latency
    }
}

#[derive(Clone)]
pub(super) struct LineTopology {
    /// Latency between DIMMs
    dimm_latency_matrix: [[usize; 4]; 4],
}

impl LineTopology {
    // Latency between ranks on the same DIMM
    // Or going from the on DIMM link controller to the rank
    const DIMM_TO_RANK_LATENCY: usize = 2;

    pub(super) fn new() -> Self {
        // 0: channel 0, dimm 0,
        // 1: channel 1, dimm 0,
        // 2, channel 0, dimm 1,
        // 3, channel 1, dimm 1

        // Topology: 0 <-> 2 <-> 1 <-> 3
        let dimm_latency_matrix = [[0, 8, 4, 12], [8, 0, 4, 4], [4, 4, 0, 8], [12, 4, 8, 0]];

        LineTopology {
            dimm_latency_matrix,
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
        return between_dimm_latency + Self::DIMM_TO_RANK_LATENCY * 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
