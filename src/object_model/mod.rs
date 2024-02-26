use std::collections::HashMap;

use crate::HeapDump;

pub trait ObjectModel {
    type Tib;
    fn restore_tibs(&mut self, heapdump: &HeapDump) -> usize;
    fn restore_objects(&mut self, heapdump: &HeapDump);
    fn scan_object<F>(o: u64, callback: F)
    where
        F: FnMut(*mut u64, u64);
    fn roots(&self) -> &[u64];
    fn objects(&self) -> &[u64];
    fn reset(&mut self);
    fn object_sizes(&self) -> &HashMap<u64, u64>;
    #[allow(clippy::missing_safety_doc)]
    unsafe fn is_objarray(o: u64) -> bool;
    fn get_tib(o: u64) -> *const Self::Tib;
}

mod bidirectional;
mod header;
mod openjdk;
pub use bidirectional::BidirectionalObjectModel;
pub use header::Header;
pub use openjdk::OpenJDKObjectModel;
