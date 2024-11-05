use std::fs;

use crate::MemoryInterface;

use super::{BumpAllocationArena, Memdump, MemdumpMapping};

/// Memory dump for making heapdumps usable on devices with physical memory only
///
/// It manages a chunk of host memory backing the device/target address space,
/// which will be later be written to disk
///
/// It also maintains a translation between heapdump/host (or other synthetic
/// workload)'s address space and the target address space (and the
/// corresponding backing memory)
#[derive(Debug)]
pub(super) struct MemdumpPhysical {
    backing_memory: *mut u8,
    backing_memory_size: usize,
    backing_memory_cursor: *mut u8,
    target_memory_base: *mut u8,
    mappings: Vec<MemdumpPhysicalMapping>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MemdumpPhysicalMapping {
    backing_memory_start: *mut u8,
    target_memory_start: *mut u8,
    host_memory_start: *mut u8,
    size: usize,
}

impl MemdumpMapping for MemdumpPhysicalMapping {
    fn to_arena(&self, align: usize) -> BumpAllocationArena {
        BumpAllocationArena::new(
            self.backing_memory_start,
            self.host_memory_start,
            self.size,
            align,
        )
    }
}

pub(super) struct MemdumpPhysicalMemoryInterface {
    sorted_mappings: Vec<MemdumpPhysicalMapping>,
}

impl Memdump for MemdumpPhysical {
    type MI = MemdumpPhysicalMemoryInterface;
    type MM = MemdumpPhysicalMapping;
    fn gen_memif(&self) -> MemdumpPhysicalMemoryInterface {
        let mut mappings = self.mappings.clone();
        mappings.sort_by_key(|x| x.host_memory_start);
        MemdumpPhysicalMemoryInterface {
            sorted_mappings: mappings,
        }
    }

    fn new_mapping(&mut self, host_memory_start: *mut u8, size: usize) -> MemdumpPhysicalMapping {
        assert_ne!(host_memory_start as usize, 0);
        let size_aligned = crate::util::align_up(size, 4096);
        let old_cursor = self.backing_memory_cursor;
        let new_cursor = self.backing_memory_cursor.wrapping_add(size_aligned);
        assert!(new_cursor <= self.backing_memory.wrapping_add(self.backing_memory_size));
        self.backing_memory_cursor = new_cursor;
        let ret = MemdumpPhysicalMapping {
            backing_memory_start: old_cursor,
            target_memory_start: unsafe {
                self.target_memory_base
                    .byte_offset(old_cursor.offset_from(self.backing_memory))
            },
            host_memory_start,
            size,
        };
        info!("MemdumpPhysical new mapping: {:?}", ret);
        self.mappings.push(ret);
        ret
    }

    unsafe fn dump_to_file(&self, output: &str) {
        let len = self.backing_memory_cursor.offset_from(self.backing_memory);
        let slice = std::slice::from_raw_parts(self.backing_memory, len as usize);
        fs::write(output, slice).unwrap();
    }
}

impl MemdumpPhysicalMemoryInterface {
    fn translate<T, F>(&self, ptr_host: *const T, key: F) -> *mut u8
    where
        F: Fn(&MemdumpPhysicalMapping) -> *mut u8,
    {
        let r = self
            .sorted_mappings
            .binary_search_by_key(&(ptr_host as *mut u8), |x| x.host_memory_start);
        match r {
            Ok(x) => key(&self.sorted_mappings[x]),
            Err(x) => unsafe {
                debug_assert!(x > 0);
                let mapping = &self.sorted_mappings[x - 1];
                let host_offset = ptr_host.byte_offset_from(mapping.host_memory_start);
                debug_assert!(host_offset > 0 && host_offset < mapping.size as isize);
                key(mapping).byte_offset(host_offset)
            },
        }
    }

    fn translate_to_target<T>(&self, ptr_host: *const T) -> *mut u8 {
        self.translate(ptr_host, |m| m.target_memory_start)
    }

