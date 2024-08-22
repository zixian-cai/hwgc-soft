use super::MemdumpWorkload;
use crate::*;

pub(super) struct HeapDumpWorkload {}

impl HeapDumpWorkload {
    pub(super) fn new() -> Self {
        HeapDumpWorkload {}
    }
}

impl MemdumpWorkload for HeapDumpWorkload {
    unsafe fn gen_memdump<O: ObjectModel>(
        &self,
        mut object_model: O,
        args: Args,
        md: &mut super::Memdump,
    ) {
        assert_eq!(
            args.paths.len(),
            1,
            "Can only convert one heapdump at a time into memdump"
        );
        assert_eq!(
            args.object_model,
            ObjectModelChoice::Bidirectional,
            "Only support bidirectional for now"
        );
        info!("Generating memdump of heapdump: {}", &args.paths[0]);
        let heapdump = HeapDump::from_binpb_zst(&args.paths[0]).unwrap();
        let space_limits = heapdump.calculate_space_limits();
        // Don't use 0x0. Otherwise, actual null pointers might be mistranslated.
        // Reserve memory for roots at host address 0x1000 plus one word for the number of words
        let roots_size = (heapdump.roots.len() + 1) * 8;
        let _roots_mapping = md.new_mapping(0x1000 as *mut u8, roots_size);
        // Reserve 8M of memory for TIBs at host address 0x1000_0000, no one is probably using that
        // low a memory, and 0x1000_0000 is high level to accommodate the number of roots
        let tib_mapping = md.new_mapping(0x1000_0000usize as *mut u8, 8 * 1024 * 1024);
        let mut tib_arena = tib_mapping.to_arena(16);
        // Now reserve memory for spaces
        for limit in &space_limits {
            if limit.space_start == u64::MAX {
                // The space is actually unused
                continue;
            }
            info!(
                "Found Space {:?}, starting at 0x{:x}, ending at 0x{:x}, size {}",
                limit.space,
                limit.space_start,
                limit.space_limit,
                limit.get_size()
            );
            md.new_mapping(limit.space_start as *mut u8, limit.get_size() as usize);
        }
        let memif = md.gen_memif();
        info!("Write out {} roots", heapdump.roots.len());
        memif.write_value_to_target(0x1000 as *mut u64, heapdump.roots.len() as u64);
        info!("Restoring TIBs onto the arena");
        object_model.restore_tibs(&heapdump, &memif, &mut tib_arena);
        info!("Restoring {} objects", heapdump.objects.len());
        object_model.restore_objects(&heapdump, &memif, &mut tib_arena);
        for (i, root) in object_model.roots().iter().enumerate() {
            memif.write_pointer_to_target(
                (0x1008usize + 8 * i) as *mut *const u64,
                *root as *const u64,
            );
        }
    }
}
