use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::{HeapDump, HeapObject, ObjectModel};

pub struct BidirectionalObjectModel {
    forwarding: HashMap<u64, u64>,
    objects: Vec<u64>,
    roots: Vec<u64>,
}

impl BidirectionalObjectModel {
    pub fn new() -> Self {
        BidirectionalObjectModel {
            forwarding: HashMap::new(),
            objects: vec![],
            roots: vec![],
        }
    }
}

impl Default for BidirectionalObjectModel {
    fn default() -> Self {
        Self::new()
    }
}

lazy_static! {
    static ref TIBS: Mutex<HashMap<u64, Arc<Tib>>> = Mutex::new(HashMap::new());
}

#[repr(C)]
#[derive(Debug)]
struct Tib {
    ttype: TibType,
    num_refs: u64,
}

#[repr(u8)]
#[derive(Debug)]
enum TibType {
    Ordinary = 0,
    ObjArray = 1,
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
            num_refs: 0,
        })
    }

    fn non_objarray(klass: u64, obj: &HeapObject) -> Arc<Tib> {
        if obj.instance_mirror_start.is_some() {
            Arc::new(Tib {
                ttype: TibType::Ordinary,
                num_refs: obj.edges.len() as u64,
            })
        } else {
            Self::insert_with_cache(klass, || Tib {
                ttype: TibType::Ordinary,
                num_refs: obj.edges.len() as u64,
            })
        }
    }

    unsafe fn scan_object(o: u64, mark_queue: &mut VecDeque<u64>) {
        let tib_ptr = *((o as *mut u64).wrapping_add(1) as *const *const Tib);
        if tib_ptr.is_null() {
            panic!("Object 0x{:x} has a null tib pointer", { o });
        }
        let tib: &Tib = &*tib_ptr;
        match tib.ttype {
            TibType::ObjArray => {
                let objarray_length = *((o as *mut u64).wrapping_add(2) as *const u64);
                for i in 0..objarray_length {
                    let slot = (o as *mut u64).wrapping_add(3 + i as usize);
                    mark_queue.push_back(*slot);
                }
            }
            TibType::Ordinary => {
                for i in 0..tib.num_refs {
                    let slot = (o as *mut u64).wrapping_add(2 + i as usize);
                    mark_queue.push_back(*slot);
                }
            }
        }
    }
}

impl ObjectModel for BidirectionalObjectModel {
    fn restore_objects(&mut self, heapdump: &HeapDump) {
        // First pass: calculate forwarding table
        for object in &heapdump.objects {
            let start = object.start;
            let end = start + object.size;
            let is_objarray = object.objarray_length.is_some();

            let new_start = if is_objarray {
                // keep the layout of obj arrays
                start
            } else {
                // for objects that are not object arrays
                // we group all references together
                // from the new start
                // it will be header (including a mark byte at the start of the object)
                // then tib
                // followed by all references, including the references
                // of mirror klass
                end - (object.edges.len() * 8 + 16) as u64
            };
            debug_assert!(new_start >= start);
            self.forwarding.insert(start, new_start);
            // println!("Forwarding 0x{:x} -> 0x{:x}", start, new_start);
        }
        for o in self.forwarding.values() {
            self.objects.push(*o);
        }

        for r in &heapdump.roots {
            self.roots.push(*self.forwarding.get(&r.objref).unwrap());
        }

        // Second pass: deserilize object and update edges
        for object in &heapdump.objects {
            let is_objarray = object.objarray_length.is_some();
            let tib = if is_objarray {
                Tib::objarray(object.klass)
            } else {
                Tib::non_objarray(object.klass, object)
            };
            if !is_objarray {
                debug_assert_eq!(tib.num_refs, object.edges.len() as u64);
            }
            // We need to leak this, so the underlying memory won't be collected
            let tib_ptr = Arc::into_raw(tib);
            let new_start = *self.forwarding.get(&object.start).unwrap();
            unsafe {
                std::ptr::write::<u64>((new_start + 8) as *mut u64, tib_ptr as u64);
            }
            // Write out array length for obj array
            if let Some(l) = object.objarray_length {
                unsafe {
                    std::ptr::write::<u64>((new_start + 16) as *mut u64, l);
                }
            }
            // Write out each non-zero ref field
            let mut ref_cursor: u64 = if is_objarray {
                new_start + 24
            } else {
                new_start + 16
            };
            for e in &object.edges {
                unsafe {
                    let new_referent = if e.objref == 0 {
                        0
                    } else {
                        *self.forwarding.get(&e.objref).unwrap()
                    };
                    std::ptr::write::<u64>(ref_cursor as *mut u64, new_referent);
                    ref_cursor += 8;
                }
            }
            debug_assert_eq!(ref_cursor, object.start + object.size);
        }
    }

    fn scan_object(&mut self, o: u64, mark_queue: &mut VecDeque<u64>) {
        unsafe { Tib::scan_object(o, mark_queue) }
    }

    fn roots(&self) -> &[u64] {
        &self.roots
    }

    fn objects(&self) -> &[u64] {
        &self.objects
    }
}
