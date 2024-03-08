use crate::{object_model::Header, ObjectModel};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct Slot(*mut u64);

unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

impl Slot {
    pub fn from_raw(ptr: *mut u64) -> Self {
        Slot(ptr)
    }

    pub fn load(&self) -> Option<Object> {
        let v = unsafe { *self.0 };
        if v == 0 {
            None
        } else {
            Some(Object(v))
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct Object(u64);

impl Object {
    fn raw(&self) -> u64 {
        self.0
    }

    pub fn scan<O: ObjectModel, F: FnMut(Slot)>(&self, mut f: F) {
        O::scan_object(self.raw(), |edge, repeat| {
            for i in 0..repeat {
                let ptr = edge.wrapping_add(i as usize);
                f(Slot(ptr));
            }
        })
    }

    pub fn mark(&self, mark_state: u8) -> bool {
        Header::attempt_mark_byte(self.raw(), mark_state)
    }
}
