# TLB and Virtual Memory Address Translation

**Date**: 2026-02-27

## Overview
The simulator lacked TLB modeling: all addresses were treated as raw `u64` values with no distinction between virtual and physical, and no translation latency. We added a set-associative TLB, a dummy page table walker (PTW), explicit `VirtualAddress`/`PhysicalAddress` types, and a VIPT (Virtually Indexed, Physically Tagged) cache latency model. The `--page-size` CLI flag controls TLB configuration at runtime.

## Architecture
- **`src/simulate/memory.rs`**: Defines `VirtualAddress(u64)` and `PhysicalAddress(u64)` newtype wrappers. `PageSize` enum (FourKB, TwoMB, FourMB, OneGB) carries TLB entry count, associativity, and page shift. `PageTableWalker` performs identity mapping (VA == PA) with a fixed 30-cycle latency. `Tlb` is a set-associative LRU cache using `lru::LruCache` per set. `DataCache::read`/`write` now accept `VirtualAddress` and return total latency including TLB effects. Both `FullyAssociativeCache` and `SetAssociativeCache` embed a `Tlb` and implement the VIPT model. All DRAM-facing code (`DDR4RankModel`, `DDR4RankNaive`, `DDR4RankDRAMsim3`, `BankState`) accepts `PhysicalAddress`.
- **`src/cli.rs`**: `PageSizeChoice` enum and `--page-size` argument on `SimulationArgs` (defaults to `FourKB`).
- **`src/simulate/nmpgc/mod.rs`**: Converts `PageSizeChoice` → `PageSize`, passes it through `NMPProcessor::new` to `SetAssociativeCache::new`. Aggregates TLB hits/misses across processors and prints them in both the aggregate summary and per-processor table.
- **`src/simulate/nmpgc/work.rs`**: Wraps `cache.read(o)` / `cache.write(o)` / `cache.read(e as u64)` in `VirtualAddress(...)`.

## Design Decisions & Lessons Learned

### 1. TLB Configuration from cpuid
**Challenge**: Choosing realistic TLB parameters without over-engineering the model.

**Solution**: We sourced entry counts and associativities from the Intel i9-12900KF (Golden Cove P-Core) L1 DTLB via cpuid: 64 entries / 4-way for 4KB, 32 / 4-way for 2MB and 4MB, 8 / fully-associative for 1GB. These are encoded in `PageSize::tlb_entries()` and `PageSize::tlb_ways()`.

### 2. VIPT Latency Model
**Challenge**: A real L1 cache indexes sets with the virtual address in parallel with TLB translation, then matches tags against the physical address. Modeling this accurately without over-complicating the simulator.

**Solution**: We studied gem5's `BaseCache::calculateAccessLatency` and adapted a simplified model:

| TLB   | Cache | Latency formula |
|-------|-------|-----------------|
| Hit   | Hit   | `HIT_LATENCY` (TLB latency hidden—concurrent with set index) |
| Hit   | Miss  | `HIT_LATENCY + DRAM` |
| Miss  | Hit   | `PTW_LATENCY + HIT_LATENCY` (tag match restarts after PTW) |
| Miss  | Miss  | `PTW_LATENCY + HIT_LATENCY + DRAM` |

On a TLB hit, the 1-cycle TLB lookup is fully overlapped with cache set indexing, so it adds zero extra latency. On a TLB miss, the 30-cycle PTW stalls the tag comparison, and the cache access is effectively restarted afterward.

### 3. Identity Mapping in the PTW
**Challenge**: A real page table walk produces arbitrary VA→PA mappings. We needed a PTW that contributes realistic *latency* without needing an actual page table.

**Solution**: `PageTableWalker::translate` returns `PhysicalAddress(va.0)` (identity mapping) after a fixed `LATENCY = 30` cycles. This isolates the TLB miss penalty without coupling to any particular OS page table format. A realistic PTW can replace this method later without changing the TLB or cache interfaces.

### 4. Testing the TLB-Miss-Cache-Hit Combination
**Mistake**: The first version of `test_vipt_tlb_miss_cache_hit` used eviction pages at VPN offsets of `i * num_sets * page_size`. These pages mapped to a *different* TLB set than the target (set 1 vs. set 0), so the target was never evicted from the TLB. After correcting the set calculation, the eviction pages' cache lines collided with the target's cache set, inadvertently evicting the target from the cache too.

**Solution**: We fixed two things: (a) eviction pages use VPN = `(1 + k * num_sets) * page_size` so they share TLB set 1 with the target, and (b) eviction accesses use an intra-page offset of `+0x40` to land in a different cache set. We also increased the cache to 256 sets to eliminate any residual collision.

