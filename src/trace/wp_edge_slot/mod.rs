mod wp;

use crossbeam::deque::Worker;

use super::TracingStats;
use crate::util::tracer::Tracer;
use crate::util::typed_obj::Slot;
use crate::util::workers::WorkerGroup;
use crate::ObjectModel;
use std::ops::Range;
use std::{
    marker::PhantomData,
    sync::{atomic::Ordering, Arc},
};

use wp::{Packet, WPWorker, GLOBAL};

static mut ROOTS: Option<*const [u64]> = None;

struct TracePacket<O: ObjectModel> {
    slots: Vec<Slot>,
    next_slots: Vec<Slot>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> TracePacket<O> {
    const CAP: usize = 512;

    fn new(slots: Vec<Slot>) -> Self {
        TracePacket {
            slots,
            next_slots: Vec::new(),
            _p: PhantomData,
        }
    }

    fn flush(&mut self, local: &Worker<Box<dyn Packet>>) {
        if !self.next_slots.is_empty() {
            let next = TracePacket::<O>::new(std::mem::take(&mut self.next_slots));
            local.push(Box::new(next));
        }
    }
}

impl<O: ObjectModel> Packet for TracePacket<O> {
    fn run(&mut self, local: &mut WPWorker) {
        let mark_state = local.global.mark_state();
        let slots = std::mem::take(&mut self.slots);
        for slot in slots {
            local.edges += 1;
            if let Some(o) = slot.load() {
                if o.mark(mark_state) {
                    local.objs += 1;
                    o.scan::<O, _>(|s| {
                        if self.next_slots.is_empty() {
                            self.next_slots.reserve(Self::CAP);
                        }
                        self.next_slots.push(s);
                        if self.next_slots.len() >= Self::CAP {
                            self.flush(&local.queue);
                        }
                    });
                }
            } else {
                local.ne_edges += 1;
            }
        }
        self.flush(&local.queue);
    }
}

struct ScanRoots<O: ObjectModel> {
    range: Range<usize>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> ScanRoots<O> {
    fn new(range: Range<usize>) -> Self {
        ScanRoots {
            range,
            _p: PhantomData,
        }
    }
}

impl<O: ObjectModel> Packet for ScanRoots<O> {
    fn run(&mut self, local: &mut WPWorker) {
        let mut buf = vec![];
        let Some(roots) = (unsafe { ROOTS }) else {
            unreachable!()
        };
        let roots = unsafe { &*roots };
        for root in &roots[self.range.clone()] {
            let slot = Slot::from_raw(root as *const u64 as *mut u64);
            if buf.is_empty() {
                buf.reserve(TracePacket::<O>::CAP);
            }
            buf.push(slot);
            if buf.len() >= TracePacket::<O>::CAP {
                let packet = TracePacket::<O>::new(buf);
                local.queue.push(Box::new(packet));
                buf = vec![];
            }
        }
        if !buf.is_empty() {
            let packet = TracePacket::<O>::new(buf);
            local.queue.push(Box::new(packet));
        }
    }
}

struct WPTracer<O: ObjectModel> {
    group: Arc<WorkerGroup<WPWorker>>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for WPTracer<O> {
    fn startup(&self) {
        self.group.spawn();
    }

    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats {
        GLOBAL.reset();
        GLOBAL.mark_state.store(mark_sense, Ordering::SeqCst);
        // Create initial root scanning packets
        let roots = object_model.roots();
        let roots_len = roots.len();
        unsafe { ROOTS = Some(roots) };
        let num_workers = self.group.workers.len();
        for id in 0..num_workers {
            let range = (roots_len * id) / num_workers..(roots_len * (id + 1)) / num_workers;
            let packet = ScanRoots::<O>::new(range);
            GLOBAL.queue.push(Box::new(packet));
        }
        // Wake up workers
        self.group.run_epoch();
        TracingStats {
            marked_objects: GLOBAL.objs.load(Ordering::SeqCst),
            slots: GLOBAL.edges.load(Ordering::SeqCst),
            non_empty_slots: GLOBAL.ne_edges.load(Ordering::SeqCst),
            sends: 0,
            shape_cache_stats: Default::default(),
        }
    }

    fn teardown(&self) {
        self.group.finish();
    }
}

impl<O: ObjectModel> WPTracer<O> {
    pub fn new() -> Self {
        Self {
            group: WorkerGroup::new(32),
            _p: PhantomData,
        }
    }
}

pub fn create_tracer<O: ObjectModel>() -> Box<dyn Tracer<O>> {
    Box::new(WPTracer::<O>::new())
}
