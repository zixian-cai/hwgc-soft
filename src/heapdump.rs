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

#[derive(Debug)]
pub enum Space {
    Immix,
    Immortal,
    Los,
    Nonmoving,
}

#[derive(Debug)]
pub(crate) struct SpaceLimit {
    pub(crate) space: Space,
    pub(crate) space_base: u64,
    pub(crate) space_start: u64,
    pub(crate) space_limit: u64,
}

impl SpaceLimit {
    pub(crate) fn get_size(&self) -> u64 {
        self.space_limit - self.space_start
    }
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

    pub(crate) fn calculate_space_limits(&self) -> Vec<SpaceLimit> {
        let mut limits = vec![];
        for s in &self.spaces {
            limits.push(SpaceLimit {
                space: Self::get_space_type(s.start),
                space_base: s.start,
                space_start: u64::MAX,
                space_limit: s.start,
            });
        }
        for o in &self.objects {
            let pos = limits.binary_search_by_key(&o.start, |s| s.space_base);
            let obj_end = o.start + o.size;
            let space_idx = match pos {
                Ok(i) => i,
                Err(i) => i - 1,
            };
            if obj_end > limits[space_idx].space_limit {
                limits[space_idx].space_limit = obj_end;
            }
            if o.start < limits[space_idx].space_start {
                limits[space_idx].space_start = o.start;
            }
        }
        limits
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
