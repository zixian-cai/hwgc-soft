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

/// A heap graph view over raw memory
///
/// Stores both objects and metadata (tib)
///
/// Note that an instance of ObjectModel should only be used on heapdumps
/// from the same benchmark invocation, where objects with the same Klass/type
/// id indeed have the same type.
pub trait ObjectModel: Send + 'static {
    type Tib: HasTibType;
    /// Restore TIBs for classes found in the heapdump
    ///
    /// Cache these TIBs across multiple calls, so already-known types
    /// don't need to have TIBs allocated multiple times.
    ///
    /// Note that InstanceMirrorKlass (i.e., those objects whose
    /// instance_mirror_start is some) is never cached.
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
pub use header::Header;
pub use openjdk::OpenJDKObjectModel;
