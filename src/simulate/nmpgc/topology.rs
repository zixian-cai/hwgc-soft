use std::fmt::Debug;

use crate::SimulationMemoryConfiguration;

// A full configuration is 64 GB system
// 4 DIMMs in two channels, 2 ranks per DIMM
// So 8 ranks in total
// 1024 Meg * 8, 8 GB per rank
//
// A particular bank is 65536x128x64 (each column has 8 bits, and reads in bursts of 8)
// So when you read a cache line, you are implictly changing the lower 3 bits of the column address
// row     rank     bank   channel col    blkoffset
// [35:20] [19:18] [17:14] [13:13] [12:6] [5:0]
fn extract_bits(start: usize, end: usize, value: u64) -> u64 {
    // End is inclusive
    let mask = ((1u64 << (end - start + 1)) - 1) << start;
    (value & mask) >> start
}

impl SimulationMemoryConfiguration {
    pub fn get_channels_bits(&self) -> u8 {
        match self {
            SimulationMemoryConfiguration::C1D1R1 => 0,
            SimulationMemoryConfiguration::C2D1R1 => 1,
            SimulationMemoryConfiguration::C1D2R1 => 0,
            SimulationMemoryConfiguration::C1D1R2 => 0,
            SimulationMemoryConfiguration::C1D2R2 => 0,
            SimulationMemoryConfiguration::C2D1R2 => 1,
            SimulationMemoryConfiguration::C2D2R1 => 1,
            SimulationMemoryConfiguration::C2D2R2 => 1,
        }
    }

    pub fn get_dimm_bits(&self) -> u8 {
        match self {
            SimulationMemoryConfiguration::C1D1R1 => 0,
            SimulationMemoryConfiguration::C2D1R1 => 0,
            SimulationMemoryConfiguration::C1D2R1 => 1,
            SimulationMemoryConfiguration::C1D1R2 => 0,
            SimulationMemoryConfiguration::C1D2R2 => 1,
            SimulationMemoryConfiguration::C2D1R2 => 0,
            SimulationMemoryConfiguration::C2D2R1 => 1,
            SimulationMemoryConfiguration::C2D2R2 => 1,
        }
    }

    pub fn get_rank_bits(&self) -> u8 {
        match self {
            SimulationMemoryConfiguration::C1D1R1 => 0,
            SimulationMemoryConfiguration::C2D1R1 => 0,
            SimulationMemoryConfiguration::C1D2R1 => 0,
            SimulationMemoryConfiguration::C1D1R2 => 1,
            SimulationMemoryConfiguration::C1D2R2 => 1,
            SimulationMemoryConfiguration::C2D1R2 => 1,
            SimulationMemoryConfiguration::C2D2R1 => 0,
            SimulationMemoryConfiguration::C2D2R2 => 1,
        }
    }

    pub fn get_total_id_bits(&self) -> u8 {
        self.get_channels_bits() + self.get_dimm_bits() + self.get_rank_bits()
    }

    pub fn get_channel(&self, addr: u64) -> u8 {
        if self.get_channels_bits() == 0 {
            return 0;
        } else {
            extract_bits(13, 13, addr) as u8
        }
    }

    pub fn get_bank(&self, addr: u64) -> u8 {
        let bank_bits_start = 13 + self.get_channels_bits();
        let bank_bits_end = bank_bits_start + 4 - 1;
        extract_bits(bank_bits_start as usize, bank_bits_end as usize, addr) as u8
    }

    pub fn get_dimm(&self, addr: u64) -> u8 {
        let dimm_bits_start = 17 + self.get_channels_bits();
        let dimm_bits_end = dimm_bits_start + self.get_dimm_bits() - 1;
        if self.get_dimm_bits() == 0 {
            return 0;
        } else {
            extract_bits(dimm_bits_start as usize, dimm_bits_end as usize, addr) as u8
        }
    }

    pub fn get_rank(&self, addr: u64) -> u8 {
        let rank_bits_start = 17 + self.get_channels_bits() + self.get_dimm_bits();
        let rank_bits_end = rank_bits_start + self.get_rank_bits() - 1;
        if self.get_rank_bits() == 0 {
            return 0;
        } else {
            extract_bits(rank_bits_start as usize, rank_bits_end as usize, addr) as u8
        }
    }

    pub fn get_row(&self, addr: u64) -> u16 {
        let row_bits_start =
            17 + self.get_channels_bits() + self.get_dimm_bits() + self.get_rank_bits();
        let row_bits_end = row_bits_start + 16 - 1; // 16 bits for the row
        extract_bits(row_bits_start as usize, row_bits_end as usize, addr) as u16
    }

