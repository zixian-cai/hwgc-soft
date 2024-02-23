#![feature(test)]
#![feature(concat_idents)]

extern crate test;
use hwgc_soft::*;
use test::Bencher;

#[bench]
fn pmd_edge_slot(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::EdgeSlot,
        "./sampled/pmd/heapdump.33.binpb.zst",
    );
}

#[bench]
fn pmd_crossbeam(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::WP,
        "./sampled/pmd/heapdump.33.binpb.zst",
    );
}

#[bench]
fn pmd_mmtk_work_packet(b: &mut Bencher) {
    run_bench(
        b,
        TracingLoopChoice::WPMMTk,
        "./sampled/pmd/heapdump.33.binpb.zst",
    );
}