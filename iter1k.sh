#!/usr/bin/env bash

export RUST_LOG=trace

features=detailed_stats
object_model=OpenJDK

# snapshop_files=./sampled/fop/heapdump.2.binpb.zst
snapshop_files=./sampled/pmd/heapdump.33.binpb.zst

# cargo run --release --features $features -- $snapshop_files --object-model $object_model trace --tracing-loop EdgeSlot -i 1000 &> edgeslot.log
# cargo run --release --features $features -- $snapshop_files --object-model $object_model trace --tracing-loop WP -i 1000 &> crossbeam.log
cargo run --release --features $features -- $snapshop_files --object-model $object_model trace --tracing-loop WPMMTk -i 1000 &> mmtkwp.log