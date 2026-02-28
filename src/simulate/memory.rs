use bitfield::bitfield;
use clap::ValueEnum;
use lru::LruCache;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::num::NonZeroUsize;

/// log2 of the cache line size in bytes.
const LOG_LINE_SIZE: usize = 6;
/// Cache line size in bytes.
const LINE_SIZE: usize = 1 << LOG_LINE_SIZE;

/// Processor Work references virtual addresses which represents heap objects and references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VirtualAddress(pub u64);

impl VirtualAddress {
    /// Returns the virtual page number (VPN): the address with page-offset bits
    /// zeroed.  This is the full address-width value with the offset portion
    /// cleared, not just the high-order page-number bits.
    pub fn vpn(self, page_size: PageSize) -> u64 {
        self.0 & page_size.page_mask()
    }

    /// Constructs a physical address by combining the given PPN with this
    /// virtual address's page offset.
    pub fn to_physical(self, ppn: u64, page_size: PageSize) -> PhysicalAddress {
        PhysicalAddress(ppn | page_size.page_offset(self.0))
    }
}

/// A physical address after address translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicalAddress(pub u64);

impl PhysicalAddress {
    /// Returns the physical page number (PPN): the address with page-offset
    /// bits zeroed.  Like VPN, this is the full address-width value with the
    /// offset portion cleared, not just the high-order bits.
    pub fn ppn(self, page_size: PageSize) -> u64 {
        self.0 & page_size.page_mask()
    }

    /// Returns the cache line index for this physical address.
    fn cache_line(self) -> u64 {
        self.0 >> LOG_LINE_SIZE
    }
}

// ---------------------------------------------------------------------------
// Page sizes and TLB configuration
// (cpuid on Intel i9-12900KF Golden Cove P-Core)
// ---------------------------------------------------------------------------

/// Supported x86_64 page sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "verbatim")]
pub enum PageSize {
    FourKB,
    TwoMB,
    FourMB,
    OneGB,
}

impl PageSize {
    /// log2 of the page size in bytes.
    pub fn page_shift(self) -> u32 {
        match self {
            PageSize::FourKB => 12,
            PageSize::TwoMB => 21,
            PageSize::FourMB => 22,
            PageSize::OneGB => 30,
        }
    }

    fn page_mask(self) -> u64 {
        !((1u64 << self.page_shift()) - 1)
    }

    /// Extracts the within-page offset from an address.
    pub fn page_offset(self, addr: u64) -> u64 {
        addr & !self.page_mask()
    }
}

// ---------------------------------------------------------------------------
// TLB statistics
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
pub(super) struct TlbStats {
    pub(super) read_hits: usize,
    pub(super) read_misses: usize,
    pub(super) write_hits: usize,
    pub(super) write_misses: usize,
}

impl TlbStats {
    #[cfg(test)]
    pub(super) fn total_hits(&self) -> usize {
        self.read_hits + self.write_hits
    }
    #[cfg(test)]
    pub(super) fn total_misses(&self) -> usize {
        self.read_misses + self.write_misses
    }
}

// ---------------------------------------------------------------------------
// Page Table Walker (dummy identity mapping)
// ---------------------------------------------------------------------------

/// Dummy page table walker that maps VA == PA.
///
/// Latency varies by page size, modelling the number of page table levels
/// traversed in an Sv39/Sv48-style radix tree (as used by RISC-V and
/// similar to x86_64 four-level paging).
struct PageTableWalker;

impl PageTableWalker {
    /// Latency in cycles for a page table walk, determined by the number
    /// of levels traversed.  Each level costs ~6 cycles (L2/L3 hit for
    /// the page table entry).
    fn latency(page_size: PageSize) -> usize {
        match page_size {
            // 4 levels: PML4 → PDP → PD → PT
            PageSize::FourKB => 30,
            // 3 levels: PML4 → PDP → PD (large page)
            PageSize::TwoMB | PageSize::FourMB => 24,
            // 2 levels: PML4 → PDP (huge page)
            PageSize::OneGB => 18,
        }
    }

