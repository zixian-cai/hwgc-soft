use clap::ValueEnum;

use crate::object_model::Header;
use crate::ObjectModel;

use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum TracingLoopChoice {
    EdgeSlot,
    EdgeObjref,
    NodeObjref,
    DistributedNodeObjref,
}

#[derive(Debug)]
pub struct TracingStats {
    pub marked_objects: u64,
    pub slots: u64,
    pub non_empty_slots: u64,
    pub sends: u64,
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

mod distributed_node_objref;
mod edge_objref;
mod edge_slot;
mod node_objref;

pub fn transitive_closure<O: ObjectModel>(
    l: TracingLoopChoice,
    mark_sense: u8,
    object_model: &mut O,
) -> TimedTracingStats {
    let start: Instant = Instant::now();
    let stats = unsafe {
        match l {
            TracingLoopChoice::EdgeObjref => {
                edge_objref::transitive_closure_edge_objref(mark_sense, object_model)
            }
            TracingLoopChoice::EdgeSlot => {
                edge_slot::transitive_closure_edge_slot(mark_sense, object_model)
            }
            TracingLoopChoice::NodeObjref => {
                node_objref::transitive_closure_node_objref(mark_sense, object_model)
            }
            TracingLoopChoice::DistributedNodeObjref => {
                distributed_node_objref::transitive_closure_distributed_node_objref(
                    mark_sense,
                    object_model,
                )
            }
        }
    };
    let elapsed = start.elapsed();
    TimedTracingStats {
        stats,
        time: elapsed,
    }
}

pub fn verify_mark<O: ObjectModel>(mark_sense: u8, object_model: &mut O) {
    for o in object_model.objects() {
        let header = Header::load(*o);
        if header.get_mark_byte() != mark_sense {
            error!("0x{:x} not marked by transitive closure", o);
        }
    }
}
