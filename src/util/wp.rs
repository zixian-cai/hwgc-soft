use crate::trace::TracingStats;
use crate::util::workers::WorkerGroup;
use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU8, AtomicUsize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::sync::{Condvar, Mutex, Weak};

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
    pub copied_objects: AtomicU64,
    pub packets: AtomicU64,
    pub total_run_time_us: AtomicU64,
    parked: AtomicUsize,
    monitor: (Mutex<bool>, Condvar),
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
            copied_objects: AtomicU64::new(0),
            packets: AtomicU64::new(0),
            total_run_time_us: AtomicU64::new(0),
            parked: AtomicUsize::new(0),
            monitor: (Mutex::new(false), Condvar::new()),
            cap: AtomicUsize::new(128),
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
        self.copied_objects.store(0, Ordering::SeqCst);
        self.packets.store(0, Ordering::SeqCst);
        self.total_run_time_us.store(0, Ordering::SeqCst);
        self.parked.store(0, Ordering::SeqCst);
        *self.monitor.0.lock().unwrap() = false;
    }

    pub fn get_stats(&self) -> TracingStats {
        TracingStats {
            marked_objects: self.objs.load(Ordering::SeqCst),
            slots: self.edges.load(Ordering::SeqCst),
            non_empty_slots: self.ne_edges.load(Ordering::SeqCst),
            copied_objects: self.copied_objects.load(Ordering::SeqCst),
            packets: self.packets.load(Ordering::SeqCst),
            total_run_time_us: self.total_run_time_us.load(Ordering::SeqCst),
            ..Default::default()
        }
    }
}

pub static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

pub struct WPWorker {
    _id: usize,
    queue: Worker<Box<dyn Packet>>,
    pub global: Arc<GlobalContext>,
    pub group: Weak<WorkerGroup<WPWorker>>,
    pub objs: u64,
    pub slots: u64,
    pub ne_slots: u64,
    pub copied_objects: u64,
    pub packets: u64,
    pub copy: LocalAllocator,
}

impl WPWorker {
    pub fn add(&self, packet: Box<dyn Packet>) {
        self.queue.push(packet);
    }
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
            copied_objects: 0,
            packets: 0,
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
        self.copied_objects = 0;
        self.packets = 0;
        let group = self.group.upgrade().unwrap();
        let t = std::time::Instant::now();
        // trace objects
        'outer: loop {
            let mut executed_packets = false;
            // Drain local queue
            while let Some(mut p) = self.queue.pop() {
                executed_packets = true;
                p.run(self);
            }
            // Steal from global queue
            match self.global.queue.steal() {
                Steal::Success(mut p) => {
                    executed_packets = true;
                    p.run(self);
                }
                Steal::Retry => continue 'outer,
                _ => {}
            }
            // Steal from other workers
            for stealer in &*group.workers {
                match stealer.steal() {
                    Steal::Success(mut p) => {
                        executed_packets = true;
                        p.run(self);
                        break;
                    }
                    Steal::Retry => continue 'outer,
                    _ => {}
                }
            }
            if executed_packets {
                continue 'outer;
            }
            break;
        }
        let elapsed = t.elapsed();
        assert!(self.queue.is_empty());
        let global = &self.global;
        global.objs.fetch_add(self.objs, Ordering::SeqCst);
        global.edges.fetch_add(self.slots, Ordering::SeqCst);
        global.ne_edges.fetch_add(self.ne_slots, Ordering::SeqCst);
        global
            .copied_objects
            .fetch_add(self.copied_objects, Ordering::SeqCst);
        global.packets.fetch_add(self.packets, Ordering::SeqCst);
        global
            .total_run_time_us
            .fetch_add(elapsed.as_micros() as u64, Ordering::SeqCst);
    }
}
