use crate::{HeapDump, HeapObject, ObjectModel};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

lazy_static! {
    static ref TIBS: Mutex<HashMap<u64, Arc<Tib>>> = Mutex::new(HashMap::new());
}

#[repr(C)]
#[derive(Debug)]
struct Tib {
    ttype: TibType,
    oop_map_blocks: Vec<OopMapBlock>,
    instance_mirror_info: Option<(u64, u64)>,
}

#[repr(u8)]
#[derive(Debug)]
enum TibType {
    Ordinary = 0,
    ObjArray = 1,
    InstanceMirror = 2,
}

impl Tib {
    fn insert_with_cache(klass: u64, tib: impl FnOnce() -> Tib) -> Arc<Tib> {
        let mut tibs = TIBS.lock().unwrap();
        tibs.entry(klass).or_insert_with(|| Arc::new(tib()));
        tibs.get(&klass).unwrap().clone()
    }

    fn objarray(klass: u64) -> Arc<Tib> {
        Self::insert_with_cache(klass, || Tib {
            ttype: TibType::ObjArray,
            oop_map_blocks: vec![],
            instance_mirror_info: None,
        })
    }

    fn encode_oop_map_blocks(obj: &HeapObject) -> Vec<OopMapBlock> {
        let mut oop_map_blocks: Vec<OopMapBlock> = vec![];
        for e in &obj.edges {
            if let Some(start) = obj.instance_mirror_start {
                let count = obj.instance_mirror_count.unwrap();
                if e.slot >= start && e.slot < start + count * 8 {
                    // This is a static field and shouldn't be encoded in an
                    // OopMapBlock
                    // println!("{:?}", oop_map_blocks);
                    continue;
                }
            }
            // This is a normal field
            if let Some(o) = oop_map_blocks.last_mut() {
                if e.slot == obj.start + o.offset + o.count * 8 {
                    o.count += 1;
                    // println!("{:?}", oop_map_blocks);
                    continue;
                }
            }
            oop_map_blocks.push(OopMapBlock {
                offset: e.slot - obj.start,
                count: 1,
            });
            // println!("{:?}", oop_map_blocks);
        }
        oop_map_blocks
    }

    fn non_objarray(klass: u64, obj: &HeapObject) -> Arc<Tib> {
        let ombs = Self::encode_oop_map_blocks(obj);
        // println!("{:?}", ombs);
        let sum: u64 = ombs.iter().map(|omb| omb.count).sum();

        // println!("ret: {:?} {:?}", ret,  Arc::as_ptr(&ret));
        if let Some(start) = obj.instance_mirror_start {
            let count = obj.instance_mirror_count.unwrap();
            debug_assert_eq!(sum + count, obj.edges.len() as u64);
            Arc::new(Tib {
                ttype: TibType::InstanceMirror,
                oop_map_blocks: ombs,
                instance_mirror_info: Some((start, count)),
            })
        } else {
            Self::insert_with_cache(klass, || Tib {
                ttype: TibType::Ordinary,
                oop_map_blocks: ombs,
                instance_mirror_info: None,
            })
        }
    }

    fn num_edges(&self) -> u64 {
        let mut sum = self.oop_map_blocks.iter().map(|omb| omb.count).sum();
        if let Some((_, count)) = self.instance_mirror_info {
            sum += count;
        }
        sum
    }

    unsafe fn scan_object<F>(o: u64, mut callback: F, objects: &HashMap<u64, HeapObject>)
    where
        F: FnMut(*mut u64),
    {
        let tib_ptr = *((o as *mut u64).wrapping_add(1) as *const *const Tib);
        if tib_ptr.is_null() {
            panic!("Object 0x{:x} has a null tib pointer", { o });
        }
        let tib: &Tib = &*tib_ptr;
        // println!("Object: {}, Tib Ptr: {:?}, Tib: {:?}", o, tib_ptr, tib);
        let mut num_edges = 0;
        match tib.ttype {
            TibType::ObjArray => {
                let objarray_length = *((o as *mut u64).wrapping_add(2) as *const u64);
                // println!("Objarray length: {}", objarray_length);
                for i in 0..objarray_length {
                    let slot = (o as *mut u64).wrapping_add(3 + i as usize);
                    callback(slot);
                    num_edges += 1;
                }
            }
            TibType::InstanceMirror => {
                for omb in &tib.oop_map_blocks {
                    for i in 0..omb.count {
                        let slot = (o as *mut u8).wrapping_add(omb.offset as usize + i as usize * 8)
                            as *mut u64;
                        callback(slot);
                        num_edges += 1;
                    }
                }
                let (start, count) = &tib.instance_mirror_info.unwrap();
                for i in 0..*count {
                    let slot = ((*start) as *mut u64).wrapping_add(i as usize);
                    callback(slot);
                    num_edges += 1;
                }
            }
            TibType::Ordinary => {
                for omb in &tib.oop_map_blocks {
                    for i in 0..omb.count {
                        let slot = (o as *mut u8).wrapping_add(omb.offset as usize + i as usize * 8)
                            as *mut u64;
                        callback(slot);
                        num_edges += 1;
                    }
                }
            }
        }
        // println!("{:?}", objects.get(&o).unwrap());
        debug_assert_eq!(num_edges, objects.get(&o).unwrap().edges.len())
    }
}

#[repr(C)]
#[derive(Debug)]
struct OopMapBlock {
    offset: u64,
    count: u64,
}

pub struct OpenJDKObjectModel {
    object_map: HashMap<u64, HeapObject>,
    objects: Vec<u64>,
    roots: Vec<u64>,
}

impl Default for OpenJDKObjectModel {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenJDKObjectModel {
    pub fn new() -> Self {
        OpenJDKObjectModel {
            // For debugging
            object_map: HashMap::new(),
            objects: vec![],
            roots: vec![],
        }
    }
}

impl ObjectModel for OpenJDKObjectModel {
    fn restore_objects(&mut self, heapdump: &HeapDump) {
        for object in &heapdump.objects {
            self.object_map.insert(object.start, object.clone());
            self.objects.push(object.start);
        }

        for root in &heapdump.roots {
            self.roots.push(root.objref);
        }

        for o in &heapdump.objects {
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

    fn scan_object<F>(&self, o: u64, callback: F)
    where
        F: FnMut(*mut u64),
    {
        unsafe {
            Tib::scan_object(o, callback, &self.object_map);
        }
    }

    fn roots(&self) -> &[u64] {
        &self.roots
    }

    fn objects(&self) -> &[u64] {
        &self.objects
    }
}
