use super::{trace_object, TracingStats};
use crate::ObjectModel;

pub(super) unsafe fn transitive_closure_edge_slot<O: ObjectModel>(
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    // Edge-Slot enqueuing
    let mut mark_queue: Vec<*mut u64> = vec![];
    let mut marked_objects: u64 = 0;
    let mut slots = 0;
    let mut non_empty_slots = 0;
    for root in object_model.roots() {
        let o = *root;
        if cfg!(feature = "detailed_stats") {
            slots += 1;
            if o != 0 {
                non_empty_slots += 1;
            }
        }
        if o != 0 && trace_object(o, mark_sense) {
            if cfg!(feature = "detailed_stats") {
                marked_objects += 1;
            }
            O::scan_object(o, |edge, repeat| {
                for i in 0..repeat {
                    mark_queue.push(edge.wrapping_add(i as usize));
                }
            })
        }
    }
    while let Some(e) = mark_queue.pop() {
        let o = *e;
        if cfg!(feature = "detailed_stats") {
            slots += 1;
        }
        if o != 0 {
            if cfg!(feature = "detailed_stats") {
                non_empty_slots += 1;
            }
            if trace_object(o, mark_sense) {
                if cfg!(feature = "detailed_stats") {
                    marked_objects += 1;
                }
                O::scan_object(o, |edge, repeat| {
                    for i in 0..repeat {
                        mark_queue.push(edge.wrapping_add(i as usize));
                    }
                })
            }
        }
    }
    TracingStats {
        marked_objects,
        slots,
        non_empty_slots,
        ..Default::default()
    }
}
