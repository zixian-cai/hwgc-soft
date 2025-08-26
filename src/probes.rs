use probe::probe;

// Make the tracepoints explicitly out-of-line to avoid create multiple uprobe_events
// when using `perf probe` and then `perf record`
#[inline(never)]
pub(crate) fn trace_heapdump_begin(heap_dump_name: *const i8) {
    probe!(hwgc_soft, heapdump_begin, heap_dump_name);
}

#[inline(never)]
pub(crate) fn trace_heapdump_end() {
    probe!(hwgc_soft, heapdump_end);
}

#[inline(never)]
pub(crate) fn trace_iteration_begin(iteration: usize) {
    probe!(hwgc_soft, iteration_begin, iteration);
}

#[inline(never)]
pub(crate) fn trace_iteration_end(iteration: usize) {
    probe!(hwgc_soft, iteration_end, iteration);
}
