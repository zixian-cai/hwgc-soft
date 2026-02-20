# Cache Interface Refactor: Eliminating Latency Queries

**Date**: 2026-02-21

## Overview

The `DataCache` and `DDR4RankModel` interfaces split memory operations into a latency query and execution. DRAMsim3 cannot query latency without mutating state, and the queries omit side effects that alter subsequent operations. We replaced this pattern with instant execution plus explicit stall insertion, fixing several simulation bugs in the process.

## Architecture

- **`memory.rs`**: `DataCache` now only exposes `read(&mut self) -> usize` and `write(&mut self) -> usize`. `DDR4RankModel` dropped `transaction_latency`. `BankState` merged its two methods into one. `DDR4RankDRAMsim3` dropped its `Mutex<LruCache>` for speculative latency caching.
- **`work.rs`**: `NMPProcessorWork::Stall(usize)` represents remaining stall cycles. `tick()` executes work immediately, then pushes `Stall(latency - 1)` to the front of the queue, replacing the `stalled_work` / `stall_ticks` / `get_latency` mechanism.
- **`mod.rs`**: Removed `stalled_work`, `stall_ticks`, and `get_latency()` from `NMPProcessor`. `locally_done()` checks only `works` and `inbox`.
- **`build.rs`**: Dynamically detects `libstdc++` version for bindgen. Fixed static library link ordering (`dramsim3_wrapper` → `dramsim3` → `stdc++`).

## Design Decisions & Lessons Learned

### 1. Latency-Query/Execute Impedance Mismatch

**Mistake**: `get_latency` for `Mark` computed `read_latency(o) + write_latency(o)`. Since `read_latency` for `DataCache` doesn't change the microarchitectural state, it doesn't allocate the cache line on miss—so `write_latency` also sees a miss, double-counting the cache miss penalty.

**Solution**: Every n-cycle work executes on the first cycle via `cache.read()`/`cache.write()` (which mutate state), then insert `Stall(n-1)`.

**Lesson**: If a side-effect-free query targets an inherently stateful system, the query is either wrong or duplicates the execute path. Execute once and stall.

### 2. Chaotic Nature of Distributed Systems

Since we simulate distributed heap traversal, changes to memory timing can cascade through load balancing and microarchitectural state across processors. For example, fixing the cache miss latency bug alone (before other changes) actually *reduced* DRAMsim3 all-reads cycles—higher per-access cost shifted load distribution, improving utilization:
```
Cache miss latency = DRAM latency
============================ Tabulate Statistics ============================
busy_ticks.sum	marked_objects.sum	read_hit_rate	read_hits.sum	read_misses.sum	ticks	time	utilization	write_hit_rate	write_hits.sum	write_misses.sum
11253492.000	93180.000	0.834	291600.000	58152.000	1823631.000	1.140	0.771	0.262	24432.000	68748.000
-------------------------- End Tabulate Statistics --------------------------

Cache miss latency = Cache hit latency + DRAM latency
============================ Tabulate Statistics ============================
busy_ticks.sum	marked_objects.sum	read_hit_rate	read_hits.sum	read_misses.sum	ticks	time	utilization	write_hit_rate	write_hits.sum	write_misses.sum
11745021.000	93180.000	0.834	291615.000	58137.000	1807643.000	1.130	0.812	0.260	24239.000	68941.000
-------------------------- End Tabulate Statistics --------------------------
```

### 3. Stall as Explicit Work Items

The old `stall_ticks` / `stalled_work` mechanism stored state outside the work queue. `Stall(n)` is now a first-class work item, excluded from the logical instruction count alongside `Idle`.

### 4. Cache Miss Latency

**Mistake**: Miss paths returned only `rank.transaction(...)`, omitting `HIT_LATENCY`. A miss must still pay the tag lookup. With posted writes completing in 1 cycle, a write miss (1 cycle) cost less than a hit (4 cycles).

**Fix**: All miss paths now return `HIT_LATENCY + rank.transaction(...)`.

### 5. Write-Through Cache

