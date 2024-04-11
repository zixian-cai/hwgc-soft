#!/usr/bin/env bash

set -ex

rm -rf ./builds

mkdir -p ./builds

cargo build --release --features slower_root_scanning

cp target/release/hwgc_soft ./builds/all_in_one

# cargo build --release --features fifo

# cp target/release/hwgc_soft ./builds/all_in_one_fifo
