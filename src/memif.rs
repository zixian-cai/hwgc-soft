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
