use std::{fmt::Debug, num::NonZeroUsize};

use lru::LruCache;

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
    cache: LruCache<u64, ()>, // We don't actually care about the content, just what's in the cache
    pub(super) stats: CacheStats,
}

impl FullyAssociativeCache {
    const HIT_LATENCY: usize = 4;
    const MISS_LATENCY: usize = 50;

    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity >= LINE_SIZE && capacity % LINE_SIZE == 0,
            "Cache capacity must be a multiple of line size"
        );
        FullyAssociativeCache {
            cache: LruCache::new(NonZeroUsize::new(capacity / LINE_SIZE).unwrap()),
            stats: CacheStats::default(),
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
            Self::MISS_LATENCY
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
            Self::MISS_LATENCY
        }
    }

    fn read_latency(&self, addr: u64) -> usize {
        if self.cache.contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            Self::MISS_LATENCY
        }
    }

    fn write_latency(&self, addr: u64) -> usize {
        if self.cache.contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            Self::MISS_LATENCY
        }
    }
}

#[derive(Clone)]
pub(super) struct SetAssociativeCache {
    cache_sets: Vec<LruCache<u64, ()>>,
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
    const MISS_LATENCY: usize = 50;

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
            Self::MISS_LATENCY
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
            Self::MISS_LATENCY
        }
    }

    fn read_latency(&self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        if self.cache_sets[set_idx].contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            Self::MISS_LATENCY
        }
    }

    fn write_latency(&self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        if self.cache_sets[set_idx].contains(&addr_to_line(addr)) {
            Self::HIT_LATENCY
        } else {
            Self::MISS_LATENCY
        }
    }
}

// Unit tests for FullyAssociativeCache
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_fully_associative_cache() {
        let mut cache = FullyAssociativeCache::new(64); // 64 B cache
        assert_eq!(cache.read(0x1000), FullyAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.write(0x1000), FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0x1000), FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0x2000), FullyAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.write(0x2000), FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0x1000), FullyAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.stats.read_hits, 1);
        assert_eq!(cache.stats.read_misses, 3);
        assert_eq!(cache.stats.write_hits, 2);
        assert_eq!(cache.stats.write_misses, 0);
    }

    #[test]
    fn test_set_associative_cache() {
        let mut cache = SetAssociativeCache::new(2, 1); // 2 sets, 1 way each
        assert_eq!(cache.read(0), SetAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.read(0), SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(64), SetAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.read(64), SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(128), SetAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.read(128), SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0), SetAssociativeCache::MISS_LATENCY);
        assert_eq!(cache.read(64), SetAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.stats.read_hits, 4);
        assert_eq!(cache.stats.read_misses, 4);
        assert_eq!(cache.stats.write_hits, 0);
        assert_eq!(cache.stats.write_misses, 0);
    }
}
