use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[repr(transparent)]
pub struct Header(u64);

impl Header {
    pub fn new() -> Self {
        Header(0)
    }

    pub fn load(o: u64) -> Self {
        unsafe { Header(*(o as *mut u64)) }
    }

    pub fn store(self, o: u64) {
        unsafe { *(o as *mut u64) = self.0 };
    }

    pub fn get_mark_byte(&self) -> u8 {
        self.get_byte(0)
    }

    pub fn set_mark_byte(&mut self, val: u8) {
        self.set_byte(val, 0);
    }

    pub fn attempt_mark_byte(o: u64, new_byte: u8) -> bool {
        let old_byte = Header::load(o).get_mark_byte();
        if old_byte == new_byte {
            return false;
        }
        let work = unsafe { &*(o as *const u64 as *const AtomicU8) };
        work.compare_exchange(old_byte, new_byte, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn get_byte(&self, offset: u8) -> u8 {
        let mask = (u8::MAX as u64) << (offset << 3);
        ((self.0 & mask) >> (offset << 3)) as u8
    }

    pub fn set_byte(&mut self, val: u8, offset: u8) {
        let mask: u64 = (u8::MAX as u64) << (offset << 3);
        let to_set_shifted = (val as u64) << (offset << 3);
        self.0 = (self.0 & !mask) | to_set_shifted;
    }
}
