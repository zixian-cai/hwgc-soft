#!/usr/bin/env bash

export RUST_LOG=trace

features=detailed_stats,forwarding,no_space_dispatch,atomic_free_farwarding
object_model=OpenJDK
release=--release


# snapshop_files=./sampled/fop/heapdump.2.binpb.zst
snapshop_files=./sampled/pmd/heapdump.33.binpb.zst

cargo run $release --features $features -- $snapshop_files --object-model $object_model trace --tracing-loop $1