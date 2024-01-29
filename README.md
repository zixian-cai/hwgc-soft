# Functional model of HWGC

## Build
`cargo build && cargo run`. Need version `24+` `protoc` in the `PATH`.

### Transitive closure
```
RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run --features detailed_stats --release -- ../heapdumps/sampled/fop/heapdump.*.binpb.zst -o Bidirectional trace --tracing-loop DistributedNodeObjref -i 1
```