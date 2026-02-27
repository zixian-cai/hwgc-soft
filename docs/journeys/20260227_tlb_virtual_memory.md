# TLB and Virtual Memory Address Translation

**Date**: 2026-02-27

## Overview
The simulator lacked TLB modeling: all addresses were raw `u64` values with no virtual/physical distinction and no translation latency. We added a set-associative TLB, a dummy page table walker (PTW), explicit `VirtualAddress`/`PhysicalAddress` newtype wrappers, and a VIPT (Virtually Indexed, Physically Tagged) cache latency model. The `--page-size` CLI flag controls TLB configuration at runtime.

## Architecture
- **`src/simulate/memory.rs`** — `VirtualAddress(u64)` and `PhysicalAddress(u64)` newtype wrappers enforce address-space separation at the type level.
- **`src/simulate/memory.rs`** — `PageSize` enum (`FourKB`, `TwoMB`, `FourMB`, `OneGB`) carries TLB entry count, associativity, and page shift.
- **`src/simulate/memory.rs`** — `PageTableWalker` performs identity mapping (VA == PA) with variable latency by page size (30/24/24/18 cycles for 4KB/2MB/4MB/1GB, modelling multi-level radix tree depth).
- **`src/simulate/memory.rs`** — `Tlb` is a set-associative LRU cache (`lru::LruCache` per set) with read/write split statistics.
- **`src/simulate/memory.rs`** — `FullyAssociativeCache` and `SetAssociativeCache` each embed a `Tlb` and implement the VIPT model. A `debug_assert!` guards the set-index-in-page-offset invariant. All DRAM-facing code (`DDR4RankModel`, `DDR4RankNaive`, `DDR4RankDRAMsim3`, `BankState`) accepts `PhysicalAddress`.
- **`src/cli.rs`** — `--page-size` argument on `SimulationArgs` (defaults to `FourMB`).
- **`src/simulate/nmpgc/mod.rs`** — Passes `PageSize` through `NMPProcessor::new` → `SetAssociativeCache::new`. Aggregates TLB hits/misses across processors for the summary table.
- **`src/simulate/nmpgc/work.rs`** — Wraps all cache access calls with `VirtualAddress(…)`.

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

On a TLB hit, the lookup overlaps with set indexing (zero added latency). On a TLB miss, the PTW stalls the tag comparison and the cache access restarts afterward.

### 3. Identity Mapping in the PTW
**Challenge**: A real page table walk produces arbitrary VA→PA mappings. We needed a PTW that contributes realistic *latency* without needing an actual page table.

**Solution**: `PageTableWalker::translate` returns `PhysicalAddress(va.0)` after a variable latency (18–30 cycles depending on page size). This isolates the TLB miss penalty without coupling to any particular OS page table format. A realistic PTW can replace this method later without changing the TLB or cache interfaces.

### 4. Testing the TLB-Miss-Cache-Hit Combination
**Mistake**: Eviction pages targeted the wrong TLB set (set 0 instead of set 1), so the target was never evicted. The first version used VPN offsets of `i * num_sets * page_size`, which mapped to a different set than the target. After correcting the set calculation, the eviction pages' cache lines collided with the target's cache set, inadvertently evicting the target from the cache too.

**Solution**: Two fixes: (a) eviction pages use VPN = `(1 + k * num_sets) * page_size` so they share TLB set 1 with the target, and (b) eviction accesses use an intra-page offset of `+0x40` to land in a different cache set. We also increased the cache to 256 sets to eliminate residual collision.

**Lesson**: When testing multi-level caching structures, verify that test addresses target the *intended* set in *every* level. Derive set indices from the actual indexing formulas (`(vpn >> page_shift) % num_sets` for TLB, `(addr >> line_shift) % num_sets` for cache) rather than assuming stride-based patterns are correct.

## Verification

All 33 unit tests pass. `cargo clippy` and `cargo fmt` produce no warnings or changes.
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
PTW latency is now variable: 30/24/24/18 cycles for 4KB/2MB/4MB/1GB.

#### `fop/heapdump.2.binpb.zst` (93,180 marked objects)

| Config | Ticks | Time (ms) | Util | Rd Hit Rate | Rd Misses | TLB Hit Rate | TLB Misses | Δ Ticks vs Master |
|:-------|------:|----------:|-----:|------------:|----------:|-------------:|-----------:|------------------:|
| Master (no TLB) | 1,687,031 | 1.054 | 0.812 | 0.717 | 155,732 | — | — | — |
| FourKB | 1,828,509 | 1.143 | 0.818 | 0.714 | 157,156 | 0.950 | 31,882 | +8.4% |
| TwoMB | 1,689,009 | 1.056 | 0.812 | 0.717 | 155,813 | 1.000 | 147 | +0.12% |
| FourMB | 1,687,203 | 1.055 | 0.812 | 0.717 | 155,635 | 1.000 | 80 | +0.01% |
| OneGB | 1,685,737 | 1.054 | 0.812 | 0.717 | 155,528 | 1.000 | 28 | −0.08% |

4KB pages incur 31,882 TLB misses (5.0% miss rate) and 8.4% more total cycles. Pages ≥2MB reduce TLB misses to near-zero on this small workload. Variable PTW latency means remaining misses at larger page sizes cost fewer cycles, so OneGB edges below Master.

#### `pmd/heapdump.33.binpb.zst` (93 MB — largest available)

| Config | Ticks | Time (ms) | Util | Rd Hit Rate | Rd Misses | TLB Hit Rate | TLB Misses | Δ Ticks vs Master |
|:-------|------:|----------:|-----:|------------:|----------:|-------------:|-----------:|------------------:|
| Master (no TLB) | 69,603,854 | 43.502 | 0.954 | 0.800 | 6,444,897 | — | — | — |
| FourKB | 75,183,455 | 46.990 | 0.946 | 0.801 | 6,428,334 | 0.965 | 1,304,338 | +8.0% |
| TwoMB | 71,391,935 | 44.620 | 0.954 | 0.800 | 6,437,133 | 0.984 | 593,234 | +2.6% |
| FourMB | 71,018,727 | 44.387 | 0.952 | 0.800 | 6,439,991 | 0.988 | 436,114 | +2.0% |
| OneGB | 69,548,166 | 43.468 | 0.953 | 0.800 | 6,433,695 | 1.000 | 29 | −0.08% |

The larger workload exposes meaningful TLB pressure across all page sizes. 4KB pages produce 1.3M TLB misses (3.5% miss rate), costing 8% more cycles. 2MB and 4MB pages show 2.0–2.6% overhead. The old fixed 30-cycle PTW produced 2.8–3.2%; the lower 24-cycle walk for these sizes accounts for the improvement. Only 1GB pages eliminate TLB overhead entirely (29 cold misses).

## Usage
```
cargo run -- <HEAPDUMP> -o OpenJDK simulate -p 8 -a NMPGC --page-size FourMB
```
Valid `--page-size` values: `FourKB`, `TwoMB`, `FourMB` (default), `OneGB`.

## Known Limitations
- The PTW uses identity mapping; realistic variable-depth page table walks are a future extension.
- Only a single-level TLB is modelled; real processors have L1/L2 TLBs.
- The TLB is not shared across processors—each `NMPProcessor` has its own independent TLB.
