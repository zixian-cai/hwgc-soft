#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

mod heapdump;
mod mark;
mod sanity;
mod tib;
mod util;

pub use crate::heapdump::{HeapDump, HeapObject, RootEdge};
pub use crate::mark::transitive_closure;
pub use crate::sanity::sanity_trace;
