# Functional model of HWGC

## Build
`cargo build && cargo run`. Need version `24+` `protoc` in the `PATH`.

Clone DRAMsim3 from `git@github.com:zixian-cai/DRAMsim3.git` and put it under `../DRAMsim3`.

### Transitive closure
```
RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run --features detailed_stats --release -- ../heapdumps/sampled/fop/heapdump.*.binpb.zst -o Bidirectional trace --tracing-loop DistributedNodeObjref -i 1
```

## Generate heapdumps
Use https://github.com/anupli/pimgc-asplos-2025/tree/main/experiments/heapdumps

Run `running runbms /path/to/results experiments/heapdumps/generate.yml` to generate heapdumps for the timing iteration for each of the benchmarks.
And then run `./experiments/heapdumps/sample.py heapdumps/alveo-2024-01-12-Fri-122525/ heapdumps/sampled` to keep up to 20 heapdumps for each benchmarks.

To generate the benchmark suite definitions using the heapdumps, run `./scripts/generate_suite_def.py ../heapdumps/sampled/`

## Running event-driven simulation
```
RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run -- ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 4 -a IdealTraceUtilization
```

## Experiments
### PGO and tracing performance
```
PATH=$HOME/protoc/bin:$PATH ./scripts/pgo.py ../heapdumps/sampled/fop/heapdump.*.binpb.zst
```

This will produce a bunch of executables under `builds`.

Use the following to understand how PGO affects performance.
```
running runbms /path/to/results ./scripts/pgo-1.yml
running runbms /path/to/results ./scripts/pgo-2.yml
```

And then use `running runbms /path/to/results ./scripts/trace.yml` to understand how object models and tracing loops affect tracing performance.

### Analyzing communication patterns
`cargo build --release` and then copy `target/release/hwgc_soft` to `builds/`.
