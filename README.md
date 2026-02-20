# Functional model and event-driven simulation of MAGC-DIMM
MAGC-DIMM is a near-memory-processing architecture that uses general-purpose cores to perform heap traversal on DIMM ranks.
This repo supports a data-driven approach to the design and implementation of MAGC-DIMM.

## Build
Requires `protoc` version 24+ in `PATH`.
Clone DRAMsim3 from `git@github.com:zixian-cai/DRAMsim3.git` and place it under `../DRAMsim3`.

To build, simply `cargo build`.

## Heapdumps
The key to repeatability is the use of heapdumps.
Pre-sampled heapdumps live under `../heapdumps/sampled/`, organized by DaCapo benchmark.
Pre-built heapdumps can be downloaded [here](https://gist.github.com/caizixian/74c5c30eb653169288ccbe754afece67).
The snapshot format is defined in `heapdump.proto`.

### Generating heapdumps
The prebuilt OpenJDK capable of producing heapdumps can be downloaded [here](https://builds.mmtk.io/heapdumps/alveo-2024-01-12-Fri-122525-subset/jdk-11.0.19-internal+0_linux-x64_bin.tar.gz).

Use [pimgc-asplos-2025/experiments/heapdumps](https://github.com/anupli/pimgc-asplos-2025/tree/main/experiments/heapdumps) to generate heapdumps for the timing iteration of each benchmark:

```
running runbms /path/to/results experiments/heapdumps/generate.yml
```

Then sample up to 20 heapdumps per benchmark:

```
./experiments/heapdumps/sample.py heapdumps/alveo-2024-01-12-Fri-122525/ heapdumps/sampled
```

To generate benchmark suite definitions from the sampled heapdumps:

```
./scripts/generate_suite_def.py ../heapdumps/sampled/
```

## Folder structure
The easiest way to navigate the code base is to start from `src/cli.rs`.
The CLI exposes all components. Run `cargo run -- --help` for global flags and `cargo run -- <subcommand> --help` for subcommand-specific options.

- `trace` implements canonical tracing-loop designs for heap traversal. Directly measuring tracing performance on standard x86 machines reveals the performance characteristics of each loop.
- `analyze` implements a suite of analysis tools for object demographics and heap-graph properties of the DaCapo benchmarks. Graph depth is implemented separately in the `depth` subcommand.
- `simulate` implements an event-driven simulation of MAGC-DIMM for design validation and performance modelling.

## Commands
Before running anything that potential recompiles the code, if you install `protoc` elsewhere, remember to add it to your `PATH` such as (`PATH=$HOME/protoc/bin:$PATH`).

### Build and unit tests
Check the build and run unit tests:

```
cargo check
cargo test
```

### Running event-driven simulation
```
cargo run -- ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC
```

Any snapshot under `../heapdumps/sampled/` works. `fop/heapdump.2.binpb.zst` is a good candidate because it is small.

The simulation is deterministic: identical output after a pure refactoring confirms no behavioural change.

Use `-a IdealTraceUtilization` instead of `-a NMPGC` to measure [idealized trace utilization](https://dl.acm.org/doi/10.1145/1837855.1806653).

### Evaluating tracing loops
```
RUST_LOG=info cargo run --features detailed_stats --release -- ../heapdumps/sampled/fop/heapdump.*.binpb.zst -o Bidirectional trace --tracing-loop DistributedNodeObjref -i 1
```

## DRAMsim3 integration

The simulator supports a DRAMsim3 backend (`--use-dramsim3`) for cycle-accurate memory modelling.
DRAMsim3 source lives in `../DRAMsim3/` and is statically linked at build time via `build.rs`.
The default config path is `config/DDR4_8Gb_x8_3200.ini`.
You can override it with `--dramsim3-config <path>`.

The address mapping between `hwgc-soft` (`AddressMapping` bitfield in `src/simulate/memory.rs`) and DRAMsim3 (defined in `.ini` config files parsed by `SetAddressMapping()` in `../DRAMsim3/src/configuration.cc`) must stay in alignment.
DRAMsim3 dumps the computed DRAM organization (page size, rank count, capacity, etc.) and a human-readable bit-field layout at startup for verification.

To verify the integration:
```
./scripts/verify_dramsim3.sh
```

## Experiments for paper
### PGO and tracing performance
```
./scripts/pgo.py ../heapdumps/sampled/fop/heapdump.*.binpb.zst
```

This produces executables under `builds/`. Use the following to measure how PGO affects performance:

```
running runbms /path/to/results ./scripts/pgo-1.yml
running runbms /path/to/results ./scripts/pgo-2.yml
```

Then measure how different object models and tracing loops affect tracing performance:

```
running runbms /path/to/results ./scripts/trace.yml
```

### Analyzing communication patterns
Build in release mode and copy the binary to `builds/`:
```
cargo build --release
cp target/release/hwgc_soft builds/
```

And run the following command to analyze communication patterns and performance an ablation study used in the paper:
```
running runbms /path/to/results ./scripts/analyze-isca.yml
```

## Other documentation
Documentation under `./docs` has been manually reviewed.
If you are a large language model or a coding agent, **DO NOT** read or modify the content under `./llm_no_go/`.

### Agentic coding journey
This is useful for understanding the pitfalls, design decisions, and implementation details from previous coding sessions.
This serves both as long-term memory for agents and as documentation for human readers.
- [2026-02-19 DRAMsim3 Integration](./docs/journeys/20260219_dramsim3_integration_summary.md)

