# TLB and Virtual Memory Address Translation

**Date**: 2026-02-27

## Overview
The simulator lacked TLB modeling: all addresses were raw `u64` values with no virtual/physical distinction and no translation latency. We added a set-associative TLB, a dummy page table walker (PTW), explicit `VirtualAddress`/`PhysicalAddress` newtype wrappers, and a VIPT (Virtually Indexed, Physically Tagged) cache latency model for set-associative caches. The `--page-size` CLI flag controls the page-size configuration at runtime.

## Architecture
- **`src/simulate/memory.rs`** — `VirtualAddress(u64)` and `PhysicalAddress(u64)` newtype wrappers enforce address-space separation at the type level.
- **`src/simulate/memory.rs`** — `PageSize` enum (`FourKB`, `TwoMB`, `FourMB`, `OneGB`) carries page shift; `Tlb` methods `tlb_entries()` and `tlb_ways()` derive entry count and associativity per page size.
- **`src/simulate/memory.rs`** — `PageTableWalker` performs identity mapping (VA == PA) with variable latency by page size (30/24/24/18 cycles for 4KB/2MB/4MB/1GB, modelling multi-level radix tree depth).
- **`src/simulate/memory.rs`** — `Tlb` is a set-associative LRU cache (`lru::LruCache` per set) with read/write split statistics.
- **`src/simulate/memory.rs`** — `FullyAssociativeCache` and `SetAssociativeCache` each embed a `Tlb`. `SetAssociativeCache` implements the VIPT latency model (TLB hit cost hidden by parallel set indexing); `FullyAssociativeCache` serializes TLB translation before cache lookup. A `debug_assert!` in `SetAssociativeCache::new` guards the set-index-in-page-offset invariant. All DRAM-facing code (`DDR4RankModel`, `DDR4RankNaive`, `DDR4RankDRAMsim3`, `BankState`) accepts `PhysicalAddress`.
- **`src/cli.rs`** — `--page-size` argument on `SimulationArgs` (defaults to `FourMB`).
- **`src/simulate/nmpgc/mod.rs`** — Passes `PageSize` through `NMPProcessor::new` → `SetAssociativeCache::new`. Aggregates TLB hits/misses across processors for the summary table.
- **`src/simulate/nmpgc/work.rs`** — Wraps all cache access calls with `VirtualAddress(…)`.

## Design Decisions & Lessons Learned

### 1. TLB Configuration from cpuid
**Challenge**: Choosing realistic TLB parameters without over-engineering the model.

**Solution**: We sourced entry counts and associativities from the Intel i9-12900KF (Golden Cove P-Core) L1 DTLB via cpuid: 64 entries / 4-way for 4KB, 32 / 4-way for 2MB and 4MB, 8 / fully-associative for 1GB. These are encoded in `Tlb::tlb_entries()` and `Tlb::tlb_ways()`.

### 2. VIPT Latency Model for Set-Associative Caches
**Challenge**: A real L1 cache indexes sets with the virtual address in parallel with TLB translation, then matches tags against the physical address. Modeling this accurately without over-complicating the simulator.

**Solution**: We studied gem5's `BaseCache::calculateAccessLatency` and adapted a simplified model:

| TLB   | Cache | Latency formula |
|-------|-------|-----------------|
| Hit   | Hit   | `HIT_LATENCY` (TLB latency hidden—concurrent with set index) |
| Hit   | Miss  | `HIT_LATENCY + DRAM` |
| Miss  | Hit   | `PTW_LATENCY + HIT_LATENCY` (tag match restarts after PTW) |
| Miss  | Miss  | `PTW_LATENCY + HIT_LATENCY + DRAM` |

On a TLB hit, the lookup overlaps with set indexing (zero added latency). On a TLB miss, the PTW stalls the tag comparison and the cache access restarts afterward.

### 3. Identity Mapping in the PTW
**Challenge**: A real distributed page table walk is very complicated. One needs to implement a distributed protcol similar to heap traversal, and that the page table entries may or may not be in the data cache. We needed a PTW that somewhat models realistic *latency* without needing an actual page table.

**Solution**: `PageTableWalker::walk` returns `(PhysicalAddress(va.0), latency)` where latency is 18–30 cycles depending on page size (larger page granularity reduces the levels of page table one needs to traverse through).

## Verification

