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
    let t = std::time::Instant::now();
    b.time(|| {
        stats = context.iter();
    });
    let elapsed = t.elapsed();
    b.add_stat("marked_objects", stats.marked_objects as u64);
    b.add_stat("slots", stats.slots as u64);
    b.add_stat("non_empty_slots", stats.non_empty_slots as u64);
    b.add_stat("copied_objects", stats.copied_objects as u64);
    b.add_stat("packets", stats.packets as u64);
    b.add_stat("total_run_time_us", stats.total_run_time_us as u64);
    b.add_stat(
        "utilization",
        stats.total_run_time_us as f32 / (32f32 * elapsed.as_micros() as f32),
    );
}
