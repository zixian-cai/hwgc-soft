use crate::HeapObject;
use crate::RootEdge;
use std::collections::HashMap;
use std::collections::HashSet;

pub fn sanity_trace(roots: &[RootEdge], objects: &HashMap<u64, HeapObject>) -> usize {
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
