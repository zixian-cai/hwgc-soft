use std::collections::HashMap;

use crate::HeapDump;

#[repr(u8)]
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum TibType {
    Ordinary = 0,
    ObjArray = 1,
    InstanceMirror = 2,
}

pub trait HasTibType {
    fn get_tib_type(&self) -> TibType;
}

pub trait ObjectModel: Send + 'static {
    type Tib: HasTibType;
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
    fn tib_lookup_required(o: u64) -> bool;
}

mod bidirectional;
mod header;
mod openjdk;
pub use bidirectional::BidirectionalObjectModel;
pub use header::{FarwardingState, Header};
pub use openjdk::{OpenJDKObjectModel, Tib as JDKTib};
