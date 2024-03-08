use super::{trace_object, TracingStats};
use crate::ObjectModel;
use std::collections::VecDeque;

pub(super) unsafe fn transitive_closure_edge_objref<O: ObjectModel>(
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    // Edge-ObjRef enqueuing
    let mut mark_queue: VecDeque<u64> = VecDeque::new();
    let mut slots = 0;
    let mut non_empty_slots = 0;
    for root in object_model.roots() {
        if cfg!(feature = "detailed_stats") {
            slots += 1;
            if *root != 0 {
                non_empty_slots += 1;
            }
        }
        mark_queue.push_back(*root);
    }
    let mut marked_objects: u64 = 0;
    while let Some(o) = mark_queue.pop_front() {
        if trace_object(o, mark_sense) {
            // not previously marked, now marked
            // now scan
            if cfg!(feature = "detailed_stats") {
                marked_objects += 1;
            }
            O::scan_object(o, |edge, repeat| {
                for i in 0..repeat {
                    let o = *edge.wrapping_add(i as usize);
                    if cfg!(feature = "detailed_stats") {
                        slots += 1;
                    }
                    if o != 0 {
                        if cfg!(feature = "detailed_stats") {
                            non_empty_slots += 1;
                        }
                        mark_queue.push_back(o)
                    }
                }
            });
        }
    }
    // println!("{} capa", mark_queue.capacity());
    TracingStats {
        marked_objects,
        slots,
        non_empty_slots,
        ..Default::default()
    }
}
