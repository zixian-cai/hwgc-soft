#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

mod heapdump;
#[cfg(feature = "m5")]
pub mod m5;
mod mark;
mod object_model;
mod sanity;
mod util;

pub use crate::heapdump::{HeapDump, HeapObject, RootEdge};
pub use crate::mark::{transitive_closure, verify_mark};
pub use crate::object_model::{ObjectModel, OpenJDKObjectModel};
pub use crate::sanity::sanity_trace;
