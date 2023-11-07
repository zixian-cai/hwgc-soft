#[macro_use]
extern crate lazy_static;

use anyhow::Result;
use prost::Message;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::{collections::HashMap, fs::File, io::Read};

include!(concat!(env!("OUT_DIR"), "/mmtk.util.sanity.rs"));

lazy_static! {
    static ref TIBS: Mutex<HashMap<u64, Arc<Tib>>> = Mutex::new(HashMap::new());
}

fn wrap_libc_call<T: PartialEq>(f: &dyn Fn() -> T, expect: T) -> Result<()> {
    let ret = f();
    if ret == expect {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn dzmmap_noreplace(start: u64, size: usize) -> Result<()> {
    let prot = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;
    let flags =
        libc::MAP_ANON | libc::MAP_PRIVATE | libc::MAP_FIXED_NOREPLACE | libc::MAP_NORESERVE;

    mmap_fixed(start, size, prot, flags)
}

fn mmap_fixed(start: u64, size: usize, prot: libc::c_int, flags: libc::c_int) -> Result<()> {
    let ptr = start as *mut libc::c_void;
    wrap_libc_call(
        &|| unsafe { libc::mmap(ptr, size, prot, flags, -1, 0) },
        ptr,
    )?;
    Ok(())
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
}

#[repr(C)]
#[derive(Debug)]
struct OopMapBlock {
    offset: u64,
    count: u64,
}

unsafe fn trace_object(o: u64, mark_sense: u8) -> bool {
    // mark sense is 1 intially, and flip every epoch
    // println!("Trace object: 0x{:x}", o as u64);
    if o == 0 {
        // skip null
        return false;
    }
    // Return false if already marked
    let mark_word = o as *mut u8;
    if *mark_word == mark_sense {
        false
    } else {
        *mark_word = mark_sense;
        true
    }
}

unsafe fn scan_object(o: u64, mark_queue: &mut VecDeque<u64>, objects: &HashMap<u64, HeapObject>) {
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
                mark_queue.push_back(*slot);
                num_edges += 1;
            }
        }
        TibType::InstanceMirror => {
            for omb in &tib.oop_map_blocks {
                for i in 0..omb.count {
                    let slot = (o as *mut u8).wrapping_add(omb.offset as usize + i as usize * 8)
                        as *mut u64;
                    mark_queue.push_back(*slot);
                    num_edges += 1;
                }
            }
            let (start, count) = &tib.instance_mirror_info.unwrap();
            for i in 0..*count {
                let slot = ((*start) as *mut u64).wrapping_add(i as usize);
                mark_queue.push_back(*slot);
                num_edges += 1;
            }
        }
        TibType::Ordinary => {
            for omb in &tib.oop_map_blocks {
                for i in 0..omb.count {
                    let slot = (o as *mut u8).wrapping_add(omb.offset as usize + i as usize * 8)
                        as *mut u64;
                    mark_queue.push_back(*slot);
                    num_edges += 1;
                }
            }
        }
    }
    // println!("{:?}", objects.get(&o).unwrap());
    debug_assert_eq!(num_edges, objects.get(&o).unwrap().edges.len())
}

unsafe fn transitive_closure(
    roots: &[RootEdge],
    objects: &HashMap<u64, HeapObject>,
    mark_sense: u8,
) {
    let start: Instant = Instant::now();
    // A queue of objref (possibly null)
    // aka node enqueuing
    let mut mark_queue: VecDeque<u64> = VecDeque::new();
    for root in roots {
        mark_queue.push_back(root.objref);
    }
    let mut marked_object: u64 = 0;
    while let Some(o) = mark_queue.pop_front() {
        if trace_object(o, mark_sense) {
            // not previously marked, now marked
            // now scan
            marked_object += 1;
            scan_object(o, &mut mark_queue, objects);
        }
    }
    let elapsed = start.elapsed();
    println!(
        "Finished marking {} objects in {} ms",
        marked_object,
        elapsed.as_micros() as f64 / 1000f64
    );
}

fn sanity_trace(roots: &[RootEdge], objects: &HashMap<u64, HeapObject>) -> usize {
    let mut reachable_objects: HashSet<u64> = HashSet::new();
    let mut mark_stack: Vec<u64> = vec![];
    for root in roots {
        debug_assert!(objects.contains_key(&root.objref));
        mark_stack.push(root.objref);
    }
    // println!("Sanity mark stack {} objects", mark_stack.len());
    while let Some(o) = mark_stack.pop() {
        // println!("Sanity mark stack {} objects", mark_stack.len());
        if reachable_objects.contains(&o) {
            continue;
        }
        reachable_objects.insert(o);
        let obj = objects.get(&o).unwrap();
        for edge in &obj.edges {
            if edge.objref != 0 {
                mark_stack.push(edge.objref);
                // println!("Sanity mark stack {} objects", mark_stack.len());
            }
        }
    }
    reachable_objects.len()
}

pub fn main() -> Result<()> {
    let file = File::open("heapdump.20.binpb.zst")?;
    let mut reader = zstd::Decoder::new(file)?;
    let mut buf = vec![];
    reader.read_to_end(&mut buf)?;
    let heapdump = HeapDump::decode(buf.as_slice())?;
    for s in &heapdump.spaces {
        println!("Mapping {} at 0x{:x}", s.name, s.start);
        dzmmap_noreplace(s.start, (s.end - s.start) as usize)?;
    }
    let mut objects: HashMap<u64, HeapObject> = HashMap::new();
    for object in &heapdump.objects {
        objects.insert(object.start, object.clone());
    }
    let start = Instant::now();
    // for o in &heapdump.objects {
    //     println!("{:?}", o);
    // }
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
    let elapsed = start.elapsed();
    println!(
        "Finish deserializing the heapdump, {} objects in {} ms",
        heapdump.objects.len(),
        elapsed.as_micros() as f64 / 1000f64
    );
    println!(
        "Sanity trace reporting {} reachable objects",
        sanity_trace(&heapdump.roots, &objects)
    );
    let mut mark_sense: u8 = 0;
    unsafe {
        for i in 0..10000 {
            mark_sense = (i % 2 != 0) as u8;
            transitive_closure(&heapdump.roots, &objects, mark_sense);
        }
    }
    for o in &heapdump.objects {
        let mark_word = o.start as *mut u8;
        if unsafe { *mark_word } != mark_sense {
            println!("{} not marked by transitive closure", o.start);
        }
    }
    Ok(())
}
