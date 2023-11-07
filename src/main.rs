use anyhow::Result;
use hwgc_soft::*;
use prost::Message;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::time::Instant;

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
