use std::{cell::UnsafeCell, sync::atomic::AtomicU8};

use super::typed_obj::Object;

const fn entries(heap_size: usize) -> usize {
    let log_heap_size = heap_size.next_power_of_two().trailing_zeros();
    1 << (log_heap_size - 7)
}

pub struct SideMarkTable {
    table: UnsafeCell<Vec<AtomicU8>>,
}

impl SideMarkTable {
    pub fn new(heap_size: usize) -> Self {
        Self {
            table: UnsafeCell::new((0..entries(heap_size)).map(|_| AtomicU8::new(0)).collect()),
        }
    }

    pub fn is_marked(&self, o: Object) -> bool {
        let a = (o.raw() - 0x200_0000_0000) >> 4;
        let idx = a >> 3;
        let mask = 1 << (a & 0x7);
        let entry = unsafe { &(*self.table.get())[idx as usize] };
        entry.load(std::sync::atomic::Ordering::Relaxed) & mask != 0
    }

    pub fn mark(&self, o: Object) -> bool {
        let a = (o.raw() - 0x200_0000_0000) >> 4;
        let idx = a >> 3;
        let mask = 1 << (a & 0x7);
        // println!("idx: {}", idx);
        debug_assert!(
            idx < unsafe { (*self.table.get()).len() as u64 },
            "{:?}",
            o.raw() as *const ()
        );
        let entry = unsafe { &(*self.table.get())[idx as usize] };
        let mut old = entry.load(std::sync::atomic::Ordering::Relaxed);
        loop {
            if old & mask != 0 {
                return false;
            }
            let new = old | mask;
            match entry.compare_exchange_weak(
                old,
                new,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(x) => old = x,
            }
        }
    }

    pub fn entries(&self) -> usize {
        unsafe { (*self.table.get()).len() }
    }

    pub fn bulk_zero(&self, range: std::ops::Range<usize>) {
        let start = unsafe { &(*self.table.get())[range.start] } as *const AtomicU8;
        unsafe {
            std::ptr::write_bytes(start as *mut u8, 0, range.end - range.start);
        }
    }
}

unsafe impl Sync for SideMarkTable {}
