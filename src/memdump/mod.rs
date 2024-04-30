use std::fs;

use crate::*;
use anyhow::Result;

mod linked_list;

#[derive(Debug)]
struct Memdump {
    start: *mut u8,
    size: usize,
    segments: Vec<MemdumpSegment>,
    cursor: *mut u8,
    mem_base: *mut u8,
}

#[derive(Debug, Clone, Copy)]
struct MemdumpSegment {
    start: *mut u8,
    size: usize,
}

impl Memdump {
    fn new(size: usize, mem_base: *mut u8) -> Self {
        assert_eq!(size % 4096, 0, "Memdump size must be mulitples of 4KB");
        let start = crate::util::mmap_anon(size).unwrap() as *mut u8;
        Memdump {
            start,
            size,
            segments: vec![],
            cursor: start,
            mem_base,
        }
    }

    fn alloc_segment(&mut self, size: usize) -> MemdumpSegment {
        let size_aligned = crate::util::align_up(size, 4096);
        let ret = self.cursor;
        let new_cursor = self.cursor.wrapping_add(size_aligned);
        assert!(new_cursor <= self.start.wrapping_add(self.size));
        self.cursor = new_cursor;
        MemdumpSegment { start: ret, size }
    }

    unsafe fn dump_to_file(&self, output: &str) {
        let len = self.cursor.offset_from(self.start);
        let slice = std::slice::from_raw_parts(self.start, len as usize);
        fs::write(output, slice).unwrap();
    }

    fn translate_to_target<T: Sized>(&self, host_ptr: *mut T) -> *mut T {
        unsafe {
            self.mem_base
                .byte_offset(host_ptr.byte_offset_from(self.start)) as *mut T
        }
    }
}

trait MemdumpWorkload {
    unsafe fn gen_memdump(&self, md: &mut Memdump);
}

// Such as PATH=$HOME/protoc/bin:$PATH cargo run --release -- ../heapdumps/sampled/luindex/* -o OpenJDK memdump --workload LinkedList --output ./1.bin --mem-start 0xc0000000
pub fn dump_mem<O: ObjectModel>(_object_model: O, args: Args) -> Result<()> {
    let memdump_args = if let Some(Commands::Memdump(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    unsafe {
        let mut memdump = Memdump::new(
            1024usize * 1024 * 1024 * 4,
            memdump_args.mem_base as *mut u8,
        );
        match memdump_args.workload {
            cli::MemdumpWorkload::LinkedList => {
                linked_list::LinkedList::new(1024).gen_memdump(&mut memdump)
            }
        }
        memdump.dump_to_file(&memdump_args.output);
    }
    Ok(())
}
