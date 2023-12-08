use crate::constants::*;
use crate::{HeapDump, HeapObject, ObjectModel};
use fixedbitset::FixedBitSet;
use std::alloc::{self, Layout};
use std::collections::HashMap;
use std::mem::size_of;
use std::ptr;
use std::sync::Mutex;

lazy_static! {
    static ref TIBS: Mutex<HashMap<u64, &'static Tib>> = Mutex::new(HashMap::new());
}

#[repr(C)]
#[derive(Debug)]
struct Tib {
    ttype: TibType,
    oop_map_blocks: Vec<OopMapBlock>,
    instance_mirror_info: Option<(u64, u64)>,
}

#[repr(u8)]
#[derive(Copy, Debug, Clone)]
enum AlignmentEncodingPattern {
    Fallback = 7,
    RefArray = 6,
    NoRef = 0,
    Ref0 = 1,
    Ref1_2_3 = 2,
    Ref4_5_6 = 3,
    Ref2 = 4,
    Ref0_1 = 5,
}

impl From<AlignmentEncodingPattern> for u8 {
    fn from(value: AlignmentEncodingPattern) -> Self {
        value as u8
    }
}

impl From<u8> for AlignmentEncodingPattern {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::NoRef,
            1 => Self::Ref0,
            2 => Self::Ref1_2_3,
            3 => Self::Ref4_5_6,
            4 => Self::Ref2,
            5 => Self::Ref0_1,
            6 => Self::RefArray,
            7 => Self::Fallback,
            _ => unreachable!(),
        }
    }
}

#[repr(u8)]
#[derive(Debug)]
enum TibType {
    Ordinary = 0,
    ObjArray = 1,
    InstanceMirror = 2,
}

struct AlignmentEncoding {}

impl AlignmentEncoding {
    const FIELD_WIDTH: u32 = 3;
    const MAX_ALIGN_WORDS: u32 = 1 << Self::FIELD_WIDTH;
    const FIELD_SHIFT: u32 = LOG_BYTES_IN_WORD as u32;
    const ALIGNMENT_INCREMENT: u32 = 1 << Self::FIELD_SHIFT;
    const KLASS_MASK: u32 = (Self::MAX_ALIGN_WORDS - 1) << Self::FIELD_SHIFT;
    const VERBOSE: bool = false;

    fn get_tib_code_for_region(tib: usize) -> AlignmentEncodingPattern {
        // println!("binding klass 0x{:x}", klass);
        let align_code = ((tib & Self::KLASS_MASK as usize) >> Self::FIELD_SHIFT) as u32;
        debug_assert!(align_code < Self::MAX_ALIGN_WORDS, "Invalid align code");
        let ret: AlignmentEncodingPattern = (align_code as u8).into();
        let inverse: u8 = ret.into();
        debug_assert_eq!(inverse, align_code as u8);
        ret
    }

    fn _get_padded_size(size: usize, align_code: Option<u8>) -> usize {
        let padding: usize = if align_code.is_some() {
            (Self::MAX_ALIGN_WORDS << Self::FIELD_SHIFT) as usize
        } else {
            0
        };
        size + padding
    }

    fn get_padded_word_size(word_size: usize, align_code: Option<u8>) -> usize {
        let padding: usize = if align_code.is_some() {
            (Self::MAX_ALIGN_WORDS) as usize
        } else {
            0
        };
        word_size + padding
    }
}

fn alloc_tib(tib: impl FnOnce() -> Tib, align_code: Option<u8>) -> &'static Tib {
    unsafe {
        let word_size = (size_of::<Tib>() + (BYTES_IN_WORD - 1)) & (!(BYTES_IN_WORD - 1));
        let padded_word_size = AlignmentEncoding::get_padded_word_size(word_size, align_code);
        let layout =
            Layout::from_size_align(padded_word_size * BYTES_IN_WORD, BYTES_IN_WORD).unwrap();
        let storage = alloc::alloc(layout) as *mut Tib;
        let mut region = storage as usize;
        let limit = region + padded_word_size * BYTES_IN_WORD;
        if let Some(a) = align_code {
            while AlignmentEncoding::get_tib_code_for_region(region) as u8 != a {
                region += AlignmentEncoding::ALIGNMENT_INCREMENT as usize;
                debug_assert!(region <= limit);
            }
        }
        if AlignmentEncoding::VERBOSE {
            eprintln!(
                "Tib: region = 0x{:x}, tib code = {}, requested = {:?}",
                region,
                AlignmentEncoding::get_tib_code_for_region(region) as u8,
                align_code
            );
        }
        debug_assert!(layout.size() >= size_of::<Tib>());
        let storage = region as *mut Tib;
        ptr::write(storage, tib());
        storage.as_ref().unwrap()
    }
}

