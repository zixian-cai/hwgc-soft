#![feature(test)]
#![feature(concat_idents)]

extern crate test;
use hwgc_soft::*;
use test::Bencher;

#[bench]
fn tradesoap_edge_slot(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::EdgeSlot,
        "./sampled/tradesoap/heapdump.89.binpb.zst",
    );
}

#[bench]
fn tradesoap_crossbeam(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::WP,
        "./sampled/tradesoap/heapdump.89.binpb.zst",
    );
}

#[bench]
fn tradesoap_mmtk_work_packet(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::WPMMTk,
        "./sampled/tradesoap/heapdump.89.binpb.zst",
    );
}
