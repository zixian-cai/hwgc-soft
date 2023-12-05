use crate::HeapDump;

pub trait ObjectModel {
    fn restore_objects(&mut self, heapdump: &HeapDump);
    fn scan_object<F>(&self, o: u64, callback: F)
    where
        F: FnMut(*mut u64);
    fn roots(&self) -> &[u64];
    fn objects(&self) -> &[u64];
}

mod bidirectional;
mod header;
mod openjdk;
pub use bidirectional::BidirectionalObjectModel;
pub use header::Header;
pub use openjdk::OpenJDKObjectModel;
