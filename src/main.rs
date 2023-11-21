#[macro_use]
extern crate log;

use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;

use hwgc_soft::*;

#[cfg(feature = "zsim")]
use zsim_hooks::*;

pub fn main() -> Result<()> {
    env_logger::init();
    let heapdump = HeapDump::from_binpb_zst("heapdump.20.binpb.zst")?;
    heapdump.map_spaces()?;
    let mut objects: HashMap<u64, HeapObject> = HashMap::new();
    for object in &heapdump.objects {
        objects.insert(object.start, object.clone());
    }
    let start = Instant::now();
    heapdump.restore_objects();
    let elapsed = start.elapsed();
    info!(
        "Finish deserializing the heapdump, {} objects in {} ms",
        heapdump.objects.len(),
        elapsed.as_micros() as f64 / 1000f64
    );
    info!(
        "Sanity trace reporting {} reachable objects",
        sanity_trace(&heapdump.roots, &objects)
    );
    let mut mark_sense: u8 = 0;
    #[cfg(feature = "m5")]
    unsafe {
        m5::m5_reset_stats(0, 0);
    }
    #[cfg(feature = "zsim")]
    zsim_roi_begin();
    unsafe {
        for i in 0..2 {
            mark_sense = (i % 2 == 0) as u8;
            transitive_closure(&heapdump.roots, &objects, mark_sense);
        }
    }
    #[cfg(feature = "m5")]
    unsafe {
        m5::m5_dump_reset_stats(0, 0);
    }
    #[cfg(feature = "zsim")]
    zsim_roi_end();
    for o in &heapdump.objects {
        let mark_word = o.start as *mut u8;
        if unsafe { *mark_word } != mark_sense {
            info!("{} not marked by transitive closure", o.start);
        }
    }
    Ok(())
}