    fn walk(&self, vaddr: VirtualAddress, page_size: PageSize) -> (PhysicalAddress, usize) {
        // FIXME: this 1:1 identity translation could produce physical addresses
        // outside the range of the underlying memory model (currently 36 bits /
        // 64 GiB), and virtual addresses that differ only in higher bits are
        // mapped to the same physical address, inflating locality.
        (PhysicalAddress(vaddr.0), Self::latency(page_size))
    }
}

// ---------------------------------------------------------------------------
// TLB
// ---------------------------------------------------------------------------

/// Response from a TLB translation.
pub(super) struct TlbResp {
    pub(super) paddr: PhysicalAddress,
    pub(super) latency: usize,
    pub(super) hit: bool,
}

pub(super) struct Tlb {
    /// Each set is an LRU cache mapping VPN → PPN.  Both VPN and PPN are
    /// zero-padded to the full address width (i.e., the page-offset bits are
    /// zeroed, not stripped).
    sets: Vec<LruCache<u64, u64>>,
    page_size: PageSize,
    ptw: PageTableWalker,
    pub(super) stats: TlbStats,
}

impl Tlb {
    // TODO: we currently have Tlb::HIT_LATENCY < SetAssociativeCache::HIT_LATENCY
    // and for the SetAssociativeCache using VIPT, there is no extra penalty
    // when both TLB and cache hit.
    // Verify whether this behavior is realistic.
    pub const HIT_LATENCY: usize = 1;

    /// Number of TLB entries for a given page size.
    fn tlb_entries(page_size: PageSize) -> usize {
        match page_size {
            PageSize::FourKB => 64,
            PageSize::TwoMB | PageSize::FourMB => 32,
            PageSize::OneGB => 8,
        }
    }

    /// Number of TLB ways (sets = entries / ways).
    fn tlb_ways(page_size: PageSize) -> usize {
        match page_size {
            PageSize::FourKB => 4,
            PageSize::TwoMB | PageSize::FourMB => 4,
            // Fully associative: ways == entries.
            PageSize::OneGB => 8,
        }
    }

    pub fn new(page_size: PageSize) -> Self {
        let entries = Self::tlb_entries(page_size);
        let ways = Self::tlb_ways(page_size);
        debug_assert!(
            entries.is_multiple_of(ways),
            "TLB entries ({}) must be divisible by ways ({})",
            entries,
            ways
        );
        let num_sets = entries / ways;
        let sets = (0..num_sets)
            .map(|_| LruCache::new(NonZeroUsize::new(ways).unwrap()))
            .collect();
        Tlb {
            sets,
            page_size,
            ptw: PageTableWalker,
            stats: TlbStats::default(),
        }
    }

    fn get_setidx(&self, vpn: u64) -> usize {
        (vpn >> self.page_size.page_shift()) as usize % self.sets.len()
    }

    /// Translates a virtual address to a physical address via the TLB.
    pub fn translate(&mut self, vaddr: VirtualAddress, is_write: bool) -> TlbResp {
        let vpn = vaddr.vpn(self.page_size);
        let setidx = self.get_setidx(vpn);
        if let Some(&ppn) = self.sets[setidx].get(&vpn) {
            if is_write {
                self.stats.write_hits += 1;
            } else {
                self.stats.read_hits += 1;
            }
            TlbResp {
                paddr: vaddr.to_physical(ppn, self.page_size),
                latency: Self::HIT_LATENCY,
                hit: true,
            }
        } else {
            if is_write {
                self.stats.write_misses += 1;
            } else {
                self.stats.read_misses += 1;
            }
            let (paddr, ptw_latency) = self.ptw.walk(vaddr, self.page_size);
            let ppn = paddr.ppn(self.page_size);
            self.sets[setidx].put(vpn, ppn);
            TlbResp {
                paddr,
                latency: ptw_latency,
                hit: false,
            }
        }
    }
}

