# DRAMsim3 Integration Summary

**Date**: 2026-02-19

## Overview

This document records the integration of [DRAMsim3](https://github.com/umd-memsys/DRAMsim3) as a cycle-accurate memory backend for `hwgc-soft`. DRAMsim3 models bank-level state, row-buffer hits, and queuing—providing higher fidelity than the fixed-latency Naive model.

## Architecture

The integration bridges the Rust event-driven simulator with the C++ DRAMsim3 library through static linking.

- **Build system** (`build.rs`): Uses the `cmake` crate to build DRAMsim3, the `cc` crate to compile a C++ shim, and `bindgen` to generate Rust FFI bindings. Includes fallback logic to locate `LIBCLANG_PATH`.
- **Shim layer** (`src/shim/dramsim3_wrapper.cc`): A thin C++ wrapper that exposes `AddTransaction`, `ClockTick`, `WillAcceptTransaction`, and `IsTransactionDone` to Rust through a C-linkage interface.
- **Rust integration** (`src/simulate/memory.rs`): `DDR4RankDRAMsim3` implements the `DDR4RankModel` trait. It wraps the C++ object in a `Mutex` for thread safety (`DDR4RankModel` requires `Send + Sync`) and uses a `Mutex<LruCache>` to cache speculative latency predictions.
- **Output management**: DRAMsim3 output is redirected to `std::env::temp_dir()` to avoid polluting the workspace. Internal logging is disabled (`output_level = 0`) by default.
- **Startup diagnostics**: `CalculateSize()` and `SetAddressMapping()` in DRAMsim3's `configuration.cc` dump the computed DRAM organization (page size, rank count, capacity) and a bit-field layout at startup for visual comparison with the Rust `AddressMapping` bitfield. Both functions use `static bool` guards to print only once, even though the config is loaded per-rank.

## Design Decisions & Lessons Learned

### 1. Address Mapping Alignment

**Challenge**: DRAMsim3's address mapping must match the bitwise layout in `hwgc-soft`'s `AddressMapping` struct exactly.

**Mistake**: We set `columns = 128` to match the 7-bit column field directly. But DRAMsim3 uses `columns` as a physical parameter: `page_size = columns × device_width / 8`. Setting `columns = 128` produced an artificially small 1 GB rank instead of the standard 8 GB. Since `channel_size` was set to 16 GB, DRAMsim3 instantiated 16 ranks—far more than the 4 that `hwgc-soft` encodes with its 2-bit rank+DIMM selector (bits 19:18). DRAMsim3 silently received out-of-bounds rank addresses.

**Solution**: We set `columns = 1024` (the true DDR4 column count, yielding an 8 KB page) and `channel_size = 32768` (32 GB). DRAMsim3 now correctly computes 32768 / 8192 = 4 ranks per channel, matching the Rust-side topology: 2 channels × 2 DIMMs × 2 ranks = 8 total ranks, 64 GB system.

**Lesson**: Configuration parameters must reflect the physical DRAM organization (page size, rank count, capacity), not interface bit widths. DRAMsim3 derives its internal structure mathematically from these values. The startup diagnostics now make mismatches visible immediately.

### 2. Transaction Latency Interface

**Challenge**: `hwgc-soft` queries latency speculatively via `transaction_latency(&self)` without committing state, but DRAMsim3 is fully stateful—every `AddTransaction` + `ClockTick` sequence mutates internal queues and bank state.

**Solution**: `DDR4RankDRAMsim3` runs the full transaction through DRAMsim3 even for speculative queries, then caches the result in an LRU cache. On the subsequent `transaction(&mut self)` call for the same address, it pops the cached latency instead of re-running the simulation.

### 3. Build Robustness

**Challenge**: Linking C++ standard libraries and locating `libclang` for `bindgen` can fail across environments.

**Solution**: `build.rs` probes common `LIBCLANG_PATH` locations (`/usr/lib/llvm-{19,18,14}/lib`) and links `stdc++` explicitly.

### 4. Posted Writes

DRAMsim3 models posted (non-blocking) writes: `Controller::AddTransaction` sets `complete_cycle = clk_ + 1` for writes and pushes them directly onto `return_queue_`, so the write callback fires on the very next `ClockTick`. The actual DRAM writeback happens asynchronously—the controller buffers writes and drains them later. Reads, by contrast, must traverse the full DRAM pipeline (activate → column access → data burst) before completion.

The write buffer capacity is `trans_queue_size` (32 in our config). `WillAcceptTransaction` rejects new writes when the buffer is full. In practice, our single-transaction-at-a-time model never fills it—each write completes in 1 tick before the next is submitted.

This behavior matches real DDR4 controllers that support posted writes. The Naive model does not distinguish reads from writes, applying the same fixed latency to both.

## Verification

We verified the integration using the `scripts/verify_dramsim3.sh` script on the `fop/heapdump.2.binpb.zst` trace.

| Metric | Naive Model | DRAMsim3 Model | DRAMsim3 (all reads) | Note |
| :--- | :--- | :--- | :--- | :--- |
| **Total Cycles** | ~1,881,176 | ~978,557 | ~1,823,631 | Posted writes account for nearly all the cycle reduction |
| **Utilization** | 0.774 | 0.753 | 0.771 | Comparable across all three |
| **Wall-Clock Time** | 1.176s | 0.612s | 1.140s | — |

The "all reads" column forces `is_write = false` in `DDR4RankDRAMsim3::run_transaction`, treating every transaction as a read. The result (1.82M cycles) is nearly identical to the Naive model (1.88M cycles), confirming that posted writes—not row-buffer hits or bank parallelism—drive the 1.9× cycle reduction.

## Known Limitations

- **Virtual addresses**: The memory model expects physical addresses, but heap dumps provide virtual addresses. The upper bits are effectively random, which distorts row-conflict modeling. See the FIXME in `memory.rs`.
- **Per-rank isolation**: Each `DDR4RankDRAMsim3` instance runs an independent DRAMsim3 simulation. Cross-rank queuing and channel-level contention are not modeled.

## Usage

Run with DRAMsim3 (config defaults to `configs/DDR4_8Gb_x8_3200.ini`):

```bash
cargo run --release -- <HEAP_DUMP> -o OpenJDK simulate -p 8 -a NMPGC --use-dramsim3
```

Run verification:

```bash
./scripts/verify_dramsim3.sh
```
