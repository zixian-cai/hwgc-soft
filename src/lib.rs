#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

#[allow(dead_code)]
mod constants;
mod heapdump;
#[cfg(feature = "m5")]
pub mod m5;
mod mark;
mod object_model;
mod sanity;
mod util;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub use crate::heapdump::{HeapDump, HeapObject, RootEdge};
pub use crate::mark::{transitive_closure, verify_mark, TracingLoopChoice};
pub use crate::object_model::{BidirectionalObjectModel, ObjectModel, OpenJDKObjectModel};
pub use crate::sanity::sanity_trace;
