use crate::util::workers::WorkerGroup;
use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU8, AtomicUsize};
use std::sync::Weak;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use super::fake_forwarding::LocalAllocator;

pub trait Packet: Send {
    fn run(&mut self, local: &mut WPWorker);
}

pub struct GlobalContext {
    pub queue: Injector<Box<dyn Packet>>,
    pub mark_state: AtomicU8,
    pub objs: AtomicU64,
    pub edges: AtomicU64,
    pub ne_edges: AtomicU64,
    pub cap: AtomicUsize,
}

impl GlobalContext {
    pub fn new() -> Self {
        Self {
            queue: Injector::new(),
            mark_state: AtomicU8::new(0),
            objs: AtomicU64::new(0),
            edges: AtomicU64::new(0),
            ne_edges: AtomicU64::new(0),
            cap: AtomicUsize::new(1024),
        }
    }

    pub fn set_cap(&self, cap: usize) {
        self.cap.store(cap, Ordering::SeqCst);
    }

    pub fn cap(&self) -> usize {
        self.cap.load(Ordering::Relaxed)
    }

    pub fn mark_state(&self) -> u8 {
        self.mark_state.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
    }
}

pub static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

pub struct WPWorker {
    _id: usize,
    pub queue: Worker<Box<dyn Packet>>,
    pub global: Arc<GlobalContext>,
    pub group: Weak<WorkerGroup<WPWorker>>,
    pub objs: u64,
    pub slots: u64,
    pub ne_slots: u64,
    pub copy: LocalAllocator,
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
            slots: 0,
            ne_slots: 0,
            copy: LocalAllocator::new(),
        }
    }

    fn new_shared(&self) -> Self::SharedWorker {
        self.queue.stealer()
    }

    fn run_epoch(&mut self) {
        self.copy.reset();
        self.objs = 0;
        self.slots = 0;
        self.ne_slots = 0;
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
        global.edges.fetch_add(self.slots, Ordering::SeqCst);
        global.ne_edges.fetch_add(self.ne_slots, Ordering::SeqCst);
    }
}
