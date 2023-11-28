use std::collections::VecDeque;

use crate::HeapDump;

pub trait ObjectModel {
    fn restore_objects(&mut self, heapdump: &HeapDump);
    fn scan_object(&mut self, o: u64, mark_queue: &mut VecDeque<u64>);
    fn roots(&self) -> &[u64];
    fn objects(&self) -> &[u64];
}

mod openjdk;
pub use openjdk::OpenJDKObjectModel;
