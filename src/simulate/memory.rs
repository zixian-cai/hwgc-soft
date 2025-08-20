use lru::LruCache;
use std::fmt::Debug;
use std::num::NonZeroUsize;
use std::ops::Add;

use crate::SimulationMemoryConfiguration;

/// Assumes reading word-aligned words
// FIXME: the memory model requires physical addresses, but right now the
// heapdumps feed virtual addresses, and the higher bits are ignored.
// This messes with the row conflict modelling.
pub(super) trait DataCache {
    /// Reads a word from the cache, returning the latency.
    fn read(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize;
    /// Check would-be read latency without modifying the cache.
    fn read_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize;
    /// Writes a word to the cache, returning the latency.
    fn write(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize;
    /// Check would-be write latency without modifying the cache.
    fn write_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize;
}

const LOG_LINE_SIZE: usize = 6; // Assuming a line size of 64 bytes
const LINE_SIZE: usize = 1 << LOG_LINE_SIZE;

fn addr_to_line(addr: u64) -> u64 {
    addr >> LOG_LINE_SIZE
}

#[derive(Default, Clone)]
pub(super) struct CacheStats {
    pub(super) read_hits: usize,
    pub(super) read_misses: usize,
    pub(super) write_hits: usize,
    pub(super) write_misses: usize,
}

#[allow(dead_code)]
pub(super) struct FullyAssociativeCache {
    cache: LruCache<u64, ()>, // We don't actually care about the content, just what's in the cache,
    rank: DDR4Rank,
    pub(super) stats: CacheStats,
}

impl FullyAssociativeCache {
    const HIT_LATENCY: usize = 4;

    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity >= LINE_SIZE && capacity % LINE_SIZE == 0,
            "Cache capacity must be a multiple of line size"
        );
        FullyAssociativeCache {
            cache: LruCache::new(NonZeroUsize::new(capacity / LINE_SIZE).unwrap()),
            stats: CacheStats::default(),
            rank: DDR4Rank::default(),
        }
    }
}

impl DataCache for FullyAssociativeCache {
    fn read(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let line = addr_to_line(addr);
        if self.cache.get(&line).is_some() {
            self.stats.read_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            self.stats.read_misses += 1;
            self.rank.transaction(mem_config, addr)
        }
    }

    fn write(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let line = addr_to_line(addr);
        if self.cache.get(&line).is_some() {
            self.stats.write_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            self.stats.write_misses += 1;
            self.rank.transaction(mem_config, addr)
        }
    }

    fn read_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        if self.cache.contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(mem_config, addr)
        }
    }

    fn write_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        if self.cache.contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(mem_config, addr)
        }
    }
}

#[derive(Clone)]
pub(super) struct SetAssociativeCache {
    cache_sets: Vec<LruCache<u64, ()>>,
    rank: DDR4Rank,
    pub(super) stats: CacheStats,
}

impl Debug for SetAssociativeCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SetAssociativeCache: {}-set {}-way)",
            self.cache_sets.len(),
            self.cache_sets[0].cap()
        )
    }
}

impl SetAssociativeCache {
    const HIT_LATENCY: usize = 4;

    pub fn new(num_sets: usize, num_ways: usize) -> Self {
        assert!(
            num_sets > 0 && num_ways > 0,
            "Number of sets and ways must be greater than zero"
        );
        let cache_sets = (0..num_sets)
            .map(|_| LruCache::new(NonZeroUsize::new(num_ways).unwrap()))
            .collect();
        SetAssociativeCache {
            cache_sets,
            stats: CacheStats::default(),
            rank: DDR4Rank::default(),
        }
    }

    fn get_set_idx(&self, addr: u64) -> usize {
        let line = addr_to_line(addr);
        (line as usize) % self.cache_sets.len()
    }
}

impl DataCache for SetAssociativeCache {
    fn read(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        let line = addr_to_line(addr);
        if self.cache_sets[set_idx].get(&line).is_some() {
            self.stats.read_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache_sets[set_idx].put(line, ());
            self.stats.read_misses += 1;
            self.rank.transaction(mem_config, addr)
        }
    }

    fn write(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        let line = addr_to_line(addr);
        if self.cache_sets[set_idx].get(&line).is_some() {
            self.stats.write_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache_sets[set_idx].put(line, ());
            self.stats.write_misses += 1;
            self.rank.transaction(mem_config, addr)
        }
    }

    fn read_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        if self.cache_sets[set_idx].contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(mem_config, addr)
        }
    }

    fn write_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        if self.cache_sets[set_idx].contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(mem_config, addr)
        }
    }
}

