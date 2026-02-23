use bitfield::bitfield;
use lru::LruCache;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::num::NonZeroUsize;

/// Assumes reading word-aligned words
// FIXME: the memory model requires physical addresses, but right now the
// heapdumps feed virtual addresses, and the higher bits are ignored.
// This messes with the row conflict modelling.
pub(super) trait DataCache {
    /// Cache hit latency in cycles.
    const HIT_LATENCY: usize = 4;
    /// Reads a word from the cache, returning the latency.
    fn read(&mut self, addr: u64) -> usize;
    /// Writes a word to the cache, returning the latency.
    fn write(&mut self, addr: u64) -> usize;
}

const LOG_LINE_SIZE: usize = 6; // Assuming a line size of 64 bytes
#[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn new(capacity: usize, rank_option: DDR4RankOption) -> Self {
        assert!(
            capacity >= LINE_SIZE && capacity % LINE_SIZE == 0,
            "Cache capacity must be a multiple of line size"
        );
        FullyAssociativeCache {
            cache: LruCache::new(NonZeroUsize::new(capacity / LINE_SIZE).unwrap()),
            stats: CacheStats::default(),
            rank: DDR4Rank::new(rank_option),
        }
    }
}

impl DataCache for FullyAssociativeCache {
    fn read(&mut self, addr: u64) -> usize {
        let line = addr_to_line(addr);
        if self.cache.get(&line).is_some() {
            self.stats.read_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            self.stats.read_misses += 1;
            Self::HIT_LATENCY + self.rank.transaction(addr, false)
        }
    }

    /// Write-through: every write is forwarded to DRAM regardless of cache
    /// state. The cache line is allocated (write-allocate) so subsequent reads
    /// can hit.
    fn write(&mut self, addr: u64) -> usize {
        let line = addr_to_line(addr);
        if self.cache.get(&line).is_some() {
            self.stats.write_hits += 1;
        } else {
            self.cache.put(line, ());
            self.stats.write_misses += 1;
        }
        Self::HIT_LATENCY + self.rank.transaction(addr, true)
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
    pub fn new(num_sets: usize, num_ways: usize, rank_option: DDR4RankOption) -> Self {
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
            rank: DDR4Rank::new(rank_option),
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
        if self.cache_sets[set_idx].get(&line).is_some() {
            self.stats.read_hits += 1;
            Self::HIT_LATENCY
        } else {
            self.cache_sets[set_idx].put(line, ());
            self.stats.read_misses += 1;
            Self::HIT_LATENCY + self.rank.transaction(addr, false)
        }
    }

    /// Write-through: every write is forwarded to DRAM regardless of cache
    /// state. The cache line is allocated (write-allocate) so subsequent reads
    /// can hit.
    fn write(&mut self, addr: u64) -> usize {
        let set_idx = self.get_set_idx(addr);
        let line = addr_to_line(addr);
        if self.cache_sets[set_idx].get(&line).is_some() {
            self.stats.write_hits += 1;
        } else {
            self.cache_sets[set_idx].put(line, ());
            self.stats.write_misses += 1;
        }
        Self::HIT_LATENCY + self.rank.transaction(addr, true)
    }
}

// dual channel, 8 ranks,
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
    pub u8, dimm, set_dimm: 18, 18;
    pub u8, rank, set_rank: 19, 19;
    pub u16, row, set_row: 35, 20;
    pub u32, rest, set_rest: 63, 36;
}

impl AddressMapping {
    /// Returns the owner thread ID based on the channel and rank.
    /// This needs to be consistent with the TopologyLocation encoding.
    pub(super) fn get_owner_id(&self) -> usize {
        let mut rank_id = RankId(0);
        rank_id.set_channel(self.channel());
        rank_id.set_dimm(self.dimm());
        rank_id.set_rank(self.rank());
        rank_id.0 as usize
    }
}

bitfield! {
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct DimmId(u8);
    impl Debug;
    pub u8, channel, set_channel: 0, 0;
    pub u8, dimm, set_dimm: 1, 1;
}

impl Display for DimmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "C{}-D{}", self.channel(), self.dimm())
    }
}

impl From<RankId> for DimmId {
    fn from(rank_id: RankId) -> Self {
        let mut dimm_id = DimmId(0);
        dimm_id.set_channel(rank_id.channel());
        dimm_id.set_dimm(rank_id.dimm());
        dimm_id
    }
}

bitfield! {
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct RankId(u8);
    impl Debug;
    pub u8, channel, set_channel: 0, 0;
    pub u8, dimm, set_dimm: 1, 1;
    pub u8, rank, set_rank: 2, 2;
}

impl Display for RankId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "C{}-D{}-R{}", self.channel(), self.dimm(), self.rank())
    }
}

