use bitfield::bitfield;
use lru::LruCache;
use std::fmt::Debug;
use std::num::NonZeroUsize;

/// Assumes reading word-aligned words
pub(super) trait DataCache {
    /// Reads a word from the cache, returning the latency.
    fn read(&mut self, addr: u64) -> usize;
    /// Check would-be read latency without modifying the cache.
    fn read_latency(&self, addr: u64) -> usize;
    /// Writes a word to the cache, returning the latency.
    fn write(&mut self, addr: u64) -> usize;
    /// Check would-be write latency without modifying the cache.
    fn write_latency(&self, addr: u64) -> usize;
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
    fn read(&mut self, addr: u64) -> usize {
        let line = addr_to_line(addr);
        if let Some(_) = self.cache.get(&line) {
            self.stats.read_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            self.stats.read_misses += 1;
            self.rank.transaction(addr)
        }
    }

    fn write(&mut self, addr: u64) -> usize {
        let line = addr_to_line(addr);
        if let Some(_) = self.cache.get(&line) {
            self.stats.write_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            self.stats.write_misses += 1;
            self.rank.transaction(addr)
        }
    }

    fn read_latency(&self, addr: u64) -> usize {
        if self.cache.contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(addr)
        }
    }

    fn write_latency(&self, addr: u64) -> usize {
        if self.cache.contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(addr)
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
    fn read(&mut self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        let line = addr_to_line(addr);
        if let Some(_) = self.cache_sets[set_idx].get(&line) {
            self.stats.read_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache_sets[set_idx].put(line, ());
            self.stats.read_misses += 1;
            self.rank.transaction(addr)
        }
    }

    fn write(&mut self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        let line = addr_to_line(addr);
        if let Some(_) = self.cache_sets[set_idx].get(&line) {
            self.stats.write_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache_sets[set_idx].put(line, ());
            self.stats.write_misses += 1;
            self.rank.transaction(addr)
        }
    }

    fn read_latency(&self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        if self.cache_sets[set_idx].contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(addr)
        }
    }

    fn write_latency(&self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        if self.cache_sets[set_idx].contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            self.rank.transaction_latency(addr)
        }
    }
}

// dual channel, quad rank,
// 1024 Meg * 8, 8 GB per rank
// 64 GB system (4 DIMMs in two channels, 2 ranks per DIMM)
// A particular bank is 65536x128x64 (each column has 8 bits, and reads in bursts of 8)
// So when you read a cache line, you are implictly changing the lower 3 bits of the column address
// row     rank     bank   channel col    blkoffset
// [35:20] [19:18] [17:14] [13:13] [12:6] [5:0]
bitfield! {
    pub struct AddressMapping(u64);
    impl Debug;
    pub u8, blkoffset, set_blkoffset: 5, 0;
    pub u8, col, set_col: 12, 6;
    pub u8, channel, set_channel: 13, 13;
    pub u8, bank, set_bank: 17, 14;
    pub u8, rank, set_rank: 19, 18;
    pub u16, row, set_row: 35, 20;
    pub u32, rest, set_rest: 63, 36;
}

impl AddressMapping {
    pub(super) fn get_owner_thread(&self) -> u64 {
        ((self.rank() << 1) | self.channel()) as u64
    }
}

#[derive(Clone, Default)]
struct BankState {
    current_row: Option<u16>,
}

impl BankState {
    fn transaction(&mut self, addr: u64) {
        let mapping = AddressMapping(addr);
        self.current_row = Some(mapping.row());
    }

    fn transaction_latency(&self, addr: u64) -> usize {
        let mapping = AddressMapping(addr);
        if self.current_row.is_none() || self.current_row.unwrap() != mapping.row() {
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
    fn transaction(&mut self, addr: u64) -> usize {
        let mapping = AddressMapping(addr);
        let latency = self.transaction_latency(addr);
        self.banks[mapping.bank() as usize].transaction(addr);
        latency
    }

    fn transaction_latency(&self, addr: u64) -> usize {
        let mapping = AddressMapping(addr);
        self.banks[mapping.bank() as usize].transaction_latency(addr)
    }
}

// Unit tests for FullyAssociativeCache
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_fully_associative_cache() {
        let mut cache = FullyAssociativeCache::new(64); // 64 B cache
        assert!(cache.read(0x1000) > FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.write(0x1000), FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0x1000), FullyAssociativeCache::HIT_LATENCY);
        assert!(cache.read(0x2000) > FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.write(0x2000), FullyAssociativeCache::HIT_LATENCY);
        assert!(cache.read(0x1000) > FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.stats.read_hits, 1);
        assert_eq!(cache.stats.read_misses, 3);
        assert_eq!(cache.stats.write_hits, 2);
        assert_eq!(cache.stats.write_misses, 0);
    }

    #[test]
    fn test_set_associative_cache() {
        let mut cache = SetAssociativeCache::new(2, 1); // 2 sets, 1 way each
        assert!(cache.read(0) > SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0), SetAssociativeCache::HIT_LATENCY);
        assert!(cache.read(64) > SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(64), SetAssociativeCache::HIT_LATENCY);
        assert!(cache.read(128) > SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(128), SetAssociativeCache::HIT_LATENCY);
        assert!(cache.read(0) > SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(64), SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.stats.read_hits, 4);
        assert_eq!(cache.stats.read_misses, 4);
        assert_eq!(cache.stats.write_hits, 0);
        assert_eq!(cache.stats.write_misses, 0);
    }
}
