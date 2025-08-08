use std::collections::HashMap;

/// Statistics about communication in a distributed near-memory GC
///
/// All slots (edges) can be categorized as follows:
/// 1. Empty slots
/// 2. Non-empty slots
///
/// 1.a Empty slot, visible: a worker scanning the object can also load from the
/// slot, and then discovers that the slot holds null. No further
/// 1.b Empty slot, invisible: a worker scanning the object has to delegate
/// someone else to load the slot (using the ProcessEdge message, or the
/// ProcessEdges message), which subsequently turns out to be null.
/// 2.a Non-empty slot, visible, visible child: a worker scanning the object
/// can also load from the slot, and then discovers a child object, which is
/// also visible. No message was sent in the process. This is rare (~1/N chance).
/// 2.b Non-empty slot, visible, invisible child: a worker scanning the object
/// can also load from the slot, and then discovers a child object, which is
/// invisible. A ProcessNode message is sent.
/// 2.c Non-empty slot, invisible, visible child: a worker scanning the object
/// has to delegate someone else to load the slot (using the ProcessEdge
/// message, or the ProcessEdges message), which is common. The child object
/// happens to be visible to the delegate, which is rare.
/// 2.d Non-empty slot, invisible, invisible child: a worker scanning the object
/// has to delegate someone else to load the slot (using the ProcessEdge
/// message, or the ProcessEdges message), which is common. The delegate
/// discovers a child object, which is invisible. A ProcessNode message is sent.
///
/// Another classification is:
/// 1. Visible slots:
/// 1.a Visible, empty slot
/// 1.b Visible, non-empty slot, visible child
/// 1.c Visible, non-empty slot, invisible child: a ProcessNode message is sent.
/// 2. Invisible slots: need to send ProcessEdge/ProcessEdges messages
/// 2.a Invisible slot, empty slot
/// 2.b Invisible slot, non-empty slot, visible child
/// 2.c Invisible slot, non-empty slot, invisible child: a ProcessNode message is
/// sent.
#[derive(Default)]
pub(super) struct AnalysisStats {
    /// Total amount of work
    ///
    /// This is equal to the the number of non-empty slots + invisible slots
    /// when the group_slots optimization is disabled.
    /// This is because each non-empty slots has a referent that needs to be called
    /// process_node on using the ProcessNode packet
    /// (a message may or may not be sent, depending on whether the child is
    /// visible to the slot loader).
    /// And each invisible slot results in a ProcessEdge packet sent to
    /// another worker.
    pub(super) total_work: u64,
    /// Distribuion of work among each worker
    pub(super) work_dist: HashMap<usize, u64>,
    /// Total objects marked
    pub(super) marked_objects: u64,
    pub(super) los_objects: u64,
    pub(super) los_objarrays: u64,
    pub(super) completely_visible_objects: u64,
    /// Total number of inter-worker messages sent
    pub(super) total_msgs: u64,
    pub(super) msg_process_node: u64,
    pub(super) msg_process_edge: u64,
    pub(super) msg_process_edges: u64,
    /// Total number of slots
    pub(super) slots: u64,
    pub(super) empty_root_slots: u64,
    pub(super) non_empty_root_slots: u64,
    pub(super) visible_empty_slots: u64,
    pub(super) visible_non_empty_slots_visible_child: u64,
    pub(super) visible_non_empty_slots_invisible_child: u64,
    pub(super) invisible_empty_slots: u64,
    pub(super) invisible_non_empty_slots_visible_child: u64,
    pub(super) invisible_non_empty_slots_invisible_child: u64,
    pub(super) objarray_slots: u64,
    pub(super) objarray_empty_slots: u64,
    /// Object sizes
    pub(super) total_object_size: u64,
    pub(super) los_object_size: u64,
    pub(super) los_objarray_size: u64,
}

impl AnalysisStats {
    pub(super) fn print(&self) {
        let mut dist: Vec<(usize, u64)> = self
            .work_dist
            .iter()
            .map(|(worker, work_cnt)| (*worker, *work_cnt))
            .collect();
        dist.sort_by_key(|(worker, _)| *worker);
        println!("============================ Tabulate Statistics ============================");
        print!(
            "obj\tobj.los\tobj.los.objarray\tobj.complete\t\
            size\tsize.los\tsize.los.objarray\t\
            msg\tmsg.pn\tmsg.pe\tmsg.pes\t\
            slots\tslots.vis.empty\tslots.vis.child.vis\tslots.vis.child.invis\t\
            slots.invis.empty\tslots.invis.child.vis\tslots.invis.child.invis\t\
            slots.root.empty\tslots.root.non_empty\t\
            slots.objarray\tslots.objarray.empty\t\
            work"
        );
        for (x, _) in &dist {
            print!("\twork.{}", x);
        }
        println!();
        print!(
            "{}\t{}\t{}\t{}\t\
            {}\t{}\t{}\t\
            {}\t{}\t{}\t{}\t\
            {}\t{}\t{}\t{}\t\
            {}\t{}\t{}\t\
            {}\t{}\t\
            {}\t{}\t\
            {}",
            self.marked_objects,
            self.los_objects,
            self.los_objarrays,
            self.completely_visible_objects,
            self.total_object_size,
            self.los_object_size,
            self.los_objarray_size,
            self.total_msgs,
            self.msg_process_node,
            self.msg_process_edge,
            self.msg_process_edges,
            self.slots,
            self.visible_empty_slots,
            self.visible_non_empty_slots_visible_child,
            self.visible_non_empty_slots_invisible_child,
            self.invisible_empty_slots,
            self.invisible_non_empty_slots_visible_child,
            self.invisible_non_empty_slots_invisible_child,
            self.empty_root_slots,
            self.non_empty_root_slots,
            self.objarray_slots,
            self.objarray_empty_slots,
            self.total_work
        );
        for (_, work_cnt) in &dist {
            print!("\t{}", work_cnt);
        }
        println!();
        println!("-------------------------- End Tabulate Statistics --------------------------");
        debug_assert_eq!(
            self.slots,
            self.visible_empty_slots
                + self.visible_non_empty_slots_visible_child
                + self.visible_non_empty_slots_invisible_child
                + self.invisible_empty_slots
                + self.invisible_non_empty_slots_visible_child
                + self.invisible_non_empty_slots_invisible_child
                + self.non_empty_root_slots
                + self.empty_root_slots
        );
        debug_assert_eq!(
            self.total_msgs,
            self.msg_process_edge + self.msg_process_edges + self.msg_process_node
        );
        debug_assert_eq!(self.total_work, self.work_dist.values().sum::<u64>());
    }
}
