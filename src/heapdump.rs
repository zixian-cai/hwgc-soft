mod generated_src {
    include!(concat!(env!("OUT_DIR"), "/heapdump.generated_src.rs"));
}
use anyhow::Result;
use prost::Message;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub use generated_src::*;

use super::util::dzmmap_noreplace;

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
            info!("Mapping {} at 0x{:x}", s.name, s.start);
            dzmmap_noreplace(s.start, (s.end - s.start) as usize)?;
        }
        Ok(())
    }
}