/// Data cache trait for word-aligned memory reads and writes.
pub(super) trait DataCache {
    /// Cache hit latency in cycles.
    const HIT_LATENCY: usize = 4;
    /// Reads a word from the cache, returning the latency.
    fn read(&mut self, addr: VirtualAddress) -> usize;
    /// Writes a word to the cache, returning the latency.
    fn write(&mut self, addr: VirtualAddress) -> usize;
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
    pub(super) tlb: Tlb,
}

impl FullyAssociativeCache {
    #[allow(dead_code)]
    pub fn new(capacity: usize, rank_option: DDR4RankOption, page_size: PageSize) -> Self {
        assert!(
            capacity >= LINE_SIZE && capacity.is_multiple_of(LINE_SIZE),
            "Cache capacity must be a multiple of line size"
        );
        FullyAssociativeCache {
            cache: LruCache::new(NonZeroUsize::new(capacity / LINE_SIZE).unwrap()),
            stats: CacheStats::default(),
            rank: DDR4Rank::new(rank_option),
            tlb: Tlb::new(page_size),
        }
    }
}

impl DataCache for FullyAssociativeCache {
    fn read(&mut self, addr: VirtualAddress) -> usize {
        // Fully-associative: no set-index bits to support VIPT, and has to be
        // physically tagged, and that address translation must complete
        // before the cache tag comparison.
        let tlb_resp = self.tlb.translate(addr, false);
        let line = tlb_resp.paddr.cache_line();
        if self.cache.get(&line).is_some() {
            self.stats.read_hits += 1;
            tlb_resp.latency + Self::HIT_LATENCY
        } else {
            self.cache.put(line, ());
            self.stats.read_misses += 1;
            tlb_resp.latency + Self::HIT_LATENCY + self.rank.transaction(tlb_resp.paddr, false)
        }
    }

    /// Write-through: every write is forwarded to DRAM regardless of cache
    /// state. The cache line is allocated (write-allocate) so subsequent reads
    /// can hit.
    fn write(&mut self, addr: VirtualAddress) -> usize {
        let tlb_resp = self.tlb.translate(addr, true);
        let line = tlb_resp.paddr.cache_line();
        if self.cache.get(&line).is_some() {
            self.stats.write_hits += 1;
        } else {
            self.cache.put(line, ());
            self.stats.write_misses += 1;
        }
        tlb_resp.latency + Self::HIT_LATENCY + self.rank.transaction(tlb_resp.paddr, true)
    }
}

pub(super) struct SetAssociativeCache {
    cache_sets: Vec<LruCache<u64, ()>>,
    rank: DDR4Rank,
    pub(super) stats: CacheStats,
    pub(super) tlb: Tlb,
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
    /// Creates a new set-associative cache.
    ///
    /// # VIPT constraint
    ///
    /// The set-index bits must fall entirely within the page offset so that
    /// the cache can be indexed with the virtual address in parallel with
    /// TLB translation (Virtually Indexed, Physically Tagged).
    ///
    /// Because set indexing uses address bits that are unchanged by
    /// translation (i.e., bits within the page offset), multiple virtual aliases
    /// of the same physical page index the same cache set and cannot cause
    /// consistency issues.
    /// See <https://comp.anu.edu.au/courses/comp3710-uarch/assets/lectures/week11-part2.pdf>.
    pub fn new(
        num_sets: usize,
        num_ways: usize,
        rank_option: DDR4RankOption,
        page_size: PageSize,
    ) -> Self {
        assert!(
            num_sets > 0 && num_ways > 0,
            "Number of sets and ways must be greater than zero"
        );
        // VIPT invariant: the highest set-index bit must be below the page
        // offset.  Set index uses bits [LOG_LINE_SIZE .. LOG_LINE_SIZE + log2(num_sets)).
        // The page offset covers bits [0 .. page_shift).  For VIPT to work
        // correctly, LOG_LINE_SIZE + log2(num_sets) <= page_shift.
        debug_assert!(
            num_sets.is_power_of_two(),
            "num_sets must be a power of two"
        );
        let set_index_bits = num_sets.trailing_zeros() as usize;
        debug_assert!(
            LOG_LINE_SIZE + set_index_bits <= page_size.page_shift() as usize,
            "VIPT invariant violated: set-index bits [{}..{}) exceed page offset {} for {:?}",
            LOG_LINE_SIZE,
            LOG_LINE_SIZE + set_index_bits,
            page_size.page_shift(),
            page_size,
        );
        let cache_sets = (0..num_sets)
            .map(|_| LruCache::new(NonZeroUsize::new(num_ways).unwrap()))
            .collect();
        SetAssociativeCache {
            cache_sets,
            stats: CacheStats::default(),
            rank: DDR4Rank::new(rank_option),
            tlb: Tlb::new(page_size),
        }
    }

