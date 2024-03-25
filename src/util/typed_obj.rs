use crate::{
    object_model::{FarwardingState, Header, JDKTib},
    trace::trace_object,
    ObjectModel,
};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[repr(transparent)]
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

    pub fn volatile_store(&self, obj: Object) {
        unsafe { std::ptr::write_volatile(self.0, obj.raw()) }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[repr(transparent)]
pub struct Object(u64);

impl Object {
    pub const fn raw(&self) -> u64 {
        self.0
    }

    fn tib<O: ObjectModel>(&self) -> &JDKTib {
        unsafe { &*(O::get_tib(self.raw()) as *const JDKTib) }
    }

    pub fn size<O: ObjectModel>(&self) -> usize {
        if let Some(scalar_size) = self.tib::<O>().scalar_size {
            debug_assert!(scalar_size >= 16);
            scalar_size as usize
        } else {
            let s = unsafe { *(self.raw() as *const u64).wrapping_add(2) };
            debug_assert!(s >= 16);
            s as usize
        }
    }

    pub fn scan<O: ObjectModel, F: FnMut(Slot)>(&self, mut f: F) {
        O::scan_object(self.raw(), |edge, repeat| {
            for i in 0..repeat {
                let ptr = edge.wrapping_add(i as usize);
                f(Slot(ptr));
            }
        })
    }

    pub fn is_marked(&self, mark_state: u8) -> bool {
        Header::is_marked(self.raw(), mark_state)
    }

    pub fn mark(&self, mark_state: u8) -> bool {
        Header::attempt_mark_byte(self.raw(), mark_state)
    }

    pub fn mark_relaxed(&self, mark_state: u8) -> bool {
        unsafe { trace_object(self.raw(), mark_state) }
    }

    pub fn attempt_to_forward(&self, mark_state: u8) -> FarwardingState {
        Header::attempt_to_forward(self.raw(), mark_state)
    }

    pub fn spin_and_get_farwarded_object(&self, mark_state: u8) -> Object {
        let o = Header::spin_and_get_farwarded_object(self.raw(), mark_state);
        Object(o)
    }

    pub fn set_as_forwarded(&self, mark_state: u8) {
        Header::set_as_forwarded(self.raw(), mark_state)
    }

    pub fn space_id(&self) -> u8 {
        // Immix space: 0x200_0000_0000
        ((self.raw() >> 40) & 0xf) as u8
    }
}
