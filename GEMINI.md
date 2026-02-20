# Functional model and event-driven simulation of MAGC-DIMM
MAGC-DIMM is a near-memory-processing architecture that use general-purpose cores to perform heap traversal on DIMM ranks.
Overall, this repo supports a data-driven approach to the design and implementation of MAGC-DIMM.

## Heap snapshots
The key to the repeatability of this repo is the use of heap snapshots.
The snapshots can be found under `../heapdumps/sampled/`, where it contains folders of heap snapshots sampled from the execution of the DaCapo benchmarks.

The format of the heap snapshots is defined in `heapdump.proto`.

## Components
There are many aspect of this repo, which can be seen via the possible CLI arguments in `cli.rs`.
If you are not sure, always remember `--help`, such as `cargo run -- --help` for the global flags, and `cargo run -- <subcommand> --help` for the subcommands.

`trace` implements many of the canonical tracing loop designs for heap traversal, and by directly measuring the tracing performance on standard x86 machines, we can understand the performance characteristics of the tracing loops.

`analyze` implements a suite of analysis tools to understand the object demographics and the (heap) graph (graph depth is implemented separately in `depth`) properties of the DaCapo benchmarks.

Finally, `simulate` implements an event-driven simulation of MAGC-DIMM to help us validate the design and model the performance.

## Commands
To check whether the repo builds, run `cargo check`.
Use `cargo test` for running unit tests.

To run the event-driven simulation of MAGC-DIMM, use `cargo run -- ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC`.
You can use other heap snapshots under `../heapdumps/sampled/` if you wish.
`fop/heapdump.2.binpb.zst` is a good candidate for testing because it is small.

If you are doing pure refactoring, the output of the simulation should be identical since the simulation is deterministic.
Use this to verify that your refactoring does not change any behavior.

## DRAMsim3 integration
The simulator supports an optional DRAMsim3 backend (`--use-dramsim3`) for cycle-accurate memory modelling.
The DRAMsim3 source lives in `../DRAMsim3/` and is linked at build time via `build.rs`.

The address mapping between `hwgc-soft` (`AddressMapping` bitfield in `src/simulate/memory.rs`) and DRAMsim3 (`SetAddressMapping()` in `DRAMsim3/src/configuration.cc`) must be kept in alignment.
DRAMsim3 dumps the computed DRAM organization (page size, rank count, capacity) and a human-readable bit-field layout at startup for verification.
DRAMsim3's debug output uses `static bool` guards to print only once even when the config is loaded per-rank.

To verify the DRAMsim3 integration, run `./scripts/verify_dramsim3.sh`.
