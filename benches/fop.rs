#![feature(test)]
#![feature(concat_idents)]

extern crate test;
use hwgc_soft::*;
use test::Bencher;

#[bench]
fn fop_edge_slot(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::EdgeSlot,
        "./sampled/fop/heapdump.2.binpb.zst",
    );
}

#[bench]
fn fop_crossbeam(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::WP,
        "./sampled/fop/heapdump.2.binpb.zst",
    );
}

#[bench]
fn fop_mmtk_work_packet(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::WPMMTk,
        "./sampled/fop/heapdump.2.binpb.zst",
    );
}
