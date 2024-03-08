use crossbeam::deque::Worker;

use super::TracingStats;
use crate::util::tracer::Tracer;
use crate::util::typed_obj::{Object, Slot};
use crate::util::workers::WorkerGroup;
use crate::util::wp::{Packet, WPWorker, GLOBAL};
use crate::{ObjectModel, TraceArgs};
use std::ops::Range;
use std::{
    marker::PhantomData,
    sync::{atomic::Ordering, Arc},
};

static mut ROOTS: Option<*const [u64]> = None;

struct TracePacket<O: ObjectModel> {
    slots: Vec<Slot>,
    next_objects: Vec<Object>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> TracePacket<O> {
    const CAP: usize = 512;

    fn new(slots: Vec<Slot>) -> Self {
        TracePacket {
            slots,
            next_objects: Vec::new(),
            _p: PhantomData,
        }
    }

    fn flush(&mut self, local: &mut WPWorker) {
        if !self.next_objects.is_empty() {
            let mut next = ScanPacket::<O>::new(std::mem::take(&mut self.next_objects));
            // local.push(Box::new(next));
            next.run(local);
        }
    }
}

impl<O: ObjectModel> Packet for TracePacket<O> {
    fn run(&mut self, local: &mut WPWorker) {
        let mark_state = local.global.mark_state();
        for slot in std::mem::take(&mut self.slots) {
            local.slots += 1;
            if let Some(o) = slot.load() {
                if o.mark(mark_state) {
                    local.objs += 1;
                    if self.next_objects.is_empty() {
                        self.next_objects.reserve(Self::CAP);
                    }
                    self.next_objects.push(o);
                    if self.next_objects.len() >= Self::CAP {
                        self.flush(local);
                    }
                } else {
                    local.ne_slots += 1;
                }
            }
            self.flush(local);
        }
    }
}

struct ScanPacket<O: ObjectModel> {
    objects: Vec<Object>,
    next_slots: Vec<Slot>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> ScanPacket<O> {
    const CAP: usize = 512;

    fn new(objects: Vec<Object>) -> Self {
        Self {
            objects,
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

impl<O: ObjectModel> Packet for ScanPacket<O> {
    fn run(&mut self, local: &mut WPWorker) {
        for o in std::mem::take(&mut self.objects) {
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

struct WPEdgeSlotDualTracer<O: ObjectModel> {
    group: Arc<WorkerGroup<WPWorker>>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for WPEdgeSlotDualTracer<O> {
    fn startup(&self) {
        info!("Use {} worker threads.", self.group.workers.len());
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
            ..Default::default()
        }
    }

    fn teardown(&self) {
        self.group.finish();
    }
}

impl<O: ObjectModel> WPEdgeSlotDualTracer<O> {
    pub fn new(num_workers: usize) -> Self {
        Self {
            group: WorkerGroup::new(num_workers),
            _p: PhantomData,
        }
    }
}

pub fn create_tracer<O: ObjectModel>(args: &TraceArgs) -> Box<dyn Tracer<O>> {
    Box::new(WPEdgeSlotDualTracer::<O>::new(args.threads))
}
