use super::{trace_object, TracingStats};
use crate::ObjectModel;
use std::collections::VecDeque;

pub(super) unsafe fn transitive_closure_node_objref<O: ObjectModel>(
    mark_sense: u8,
    object_model: &mut O,
) -> TracingStats {
    // Node-ObjRef enqueuing
    let mut scan_queue: VecDeque<u64> = VecDeque::new();
    let mut marked_objects: u64 = 0;
    let mut slots: u64 = 0;
    let mut non_empty_slots: u64 = 0;
    for root in object_model.roots() {
        let o = *root;
        if cfg!(feature = "detailed_stats") {
            slots += 1;
            non_empty_slots += 1;
        }
        if o != 0 && trace_object(o, mark_sense) {
            if cfg!(feature = "detailed_stats") {
                marked_objects += 1;
            }
            scan_queue.push_back(o);
        }
    }
    while let Some(o) = scan_queue.pop_front() {
        object_model.scan_object(o, |edge| {
            let child = *edge;
            if cfg!(feature = "detailed_stats") {
                slots += 1;
            }
            if child != 0 {
                if cfg!(feature = "detailed_stats") {
                    non_empty_slots += 1;
                }
                if trace_object(child, mark_sense) {
                    if cfg!(feature = "detailed_stats") {
                        marked_objects += 1;
                    }
                    scan_queue.push_back(child);
                }
            }
        });
    }
    TracingStats {
        marked_objects,
        slots,
        non_empty_slots,
    }
}
