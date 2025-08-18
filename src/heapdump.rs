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
    fn from_binpb_zst(p: impl AsRef<Path>) -> Result<HeapDump> {
        let file = File::open(p)?;
        let mut reader = zstd::Decoder::new(file)?;
        let mut buf = vec![];
        reader.read_to_end(&mut buf)?;
        Ok(HeapDump::decode(buf.as_slice())?)
    }

    pub fn from_path(path: &str) -> Result<HeapDump> {
        let hd = if path.starts_with("[synthetic]") {
            match path.strip_prefix("[synthetic]") {
                Some(name) => {
                    if name.starts_with("linked_list") {
                        LinkedListHeapDump::new(name).to_heapdump()
                    } else {
                        return Err(anyhow::anyhow!("Invalid synthetic heapdump name: {}", path));
                    }
                }
                None => {
                    return Err(anyhow::anyhow!("Invalid synthetic heapdump name: {}", path));
                }
            }
        } else {
            HeapDump::from_binpb_zst(path)?
        };
        Ok(hd)
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

// To test
// RUST_BACKTRACE=1 RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run --release -- [synthetic]linked_list_16777216 -o OpenJDK trace -t EdgeSlot
pub struct LinkedListHeapDump {
    num_nodes: usize,
}

impl LinkedListHeapDump {
    pub fn new(path: &str) -> Self {
        let num_nodes = path
            .strip_prefix("linked_list_")
            .expect("Specify the number of nodes after \"[synthetic]linked_list_\"")
            .parse::<usize>()
            .expect("Invalid number for the number of nodes in the linked list");
        LinkedListHeapDump { num_nodes }
    }

    pub fn to_heapdump(&self) -> HeapDump {
        let object_size = 4 * 8; // four words, header, klass, val, next
        let immix_space = generated_src::Space {
            name: "immix".to_string(),
            start: 0x20000000000,
            end: 0x20000000000 + (self.num_nodes * object_size) as u64,
        };
        let spaces = vec![immix_space];
        let root_edge = generated_src::RootEdge {
            objref: 0x20000000000,
        };
        let objects: Vec<HeapObject> = (0..self.num_nodes)
            .map(|i| {
                let start = 0x20000000000 + (i * object_size) as u64;
                let would_be_next_node = 0x20000000000 + ((i + 1) * object_size) as u64;
                let mut edges = vec![];
                if i < self.num_nodes - 1 {
                    edges.push(generated_src::NormalEdge {
                        slot: start + 16,
                        objref: would_be_next_node,
                    });
                }
                generated_src::HeapObject {
                    start,
                    // Doesn't need to be a valid pointer, since the Klass
                    // objects are inferred and constructed when the heapdump is mapped
                    klass: 42,
                    size: object_size as u64,
                    objarray_length: None,
                    instance_mirror_start: None,
                    instance_mirror_count: None,
                    edges,
                }
            })
            .collect();
        let roots = vec![root_edge];
        HeapDump {
            objects,
            roots,
            spaces,
        }
    }
}
