use crate::ObjectModel;
use crate::RootEdge;

use std::collections::VecDeque;
use std::time::Instant;

unsafe fn trace_object(o: u64, mark_sense: u8) -> bool {
    // mark sense is 1 intially, and flip every epoch
    // println!("Trace object: 0x{:x}", o as u64);
    if o == 0 {
        // skip null
        return false;
    }
    // Return false if already marked
    let mark_word = o as *mut u8;
    if *mark_word == mark_sense {
        false
    } else {
        *mark_word = mark_sense;
        true
    }
}

pub unsafe fn transitive_closure<O: ObjectModel>(
    roots: &[RootEdge],
    mark_sense: u8,
    object_model: &mut O,
) {
    let start: Instant = Instant::now();
    // A queue of objref (possibly null)
    // aka node enqueuing
    let mut mark_queue: VecDeque<u64> = VecDeque::new();
    for root in roots {
        mark_queue.push_back(root.objref);
    }
    let mut marked_object: u64 = 0;
    while let Some(o) = mark_queue.pop_front() {
        if trace_object(o, mark_sense) {
            // not previously marked, now marked
            // now scan
            marked_object += 1;
            object_model.scan_object(o, &mut mark_queue);
        }
    }
    let elapsed = start.elapsed();
    info!(
        "Finished marking {} objects in {} ms",
        marked_object,
        elapsed.as_micros() as f64 / 1000f64
    );
}
