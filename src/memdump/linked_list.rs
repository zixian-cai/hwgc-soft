use crate::{Args, MemoryInterface, ObjectModel};

use super::MemdumpWorkload;

pub(super) struct LinkedList {
    num_nodes: usize,
}

impl LinkedList {
    pub(super) fn new(num_nodes: usize) -> Self {
        LinkedList { num_nodes }
    }
}

impl MemdumpWorkload for LinkedList {
    unsafe fn gen_memdump<O: ObjectModel>(
        &self,
        _object_model: O,
        _args: Args,
        md: &mut super::Memdump,
    ) {
        info!("Synthetic memdump of a linked list");
        // The address space of the linked list looks like
        // 0x0: number of root pointers, which is one
        // 0x8: the signular pointer to the start of the linked list
        // 0x1000: stores the linked list nodes
        // ...
        // 0x1000 + num_nodes * 16
        // A total of 2 mappings needs to be allocated
        let num_nodes = self.num_nodes;
        md.new_mapping(0 as *mut u8, 16);
        md.new_mapping(0x1000 as *mut u8, 16 * self.num_nodes);
        let memif = md.gen_memif();
        memif.write_value_to_target(0 as *mut u64, 1);
        memif.write_pointer_to_target(0x8 as *mut *const u64, 0x1000 as *const u64);

        for i in 0..num_nodes {
            // a next pointer and then a value
            if i != num_nodes - 1 {
                memif.write_pointer_to_target(
                    (0x1000 + 16 * i) as *mut *const u64,
                    (0x1000 + 16 * (i + 1)) as *const u64,
                );
            }
            memif.write_value_to_target((0x1000 + 16 * i + 8) as *mut u64, i as u64);
        }
    }
}
