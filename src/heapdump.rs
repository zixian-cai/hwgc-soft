mod generated_src {
    include!(concat!(env!("OUT_DIR"), "/heapdump.generated_src.rs"));
}
use anyhow::Result;
use prost::Message;
use rand::seq::SliceRandom;
use rand::{rngs::SmallRng, SeedableRng};
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
                    } else if name.starts_with("objarray") {
                        LeafObjectArrayHeapDump::new(name).to_heapdump()
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
// RUST_BACKTRACE=1 RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run --release -- [synthetic]linked_list_2097152  -o OpenJDK simulate -a NMPGC -p 8
pub struct LinkedListHeapDump {
    num_nodes: usize,
    sequential: bool,
}

impl LinkedListHeapDump {
    pub fn new(path: &str) -> Self {
        let arguments = path
            .strip_prefix("linked_list_")
            .expect("The argument format is \"[synthetic]linked_list_<num nodes>_<sequential: true or false, default true>\"");
        let parts: Vec<&str> = arguments.split('_').collect();
        let num_nodes = parts[0]
            .parse::<usize>()
            .expect("Invalid number for the number of nodes in the linked list");
        let sequential = if parts.len() > 1 {
            parts[1]
                .parse::<bool>()
                .expect("Invalid value for sequential, must be true or false")
        } else {
            true
        };
        LinkedListHeapDump {
            num_nodes,
            sequential,
        }
    }

    fn sequential_objects(&self) -> Vec<HeapObject> {
        let object_size = 4 * 8; // four words, header, klass, val, next
        (0..self.num_nodes)
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
            .collect()
    }

    fn random_objects(&self) -> Vec<HeapObject> {
        let object_size = 4 * 8; // four words, header, klass, val, next
        let mut objects: Vec<HeapObject> = (0..self.num_nodes)
            .map(|i| {
                let start = 0x20000000000 + (i * object_size) as u64;
                generated_src::HeapObject {
                    start,
                    // Doesn't need to be a valid pointer, since the Klass
                    // objects are inferred and constructed when the heapdump is mapped
                    klass: 42,
                    size: object_size as u64,
                    objarray_length: None,
                    instance_mirror_start: None,
                    instance_mirror_count: None,
                    edges: vec![],
                }
            })
            .collect();
        let mut rng = SmallRng::seed_from_u64(42); // Fixed seed for reproducibility
        objects.shuffle(&mut rng);
        for i in 0..self.num_nodes - 1 {
            let next_node = objects[i + 1].start;
            let first_slot = objects[i].start + 16;
            objects[i].edges.push(generated_src::NormalEdge {
                slot: first_slot,
                objref: next_node,
            });
        }
        objects
    }

    pub fn to_heapdump(&self) -> HeapDump {
        let object_size = 4 * 8; // four words, header, klass, val, next
        let immix_space = generated_src::Space {
            name: "immix".to_string(),
            start: 0x20000000000,
            end: 0x20000000000 + (self.num_nodes * object_size) as u64,
        };
        let spaces = vec![immix_space];
        let objects = if self.sequential {
            self.sequential_objects()
        } else {
            self.random_objects()
        };
        let root_edge = generated_src::RootEdge {
            objref: objects[0].start,
        };
        let roots = vec![root_edge];
        HeapDump {
            objects,
            roots,
            spaces,
        }
    }
}

// RUST_BACKTRACE=1 RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run --release -- [synthetic]objarray_33554432 -o OpenJDK simulate -a NMPGC -p 8
// The utlization is actually quite bad, why?
pub struct LeafObjectArrayHeapDump {
    num_objs: usize,
}

impl LeafObjectArrayHeapDump {
    pub fn new(path: &str) -> Self {
        let num_objs = path
            .strip_prefix("objarray_")
            .expect("Specify the number of nodes after \"[synthetic]objarray_\"")
            .parse::<usize>()
            .expect("Invalid number for the number of objects in the leaf object array");
        LeafObjectArrayHeapDump { num_objs }
    }

    pub fn to_heapdump(&self) -> HeapDump {
        let object_size = 2 * 8; // two words, header, klass
        let array_size = 3 * 8 + self.num_objs * 8; // header, Klass, array length, and the references
        let objects_start = (0x20000000000 + array_size as u64).next_multiple_of(16); // Alignment
        let immix_space = generated_src::Space {
            name: "immix".to_string(),
            start: 0x20000000000,
            end: 0x20000000000 + (self.num_objs * object_size + array_size) as u64,
        };
        let spaces = vec![immix_space];
        let root_edge = generated_src::RootEdge {
            objref: 0x20000000000,
        };

        let roots = vec![root_edge];
        let arary_content: Vec<NormalEdge> = (0..self.num_objs)
            .map(|i| generated_src::NormalEdge {
                slot: (0x20000000000 + 3 * 8 + i * 8) as u64,
                objref: objects_start + (i * object_size) as u64,
            })
            .collect();
        let mut objects: Vec<HeapObject> = vec![generated_src::HeapObject {
            start: 0x20000000000,
            klass: 42, // Klass for the java.lang.Object[
            size: array_size as u64,
            objarray_length: Some(self.num_objs as u64),
            instance_mirror_start: None,
            instance_mirror_count: None,
            edges: arary_content,
        }];

        (0..self.num_objs).for_each(|i| {
            let start = objects_start + (i * object_size) as u64;
            objects.push(generated_src::HeapObject {
                start,
                klass: 43,
                size: object_size as u64,
                objarray_length: None,
                instance_mirror_start: None,
                instance_mirror_count: None,
                edges: vec![], // Leaf object with no outgoing pointers
            });
        });

        HeapDump {
            objects,
            roots,
            spaces,
        }
    }
}
