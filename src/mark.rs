use clap::ValueEnum;

use crate::object_model::Header;
use crate::ObjectModel;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum TracingLoopChoice {
    EdgeSlot,
    EdgeObjref,
    NodeObjref,
}

#[derive(Debug)]
pub struct TracingStats {
    pub marked_objects: u64,
}

#[derive(Debug)]
pub struct TimedTracingStats {
    pub stats: TracingStats,
    pub time: Duration,
}

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

pub fn transitive_closure<O: ObjectModel>(
    l: TracingLoopChoice,
    mark_sense: u8,
    object_model: &mut O,
) -> TimedTracingStats {
    let start: Instant = Instant::now();
    let stats = unsafe {
        match l {
            TracingLoopChoice::EdgeObjref => {
                transitive_closure_edge_objref(mark_sense, object_model)
            }
            TracingLoopChoice::EdgeSlot => transitive_closure_edge_slot(mark_sense, object_model),
            TracingLoopChoice::NodeObjref => {
                transitive_closure_node_objref(mark_sense, object_model)
            }
        }
    };
    let elapsed = start.elapsed();
    TimedTracingStats {
        stats,
        time: elapsed,
    }
}

unsafe fn transitive_closure_edge_objref<O: ObjectModel>(
    mark_sense: u8,
    object_model: &mut O,
) -> TracingStats {
    // Edge-ObjRef enqueuing
    let mut mark_queue: VecDeque<u64> = VecDeque::new();
    for root in object_model.roots() {
        mark_queue.push_back(*root);
    }
    let mut marked_objects: u64 = 0;
    while let Some(o) = mark_queue.pop_front() {
        if trace_object(o, mark_sense) {
            // not previously marked, now marked
            // now scan
            marked_objects += 1;
            object_model.scan_object(o, |edge| {
                let o = *edge;
                if o != 0 {
                    mark_queue.push_back(o)
                }
            });
        }
    }
    TracingStats { marked_objects }
}

unsafe fn transitive_closure_node_objref<O: ObjectModel>(
    mark_sense: u8,
    object_model: &mut O,
) -> TracingStats {
    // Node-ObjRef enqueuing
    let mut scan_queue: VecDeque<u64> = VecDeque::new();
    let mut marked_objects: u64 = 0;
    for root in object_model.roots() {
        let o = *root;
        if o != 0 && trace_object(o, mark_sense) {
            marked_objects += 1;
            scan_queue.push_back(o);
        }
    }
    while let Some(o) = scan_queue.pop_front() {
        object_model.scan_object(o, |edge| {
            let child = *edge;
            if child != 0 && trace_object(child, mark_sense) {
                marked_objects += 1;
                scan_queue.push_back(child);
            }
        });
    }
    TracingStats { marked_objects }
}

unsafe fn transitive_closure_edge_slot<O: ObjectModel>(
    mark_sense: u8,
    object_model: &mut O,
) -> TracingStats {
    // Edge-Slot enqueuing
    let mut mark_queue: VecDeque<*mut u64> = VecDeque::new();
    let mut marked_objects: u64 = 0;
    for root in object_model.roots() {
        let o = *root;
        if o != 0 && trace_object(o, mark_sense) {
            marked_objects += 1;
            object_model.scan_object(o, |edge| mark_queue.push_back(edge))
        }
    }
    while let Some(e) = mark_queue.pop_front() {
        let o = *e;
        if o != 0 && trace_object(o, mark_sense) {
            marked_objects += 1;
            object_model.scan_object(o, |edge| mark_queue.push_back(edge))
        }
    }
    TracingStats { marked_objects }
}

pub fn verify_mark<O: ObjectModel>(mark_sense: u8, object_model: &mut O) {
    for o in object_model.objects() {
        let header = Header::load(*o);
        if header.get_mark_byte() != mark_sense {
            error!("0x{:x} not marked by transitive closure", o);
        }
    }
}
