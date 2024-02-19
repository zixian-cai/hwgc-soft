mod generated_src {
    include!(concat!(env!("OUT_DIR"), "/heapdump.generated_src.rs"));
}
use anyhow::Result;
use prost::Message;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub use generated_src::*;

use super::util::{dzmmap_noreplace, munmap};

pub enum Space {
    Immix,
    Immortal,
    Los,
    Nonmoving,
}

impl HeapDump {
    pub fn from_binpb_zst(p: impl AsRef<Path>) -> Result<HeapDump> {
        let file = File::open(p)?;
        let mut reader = zstd::Decoder::new(file)?;
        let mut buf = vec![];
        reader.read_to_end(&mut buf)?;
        Ok(HeapDump::decode(buf.as_slice())?)
    }

    pub fn map_spaces(&self) -> Result<()> {
        for s in &self.spaces {
            debug!("Mapping {} at 0x{:x}", s.name, s.start);
            dzmmap_noreplace(s.start, (s.end - s.start) as usize)?;
        }
        Ok(())
    }

    pub fn unmap_spaces(&self) -> Result<()> {
        for s in &self.spaces {
            debug!("Unmapping {} at 0x{:x}", s.name, s.start);
            munmap(s.start, (s.end - s.start) as usize)?;
        }
        Ok(())
    }

    pub fn get_space_type(o: u64) -> Space {
        let space_mask: u64 = 0xe0000000000;
        let space_shift: u64 = 41;
        match (o & space_mask) >> space_shift {
            1 => Space::Immix,
            2 => Space::Immortal,
            3 => Space::Los,
            4 => Space::Nonmoving,
            _ => unreachable!(),
        }
    }
}
