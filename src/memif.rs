use crate::util::align_up;

/// Memory interface for deserializing heapdumps onto a target
///
/// Pointers in heapdump live in the host address space.
/// When deserializing, the memory interface will potentially translate the value
/// to be written (if the value is a pointer) and the destination.
pub trait MemoryInterface {
    unsafe fn write_pointer_to_target<T>(&self, dst_host: *mut *const T, src_host: *const T);
    unsafe fn write_value_to_target<T>(&self, dst_host: *mut T, src: T);
}

/// Memory Interface for when host and target address space are the same.
///
/// For example, this happens when calling heapdump.map_spaces.
pub struct NoOpMemoryInterface {}

impl Default for NoOpMemoryInterface {
    fn default() -> Self {
        Self::new()
    }
}

impl NoOpMemoryInterface {
    pub fn new() -> Self {
        NoOpMemoryInterface {}
    }
}

impl MemoryInterface for NoOpMemoryInterface {
    unsafe fn write_pointer_to_target<T>(&self, dst_host: *mut *const T, src_host: *const T) {
        std::ptr::write::<*const T>(dst_host, src_host);
    }

    unsafe fn write_value_to_target<T>(&self, dst_host: *mut T, src: T) {
        std::ptr::write::<T>(dst_host, src);
    }
}

pub struct BumpAllocationArena {
    cursor: *mut u8,
    limit: *mut u8,
    align: usize,
}

impl BumpAllocationArena {
    pub fn new(start: *mut u8, size: usize, align: usize) -> Self {
        assert_eq!(start as usize % align, 0);
        assert_eq!(size % align, 0);
        Self {
            cursor: start,
            limit: unsafe { start.byte_offset(size as isize) },
            align,
        }
    }

    pub fn alloc(&mut self, size: usize) -> *mut u8 {
        let ret = self.cursor;
        unsafe {
            assert!(self.cursor.byte_offset(size as isize) <= self.limit);
            self.cursor = self.cursor.byte_offset(align_up(size, self.align) as isize);
        }
        ret
    }
}