The previously cache implementation is incorrect because it implements neither write-back nor write-through. Line dirty state is not tracked, and no memory latency penalty is paid on eviction of dirty lines.

Write-through is easier to implement since we don't need to implement a dirty bit. `write()` unconditionally forwards to DRAM. The cache still uses write-allocate so subsequent reads hit.

### 6. Missing Read in Mark Operations

**Mistake**: Master's `tick()` for `Mark` only called `cache.write(o)`—it never called `cache.read(o)`. A real mark operation must read the object header to check the mark bit before writing it.

**Fix**: `tick()` now calls `cache.read(o)` before `cache.write(o)`. This adds ~93K read operations (one per marked object), which is why the read hit rate drops from 0.834 to 0.714 and read misses rise from ~58K to ~157K.

### 7. Build System

`build.rs` hardcoded `-I/usr/include/c++/15` for bindgen. `find_libstdcpp_includes()` now probes `/usr/include/c++/` for the highest version. Separately, the GNU linker resolves symbols left-to-right; reordering static library directives so dependents precede dependencies fixed `cargo test`.

## Verification

Verified on `fop/heapdump.2.binpb.zst` with 8 processors. "Master" is commit `us` (`9f`). "All reads" forces `is_write = false` to isolate posted-write effects. All runs produce 93,180 marked objects.

| Metric | Master Naive | Current Naive | Master DRAMsim3 | Current DRAMsim3 | Master DRAMsim3 (all reads) | Current DRAMsim3 (all reads) |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| **Total Cycles** | 1,881,176 | 1,802,370 | 978,557 | 1,658,059 | 1,823,631 | 1,926,712 |
| **Utilization** | 0.774 | 0.818 | 0.753 | 0.823 | 0.771 | 0.817 |
| **Busy Ticks (sum)** | 11,651,366 | 11,789,730 | 5,894,569 | 10,912,492 | 11,253,492 | 12,600,434 |
| **Read Hit Rate** | 0.834 | 0.714 | 0.834 | 0.715 | 0.834 | 0.714 |
| **Read Misses (sum)** | 58,007 | 157,089 | 58,101 | 157,000 | 58,152 | 157,549 |
| **Write Hit Rate** | 0.261 | 1.000 | 0.260 | 1.000 | 0.262 | 1.000 |
| **Write Misses (sum)** | 68,879 | 0 | 68,988 | 0 | 68,748 | 0 |

Master DRAMsim3 (978K cycles) was artificially fast: ~69K write misses each cost 1 DRAM cycle (posted writes), and the missing `HIT_LATENCY` on miss further reduced stall time. Forcing all reads raises it to 1.82M—close to Naive—confirming posted writes drove the anomaly.

After the fixes, write hit rate reaches 1.000 (mark writes always follow a read that allocates the line). Read hit rate drops from 0.834 to 0.714 because `tick()` now adds a `cache.read(o)` call per Mark that master lacked—these ~93K additional reads (which might miss or evict other useful lines) reduce the hit rate. DRAMsim3 (1.66M) is fastest (posted writes help), followed by Naive (1.80M), then all-reads (1.93M).

Per-hart instruction counts are identical across all four configurations (master/current × Naive/DRAMsim3), confirming using `Stall` to implement multi-cycle work is preserve the semantics of the existing output stats.

| Hart | Objects | Instructions |
| :--- | ---: | ---: |
| 0 | 10,112 | 129,900 |
| 1 | 10,311 | 128,354 |
| 2 | 12,051 | 124,477 |
| 3 | 10,999 | 121,586 |
| 4 | 13,714 | 150,725 |
| 5 | 12,483 | 142,955 |
| 6 | 11,896 | 146,854 |
| 7 | 11,614 | 141,802 |

Unit tests: 9/9 passed (debug). The link ordering fix resolved a pre-existing test build failure.

## Known Limitations

- **Out-of-sync DRAMsim3 ticks**: DRAMsim3 instances only tick during `run_transaction`. Idle periods skip `ClockTick`, under-modeling DRAM refreshes.
- Virtual addresses and per-rank isolation limitations from the [previous journey](./20260219_dramsim3_integration_summary.md) still apply.
