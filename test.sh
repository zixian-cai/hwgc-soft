#!/usr/bin/env bash

RUST_LOG=trace cargo run --features detailed_stats -- ./sampled/fop/heapdump.2.binpb.zst --object-model OpenJDK trace --tracing-loop EdgeSlot