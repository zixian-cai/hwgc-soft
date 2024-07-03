#![allow(stable_features)]
#![allow(incomplete_features)]
#![feature(thread_local)]
#![feature(test)]
#![feature(lazy_cell)]
#![feature(duration_millis_float)]
#![feature(adt_const_params)]

extern crate test;

#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

#[macro_use]
mod util;

mod analysis;
mod cli;
#[allow(dead_code)]
mod constants;
mod heapdump;
#[cfg(feature = "m5")]
pub mod m5;
mod object_model;
mod trace;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub use crate::analysis::depth::object_depth;
pub use crate::analysis::reified_analysis;
pub use crate::cli::*;
pub use crate::heapdump::{HeapDump, HeapObject, RootEdge};
pub use crate::object_model::{BidirectionalObjectModel, ObjectModel, OpenJDKObjectModel};
pub use crate::trace::TracingLoopChoice;
pub use crate::trace::{bench, reified_trace};
