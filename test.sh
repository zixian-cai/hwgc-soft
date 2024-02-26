#!/usr/bin/env bash

export RUST_LOG=trace

features=detailed_stats
object_model=OpenJDK
tracing_loop=WPMMTk
if [ "$1" = "--wpmmtk" ]; then
    tracing_loop=WPMMTk
fi
if [ "$1" = "--wp" ]; then
    tracing_loop=WP
fi
if [ "$1" = "--wp2" ]; then
    tracing_loop=WP2
fi


# snapshop_files=./sampled/fop/heapdump.2.binpb.zst
snapshop_files=./sampled/pmd/heapdump.33.binpb.zst

cargo run --release --features $features -- $snapshop_files --object-model $object_model trace --tracing-loop $tracing_loop