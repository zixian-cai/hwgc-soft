use crate::object_model::Header;
use crate::ObjectModel;

use std::collections::VecDeque;

unsafe fn trace_object(o: u64, mark_sense: u8) -> bool {
    // mark sense is 1 intially, and flip every epoch
    // println!("Trace object: 0x{:x}", o as u64);
    debug_assert_ne!(o, 0);
    let mut header = Header::load(o);
    // Return false if already marked
    let mark_byte = header.get_mark_byte();
    if mark_byte == mark_sense {
        false
    } else {
        header.set_mark_byte(mark_sense);
        header.store(o);
        true
    }
}

pub unsafe fn transitive_closure_node<O: ObjectModel>(mark_sense: u8, object_model: &mut O) -> u64 {
    // Edge-ObjRef enqueuing
    let mut mark_queue: VecDeque<u64> = VecDeque::new();
    for root in object_model.roots() {
        mark_queue.push_back(*root);
    }
    let mut marked_object: u64 = 0;
    while let Some(o) = mark_queue.pop_front() {
        if trace_object(o, mark_sense) {
            // not previously marked, now marked
            // now scan
            marked_object += 1;
            object_model.scan_object(o, |edge| {
                let o = *edge;
                if o != 0 {
                    mark_queue.push_back(o)
                }
            });
        }
    }
    marked_object
}

pub unsafe fn transitive_closure_edge<O: ObjectModel>(mark_sense: u8, object_model: &mut O) -> u64 {
    // Edge-Slot enqueuing
    let mut mark_queue: VecDeque<*mut u64> = VecDeque::new();
    let mut marked_object: u64 = 0;
    for root in object_model.roots() {
        let o = *root;
        if o != 0 && trace_object(o, mark_sense) {
            marked_object += 1;
            object_model.scan_object(o, |edge| mark_queue.push_back(edge))
        }
    }
    while let Some(e) = mark_queue.pop_front() {
        let o = *e;
        if o != 0 && trace_object(o, mark_sense) {
            marked_object += 1;
            object_model.scan_object(o, |edge| mark_queue.push_back(edge))
        }
    }
    marked_object
}

pub fn verify_mark<O: ObjectModel>(mark_sense: u8, object_model: &mut O) {
    for o in object_model.objects() {
        let header = Header::load(*o);
        if header.get_mark_byte() != mark_sense {
            info!("0x{:x} not marked by transitive closure", o);
        }
    }
}