impl Tib {
    fn insert_with_cache(
        klass: u64,
        tib: impl FnOnce() -> Tib,
        encoded_value: Option<u8>,
    ) -> &'static Tib {
        let mut tibs = TIBS.lock().unwrap();
        tibs.entry(klass)
            .or_insert_with(|| alloc_tib(tib, encoded_value));
        tibs.get(&klass).unwrap()
    }

    fn objarray<const AE: bool>(klass: u64) -> &'static Tib {
        Self::insert_with_cache(
            klass,
            || Tib {
                ttype: TibType::ObjArray,
                oop_map_blocks: vec![],
                instance_mirror_info: None,
            },
            if AE {
                Some(AlignmentEncodingPattern::RefArray as u8)
            } else {
                None
            },
        )
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

    fn alignment_encode_omb(ombs: &[OopMapBlock]) -> AlignmentEncodingPattern {
        let mut fields = FixedBitSet::with_capacity(7);
        for omb in ombs {
            let first_field = (omb.offset >> LOG_BYTES_IN_WORD) - 2;
            let last_field = first_field + omb.count - 1;
            if first_field > 6 || last_field > 6 {
                return AlignmentEncodingPattern::Fallback;
            }
            fields.set_range(
                (first_field as usize)..((first_field + omb.count) as usize),
                true,
            );
        }
        let bits = fields.as_slice()[0];
        match bits {
            0b0000000 => AlignmentEncodingPattern::NoRef,
            0b0000001 => AlignmentEncodingPattern::Ref0,
            0b0000011 => AlignmentEncodingPattern::Ref0_1,
            0b0000100 => AlignmentEncodingPattern::Ref2,
            0b0001110 => AlignmentEncodingPattern::Ref1_2_3,
            0b1110000 => AlignmentEncodingPattern::Ref4_5_6,
            _ => AlignmentEncodingPattern::Fallback,
        }
    }

    fn non_objarray<const AE: bool>(klass: u64, obj: &HeapObject) -> &'static Tib {
        let ombs = Self::encode_oop_map_blocks(obj);
        // println!("{:?}", ombs);
        let sum: u64 = ombs.iter().map(|omb| omb.count).sum();

        // println!("ret: {:?} {:?}", ret,  Arc::as_ptr(&ret));
        if let Some(start) = obj.instance_mirror_start {
            let count = obj.instance_mirror_count.unwrap();
            debug_assert_eq!(sum + count, obj.edges.len() as u64);
            let align_code = if AE {
                Some(Self::alignment_encode_omb(&ombs) as u8)
            } else {
                None
            };
            alloc_tib(
                || Tib {
                    ttype: TibType::InstanceMirror,
                    oop_map_blocks: ombs,
                    instance_mirror_info: Some((start, count)),
                },
                align_code,
            )
        } else {
            let align_code = if AE {
                Some(Self::alignment_encode_omb(&ombs) as u8)
            } else {
                None
            };
            Self::insert_with_cache(
                klass,
                || Tib {
                    ttype: TibType::Ordinary,
                    oop_map_blocks: ombs,
                    instance_mirror_info: None,
                },
                align_code,
            )
        }
    }

    fn num_edges(&self) -> u64 {
        let mut sum = self.oop_map_blocks.iter().map(|omb| omb.count).sum();
        if let Some((_, count)) = self.instance_mirror_info {
            sum += count;
        }
        sum
    }

    unsafe fn scan_object_fallback<F>(
        tib: &Tib,
        o: u64,
        mut callback: F,
        objects: &HashMap<u64, HeapObject>,
    ) where
        F: FnMut(*mut u64),
    {
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
        debug_assert_eq!(num_edges, objects.get(&o).unwrap().edges.len());
    }

    unsafe fn scan_object<const AE: bool, F>(
        o: u64,
        mut callback: F,
        objects: &HashMap<u64, HeapObject>,
    ) where
        F: FnMut(*mut u64),
    {
        let tib_ptr = *((o as *mut u64).wrapping_add(1) as *const *const Tib);
        if tib_ptr.is_null() {
            panic!("Object 0x{:x} has a null tib pointer", { o });
        }
        if !AE {
            let tib: &Tib = &*tib_ptr;
            Self::scan_object_fallback(tib, o, callback, objects);
            return;
        }
        let pattern = AlignmentEncoding::get_tib_code_for_region(tib_ptr as usize);
        match pattern {
            AlignmentEncodingPattern::Fallback => {
                let tib: &Tib = &*tib_ptr;
                Self::scan_object_fallback(tib, o, callback, objects);
            }
            AlignmentEncodingPattern::RefArray => {
                let objarray_length = *((o as *mut u64).wrapping_add(2) as *const u64);
                for i in 0..objarray_length {
                    let slot = (o as *mut u64).wrapping_add(3 + i as usize);
                    callback(slot);
                }
            }
            AlignmentEncodingPattern::NoRef => {}
            AlignmentEncodingPattern::Ref0 => {
                callback((o as *mut u64).wrapping_add(2));
            }
            AlignmentEncodingPattern::Ref1_2_3 => {
                callback((o as *mut u64).wrapping_add(3));
                callback((o as *mut u64).wrapping_add(4));
                callback((o as *mut u64).wrapping_add(5));
            }
            AlignmentEncodingPattern::Ref4_5_6 => {
                callback((o as *mut u64).wrapping_add(6));
                callback((o as *mut u64).wrapping_add(7));
                callback((o as *mut u64).wrapping_add(8));
            }
            AlignmentEncodingPattern::Ref2 => {
                callback((o as *mut u64).wrapping_add(4));
            }
            AlignmentEncodingPattern::Ref0_1 => {
                callback((o as *mut u64).wrapping_add(2));
                callback((o as *mut u64).wrapping_add(3));
            }
        }
    }
}

