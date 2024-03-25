use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FarwardingState {
    NotForwarded,
    Forwarding,
    Forwarded,
}

impl FarwardingState {
    pub fn is_forwarded_or_being_forwarded(&self) -> bool {
        match self {
            FarwardingState::Forwarded => true,
            FarwardingState::Forwarding => true,
            _ => false,
        }
    }
}

#[repr(transparent)]
pub struct Header(u64);

impl Header {
    pub fn new() -> Self {
        Header(0)
    }

    pub fn load(o: u64) -> Self {
        unsafe { Header(*(o as *mut u64)) }
    }

    pub fn volatile_load(o: u64) -> Self {
        unsafe { Header(std::ptr::read_volatile(o as *mut u64)) }
    }

    pub fn store(self, o: u64) {
        unsafe { *(o as *mut u64) = self.0 };
    }

    fn get_fwd_byte(&self) -> u8 {
        self.get_byte(7)
    }

    pub fn get_mark_byte(&self) -> u8 {
        self.get_byte(0)
    }

    pub fn set_mark_byte(&mut self, val: u8) {
        self.set_byte(val, 0);
    }

    pub fn is_marked(o: u64, new_byte: u8) -> bool {
        let old_byte = Header::load(o).get_mark_byte();
        return old_byte == new_byte;
    }

    pub fn attempt_mark_byte(o: u64, new_byte: u8) -> bool {
        let old_byte = Header::load(o).get_mark_byte();
        if old_byte == new_byte {
            return false;
        }
        let byte = unsafe { &*(o as *const u64 as *const AtomicU8) };
        byte.compare_exchange(old_byte, new_byte, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn attempt_to_forward(o: u64, forwarded_state: u8) -> FarwardingState {
        let byte = unsafe { &*(o as *const u64 as *const AtomicU8).add(7) };
        let old_byte = byte.load(Ordering::SeqCst);
        if old_byte == forwarded_state {
            return FarwardingState::Forwarded;
        }
        if old_byte == 0xff {
            return FarwardingState::Forwarding;
        }
        debug_assert_ne!(old_byte, 0xff);
        debug_assert_ne!(forwarded_state, 0xff);
        let result = byte.compare_exchange(old_byte, 0xff, Ordering::SeqCst, Ordering::SeqCst);
        match result {
            Ok(_) => FarwardingState::NotForwarded,
            Err(old_state) => {
                if old_state == 0xff {
                    FarwardingState::Forwarding
                } else {
                    debug_assert_eq!(old_state, forwarded_state);
                    FarwardingState::Forwarded
                }
            }
        }
    }

    pub fn spin_and_get_farwarded_object(o: u64, forwarded_state: u8) -> u64 {
        loop {
            std::hint::spin_loop();
            let state = Header::volatile_load(o).get_fwd_byte();
            if state == forwarded_state {
                return o;
            }
            if state == 0xff {
                continue;
            }
        }
    }

    pub fn set_as_forwarded(o: u64, forwarded_state: u8) {
        let byte = unsafe { &*(o as *const u64 as *const AtomicU8).add(7) };
        byte.store(forwarded_state, Ordering::SeqCst);
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
