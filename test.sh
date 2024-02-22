#!/usr/bin/env bash

export RUST_LOG=trace

features=detailed_stats
object_model=OpenJDK
tracing_loop=WP

snapshop_files=./sampled/fop/heapdump.2.binpb.zst

cargo run --release --features $features -- $snapshop_files --object-model $object_model trace --tracing-loop $tracing_loop