use crate::{object_model::Header, Args, MemoryInterface, ObjectModel};

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
        // 0x1000: number of root pointers, which is one
        // 0x1008: the signular pointer to the start of the linked list
        // 0x2000: stores the linked list nodes
        // ...
        // 0x2000 + num_nodes * 16
        // A total of 2 mappings needs to be allocated
        let num_nodes = self.num_nodes;
        md.new_mapping(0x1000 as *mut u8, 16);
        md.new_mapping(0x2000 as *mut u8, 16 * self.num_nodes);
        let memif = md.gen_memif();
        memif.write_value_to_target(0x1000 as *mut u64, 1);
        memif.write_pointer_to_target(0x1008 as *mut *const u64, 0x2000 as *const u64);

        for i in 0..num_nodes {
            // a next pointer and then a value
            if i != num_nodes - 1 {
                memif.write_pointer_to_target(
                    (0x2000 + 16 * i) as *mut *const u64,
                    (0x2000 + 16 * (i + 1)) as *const u64,
                );
            }
            memif.write_value_to_target((0x2000 + 16 * i + 8) as *mut u64, i as u64);
        }
    }
}

pub(super) struct HeapLinkedList {
    num_nodes: usize,
    log_num_thread: u8,
    owner_shift: u8,
    owner: Option<u8>,
}

impl HeapLinkedList {
    pub(super) fn new(
        num_nodes: usize,
        log_num_thread: u8,
        owner_shift: u8,
        owner: Option<u8>,
    ) -> Self {
        HeapLinkedList {
            num_nodes,
            log_num_thread,
            owner_shift,
            owner,
        }
    }
}

impl MemdumpWorkload for HeapLinkedList {
    unsafe fn gen_memdump<O: ObjectModel>(
        &self,
        _object_model: O,
        _args: Args,
        md: &mut super::Memdump,
    ) {
        info!("Synthetic memdump of a linked list using bidirectional object model");
        // The address space of the linked list looks like
        // 0x1000: number of root pointers, which is one
        // 0x1008: the signular pointer to the start of the linked list
        // 0x2000: stores the linked list nodes
        // ...
        // 0x2000 + num_nodes * 16
        // A total of 2 mappings needs to be allocated
        let num_nodes = self.num_nodes;
        // value, header, tib, next per bidirectional model
        let obj_size = 8 * 4;
        let mut space_required = num_nodes * obj_size;
        // Let's make sure that each object is at least completely visible to a thread
        // Note that one 64B cache line can fit two objects
        assert!(self.owner_shift >= 5);
        assert!(self.owner_shift < 12); // Can't be more than a page
        if self.owner.is_some() {
            // We want all nodes be owned by the same thread
            // so we need to over allocate
            space_required *= 1 << self.log_num_thread;
        }
        md.new_mapping(0x1000 as *mut u8, 16);
        md.new_mapping(0x2000 as *mut u8, space_required);
        let memif = md.gen_memif();
        memif.write_value_to_target(0x1000 as *mut u64, 1);

        let (node_groups, group_size) = if self.owner.is_some() {
            let visibility_size = 1 << self.owner_shift;
            let group_size = visibility_size / obj_size;
            assert!(group_size >= 1);
            // ceiling divide
            let node_groups = (num_nodes + group_size - 1) / group_size;
            (node_groups, group_size)
        } else {
            // There's no ownership constraint.
            // Generate nodes in one big group
            (1, self.num_nodes)
        };
        let num_threads = 1 << self.log_num_thread;

        let get_obj_address = |idx: usize| {
            let group_idx = idx / group_size;
            let pos_in_group = idx % group_size;
            let group_start = if let Some(o) = self.owner {
                0x2000
                    + group_idx * group_size * num_threads * obj_size
                    + o as usize * group_size * obj_size
            } else {
                0x2000
            } as *mut u8;
            group_start.wrapping_add(pos_in_group * obj_size)
        };
        // the 8 offset is because for bidirectional
        // object ref is obj start + 8 * num of primitive fields
        memif.write_pointer_to_target(0x1008 as *mut *const u64, get_obj_address(0).wrapping_add(8) as *const u64);

        for i in 0..node_groups {
            for j in 0..group_size {
                let obj_idx = i * group_size + j;
                if obj_idx >= self.num_nodes {
                    // This should break out the outer loop as well
                    assert_eq!(i, node_groups - 1);
                    break;
                }
                let is_last = obj_idx == (self.num_nodes - 1);
                let obj_start = get_obj_address(obj_idx);
                let next_obj_start = get_obj_address(obj_idx + 1);
                // value, header, tib, next per bidirectional model
                memif.write_value_to_target(obj_start as *mut u64, obj_idx as u64);
                // header
                let mut header = Header::new();
                const STATUS_BYTE_OFFSET: u8 = 1;
                const NUMREFS_BYTE_OFFSET: u8 = 2;
                header.set_byte(1, STATUS_BYTE_OFFSET);
                header.set_byte(1, NUMREFS_BYTE_OFFSET);
                memif.write_value_to_target((obj_start.wrapping_add(8)) as *mut u64, header.into());
                // Header encoding is sufficient for object scanning, give TIB a null pointer
                memif.write_pointer_to_target(
                    (obj_start.wrapping_add(16)) as *mut *const u64,
                    std::ptr::null(),
                );
                // Write out the next node if we are not the last
                if !is_last {
                    memif.write_pointer_to_target(
                        (obj_start.wrapping_add(24)) as *mut *const u64,
                        next_obj_start.wrapping_add(8) as *const u64,
                    );
                }
            }
        }
    }
}