**Lesson**: When testing multi-level caching structures, verify that test addresses target the *intended* set in *every* level. Derive set indices from the actual indexing formulas (`(vpn >> page_shift) % num_sets` for TLB, `(addr >> line_shift) % num_sets` for cache) rather than assuming stride-based patterns are correct.

## Verification

All 33 unit tests pass:
```
$ cargo test
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The 7 new tests cover:

| Test | What it validates |
|------|-------------------|
| `test_tlb_hit_miss` | Identity mapping correctness, hit/miss latency, stats counters |
| `test_tlb_eviction` | LRU eviction within a single TLB set |
| `test_tlb_page_sizes` | All four page sizes produce correct hit/miss behaviour |
| `test_vipt_tlb_hit_cache_hit` | Concurrent TLB+cache hit returns `HIT_LATENCY` |
| `test_vipt_tlb_hit_cache_miss` | TLB hit with cache miss: no PTW penalty |
| `test_vipt_tlb_miss_cache_hit` | TLB miss with cache hit: PTW + HIT_LATENCY |
| `test_vipt_tlb_miss_cache_miss` | TLB miss with cache miss: PTW + HIT_LATENCY + DRAM |

### Performance Comparison

Naive simulation, 8 processors. "Master" is commit `6073686537bc` (no TLB).

#### `fop/heapdump.2.binpb.zst` (93,180 marked objects)

| Config | Ticks | Time (ms) | Util | Rd Hit Rate | Rd Misses | TLB Hit Rate | TLB Misses | Δ Ticks vs Master |
|:-------|------:|----------:|-----:|------------:|----------:|-------------:|-----------:|------------------:|
| Master (no TLB) | 1,687,031 | 1.054 | 0.812 | 0.717 | 155,732 | — | — | — |
| FourKB | 1,828,509 | 1.143 | 0.818 | 0.714 | 157,156 | 0.950 | 31,882 | +8.4% |
| TwoMB | 1,685,585 | 1.053 | 0.812 | 0.717 | 155,628 | 1.000 | 147 | −0.09% |
| FourMB | 1,685,969 | 1.054 | 0.812 | 0.717 | 155,636 | 1.000 | 80 | −0.06% |
| OneGB | 1,686,293 | 1.054 | 0.812 | 0.717 | 155,655 | 1.000 | 28 | −0.04% |

4KB pages incur 31,882 TLB misses (5.0% miss rate) and 8.4% more total cycles. Pages ≥2MB reduce TLB misses to near-zero on this small workload.

#### `pmd/heapdump.33.binpb.zst` (93 MB — largest available)

| Config | Ticks | Time (ms) | Util | Rd Hit Rate | Rd Misses | TLB Hit Rate | TLB Misses | Δ Ticks vs Master |
|:-------|------:|----------:|-----:|------------:|----------:|-------------:|-----------:|------------------:|
| Master (no TLB) | 69,603,854 | 43.502 | 0.954 | 0.800 | 6,444,897 | — | — | — |
| FourKB | 75,183,455 | 46.990 | 0.946 | 0.801 | 6,428,334 | 0.965 | 1,304,338 | +8.0% |
| TwoMB | 71,850,957 | 44.907 | 0.952 | 0.801 | 6,430,059 | 0.984 | 581,620 | +3.2% |
| FourMB | 71,584,801 | 44.741 | 0.950 | 0.800 | 6,443,929 | 0.988 | 437,852 | +2.8% |
| OneGB | 69,666,466 | 43.542 | 0.953 | 0.800 | 6,442,425 | 1.000 | 29 | +0.09% |

The larger workload exposes meaningful TLB pressure across all page sizes. 4KB pages produce 1.3M TLB misses (3.5% miss rate), costing 8% more cycles. Even 2MB and 4MB pages show 2.8–3.2% overhead due to hundreds of thousands of TLB misses — this workload's address space exceeds the 32-entry TLB capacity at these page sizes. Only 1GB pages eliminate TLB overhead entirely (29 cold misses).

## Usage
```
cargo run -- <HEAPDUMP> -o OpenJDK simulate -p 8 -a NMPGC --page-size FourMB
```
Valid `--page-size` values: `FourKB`, `TwoMB`, `FourMB` (default), `OneGB`.

## Known Limitations
- The PTW uses identity mapping. Realistic page table walks with variable latency are not modelled.
- Only a single-level TLB is modelled; real processors have L1/L2 TLBs.
- The TLB is not shared across processors—each `NMPProcessor` has its own independent TLB.
