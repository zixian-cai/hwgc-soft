use std::fs;

use crate::*;
use anyhow::Result;

#[derive(Debug)]
struct Memdump {
    start: *mut u8,
    size: usize,
    segments: Vec<MemdumpSegment>,
    cursor: *mut u8,
}

#[derive(Debug, Clone, Copy)]
struct MemdumpSegment {
    start: *mut u8,
    size: usize,
}

impl Memdump {
    fn new(size: usize) -> Self {
        assert_eq!(size % 4096, 0, "Memdump size must be mulitples of 4KB");
        let start = crate::util::mmap_anon(size).unwrap() as *mut u8;
        Memdump {
            start,
            size,
            segments: vec![],
            cursor: start,
        }
    }

    fn alloc_segment(&mut self, size: usize) -> MemdumpSegment {
        let cursor = self.cursor.wrapping_add(size);
        assert!(cursor <= self.start.wrapping_add(self.size));
        self.cursor = cursor;
        todo!()
    }

    unsafe fn dump_to_file(&self, output: &str) {
        let len = self.cursor.offset_from(self.start);
        let slice = std::slice::from_raw_parts(self.start, len as usize);
        fs::write(output, slice).unwrap();
    }
}

// Such as PATH=$HOME/protoc/bin:$PATH cargo run --release -- ../heapdumps/sampled/luindex/* -o OpenJDK memdump --workload LinkedList --output ./1.bin --mem-start 0xc0000000
pub fn dump_mem<O: ObjectModel>(_object_model: O, args: Args) -> Result<()> {
    let memdump_args = if let Some(Commands::Memdump(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    let memdump = Memdump::new(1024usize * 1024 * 1024 * 4);
    unsafe { memdump.dump_to_file(&memdump_args.output) };
    Ok(())
}
