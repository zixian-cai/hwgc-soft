use crate::util::align_up;

/// Memory interface for deserializing heapdumps onto a target
///
/// Pointers in heapdump live in the host address space.
/// When deserializing, the memory interface will potentially translate the value
/// to be written (if the value is a pointer) and the destination.
pub trait MemoryInterface {
    unsafe fn write_pointer_to_target<T>(&self, dst_host: *mut *const T, src_host: *const T);
    unsafe fn write_value_to_target<T: std::fmt::Debug>(&self, dst_host: *mut T, src: T);
    unsafe fn translate_host_to_target<T>(&self, ptr_host: *const T) -> *const T;
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

    unsafe fn translate_host_to_target<T>(&self, ptr_host: *const T) -> *const T {
        ptr_host
    }
}

/// Target/host address space aware bump allocation arena
pub struct BumpAllocationArena {
    backing_mem_base: *mut u8,
    backing_mem_cursor: *mut u8,
    backing_mem_limit: *mut u8,
    host_mem_base: *mut u8,
    align: usize,
}

impl BumpAllocationArena {
    pub fn new(
        backing_mem_base: *mut u8,
        host_mem_base: *mut u8,
        size: usize,
        align: usize,
    ) -> Self {
        assert_eq!(backing_mem_base as usize % align, 0);
        assert_eq!(size % align, 0);
        Self {
            backing_mem_base,
            backing_mem_cursor: backing_mem_base,
            backing_mem_limit: unsafe { backing_mem_base.byte_offset(size as isize) },
            host_mem_base,
            align,
        }
    }

    pub fn alloc(&mut self, size: usize) -> (*mut u8, *mut u8) {
        let backing_ret = self.backing_mem_cursor;
        unsafe {
            assert!(self.backing_mem_cursor.byte_offset(size as isize) <= self.backing_mem_limit);
            self.backing_mem_cursor = self
                .backing_mem_cursor
                .byte_offset(align_up(size, self.align) as isize);
        }
        unsafe {
            (
                backing_ret,
                self.host_mem_base
                    .byte_offset(backing_ret.byte_offset_from(self.backing_mem_base)),
            )
        }
    }
}