#[repr(C)]
#[derive(Debug)]
struct OopMapBlock {
    offset: u64,
    count: u64,
}

pub struct OpenJDKObjectModel<const AE: bool> {
    object_map: HashMap<u64, HeapObject>,
    objects: Vec<u64>,
    roots: Vec<u64>,
}

impl<const AE: bool> Default for OpenJDKObjectModel<AE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const AE: bool> OpenJDKObjectModel<AE> {
    pub fn new() -> Self {
        OpenJDKObjectModel {
            // For debugging
            object_map: HashMap::new(),
            objects: vec![],
            roots: vec![],
        }
    }
}

impl<const AE: bool> ObjectModel for OpenJDKObjectModel<AE> {
    fn reset(&mut self) {
        self.object_map.clear();
        self.roots.clear();
        self.objects.clear();
    }

    fn restore_tibs(&mut self, heapdump: &HeapDump) -> usize {
        let before_size = TIBS.lock().unwrap().len();
        for object in &heapdump.objects {
            let is_objarray = object.objarray_length.is_some();
            if is_objarray {
                let _tib = Tib::objarray::<AE>(object.klass);
            } else if object.instance_mirror_start.is_none() {
                let _tib = Tib::non_objarray::<AE>(object.klass, object);
            };
        }
        let after_size = TIBS.lock().unwrap().len();
        after_size - before_size
    }

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
                Tib::objarray::<AE>(o.klass)
            } else {
                Tib::non_objarray::<AE>(o.klass, o)
            };
            if o.objarray_length.is_none() {
                debug_assert_eq!(tib.num_edges(), o.edges.len() as u64);
            }
            let tib_ptr = tib as *const Tib;
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
            Tib::scan_object::<AE, _>(o, callback, &self.object_map);
        }
    }

    fn roots(&self) -> &[u64] {
        &self.roots
    }

    fn objects(&self) -> &[u64] {
        &self.objects
    }
}
