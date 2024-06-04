use std::alloc::{self, Layout};
use std::collections::HashMap;
use std::ptr;
use std::sync::Mutex;

use crate::{BumpAllocationArena, HeapDump, HeapObject, MemoryInterface, ObjectModel};

use super::{HasTibType, Header, TibType};

pub struct BidirectionalObjectModel<const HEADER: bool> {
    forwarding: HashMap<u64, u64>,
    objects: Vec<u64>,
    roots: Vec<u64>,
    object_sizes: HashMap<u64, u64>,
}

impl<const HEADER: bool> BidirectionalObjectModel<HEADER> {
    pub fn new() -> Self {
        BidirectionalObjectModel {
            forwarding: HashMap::new(),
            objects: vec![],
            roots: vec![],
            object_sizes: HashMap::new(),
        }
    }
}

impl<const HEADER: bool> Default for BidirectionalObjectModel<HEADER> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct TibPtr {
    backing_storage: u64,
    host_ptr: u64,
}

impl TibPtr {
    fn get_ref(&self) -> &'static Tib {
        unsafe { &*(self.backing_storage as *const Tib) as &'static Tib }
    }
}

fn alloc_tib(tib: impl FnOnce() -> Tib, tib_arena: &mut BumpAllocationArena) -> TibPtr {
    unsafe {
        let (backing_storage, host_ptr) = tib_arena.alloc(Layout::new::<Tib>().size());
        ptr::write(backing_storage as *mut Tib, tib());
        TibPtr {
            backing_storage: backing_storage as u64,
            host_ptr: host_ptr as u64,
        }
    }
}

lazy_static! {
    static ref TIBS: Mutex<HashMap<u64, TibPtr>> = Mutex::new(HashMap::new());
}

#[repr(C)]
#[derive(Debug)]
pub struct Tib {
    num_refs: u64,
    ttype: TibType
}

impl HasTibType for Tib {
    fn get_tib_type(&self) -> TibType {
        self.ttype
    }
}

#[repr(u8)]
#[derive(Debug)]
enum StatusByte {
    Fallback = u8::MAX,
    NoRef = 0,
    Ordinary = 1,
    ObjArray = 2,
}

impl Tib {
    const STATUS_BYTE_OFFSET: u8 = 1;
    const NUMREFS_BYTE_OFFSET: u8 = 2;

    fn insert_with_cache(
        klass: u64,
        tib: impl FnOnce() -> Tib,
        tib_arena: &mut BumpAllocationArena,
    ) -> TibPtr {
        let mut tibs = TIBS.lock().unwrap();
        tibs.entry(klass)
            .or_insert_with(|| alloc_tib(tib, tib_arena));
        *tibs.get(&klass).unwrap()
    }

    fn objarray(klass: u64, tib_arena: &mut BumpAllocationArena) -> TibPtr {
        Self::insert_with_cache(
            klass,
            || Tib {
                ttype: TibType::ObjArray,
                num_refs: 0,
            },
            tib_arena,
        )
    }

    fn non_objarray(klass: u64, obj: &HeapObject, tib_arena: &mut BumpAllocationArena) -> TibPtr {
        if obj.instance_mirror_start.is_some() {
            alloc_tib(
                || Tib {
                    ttype: TibType::Ordinary,
                    num_refs: obj.edges.len() as u64,
                },
                tib_arena,
            )
        } else {
            Self::insert_with_cache(
                klass,
                || Tib {
                    ttype: TibType::Ordinary,
                    num_refs: obj.edges.len() as u64,
                },
                tib_arena,
            )
        }
    }

    unsafe fn scan_object_fallback<F>(o: u64, mut callback: F)
    where
        F: FnMut(*mut u64, u64),
    {
        let tib_ptr = BidirectionalObjectModel::<false>::get_tib(o);
        if tib_ptr.is_null() {
            panic!("Object 0x{:x} has a null tib pointer", { o });
        }
        let tib: &Tib = &*tib_ptr;
        match tib.ttype {
            TibType::ObjArray => {
                let objarray_length = *((o as *mut u64).wrapping_add(2) as *const u64);
                callback((o as *mut u64).wrapping_add(3), objarray_length);
            }
            TibType::Ordinary => {
                callback((o as *mut u64).wrapping_add(2), tib.num_refs);
            }
            TibType::InstanceMirror => {
                unreachable!("Instance mirror shouldn't be necessary for bidirectional")
            }
        }
    }

    unsafe fn scan_object_header<F>(o: u64, mut callback: F)
    where
        F: FnMut(*mut u64, u64),
    {
        let header = Header::load(o);
        let status_byte = header.get_byte(Self::STATUS_BYTE_OFFSET);
        match status_byte {
            0 => {
                // no ref
            }
            1 => {
                let num_refs = header.get_byte(Self::NUMREFS_BYTE_OFFSET);
                callback((o as *mut u64).wrapping_add(2), num_refs as u64);
            }
            2 => {
                let objarray_length = *((o as *mut u64).wrapping_add(2) as *const u64);
                callback((o as *mut u64).wrapping_add(3), objarray_length);
            }
            u8::MAX => Self::scan_object_fallback(o, callback),
            _ => {
                unreachable!()
            }
        }
    }