impl RankId {
    #[allow(dead_code)]
    pub(crate) fn to_dict(&self) -> HashMap<String, Value> {
        let mut dict = HashMap::new();
        dict.insert("channel".to_string(), json!(self.channel()));
        dict.insert("dimm".to_string(), json!(self.dimm()));
        dict.insert("rank".to_string(), json!(self.rank()));
        dict
    }
}

#[derive(Clone, Default, Debug)]
struct BankState {
    current_row: Option<u16>,
}

impl BankState {
    /// Performs a transaction and returns the latency in cycles.
    fn transaction(&mut self, addr: u64) -> usize {
        let mapping = AddressMapping(addr);
        let latency = if self.current_row.is_none() || self.current_row.unwrap() != mapping.row() {
            // DDR4-3200 Speed Bin -062Y
            // https://www.mouser.com/datasheet/2/671/Micron_05092023_8gb_ddr4_sdram-3175546.pdf
            //  tRP + tRCD + tCAS + 4 (double data rate, and burst of 8)
            22 + 22 + 22 + 4
        } else {
            // tCAS + 4 (double data rate, and burst of 8)
            22 + 4
        };
        self.current_row = Some(mapping.row());
        latency
    }
}

trait DDR4RankModel: Debug + Send + Sync {
    fn transaction(&mut self, addr: u64, is_write: bool) -> usize;
    fn clone_box(&self) -> Box<dyn DDR4RankModel>;
}

impl Clone for Box<dyn DDR4RankModel> {
    fn clone(&self) -> Box<dyn DDR4RankModel> {
        self.clone_box()
    }
}

#[derive(Debug, Clone)]
struct DDR4RankNaive {
    banks: Vec<BankState>,
}

impl Default for DDR4RankNaive {
    fn default() -> Self {
        Self {
            banks: vec![BankState::default(); 16],
        }
    }
}

impl DDR4RankModel for DDR4RankNaive {
    fn transaction(&mut self, addr: u64, _is_write: bool) -> usize {
        let mapping = AddressMapping(addr);
        let bank_idx = mapping.bank() as usize;
        self.banks[bank_idx].transaction(addr)
    }

    fn clone_box(&self) -> Box<dyn DDR4RankModel> {
        Box::new(self.clone())
    }
}

use crate::shim::ffi;
use std::ffi::CString;
use std::sync::Mutex;

#[derive(Debug)]
struct DRAMSim3 {
    wrapper: *mut ffi::CDRAMSim3,
}

// DRAMSim3 holds a raw pointer to a C++ object, which is not thread-safe.
// However, we wrap it in a Mutex, which requires T to be Send.
// We assert Send because we only move the wrapper between threads, never sharing it
// concurrently without synchronization (enforced by Mutex).
unsafe impl Send for DRAMSim3 {}

impl DRAMSim3 {
    fn new(config_file: &str, output_dir: &str) -> Self {
        let config_file = CString::new(config_file).expect("Config file path contains null byte");
        let output_dir = CString::new(output_dir).expect("Output dir path contains null byte");
        let wrapper =
            unsafe { ffi::new_dramsim3_wrapper(config_file.as_ptr(), output_dir.as_ptr()) };
        Self { wrapper }
    }

    fn add_transaction(&self, addr: u64, is_write: bool) {
        unsafe {
            ffi::dramsim3_add_transaction(self.wrapper, addr, is_write);
        }
    }

    fn clock_tick(&self) {
        unsafe {
            ffi::dramsim3_clock_tick(self.wrapper);
        }
    }

    fn will_accept_transaction(&self, addr: u64, is_write: bool) -> bool {
        unsafe { ffi::dramsim3_will_accept_transaction(self.wrapper, addr, is_write) }
    }

    fn is_transaction_done(&self, addr: u64, is_write: bool) -> bool {
        unsafe { ffi::dramsim3_is_transaction_done(self.wrapper, addr, is_write) }
    }
}

impl Drop for DRAMSim3 {
    fn drop(&mut self) {
        unsafe {
            ffi::delete_dramsim3_wrapper(self.wrapper);
        }
    }
}

#[derive(Debug)]
struct DDR4RankDRAMsim3 {
    dramsim3: Mutex<DRAMSim3>,
    config_file: String,
    output_dir: String,
}

impl DDR4RankDRAMsim3 {
    fn new(config_file: &str, output_dir: &str) -> Self {
        Self {
            dramsim3: Mutex::new(DRAMSim3::new(config_file, output_dir)),
            config_file: config_file.to_string(),
            output_dir: output_dir.to_string(),
        }
    }