All 34 unit tests pass. `cargo clippy` and `cargo fmt` produce no warnings or changes.
```
$ cargo test
test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The 8 new tests cover:

| Test | What it validates |
|------|-------------------|
| `test_tlb_hit_miss` | Identity mapping correctness, hit/miss latency, stats counters |
| `test_tlb_eviction` | LRU eviction within a single TLB set |
| `test_tlb_page_sizes` | All four page sizes produce correct hit/miss behaviour |
| `test_tlb_read_write_stats` | Read/write split statistics are tracked independently |
| `test_vipt_tlb_hit_cache_hit` | Concurrent TLB+cache hit returns `HIT_LATENCY` |
| `test_vipt_tlb_hit_cache_miss` | TLB hit with cache miss: no PTW penalty |
| `test_vipt_tlb_miss_cache_hit` | TLB miss with cache hit: PTW + HIT_LATENCY |
| `test_vipt_tlb_miss_cache_miss` | TLB miss with cache miss: PTW + HIT_LATENCY + DRAM |

### Performance Comparison

Naive simulation, 8 processors. "master" is commit `6073686537bc` (no TLB).
PTW latency is now variable: 30/24/24/18 cycles for 4KB/2MB/4MB/1GB.

#### `fop/heapdump.2.binpb.zst` (93,180 marked objects)

| Config | Ticks | Time (ms) | Util | Rd Hit Rate | Rd Misses | TLB Hit Rate | TLB Misses | Δ Ticks vs master |
|:-------|------:|----------:|-----:|------------:|----------:|-------------:|-----------:|------------------:|
| master (no TLB) | 1,687,031 | 1.054 | 0.812 | 0.717 | 155,732 | — | — | — |
| FourKB | 1,828,509 | 1.143 | 0.818 | 0.714 | 157,156 | 0.950 | 31,882 | +8.4% |
| TwoMB | 1,689,009 | 1.056 | 0.812 | 0.717 | 155,813 | 1.000 | 147 | +0.12% |
| FourMB | 1,687,203 | 1.055 | 0.812 | 0.717 | 155,635 | 1.000 | 80 | +0.01% |
| OneGB | 1,685,737 | 1.054 | 0.812 | 0.717 | 155,528 | 1.000 | 28 | −0.08% |

4KB pages incur 31,882 TLB misses (5.0% miss rate) and 8.4% more total cycles. Pages ≥2MB reduce TLB misses to near-zero on this small workload. OneGB is slightly faster than master due to the bufferfly effect.

#### `pmd/heapdump.33.binpb.zst` (93 MB — largest available)

| Config | Ticks | Time (ms) | Util | Rd Hit Rate | Rd Misses | TLB Hit Rate | TLB Misses | Δ Ticks vs master |
|:-------|------:|----------:|-----:|------------:|----------:|-------------:|-----------:|------------------:|
| master (no TLB) | 69,603,854 | 43.502 | 0.954 | 0.800 | 6,444,897 | — | — | — |
| FourKB | 75,183,455 | 46.990 | 0.946 | 0.801 | 6,428,334 | 0.965 | 1,304,338 | +8.0% |
| TwoMB | 71,391,935 | 44.620 | 0.954 | 0.800 | 6,437,133 | 0.984 | 593,234 | +2.6% |
| FourMB | 71,018,727 | 44.387 | 0.952 | 0.800 | 6,439,991 | 0.988 | 436,114 | +2.0% |
| OneGB | 69,548,166 | 43.468 | 0.953 | 0.800 | 6,433,695 | 1.000 | 29 | −0.08% |

The larger workload exposes meaningful TLB pressure across all page sizes. 4KB pages produce 1.3M TLB misses (3.5% miss rate), costing 8% more cycles. 2MB and 4MB pages show 2.0–2.6% overhead. Only 1GB pages impose negligible TLB overhead.

## Usage
```
cargo run -- <HEAPDUMP> -o OpenJDK simulate -p 8 -a NMPGC --page-size FourMB
```
Valid `--page-size` values: `FourKB`, `TwoMB`, `FourMB` (default), `OneGB`.

## Known Limitations
- The PTW uses identity mapping and a simplistic latency model; a more realistic implementation would need to implement distributed PTWs, implement a physical memory allocation scheme that produces a concrete page table, and make each level of page table entries cacheable.
- Currently we assume that the memory traffic from page table walks do not compete with the heap traversal traffic, which is too optimistic.