    /// Extracts the set-index bits from the virtual address.
    ///
    /// See the VIPT invariant in the constructor.
    ///
    /// Note that the following implementation is probably equivalent
    /// ```rust
    /// let line = vaddr.0 >> LOG_LINE_SIZE;
    /// (line as usize) % self.cache_sets.len()
    /// ```
    ///
    /// But the following implementation is a more straightforward implementation
    /// of the spec.
    fn get_setidx(&self, vaddr: VirtualAddress) -> usize {
        let set_index_mask = (self.cache_sets.len() - 1) as u64;
        ((vaddr.0 >> LOG_LINE_SIZE) & set_index_mask) as usize
    }
}

impl DataCache for SetAssociativeCache {
    fn read(&mut self, addr: VirtualAddress) -> usize {
        let setidx = self.get_setidx(addr);
        let tlb_resp = self.tlb.translate(addr, false);
        let line = tlb_resp.paddr.cache_line();
        if self.cache_sets[setidx].get(&line).is_some() {
            self.stats.read_hits += 1;
            if tlb_resp.hit {
                Self::HIT_LATENCY
            } else {
                tlb_resp.latency + Self::HIT_LATENCY
            }
        } else {
            self.cache_sets[setidx].put(line, ());
            self.stats.read_misses += 1;
            if tlb_resp.hit {
                Self::HIT_LATENCY + self.rank.transaction(tlb_resp.paddr, false)
            } else {
                tlb_resp.latency + Self::HIT_LATENCY + self.rank.transaction(tlb_resp.paddr, false)
            }
        }
    }

