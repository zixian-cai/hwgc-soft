#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

mod analysis;
mod cli;
#[allow(dead_code)]
mod constants;
mod heapdump;
#[cfg(feature = "m5")]
pub mod m5;
mod memdump;
mod memif;
mod object_model;
mod trace;
mod util;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub use crate::analysis::depth::object_depth;
pub use crate::analysis::reified_analysis;
pub use crate::cli::*;
pub use crate::heapdump::{HeapDump, HeapObject, RootEdge};
pub use crate::memdump::dump_mem;
pub use crate::memif::*;
pub use crate::object_model::{BidirectionalObjectModel, ObjectModel, OpenJDKObjectModel};
pub use crate::trace::reified_trace;
pub use crate::trace::TracingLoopChoice;
