use wp::{Object, Slot};

use super::{trace_object, TracingStats};
use crate::{ObjectModel, OpenJDKObjectModel};
use std::sync::atomic::{AtomicU64, Ordering};

type O = OpenJDKObjectModel<false>;

trait ObjectOps {
    fn get(&self) -> u64;
    fn scan_object<F: FnMut(Slot)>(&self, mut f: F) {
        O::scan_object(self.get(), |edge, repeat| {
            for i in 0..repeat {
                let ptr = edge.wrapping_add(i as usize);
                f(Slot(ptr));
            }
        })
    }
    fn mark(&self) -> bool {
        unsafe { trace_object(self.get(), 1) }
    }
}

impl ObjectOps for Object {
    fn get(&self) -> u64 {
        self.0
    }
}

pub static MARK_QUEUE: wp::LocalQueue<Slot> = wp::LocalQueue::new(|slot| {
    // for slot in slots {
    SLOTS.fetch_add(1, Ordering::SeqCst);
    if let Some(o) = slot.load() {
        if o.mark() {
            MARKED_OBJECTS.fetch_add(1, Ordering::SeqCst);
            o.scan_object(|s| MARK_QUEUE.push(s));
        }
    } else {
        NON_EMPTY_SLOTS.fetch_add(1, Ordering::SeqCst);
    }
    // }
});

pub(super) unsafe fn transitive_closure<O: ObjectModel>(
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    for root in object_model.roots() {
        MARK_QUEUE.push(Slot(root as *const u64 as *mut u64));
    }
    MARK_QUEUE.consume();
    TracingStats {
        marked_objects: MARKED_OBJECTS.load(Ordering::SeqCst),
        slots: SLOTS.load(Ordering::SeqCst),
        non_empty_slots: NON_EMPTY_SLOTS.load(Ordering::SeqCst),
        sends: 0,
    }
}

static MARKED_OBJECTS: AtomicU64 = AtomicU64::new(0);
static SLOTS: AtomicU64 = AtomicU64::new(0);
static NON_EMPTY_SLOTS: AtomicU64 = AtomicU64::new(0);
