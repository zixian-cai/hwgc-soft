use crossbeam::deque::{Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use wp::Slot;

use super::TracingStats;
use crate::util::workers::Context;
use crate::util::{workers::WorkerGroup, ObjectOps};
use crate::ObjectModel;
use std::sync::atomic::AtomicU8;
use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

const N_WORKERS: usize = 32;

static mut ROOTS: Option<*const [u64]> = None;

static WORKER_GROUP: Lazy<WorkerGroup<GlobalContext>> =
    Lazy::new(|| WorkerGroup::new(32, GlobalContext::new()));

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

    fn flush(&mut self, local: &Worker<TracePacket<O>>) {
        if !self.next_slots.is_empty() {
            let next = TracePacket::new(std::mem::take(&mut self.next_slots));
            local.push(next);
        }
    }

    fn run(&mut self, local: &mut WPWorker<O>) {
        let mark_state = WORKER_GROUP.shared.mark_state();
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
                            self.flush(&local.worker);
                        }
                    });
                }
            } else {
                local.ne_edges += 1;
            }
        }
        self.flush(&local.worker);
    }
}

struct GlobalContext {
    mark_state: AtomicU8,
    objs: AtomicU64,
    edges: AtomicU64,
    ne_edges: AtomicU64,
}

impl GlobalContext {
    fn new() -> Self {
        Self {
            mark_state: AtomicU8::new(0),
            objs: AtomicU64::new(0),
            edges: AtomicU64::new(0),
            ne_edges: AtomicU64::new(0),
        }
    }

    fn set_mark_state(&self, mark_state: u8) {
        self.mark_state.store(mark_state, Ordering::SeqCst);
    }

    fn mark_state(&self) -> u8 {
        self.mark_state.load(Ordering::Relaxed)
    }
}

impl Context for GlobalContext {
    fn reset(&self) {
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
    }
}

struct WPWorker<O: ObjectModel> {
    id: usize,
    worker: Worker<TracePacket<O>>,
    stealers: Arc<Vec<Stealer<TracePacket<O>>>>,
    objs: u64,
    edges: u64,
    ne_edges: u64,
}

impl<O: ObjectModel> crate::util::workers::Worker for WPWorker<O> {
    type Shared = Stealer<TracePacket<O>>;

    fn new(id: usize) -> Self {
        Self {
            id,
            worker: Worker::new_lifo(),
            stealers: Arc::new(vec![]),
            objs: 0,
            edges: 0,
            ne_edges: 0,
        }
    }

    fn new_shared(&self) -> Self::Shared {
        self.worker.stealer()
    }

    fn init(&mut self, stealers: Arc<Vec<Self::Shared>>) {
        self.stealers = stealers;
    }

    fn run_epoch(&mut self) {
        self.objs = 0;
        self.edges = 0;
        self.ne_edges = 0;
        // scan roots
        if let Some(roots) = unsafe { ROOTS } {
            let roots = unsafe { &*roots };
            let range =
                (roots.len() * self.id) / N_WORKERS..(roots.len() * (self.id + 1)) / N_WORKERS;
            let mut buf = vec![];
            for root in &roots[range] {
                let slot = Slot(root as *const u64 as *mut u64);
                if buf.is_empty() {
                    buf.reserve(TracePacket::<O>::CAP);
                }
                buf.push(slot);
                if buf.len() >= TracePacket::<O>::CAP {
                    let packet = TracePacket::<O>::new(buf);
                    self.worker.push(packet);
                    buf = vec![];
                }
            }
            if !buf.is_empty() {
                let packet = TracePacket::<O>::new(buf);
                self.worker.push(packet);
            }
        }
        WORKER_GROUP.sync();
        // trace objects
        'outer: loop {
            // Drain local queue
            while let Some(mut p) = self.worker.pop() {
                p.run(self);
            }
            // Steal from other workers
            for stealer in &*self.stealers {
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
        assert!(self.worker.is_empty());
        let shared = &WORKER_GROUP.shared;
        shared.objs.fetch_add(self.objs, Ordering::SeqCst);
        shared.edges.fetch_add(self.edges, Ordering::SeqCst);
        shared.ne_edges.fetch_add(self.ne_edges, Ordering::SeqCst);
    }
}

pub fn prologue<O: ObjectModel>() {
    WORKER_GROUP.spawn::<WPWorker<O>>();
}

pub fn transitive_closure<O: ObjectModel>(mark_sense: u8, object_model: &O) -> TracingStats {
    WORKER_GROUP.shared.set_mark_state(mark_sense);
    // Get roots
    unsafe { ROOTS = Some(object_model.roots()) };
    // Wake up workers
    WORKER_GROUP.run_epoch();
    let shared = &WORKER_GROUP.shared;
    TracingStats {
        marked_objects: shared.objs.load(Ordering::SeqCst),
        slots: shared.edges.load(Ordering::SeqCst),
        non_empty_slots: shared.ne_edges.load(Ordering::SeqCst),
        sends: 0,
    }
}

pub fn epilogue<O: ObjectModel>() {
    WORKER_GROUP.finish();
}
