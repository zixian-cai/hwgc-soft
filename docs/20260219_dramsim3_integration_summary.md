# DRAMsim3 Integration Summary

**Date**: 2026-02-19
**Author**: Antigravity

## Overview

We have successfully integrated [DRAMsim3](https://github.com/umd-memsys/DRAMsim3) as a cycle-accurate memory simulation backend for `hwgc-soft`. This allows for more realistic performance modeling of Near-Memory Processing (NMP) architectures compared to the existing fixed-latency "Naive" model.

## Key Architecture

The integration bridges the Rust-based event-driven simulator with the C++ DRAMsim3 library using a static linking approach.

- **Build System**: `build.rs` uses the `cmake` crate to build DRAMsim3 from source and the `cc` crate to build a C++ shim. `bindgen` generates the Rust FFI bindings.
- **Shim Layer**: A thin C++ wrapper (`src/shim/dramsim3_wrapper.cc`) exposes a simplified interface (`add_transaction`, `tick`, `is_transaction_done`) to Rust.
- **Rust Integration**: The `DDR4RankDRAMsim3` struct in `src/simulate/memory.rs` implements the `DDR4RankModel` trait. It uses `RefCell` to handle the interior mutability required by the stateful DRAMsim3 simulation within the immutable-reference context of `transaction_latency` checks.
- **Output Management**: DRAMsim3 output is redirected to the system temporary directory (`std::env::temp_dir()`) to prevent workspace pollution. Internal logging is disabled (`output_level = 0`) by default for performance.
- **Startup Diagnostics**: Both `CalculateSize()` and `SetAddressMapping()` in DRAMsim3's `configuration.cc` dump annotated size calculations and a human-readable bit-field layout at startup, matching the Rust `AddressMapping` bitfield in `memory.rs`. All diagnostic output uses `static bool` guards to print only once even though the config is loaded per-rank.

## Design Decisions & Lessons Learned

### 1. Geometry Alignment
**Challenge**: DRAMsim3's address mapping must exactly match the bitwise interpretation in `hwgc-soft`'s `AddressMapping` struct.
**Initial Mistake**: I mistakenly mapped the `hwgc-soft` 64-byte burst index (7 bits, or 128 units) directly to the `columns` configuration variable. However, DRAMsim3 calculates physical layout mathematically using these configurations (`page_size` = `columns` * `device_width` / 8, `channel_capacity` = `megs_per_rank` * `ranks`). Entering `columns = 128` forced DRAMsim3 to synthesize artificially small 1GB ranks instead of the standard 8GB. Since `channel_size` was set to `16GB`, DRAMsim3 incorrectly instantiated 16 ranks. `hwgc-soft` only assigned 2 address bits (4 ranks total per channel) to the rank selector, resulting in DRAMsim3 silently receiving invalid and out-of-bounds requests.
**Solution**: We aligned `configs/DDR4_8Gb_x8_3200.ini` to properly adhere to the true standard definitions: `columns = 1024` (yielding the expected 8KB page size array configuration) and `channel_size = 32768` (32 GB per channel). Now DRAMsim3 correctly computes `32768 / 8192 MB per rank = 4 ranks`, matching `hwgc-soft`'s expected topology of 64 GB over two channels, 2 DIMMs per channel, and 2 ranks per DIMM.
**Lesson**: Do not guess internal simulation constraints from interface bit lengths. Configuration geometries influence both bitwise addressing and internal memory pool allocation size dynamically. Understanding the explicit mathematical pipeline DRAMsim3 uses is crucial to preventing silent, catastrophic geometry mismatches. To catch these early, DRAMsim3 now dumps its computed geometry and address mapping at startup (see Startup Diagnostics above) so both sides can be visually compared.

### 2. Transaction Latency Interface
**Challenge**: `hwgc-soft` asks "how long will this take?" (`transaction_latency`) without immediately committing to the transaction, whereas DRAMsim3 is state-driven.
**Solution**: We implemented a "speculative" check using a cache or by checking the state. Currently, `DDR4RankDRAMsim3` assumes a standard latency for the *prediction* but delegates the actual *completion* check to the `DRAMsim3Wrapper` during the simulation loop.

### 3. Build Robustness
**Challenge**: Linking C++ standard libraries and finding `libclang` for bindgen can be flaky.
**Solution**: `build.rs` includes logic to detect `LIBCLANG_PATH` and link `stdc++` (or `c++` on macOS).

## Verification Results

We verified the integration using the `scripts/verify_dramsim3.sh` script on the `heapdump.2.binpb.zst` trace.

| Metric | Naive Model | DRAMsim3 Model | Note |
| :--- | :--- | :--- | :--- |
| **Total Cycles** | ~1,881,176 | ~978,557 | DRAMsim3 is faster due to banked parallelism |
| **Utilization** | 0.774 | 0.753 | Comparable visualization of bus usage |
| **Simulation Time** | 1.176s | 0.612s | Wall-clock time |

The discrepancy in cycles is expected: the Naive model uses a simple fixed-latency-plus-contention model, while DRAMsim3 models the full internal state of banks, allowing for row-buffer hits and parallel bank access, which significantly reduces effective latency for streaming patterns often seen in GC.

## Usage

To run with DRAMsim3:

```bash
cargo run --release -- <HEAP_DUMP> -o OpenJDK simulate -p 8 -a NMPGC --use-dramsim3 configs/DDR4_8Gb_x8_3200.ini
```

To verify:

```bash
./scripts/verify_dramsim3.sh
```
