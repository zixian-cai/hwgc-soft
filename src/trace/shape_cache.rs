use super::{trace_object, TracingStats};
use crate::{ObjectModel, TraceArgs};
use lru::LruCache;
use std::{collections::VecDeque, num::NonZeroUsize};

pub(super) unsafe fn transitive_closure_shape_cache<O: ObjectModel>(
    args: TraceArgs,
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    // Edge-Slot enqueuing
    let mut mark_queue: VecDeque<*mut u64> = VecDeque::new();
    let mut shape_cache_hits = 0;
    let mut shape_cache_misses = 0;
    let mut marked_objects: u64 = 0;
    let mut shape_cache: LruCache<*const O::Tib, ()> =
        LruCache::new(NonZeroUsize::new(args.shape_cache_size).unwrap());
    for root in object_model.roots() {
        let o = *root;
        if o != 0 && trace_object(o, mark_sense) {
            marked_objects += 1;
            if shape_cache.get(&O::get_tib(o)).is_some() {
                shape_cache_hits += 1;
            } else {
                shape_cache_misses += 1;
                shape_cache.put(O::get_tib(o), ());
            }
            O::scan_object(o, |edge, repeat| {
                for i in 0..repeat {
                    mark_queue.push_back(edge.wrapping_add(i as usize));
                }
            })
        }
    }
    while let Some(e) = mark_queue.pop_front() {
        let o = *e;
        if o != 0 && trace_object(o, mark_sense) {
            marked_objects += 1;
            if shape_cache.get(&O::get_tib(o)).is_some() {
                shape_cache_hits += 1;
            } else {
                shape_cache_misses += 1;
                shape_cache.put(O::get_tib(o), ());
            }
            O::scan_object(o, |edge, repeat| {
                for i in 0..repeat {
                    mark_queue.push_back(edge.wrapping_add(i as usize));
                }
            })
        }
    }
    debug_assert_eq!(marked_objects, shape_cache_hits + shape_cache_misses);
    TracingStats {
        marked_objects,
        shape_cache_hits,
        shape_cache_misses,
        ..Default::default()
    }
}