    unsafe fn scan_object<const HEADER: bool, F>(o: u64, callback: F)
    where
        F: FnMut(*mut u64, u64),
    {
        if HEADER {
            Self::scan_object_header(o, callback);
        } else {
            Self::scan_object_fallback(o, callback);
        }
    }

    fn encode_header(&self) -> Header {
        let mut header = Header::new();
        match self.ttype {
            TibType::Ordinary => {
                if self.num_refs > u8::MAX as u64 {
                    header.set_byte(StatusByte::Fallback as u8, Self::STATUS_BYTE_OFFSET);
                } else if self.num_refs == 0 {
                    header.set_byte(StatusByte::NoRef as u8, Self::STATUS_BYTE_OFFSET);
                } else {
                    header.set_byte(StatusByte::Ordinary as u8, Self::STATUS_BYTE_OFFSET);
                    header.set_byte(self.num_refs as u8, Self::NUMREFS_BYTE_OFFSET);
                }
            }
            TibType::ObjArray => {
                header.set_byte(StatusByte::ObjArray as u8, Self::STATUS_BYTE_OFFSET);
            }
            TibType::InstanceMirror => {
                unreachable!("Instance mirror shouldn't be necessary for bidirectional")
            }
        }
        header
    }
}

impl<const HEADER: bool> ObjectModel for BidirectionalObjectModel<HEADER> {
    type Tib = Tib;

    fn reset(&mut self) {
        self.objects.clear();
        self.forwarding.clear();
        self.roots.clear();
        self.object_sizes.clear();
    }

    fn restore_tibs<M>(
        &mut self,
        heapdump: &HeapDump,
        memif: &M,
        tib_arena: &mut BumpAllocationArena,
    ) -> usize
    where
        M: MemoryInterface,
    {
        let before_size = TIBS.lock().unwrap().len();
        for object in &heapdump.objects {
            let is_objarray = object.objarray_length.is_some();
            if is_objarray {
                let _tib = Tib::objarray(object.klass, tib_arena);
            } else if object.instance_mirror_start.is_none() {
                let _tib = Tib::non_objarray(object.klass, object, tib_arena);
            };
        }
        let after_size = TIBS.lock().unwrap().len();
        after_size - before_size
    }

    fn restore_objects<M>(
        &mut self,
        heapdump: &HeapDump,
        memif: &M,
        tib_arena: &mut BumpAllocationArena,
    ) where
        M: MemoryInterface,
    {
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
            let tib_ptr = if is_objarray {
                Tib::objarray(object.klass, tib_arena)
            } else {
                Tib::non_objarray(object.klass, object, tib_arena)
            };
            let tib = tib_ptr.get_ref();
            if !is_objarray {
                debug_assert_eq!(tib.num_refs, object.edges.len() as u64);
            }
            let header = tib.encode_header();
            let new_start = *self.forwarding.get(&object.start).unwrap();
            unsafe {
                if HEADER {
                    header.store_via_memif(new_start, memif);
                }
                memif.write_pointer_to_target(
                    (new_start + 8) as *mut *const Tib,
                    tib_ptr.host_ptr as *const Tib,
                );
            }
            // Write out array length for obj array
            if let Some(l) = object.objarray_length {
                unsafe {
                    memif.write_value_to_target((new_start + 16) as *mut u64, l);
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
                    memif.write_pointer_to_target(
                        ref_cursor as *mut *const u64,
                        new_referent as *const u64,
                    );
                    ref_cursor += 8;
                }
            }
            debug_assert_eq!(ref_cursor, object.start + object.size);
            self.object_sizes.insert(new_start, object.size);
            // println!("Obj {:?}, refs {}", unsafe {memif.translate_host_to_target(new_start as *const u64)}, object.edges.len());
        }
    }

    fn scan_object<F>(o: u64, callback: F)
    where
        F: FnMut(*mut u64, u64),
    {
        unsafe { Tib::scan_object::<HEADER, _>(o, callback) }
    }

    fn roots(&self) -> &[u64] {
        &self.roots
    }

    fn objects(&self) -> &[u64] {
        &self.objects
    }

    fn object_sizes(&self) -> &HashMap<u64, u64> {
        &self.object_sizes
    }

    unsafe fn is_objarray(o: u64) -> bool {
        let tib_ptr = Self::get_tib(o);
        if tib_ptr.is_null() {
            panic!("Object 0x{:x} has a null tib pointer", { o });
        }
        let tib: &Tib = &*tib_ptr;
        matches!(tib.ttype, TibType::ObjArray)
    }

    fn get_tib(o: u64) -> *const Self::Tib {
        unsafe { *((o as *mut u64).wrapping_add(1) as *const *const Tib) }
    }

    fn tib_lookup_required(o: u64) -> bool {
        if HEADER {
            let header = Header::load(o);
            let status_byte = header.get_byte(Tib::STATUS_BYTE_OFFSET);
            // Too many refs, so the number of refs cannot be encoded in the
            // header
            status_byte == u8::MAX
        } else {
            // If the number of refs is not encoded in the header
            // A tib lookup is always required
            true
        }
    }
}