    fn translate_to_backing<T>(&self, ptr_host: *const T) -> *mut u8 {
        self.translate(ptr_host, |m| m.backing_memory_start)
    }
}

impl MemoryInterface for MemdumpPhysicalMemoryInterface {
    unsafe fn write_pointer_to_target<T>(&self, dst_host: *mut *const T, src_host: *const T) {
        let dst_backing = self.translate_to_backing(dst_host);
        let src_target = if src_host == std::ptr::null() {
            std::ptr::null()
        } else {
            self.translate_to_target(src_host)
        };
        // dbg!(dst_host, src_host);
        // dbg!(dst_backing, src_target);
        std::ptr::write(dst_backing as *mut *const T, src_target as *const T);
    }

    unsafe fn write_value_to_target<T: std::fmt::Debug>(&self, dst_host: *mut T, src: T) {
        let dst_backing = self.translate_to_backing(dst_host);
        // dbg!(dst_host, &src);
        // dbg!(dst_backing);
        std::ptr::write(dst_backing as *mut T, src);
    }

    unsafe fn translate_host_to_target<T>(&self, ptr_host: *const T) -> *const T {
        self.translate_to_target(ptr_host) as *const T
    }
}

impl MemdumpPhysical {
    pub(super) fn new(size: usize, target_memory_base: *mut u8) -> Self {
        assert_eq!(
            size % 4096,
            0,
            "MemdumpPhysical size must be mulitples of 4KB"
        );
        let backing_memory = crate::util::mmap_anon(size).unwrap() as *mut u8;
        MemdumpPhysical {
            backing_memory,
            backing_memory_size: size,
            backing_memory_cursor: backing_memory,
            target_memory_base,
            mappings: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_memif() -> MemdumpPhysicalMemoryInterface {
        let immix = MemdumpPhysicalMapping {
            backing_memory_start: 0x1_0000_0000usize as _,
            target_memory_start: 0x8000_0000usize as _,
            host_memory_start: 0x200_0000_0000usize as _,
            size: 100usize * 1024 * 1024, // 100 MiB
        };
        let immortal = MemdumpPhysicalMapping {
            backing_memory_start: (0x1_0000_0000usize + 0x6400000) as _,
            target_memory_start: (0x8000_0000usize + 0x6400000) as _,
            host_memory_start: 0x400_0000_0000usize as _,
            size: 100usize * 1024 * 1024, // 100 MiB
        };
        let los = MemdumpPhysicalMapping {
            backing_memory_start: (0x1_0000_0000usize + 0xc800000) as _,
            target_memory_start: (0x8000_0000usize + 0xc800000) as _,
            host_memory_start: 0x600_0000_0000usize as _,
            size: 100usize * 1024 * 1024, // 100 MiB
        };
        let nonmoving = MemdumpPhysicalMapping {
            backing_memory_start: (0x1_0000_0000usize + 0x12c00000) as _,
            target_memory_start: (0x8000_0000usize + 0x12c00000) as _,
            host_memory_start: 0x800_0000_0000usize as _,
            size: 100usize * 1024 * 1024, // 100 MiB
        };
        let mappings = vec![immix, immortal, los, nonmoving];
        MemdumpPhysicalMemoryInterface {
            sorted_mappings: mappings,
        }
    }

    #[test]
    fn test_backing() {
        let memif = get_memif();
        assert_eq!(
            memif.translate_to_backing(0x200_0000_0000usize as *const u8),
            0x1_0000_0000usize as _
        );
        assert_eq!(
            memif.translate_to_backing(0x200_0000_0008usize as *const u8),
            0x1_0000_0008usize as _
        );
    }

    #[test]
    #[should_panic(expected = "assertion failed")]
    fn test_backing_below_lower_bound() {
        let memif = get_memif();
        memif.translate_to_backing(0x100_0000_0000usize as *const u8);
    }

    #[test]
    #[should_panic(expected = "assertion failed")]
    fn test_backing_above_upper_bound() {
        let memif = get_memif();
        memif.translate_to_backing(0x900_0000_0000usize as *const u8);
    }

    #[test]
    #[should_panic(expected = "assertion failed")]
    fn test_backing_in_a_hole() {
        let memif = get_memif();
        memif.translate_to_backing(0x300_0000_0000usize as *const u8);
    }
}