#[derive(Clone, Default)]
struct BankState {
    current_row: Option<u16>,
}

impl BankState {
    fn transaction(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) {
        self.current_row = Some(mem_config.get_row(addr) as u16);
    }

    fn transaction_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let row = mem_config.get_row(addr);
        if self.current_row.is_none() || self.current_row.unwrap() != row as u16 {
            // DDR4-3200 Speed Bin -062Y
            // https://www.mouser.com/datasheet/2/671/Micron_05092023_8gb_ddr4_sdram-3175546.pdf
            //  tRP + tRCD + tCAS + 4 (double data rate, and burst of 8)
            22 + 22 + 22 + 4
        } else {
            // tCAS + 4 (double data rate, and burst of 8)
            22 + 4
        }
    }
}

#[derive(Clone, Default)]
struct DDR4Rank {
    banks: [BankState; 16], // 16 banks per rank
}

impl DDR4Rank {
    fn transaction(&mut self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let bank = mem_config.get_bank(addr);
        let latency = self.transaction_latency(mem_config, addr);
        self.banks[bank as usize].transaction(mem_config, addr);
        latency
    }

    fn transaction_latency(&self, mem_config: SimulationMemoryConfiguration, addr: u64) -> usize {
        let bank = mem_config.get_bank(addr);
        self.banks[bank as usize].transaction_latency(mem_config, addr)
    }
}

// Unit tests for FullyAssociativeCache
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_fully_associative_cache() {
        let mut cache = FullyAssociativeCache::new(64); // 64 B cache
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0x1000)
                > FullyAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.write(SimulationMemoryConfiguration::C2D2R2, 0x1000),
            FullyAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0x1000),
            FullyAssociativeCache::HIT_LATENCY
        );
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0x2000)
                > FullyAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.write(SimulationMemoryConfiguration::C2D2R2, 0x2000),
            FullyAssociativeCache::HIT_LATENCY
        );
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0x1000)
                > FullyAssociativeCache::HIT_LATENCY
        );
        assert_eq!(cache.stats.read_hits, 1);
        assert_eq!(cache.stats.read_misses, 3);
        assert_eq!(cache.stats.write_hits, 2);
        assert_eq!(cache.stats.write_misses, 0);
    }

    #[test]
    fn test_set_associative_cache() {
        let mut cache = SetAssociativeCache::new(2, 1); // 2 sets, 1 way each
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0) > SetAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0),
            SetAssociativeCache::HIT_LATENCY
        );
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 64)
                > SetAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 64),
            SetAssociativeCache::HIT_LATENCY
        );
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 128)
                > SetAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 128),
            SetAssociativeCache::HIT_LATENCY
        );
        assert!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 0) > SetAssociativeCache::HIT_LATENCY
        );
        assert_eq!(
            cache.read(SimulationMemoryConfiguration::C2D2R2, 64),
            SetAssociativeCache::HIT_LATENCY
        );
        assert_eq!(cache.stats.read_hits, 4);
        assert_eq!(cache.stats.read_misses, 4);
        assert_eq!(cache.stats.write_hits, 0);
        assert_eq!(cache.stats.write_misses, 0);
    }

    #[test]
    fn test_bank_state() {
        let mut bank_state = BankState::default();
        let addr = 0b0_0_0000000_000000;
        assert_eq!(
            bank_state.transaction_latency(SimulationMemoryConfiguration::C2D2R2, addr),
            22 + 22 + 22 + 4
        );
        bank_state.transaction(SimulationMemoryConfiguration::C2D2R2, addr);
        assert_eq!(bank_state.current_row, Some(0));
        assert_eq!(
            bank_state.transaction_latency(SimulationMemoryConfiguration::C2D2R2, addr),
            22 + 4
        );
        // Differnt row
        let addr = 0b1_00_0000_0_0000000_000000;
        assert_eq!(
            bank_state.transaction_latency(SimulationMemoryConfiguration::C2D2R2, addr),
            22 + 22 + 22 + 4
        );
        bank_state.transaction(SimulationMemoryConfiguration::C2D2R2, addr);
        assert_eq!(bank_state.current_row, Some(1));
        assert_eq!(
            bank_state.transaction_latency(SimulationMemoryConfiguration::C2D2R2, addr),
            22 + 4
        );
        let addr = 0b0_0_0000000_000000;
        assert_eq!(
            bank_state.transaction_latency(SimulationMemoryConfiguration::C2D2R2, addr),
            22 + 22 + 22 + 4
        );
        let addr = 0b1_00_0000_0_0000001_000000;
        assert_eq!(
            bank_state.transaction_latency(SimulationMemoryConfiguration::C2D2R2, addr),
            22 + 4
        );
    }
}
