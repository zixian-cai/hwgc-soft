use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use wp::Slot;

use super::TracingStats;
use crate::util::tracer::Tracer;
use crate::util::{workers::WorkerGroup, ObjectOps};
use crate::ObjectModel;
use std::ops::Range;
use std::sync::atomic::AtomicU8;
use std::sync::Weak;
use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

static mut ROOTS: Option<*const [u64]> = None;

trait Packet: Send {
    fn run(&mut self, local: &mut WPWorker);
}

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
                    o.scan_object::<O, _>(|s| {
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

impl<O: ObjectModel> Packet for ScanRoots<O> {
    fn run(&mut self, local: &mut WPWorker) {
        let mut buf = vec![];
        let Some(roots) = (unsafe { ROOTS }) else {
            unreachable!()
        };
        let roots = unsafe { &*roots };
        for root in &roots[self.range.clone()] {
            let slot = Slot(root as *const u64 as *mut u64);
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

struct WPWorker {
    _id: usize,
    queue: Worker<Box<dyn Packet>>,
    global: Arc<GlobalContext>,
    group: Weak<WorkerGroup<WPWorker>>,
    objs: u64,
    edges: u64,
    ne_edges: u64,
}

impl crate::util::workers::Worker for WPWorker {
    type SharedWorker = Stealer<Box<dyn Packet>>;

    fn new(id: usize, group: Weak<WorkerGroup<Self>>) -> Self {
        Self {
            _id: id,
            queue: Worker::new_lifo(),
            group,
            global: GLOBAL.clone(),
            objs: 0,
            edges: 0,
            ne_edges: 0,
        }
    }

    fn new_shared(&self) -> Self::SharedWorker {
        self.queue.stealer()
    }

    fn run_epoch(&mut self) {
        self.objs = 0;
        self.edges = 0;
        self.ne_edges = 0;
        let group = self.group.upgrade().unwrap();
        // trace objects
        'outer: loop {
            // Drain local queue
            while let Some(mut p) = self.queue.pop() {
                p.run(self);
            }
            // Steal from global queue
            while let Steal::Success(mut p) = self.global.queue.steal() {
                p.run(self);
            }
            // Steal from other workers
            for stealer in &*group.workers {
                match stealer.steal() {
                    Steal::Success(mut p) => {
                        p.run(self);
                        continue 'outer;
                    }
                    Steal::Retry => continue 'outer,
                    _ => {}
                }
            }
            break;
        }
        assert!(self.queue.is_empty());
        let global = &self.global;
        global.objs.fetch_add(self.objs, Ordering::SeqCst);
        global.edges.fetch_add(self.edges, Ordering::SeqCst);
        global.ne_edges.fetch_add(self.ne_edges, Ordering::SeqCst);
    }
}

struct GlobalContext {
    queue: Injector<Box<dyn Packet>>,
    mark_state: AtomicU8,
    objs: AtomicU64,
    edges: AtomicU64,
    ne_edges: AtomicU64,
}

impl GlobalContext {
    fn new() -> Self {
        Self {
            queue: Injector::new(),
            mark_state: AtomicU8::new(0),
            objs: AtomicU64::new(0),
            edges: AtomicU64::new(0),
            ne_edges: AtomicU64::new(0),
        }
    }

    fn mark_state(&self) -> u8 {
        self.mark_state.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
    }
}

static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

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
        // Get roots
        let roots = object_model.roots();
        let roots_len = roots.len();
        unsafe { ROOTS = Some(roots) };
        let num_workers = self.group.workers.len();
        for id in 0..num_workers {
            let range = (roots_len * id) / num_workers..(roots_len * (id + 1)) / num_workers;
            let packet = ScanRoots::<O> {
                range,
                _p: PhantomData,
            };
            GLOBAL.queue.push(Box::new(packet));
        }
        // Wake up workers
        self.group.run_epoch();
        TracingStats {
            marked_objects: GLOBAL.objs.load(Ordering::SeqCst),
            slots: GLOBAL.edges.load(Ordering::SeqCst),
            non_empty_slots: GLOBAL.ne_edges.load(Ordering::SeqCst),
            sends: 0,
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