    /// Write-through: every write is forwarded to DRAM regardless of cache
    /// state. The cache line is allocated (write-allocate) so subsequent reads
    /// can hit.
    fn write(&mut self, addr: VirtualAddress) -> usize {
        let setidx = self.get_setidx(addr);
        let tlb_resp = self.tlb.translate(addr, true);
        let line = tlb_resp.paddr.cache_line();
        if self.cache_sets[setidx].get(&line).is_some() {
            self.stats.write_hits += 1;
        } else {
            self.cache_sets[setidx].put(line, ());
            self.stats.write_misses += 1;
        }
        let base = if tlb_resp.hit {
            Self::HIT_LATENCY
        } else {
            tlb_resp.latency + Self::HIT_LATENCY
        };
        base + self.rank.transaction(tlb_resp.paddr, true)
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
    pub(crate) fn as_dict(&self) -> HashMap<String, Value> {
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
    fn transaction(&mut self, addr: PhysicalAddress) -> usize {
        let mapping = AddressMapping(addr.0);
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
    fn transaction(&mut self, addr: PhysicalAddress, is_write: bool) -> usize;
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
    fn transaction(&mut self, addr: PhysicalAddress, _is_write: bool) -> usize {
        let mapping = AddressMapping(addr.0);
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

    fn add_transaction(&self, addr: PhysicalAddress, is_write: bool) {
        unsafe {
            ffi::dramsim3_add_transaction(self.wrapper, addr.0, is_write);
        }
    }

    fn clock_tick(&self) {
        unsafe {
            ffi::dramsim3_clock_tick(self.wrapper);
        }
    }

    fn will_accept_transaction(&self, addr: PhysicalAddress, is_write: bool) -> bool {
        unsafe { ffi::dramsim3_will_accept_transaction(self.wrapper, addr.0, is_write) }
    }

    fn is_transaction_done(&self, addr: PhysicalAddress, is_write: bool) -> bool {
        unsafe { ffi::dramsim3_is_transaction_done(self.wrapper, addr.0, is_write) }
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

    fn run_transaction(&self, addr: PhysicalAddress, is_write: bool) -> usize {
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
                    addr.0
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
                    addr.0
                );
                break;
            }
        }
        ticks
    }
}

impl DDR4RankModel for DDR4RankDRAMsim3 {
    fn transaction(&mut self, addr: PhysicalAddress, is_write: bool) -> usize {
        self.run_transaction(addr, is_write)
    }

    fn clone_box(&self) -> Box<dyn DDR4RankModel> {
        // Create a new instance with the same configuration.
        // This effectively gives a fresh memory simulation for the new rank.
        Box::new(Self::new(&self.config_file, &self.output_dir))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DDR4RankOption {
    #[default]
    Naive,
    DRAMsim3 {
        config_file: String,
        output_dir: String,
    },
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

    fn transaction(&mut self, addr: PhysicalAddress, is_write: bool) -> usize {
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
        let mut cache = FullyAssociativeCache::new(64, DDR4RankOption::Naive, PageSize::FourKB);
        // First access to page: TLB miss, cache miss → includes PTW + DRAM
        assert!(cache.read(VirtualAddress(0x1000)) > FullyAssociativeCache::HIT_LATENCY);
        // Same page, cache hit, TLB hit → write still goes to DRAM (write-through)
        assert!(cache.write(VirtualAddress(0x1000)) > FullyAssociativeCache::HIT_LATENCY);
        // Same line, TLB hit (serialized), cache hit
        assert_eq!(
            cache.read(VirtualAddress(0x1000)),
            Tlb::HIT_LATENCY + FullyAssociativeCache::HIT_LATENCY
        );
        // Different page: TLB miss, cache miss
        assert!(cache.read(VirtualAddress(0x2000)) > FullyAssociativeCache::HIT_LATENCY);
        // Same page as 0x2000: TLB hit, write always → DRAM
        assert!(cache.write(VirtualAddress(0x2000)) > FullyAssociativeCache::HIT_LATENCY);
        // 0x1000 evicted from cache (capacity = 1 line) → cache miss
        assert!(cache.read(VirtualAddress(0x1000)) > FullyAssociativeCache::HIT_LATENCY);
        assert_eq!(cache.stats.read_hits, 1);
        assert_eq!(cache.stats.read_misses, 3);
        assert_eq!(cache.stats.write_hits, 2);
        assert_eq!(cache.stats.write_misses, 0);
    }

    #[test]
    fn test_set_associative_cache() {
        let mut cache = SetAssociativeCache::new(2, 1, DDR4RankOption::Naive, PageSize::FourKB);
        // First access: TLB miss + cache miss
        assert!(cache.read(VirtualAddress(0)) > SetAssociativeCache::HIT_LATENCY);
        // Same page + same line: TLB hit + cache hit
        assert_eq!(
            cache.read(VirtualAddress(0)),
            SetAssociativeCache::HIT_LATENCY
        );
        // Same page, different line
        assert!(cache.read(VirtualAddress(64)) > SetAssociativeCache::HIT_LATENCY);
        assert_eq!(
            cache.read(VirtualAddress(64)),
            SetAssociativeCache::HIT_LATENCY
        );
        // Same page, another line → evicts line 0 from its set
        assert!(cache.read(VirtualAddress(128)) > SetAssociativeCache::HIT_LATENCY);
        assert_eq!(
            cache.read(VirtualAddress(128)),
            SetAssociativeCache::HIT_LATENCY
        );
        // Line 0 was evicted → cache miss (TLB still hit for this page)
        assert!(cache.read(VirtualAddress(0)) > SetAssociativeCache::HIT_LATENCY);
        // Line 64 should still be in cache (different set)
        assert_eq!(
            cache.read(VirtualAddress(64)),
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
        let addr = PhysicalAddress(0b0_0_0000000_000000);
        // First access to a new row: row miss
        assert_eq!(bank_state.transaction(addr), 22 + 22 + 22 + 4);
        assert_eq!(bank_state.current_row, Some(0));
        // Same row: row hit
        assert_eq!(bank_state.transaction(addr), 22 + 4);
        // Different row: row miss
        let addr = PhysicalAddress(0b1_00_0000_0_0000000_000000);
        assert_eq!(bank_state.transaction(addr), 22 + 22 + 22 + 4);
        assert_eq!(bank_state.current_row, Some(1));
        // Same row: row hit
        assert_eq!(bank_state.transaction(addr), 22 + 4);
        // Back to row 0: row miss
        let addr = PhysicalAddress(0b0_0_0000000_000000);
        assert_eq!(bank_state.transaction(addr), 22 + 22 + 22 + 4);
        // Same row (row 0), different column: row hit
        let addr = PhysicalAddress(0b0_00_0000_0_0000001_000000);
        assert_eq!(bank_state.transaction(addr), 22 + 4);
    }

    // ------- TLB-specific tests -------

    #[test]
    fn test_tlb_hit_miss() {
        let mut tlb = Tlb::new(PageSize::FourKB);
        // Miss on first access (read)
        let resp = tlb.translate(VirtualAddress(0x1000), false);
        assert_eq!(resp.paddr, PhysicalAddress(0x1000)); // identity mapping
        assert_eq!(resp.latency, PageTableWalker::latency(PageSize::FourKB));
        assert!(!resp.hit);
        assert_eq!(tlb.stats.read_misses, 1);

        // Hit on same page (read)
        let resp = tlb.translate(VirtualAddress(0x1042), false);
        assert_eq!(resp.paddr, PhysicalAddress(0x1042));
        assert_eq!(resp.latency, Tlb::HIT_LATENCY);
        assert!(resp.hit);
        assert_eq!(tlb.stats.read_hits, 1);
    }

    #[test]
    fn test_tlb_eviction() {
        let mut tlb = Tlb::new(PageSize::FourKB);
        // 64 entries, 4-way, 16 sets. Fill one set (4 pages mapping to same set)
        let pages_per_set = Tlb::tlb_ways(PageSize::FourKB);
        let num_sets = Tlb::tlb_entries(PageSize::FourKB) / pages_per_set;
        // Access pages that all map to set 0: strides of num_sets pages
        for i in 0..=pages_per_set {
            let addr = VirtualAddress((i as u64) * (num_sets as u64) * (1u64 << 12));
            tlb.translate(addr, false);
        }
        // The first page should have been evicted (LRU), re-access → miss
        let resp = tlb.translate(VirtualAddress(0), false);
        assert_eq!(resp.latency, PageTableWalker::latency(PageSize::FourKB));
        assert!(!resp.hit);
    }

    #[test]
    fn test_tlb_page_sizes() {
        for ps in [
            PageSize::FourKB,
            PageSize::TwoMB,
            PageSize::FourMB,
            PageSize::OneGB,
        ] {
            let mut tlb = Tlb::new(ps);
            let base = 1u64 << ps.page_shift();
            // First access: miss
            let resp = tlb.translate(VirtualAddress(base), false);
            assert_eq!(resp.latency, PageTableWalker::latency(ps));
            assert!(!resp.hit);
            // Same page, different offset: hit
            let resp = tlb.translate(VirtualAddress(base + 64), false);
            assert_eq!(resp.latency, Tlb::HIT_LATENCY);
            assert!(resp.hit);
        }
    }

    #[test]
    fn test_tlb_read_write_stats() {
        let mut tlb = Tlb::new(PageSize::FourKB);
        // Read miss
        tlb.translate(VirtualAddress(0x1000), false);
        assert_eq!(tlb.stats.read_misses, 1);
        assert_eq!(tlb.stats.write_misses, 0);
        // Write hit (same page)
        tlb.translate(VirtualAddress(0x1040), true);
        assert_eq!(tlb.stats.write_hits, 1);
        assert_eq!(tlb.stats.read_hits, 0);
        // Read hit
        tlb.translate(VirtualAddress(0x1080), false);
        assert_eq!(tlb.stats.read_hits, 1);
        // Write miss (new page)
        tlb.translate(VirtualAddress(0x2000), true);
        assert_eq!(tlb.stats.write_misses, 1);
        // Totals
        assert_eq!(tlb.stats.total_hits(), 2);
        assert_eq!(tlb.stats.total_misses(), 2);
    }

    // ------- VIPT combination tests -------

    #[test]
    fn test_vipt_tlb_hit_cache_hit() {
        let mut cache = SetAssociativeCache::new(16, 4, DDR4RankOption::Naive, PageSize::FourKB);
        // Warm up both TLB and cache
        cache.read(VirtualAddress(0x1000));
        // TLB hit + cache hit
        let lat = cache.read(VirtualAddress(0x1000));
        assert_eq!(lat, SetAssociativeCache::HIT_LATENCY);
    }

    #[test]
    fn test_vipt_tlb_hit_cache_miss() {
        let mut cache = SetAssociativeCache::new(16, 4, DDR4RankOption::Naive, PageSize::FourKB);
        // Warm up TLB for 0x1xxx page
        cache.read(VirtualAddress(0x1000));
        // Access different line on same page: TLB hit, cache miss
        let lat = cache.read(VirtualAddress(0x1100));
        let ptw = PageTableWalker::latency(PageSize::FourKB);
        // cache miss → HIT_LATENCY + DRAM, no PTW penalty
        assert!(lat > SetAssociativeCache::HIT_LATENCY);
        assert!(lat < ptw + SetAssociativeCache::HIT_LATENCY);
    }

    #[test]
    fn test_vipt_tlb_miss_cache_hit() {
        // 64 sets is the maximum for VIPT with 4KB pages (set-index bits [6..12)
        // must stay within the 12-bit page offset).
        let mut cache = SetAssociativeCache::new(64, 4, DDR4RankOption::Naive, PageSize::FourKB);
        let ptw = PageTableWalker::latency(PageSize::FourKB);
        // Warm TLB + cache for page 0x1000 (VPN page number 1, TLB set 1).
        cache.read(VirtualAddress(0x1000));
        assert_eq!(
            cache.read(VirtualAddress(0x1000)),
            SetAssociativeCache::HIT_LATENCY
        );
        // Evict TLB entry by filling TLB set 1 with other pages.
        // Pages whose page number ≡ 1 (mod num_sets) share TLB set 1:
        //   page numbers 1, 17, 33, 49, 65  (i.e., 1 + k*16 for k=0..4)
        // We skip k=0 (that's the target page 0x1000) and use k=1..=4.
        let num_sets = Tlb::tlb_entries(PageSize::FourKB) / Tlb::tlb_ways(PageSize::FourKB);
        for k in 1..=Tlb::tlb_ways(PageSize::FourKB) {
            let page_num = 1 + k * num_sets;
            let base = (page_num as u64) * (1u64 << 12);
            // Offset by +0x40 to avoid colliding with 0x1000's cache set.
            cache.read(VirtualAddress(base + 0x40));
        }
        // 0x1000's TLB entry was evicted (LRU), but its cache line survives.
        let lat = cache.read(VirtualAddress(0x1000));
        assert_eq!(lat, ptw + SetAssociativeCache::HIT_LATENCY);
    }

    #[test]
    fn test_vipt_tlb_miss_cache_miss() {
        let mut cache = SetAssociativeCache::new(16, 4, DDR4RankOption::Naive, PageSize::FourKB);
        let ptw = PageTableWalker::latency(PageSize::FourKB);
        // Very first access: TLB miss + cache miss
        let lat = cache.read(VirtualAddress(0x1000));
        // Must include PTW + cache hit latency + DRAM
        assert!(lat >= ptw + SetAssociativeCache::HIT_LATENCY);
    }
}
