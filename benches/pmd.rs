use harness::{bench, Bencher};
use hwgc_soft::bench::{BenchContext, TracingStats};
use hwgc_soft::ObjectModelChoice;
use std::sync::Mutex;

static CONTEXT: Mutex<Option<Box<dyn BenchContext>>> = Mutex::new(None);

fn startup() {
    let tracing_loop = std::env::var("TRACING_LOOP").unwrap_or("WPEdgeSlot".to_string());
    let context = hwgc_soft::bench::prepare(
        ObjectModelChoice::OpenJDK,
        &tracing_loop,
        "./sampled/pmd/heapdump.33.binpb.zst",
    )
    .unwrap();
    *CONTEXT.lock().unwrap() = Some(context);
}

fn teardown() {
    let _context = CONTEXT.lock().unwrap().take().unwrap();
}

#[bench(startup=startup, teardown=teardown)]
fn pmd(b: &Bencher) {
    let guard = CONTEXT.lock().unwrap();
    let context = guard.as_ref().unwrap();
    let mut stats = TracingStats::default();
    b.time(|| {
        stats = context.iter();
    });
    context.finalize(b, stats);
}
