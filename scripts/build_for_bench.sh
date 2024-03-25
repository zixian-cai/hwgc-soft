#!/usr/bin/env bash

set -ex

rm -r ./builds

cargo build --release --features forwarding

cp target/release/hwgc_soft ./builds/all_in_one

cargo build --release --features forwarding,no_space_dispatch

cp target/release/hwgc_soft ./builds/all_in_one_no_space_dispatch
