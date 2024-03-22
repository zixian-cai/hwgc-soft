use super::TracingStats;
use crate::{
    util::{tracer::Tracer, typed_obj::Slot},
    ObjectModel,
};
use std::{collections::VecDeque, marker::PhantomData};

struct EdgeSlotTracer<O: ObjectModel> {
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for EdgeSlotTracer<O> {
    fn startup(&self) {}

    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats {
        let mut mark_queue: VecDeque<Slot> = VecDeque::new();
        let mut marked_objects: u64 = 0;
        let mut slots = 0;
        let mut non_empty_slots = 0;
        for root in object_model.roots() {
            let slot = Slot::from_raw(root as *const u64 as *mut u64);
            if let Some(_) = slot.load() {
                mark_queue.push_back(slot);
            } else {
                slots += 1;
            }
        }
        while let Some(e) = mark_queue.pop_front() {
            slots += 1;
            if let Some(o) = e.load() {
                non_empty_slots += 1;
                if o.mark_relaxed(mark_sense) {
                    marked_objects += 1;
                    o.scan::<O, _>(|s| {
                        let Some(c) = s.load() else { return };
                        if c.is_marked(mark_sense) {
                            return;
                        }
                        mark_queue.push_back(s);
                    })
                }
            }
        }
        TracingStats {
            marked_objects,
            slots,
            non_empty_slots,
            ..Default::default()
        }
    }

    fn teardown(&self) {}
}

impl<O: ObjectModel> EdgeSlotTracer<O> {
    pub fn new() -> Self {
        Self { _p: PhantomData }
    }
}

pub fn create_tracer<O: ObjectModel>() -> Box<dyn Tracer<O>> {
    Box::new(EdgeSlotTracer::<O>::new())
}