    fn run_transaction(&self, addr: u64, is_write: bool) -> usize {
        let dramsim3 = self.dramsim3.lock().unwrap();

        let mut ticks = 0;
        // Wait until transaction is accepted
        loop {
            if dramsim3.will_accept_transaction(addr, is_write) {
                dramsim3.add_transaction(addr, is_write);
                break;
            }
            dramsim3.clock_tick();
            ticks += 1;
            // Safety break for acceptance
            if ticks > 1000000 {
                error!(
                    "DRAMsim3 transaction acceptance timed out for addr {:#x}",
                    addr
                );
                return ticks; // Return what we have, though it failed
            }
        }

        // Wait until transaction is done
        loop {
            dramsim3.clock_tick();
            ticks += 1;
            if dramsim3.is_transaction_done(addr, is_write) {
                break;
            }
            // Safety break for completion
            if ticks > 10000000 {
                // Increased timeout for completion
                error!(
                    "DRAMsim3 transaction completion timed out for addr {:#x}",
                    addr
                );
                break;
            }
        }
        ticks
    }
}

impl DDR4RankModel for DDR4RankDRAMsim3 {
    fn transaction(&mut self, addr: u64, is_write: bool) -> usize {
        self.run_transaction(addr, is_write)
    }

    fn clone_box(&self) -> Box<dyn DDR4RankModel> {
        // Create a new instance with the same configuration.
        // This effectively gives a fresh memory simulation for the new rank.
        Box::new(Self::new(&self.config_file, &self.output_dir))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DDR4RankOption {
    Naive,
    DRAMsim3 {
        config_file: String,
        output_dir: String,
    },
}

impl Default for DDR4RankOption {
    fn default() -> Self {
        Self::Naive
    }
}

#[derive(Clone)]
struct DDR4Rank {
    inner: Box<dyn DDR4RankModel>,
}

impl DDR4Rank {
    fn new(option: DDR4RankOption) -> Self {
        match option {
            DDR4RankOption::Naive => Self {
                inner: Box::new(DDR4RankNaive::default()),
            },
            DDR4RankOption::DRAMsim3 {
                config_file,
                output_dir,
            } => Self {
                inner: Box::new(DDR4RankDRAMsim3::new(&config_file, &output_dir)),
            },
        }
    }

    fn transaction(&mut self, addr: u64, is_write: bool) -> usize {
        self.inner.transaction(addr, is_write)
    }
}

impl Default for DDR4Rank {
    fn default() -> Self {
        Self::new(DDR4RankOption::default())
    }
}

// Unit tests for FullyAssociativeCache
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_fully_associative_cache() {
        let mut cache = FullyAssociativeCache::new(64, DDR4RankOption::Naive); // 64 B cache
        assert!(cache.read(0x1000) > FullyAssociativeCache::HIT_LATENCY);
        // Write-through: write always goes to DRAM, even on a cache hit
        assert!(cache.write(0x1000) > FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.read(0x1000), FullyAssociativeCache::HIT_LATENCY);
        assert!(cache.read(0x2000) > FullyAssociativeCache::HIT_LATENCY);
        assert!(cache.write(0x2000) > FullyAssociativeCache::HIT_LATENCY);
        assert!(cache.read(0x1000) > FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.stats.read_hits, 1);
        assert_eq!(cache.stats.read_misses, 3);
        assert_eq!(cache.stats.write_hits, 2);
        assert_eq!(cache.stats.write_misses, 0);
    }

    #[test]
    fn test_set_associative_cache() {
        let mut cache = SetAssociativeCache::new(2, 1, DDR4RankOption::Naive); // 2 sets, 1 way each
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

    #[test]
    fn test_bank_state() {
        let mut bank_state = BankState::default();
        let addr = 0b0_0_0000000_000000;
        // First access to a new row: row miss
        assert_eq!(bank_state.transaction(addr), 22 + 22 + 22 + 4);
        assert_eq!(bank_state.current_row, Some(0));
        // Same row: row hit
        assert_eq!(bank_state.transaction(addr), 22 + 4);
        // Different row: row miss
        let addr = 0b1_00_0000_0_0000000_000000;
        assert_eq!(bank_state.transaction(addr), 22 + 22 + 22 + 4);
        assert_eq!(bank_state.current_row, Some(1));
        // Same row: row hit
        assert_eq!(bank_state.transaction(addr), 22 + 4);
        // Back to row 0: row miss
        let addr = 0b0_0_0000000_000000;
        assert_eq!(bank_state.transaction(addr), 22 + 22 + 22 + 4);
        // Same row (row 0), different column: row hit
        let addr = 0b0_00_0000_0_0000001_000000;
        assert_eq!(bank_state.transaction(addr), 22 + 4);
    }
}
