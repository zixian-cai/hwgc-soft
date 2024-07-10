#!/usr/bin/env bash

set -ex

rm -rf ./builds

mkdir -p ./builds

function build {
    cargo build --release --features $1
    cp target/release/hwgc_soft ./builds/$2
}

build deque_overflow all_in_one
