use super::{trace_object, TracingStats};
use crate::object_model::{HasTibType, TibType};
use crate::{ObjectModel, TraceArgs};
use lru::LruCache;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    num::NonZeroUsize,
};

pub(crate) struct ShapeLruCache<O: ObjectModel> {
    cache: LruCache<*const O::Tib, ()>,
    stats: HashMap<ShapeCacheResponse, usize>,
    tib_seen: HashSet<*const O::Tib>,
}
#[derive(Default, Debug)]
pub(crate) struct ShapeCacheStats {
    hits: usize,
    capacity_misses: usize,
    compulsory_misses_instance: usize,
    compulsory_misses_instance_mirror: usize,
}

impl ShapeCacheStats {
    pub(crate) fn get_stats_header(&self) -> &str {
        "shape_cache.hit\tshape_cache.cap_miss\tshape_cache.comp_miss_inst\tshape_cache.comp_miss_mirror"
    }

    pub(crate) fn get_stats_value(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.hits,
            self.capacity_misses,
            self.compulsory_misses_instance,
            self.compulsory_misses_instance_mirror
        )
    }

    pub(crate) fn add(&mut self, other: &Self) {
        self.hits += other.hits;
        self.capacity_misses += other.capacity_misses;
        self.compulsory_misses_instance += other.compulsory_misses_instance;
        self.compulsory_misses_instance_mirror += other.compulsory_misses_instance_mirror;
    }
}

impl<O: ObjectModel> ShapeLruCache<O> {
    pub(crate) fn new(capacity: usize) -> Self {
        ShapeLruCache {
            cache: LruCache::new(NonZeroUsize::new(capacity).unwrap()),
            stats: HashMap::new(),
            tib_seen: HashSet::new(),
        }
    }

    fn update(&mut self, tib: *const O::Tib) {
        let ttype: TibType = unsafe { &*tib as &O::Tib }.get_tib_type();
        if matches!(ttype, TibType::InstanceMirror) {
            *self
                .stats
                .entry(ShapeCacheResponse::CompulsoryMissInstanceMirror)
                .or_default() += 1;
        } else if self.tib_seen.contains(&tib) {
            // We have seen this type before
            if self.cache.get(&tib).is_some() {
                // And it's in the cache, so it's a hit
                *self.stats.entry(ShapeCacheResponse::Hit).or_default() += 1;
            } else {
                // Now it's not in the cache, so it's a capacity miss
                *self
                    .stats
                    .entry(ShapeCacheResponse::CapacityMiss)
                    .or_default() += 1;
                self.cache.put(tib, ());
            }
        } else {
            // This is the first time we see this type, resulting in a
            // compulsory miss
            self.cache.put(tib, ());
            *self
                .stats
                .entry(ShapeCacheResponse::CompulsoryMissInstance)
                .or_default() += 1;
            self.tib_seen.insert(tib);
        }
    }

    fn get_stats_and_clear(&mut self) -> ShapeCacheStats {
        // This is the stats for one iteration
        let ret = ShapeCacheStats {
            hits: *self.stats.get(&ShapeCacheResponse::Hit).unwrap_or(&0),
            capacity_misses: *self
                .stats
                .get(&ShapeCacheResponse::CapacityMiss)
                .unwrap_or(&0),
            compulsory_misses_instance: *self
                .stats
                .get(&ShapeCacheResponse::CompulsoryMissInstance)
                .unwrap_or(&0),
            compulsory_misses_instance_mirror: *self
                .stats
                .get(&ShapeCacheResponse::CompulsoryMissInstanceMirror)
                .unwrap_or(&0),
        };
        self.stats.clear();
        ret
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
enum ShapeCacheResponse {
    Hit = 0,
    CapacityMiss = 1,
    CompulsoryMissInstance = 2,
    CompulsoryMissInstanceMirror = 3,
}

pub(super) unsafe fn transitive_closure_shape_cache<O: ObjectModel>(
    _args: TraceArgs,
    mark_sense: u8,
    object_model: &O,
    shape_cache: &mut ShapeLruCache<O>,
) -> TracingStats {
    // Edge-Slot enqueuing
    let mut mark_queue: VecDeque<*mut u64> = VecDeque::new();
    let mut marked_objects: u64 = 0;
    // println!("{}", shape_cache.len());
    // shape_cache.clear();
    for root in object_model.roots() {
        let o = *root;
        if o != 0 && trace_object(o, mark_sense) {
            marked_objects += 1;
            if O::tib_lookup_required(o) {
                shape_cache.update(O::get_tib(o));
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
            if O::tib_lookup_required(o) {
                shape_cache.update(O::get_tib(o));
            }
            O::scan_object(o, |edge, repeat| {
                for i in 0..repeat {
                    mark_queue.push_back(edge.wrapping_add(i as usize));
                }
            })
        }
    }
    TracingStats {
        marked_objects,
        shape_cache_stats: shape_cache.get_stats_and_clear(),
        ..Default::default()
    }
}