    pub fn get_global_rank_id(&self, addr: u64) -> u8 {
        let channel = self.get_channel(addr);
        let dimm = self.get_dimm(addr);
        let rank = self.get_rank(addr);
        (rank << (self.get_dimm_bits() + self.get_channels_bits()))
            | (dimm << self.get_channels_bits())
            | channel
    }

    pub fn global_rank_id_to_name(&self, id: u8) -> String {
        let channel = id & ((1 << self.get_channels_bits()) - 1);
        let dimm = (id >> self.get_channels_bits()) & ((1 << self.get_dimm_bits()) - 1);
        let rank = (id >> (self.get_channels_bits() + self.get_dimm_bits()))
            & ((1 << self.get_rank_bits()) - 1);
        format!("C{}D{}R{}", channel, dimm, rank)
    }

    pub fn get_global_dimm_id(&self, addr: u64) -> u8 {
        let channel = self.get_channel(addr);
        let dimm = self.get_dimm(addr);
        (channel << self.get_dimm_bits()) | dimm
    }
}

pub(super) trait Topology {
    fn get_latency(&self, mem_config: SimulationMemoryConfiguration, from: u8, to: u8) -> usize;
}

struct FullyConnectedUniformTopology {
    latency: usize,
}

impl Topology for FullyConnectedUniformTopology {
    fn get_latency(&self, _mem_config: SimulationMemoryConfiguration, _from: u8, _to: u8) -> usize {
        self.latency
    }
}

#[derive(Clone)]
pub(super) struct LineTopology {}

impl LineTopology {
    // Latency between ranks on the same DIMM
    // Or going from the on DIMM link controller to the rank
    const DIMM_TO_RANK_LATENCY: usize = 2;
    const DIMM_TO_DIMM_LATENCY: usize = 4;

    pub(super) fn new() -> Self {
        LineTopology {}
    }
}

impl Debug for LineTopology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LineTopology")
    }
}

impl Topology for LineTopology {
    fn get_latency(&self, from: u8, to: u8) -> usize {
        // debug_assert_ne!(from, to);
        // let from_id = RankID(from);
        // let to_id = RankID(to);
        // let mut from_dimm = RankID(from_id.0);
        // from_dimm.set_rank(0);
        // let mut to_dimm = RankID(to_id.0);
        // to_dimm.set_rank(0);

        // if from_dimm == to_dimm {
        //     return Self::DIMM_TO_RANK_LATENCY;
        // }

        // let between_dimm_latency =
        //     self.dimm_latency_matrix[from_dimm.0 as usize][to_dimm.0 as usize];
        // return between_dimm_latency + Self::DIMM_TO_RANK_LATENCY * 2;
        todo!();
        4
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_line_topology_latency_same_dimm() {
//         let topology = LineTopology::new();
//         let from = RankID(1);
//         let mut to = RankID(1);
//         to.set_rank(1);
//         assert_ne!(from, to);
//         assert_eq!(
//             topology.get_latency(from.0, to.0),
//             LineTopology::DIMM_TO_RANK_LATENCY
//         );
//     }

//     #[test]
//     fn test_line_topology_latency_different_dimms() {
//         let topology = LineTopology::new();
//         // 0 -> 2 -> 1
//         let latency = topology.get_latency(0, 1);
//         assert_eq!(latency, 8 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
//     }

//     #[test]
//     fn test_line_topology_latency_reverse_path() {
//         let topology = LineTopology::new();
//         // 1 -> 2 -> 0
//         let latency = topology.get_latency(1, 0);
//         assert_eq!(latency, 8 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
//     }

//     #[test]
//     #[should_panic]
//     fn test_line_topology_latency_same_rank() {
//         let topology = LineTopology::new();
//         topology.get_latency(0, 0); // Should panic due to debug_assert_ne!
//     }

//     #[test]
//     fn test_line_topology_single_hop() {
//         let topology = LineTopology::new();
//         // 0 -> 2
//         let latency = topology.get_latency(0, 2);
//         assert_eq!(latency, 4 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
//     }

//     #[test]
//     fn test_line_topology_three_hops() {
//         let topology = LineTopology::new();
//         // 0 -> 2 -> 1 -> 3
//         let latency = topology.get_latency(0, 3);
//         assert_eq!(latency, 12 + LineTopology::DIMM_TO_RANK_LATENCY * 2);
//     }
// }
