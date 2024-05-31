use super::MemdumpWorkload;

///
pub(super) struct LinkedList {
    num_nodes: usize,
}

impl LinkedList {
    pub(super) fn new(num_nodes: usize) -> Self {
        LinkedList { num_nodes }
    }
}

impl MemdumpWorkload for LinkedList {
    unsafe fn gen_memdump(&self, md: &mut super::Memdump) {
        info!("Synthesized memdump of a linked list");
        let word_size = 8_usize;
        // 1 root slot plus 1 word for the number of root slots
        let roots_size = word_size + 1;
        let segment_roots = md.alloc_segment(roots_size);
        let num_root_ptr = segment_roots.start as *mut usize;
        info!(
            "The number of roots is stored at {:?}",
            md.translate_to_target(num_root_ptr)
        );
        let root_slot_ptr = num_root_ptr.wrapping_add(1);
        info!(
            "Roots are stored from {:?}",
            md.translate_to_target(root_slot_ptr)
        );
        num_root_ptr.write(1);
        // a next pointer plus a valuel
        let node_size = word_size * 2;
        let num_nodes = 1024;
        let segment_nodes = md.alloc_segment(node_size * self.num_nodes);
        info!(
            "Generating {} linked list nodes stored from {:?}",
            num_nodes,
            md.translate_to_target(segment_nodes.start)
        );
        root_slot_ptr.write(md.translate_to_target(segment_nodes.start) as _);
        let mut cursor = segment_nodes.start as *mut usize;
        for i in 0..num_nodes {
            if i != num_nodes - 1 {
                cursor.write(md.translate_to_target(cursor.wrapping_add(2)) as _);
            }
            cursor.wrapping_add(1).write(i);
            cursor = cursor.wrapping_add(2);
        }
    }
}
