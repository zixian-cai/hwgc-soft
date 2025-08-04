use std::num::NonZeroUsize;

use lru::LruCache;

pub(super) trait DataCache {
    fn read(&mut self, key: u64) -> usize;
    fn write(&mut self, key: u64) -> usize;
}

const LOG_LINE_SIZE: usize = 6; // Assuming a line size of 64 bytes
const LINE_SIZE: usize = 1 << LOG_LINE_SIZE;

fn addr_to_line(addr: u64) -> u64 {
    addr >> LOG_LINE_SIZE
}

pub(super) struct FullyAssociativeCache {
    cache: LruCache<u64, ()>, // We don't actually care about the content, just what's in the cache
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
        }
    }
}

impl DataCache for FullyAssociativeCache {
    fn read(&mut self, key: u64) -> usize {
        let line = addr_to_line(key);
        if let Some(_) = self.cache.get(&line) {
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            Self::MISS_LATENCY
        }
    }

    fn write(&mut self, key: u64) -> usize {
        let line = addr_to_line(key);
        if let Some(_) = self.cache.get(&line) {
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
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
    }
}
