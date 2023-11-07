mod generated_src {
    include!(concat!(env!("OUT_DIR"), "/heapdump.generated_src.rs"));
}
use anyhow::Result;
use prost::Message;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub use generated_src::*;

use super::tib::Tib;
use super::util::dzmmap_noreplace;

impl HeapDump {
    pub fn from_binpb_zst(_p: impl AsRef<Path>) -> Result<HeapDump> {
        let file = File::open("heapdump.20.binpb.zst")?;
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

    pub fn restore_objects(&self) {
        for o in &self.objects {
            // unsafe {
            //     std::ptr::write::<u64>((o.start + 8) as *mut u64, o.start);
            // }
            let tib = if o.objarray_length.is_some() {
                Tib::objarray(o.klass)
            } else {
                Tib::non_objarray(o.klass, o)
            };
            if o.objarray_length.is_none() {
                debug_assert_eq!(tib.num_edges(), o.edges.len() as u64);
            }
            // We need to leak this, so the underlying memory won't be collected
            let tib_ptr = Arc::into_raw(tib);
            // println!(
            //     "Object: 0x{:x}, Klass: 0x{:x}, TIB: {:?}, TIB ptr: 0x{:x}",
            //     o.start, o.klass, tib , tib_ptr as u64
            // );
            // Initialize the object
            // Set tib
            unsafe {
                std::ptr::write::<u64>((o.start + 8) as *mut u64, tib_ptr as u64);
            }
            // Write out array length for obj array
            if let Some(l) = o.objarray_length {
                unsafe {
                    std::ptr::write::<u64>((o.start + 16) as *mut u64, l);
                }
            }
            // Write out each non-zero ref field
            for e in &o.edges {
                unsafe {
                    std::ptr::write::<u64>(e.slot as *mut u64, e.objref);
                }
            }
        }
    }
}
