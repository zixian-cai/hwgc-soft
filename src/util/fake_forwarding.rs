use std::{ops::Range, sync::Mutex};

use crate::ObjectModel;

use super::typed_obj::Object;

pub struct ToSpace {
    to_space: Mutex<Vec<Vec<u8>>>,
}

impl ToSpace {
    pub const fn new() -> Self {
        Self {
            to_space: Mutex::new(Vec::new()),
        }
    }

    fn alloc_tlab(&self, obj_size: usize) -> Option<Range<*mut u8>> {
        let tlab_size = usize::max(obj_size, 32 << 10).next_power_of_two();
        let mut buf = vec![0; tlab_size];
        let range = buf.as_mut_ptr_range();
        let mut to_space = self.to_space.lock().unwrap();
        to_space.push(buf);
        Some(range)
    }

    pub fn reset(&self) {
        self.to_space.lock().unwrap().clear();
    }
}

pub static TO_SPACE: ToSpace = ToSpace::new();

pub struct LocalAllocator {
    cursor: *mut u8,
    limit: *mut u8,
}

unsafe impl Send for LocalAllocator {}
unsafe impl Sync for LocalAllocator {}

impl LocalAllocator {
    pub const fn new() -> Self {
        Self {
            cursor: 0 as _,
            limit: 0 as _,
        }
    }

    fn align_up(&self, ptr: *mut u8) -> *mut u8 {
        let align = std::mem::size_of::<usize>();
        let align_mask = align - 1;
        ((ptr as usize + align_mask) & !align_mask) as *mut u8
    }

    fn copy_alloc_slow(&mut self, obj_size: usize) -> Option<*mut u8> {
        unsafe {
            let tlab = TO_SPACE.alloc_tlab(obj_size)?;
            let ptr = self.align_up(tlab.start);
            self.cursor = ptr.add(obj_size);
            self.limit = tlab.end;
            debug_assert!(!ptr.is_null());
            debug_assert!(self.cursor <= self.limit);
            Some(ptr)
        }
    }

    fn copy_alloc(&mut self, obj_size: usize) -> Option<*mut u8> {
        unsafe {
            let ptr = self.align_up(self.cursor);
            let new_cursor = ptr.add(obj_size);
            if new_cursor <= self.limit {
                self.cursor = new_cursor;
                debug_assert!(!ptr.is_null());
                debug_assert!(self.cursor <= self.limit);
                Some(ptr)
            } else {
                self.copy_alloc_slow(obj_size)
            }
        }
    }

    pub fn copy_object<O: ObjectModel>(&mut self, o: Object) {
        let size = o.size::<O>();
        let ptr = self.copy_alloc(size).unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping::<u8>(o.raw() as *const u8, ptr, size);
        }
    }

    pub fn reset(&mut self) {
        self.cursor = 0 as _;
        self.limit = 0 as _;
    }
}
