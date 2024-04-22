use crate::trace::TracingStats;
use crate::util::workers::WorkerGroup;
use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicUsize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::sync::{Condvar, Mutex, Weak};

pub trait Packet: Send {
    fn run(&mut self);
}

pub struct GlobalContext {
    pub queue: Injector<Box<dyn Packet>>,
    pub mark_state: AtomicU8,
    pub objs: AtomicU64,
    pub edges: AtomicU64,
    parked: AtomicUsize,
    monitor: (Mutex<bool>, Condvar),
    pub ne_edges: AtomicU64,
    pub cap: AtomicUsize,
    cvar: Condvar,
    temp_yield: Mutex<usize>,
    yielded: AtomicUsize,
}

impl GlobalContext {
    pub fn new() -> Self {
        Self {
            queue: Injector::new(),
            mark_state: AtomicU8::new(0),
            objs: AtomicU64::new(0),
            edges: AtomicU64::new(0),
            ne_edges: AtomicU64::new(0),
            cap: AtomicUsize::new(4096),
            parked: AtomicUsize::new(0),
            monitor: (Mutex::new(false), Condvar::new()),
            cvar: Condvar::new(),
            temp_yield: Mutex::new(0),
            yielded: AtomicUsize::new(0),
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
        let mut yielded = GLOBAL.temp_yield.lock().unwrap();
        *yielded = 0;
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
        self.parked.store(0, Ordering::SeqCst);
        *self.monitor.0.lock().unwrap() = false;
        self.yielded.store(0, Ordering::SeqCst);
    }

    pub fn get_stats(&self) -> TracingStats {
        TracingStats {
            marked_objects: self.objs.load(Ordering::SeqCst),
            slots: self.edges.load(Ordering::SeqCst),
            non_empty_slots: self.ne_edges.load(Ordering::SeqCst),
            ..Default::default()
        }
    }
}

pub static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

thread_local! {
    static LOCAL: Cell<*mut WPWorker> = Cell::new(std::ptr::null_mut());
}

pub struct WPWorker {
    _id: usize,
    queue: Worker<Box<dyn Packet>>,
    pub global: Arc<GlobalContext>,
    pub group: Weak<WorkerGroup<WPWorker>>,
    pub objs: u64,
    pub slots: u64,
    pub ne_slots: u64,
}

impl WPWorker {
    pub fn spawn<P: Packet + 'static>(&self, packet: P) {
        self.queue.push(Box::new(packet));
        if GLOBAL.yielded.load(Ordering::SeqCst) > 0 {
            self.global.cvar.notify_one();
        }
    }

    pub fn current() -> &'static mut WPWorker {
        unsafe { &mut *LOCAL.get() }
    }

    fn run_packet(&self, mut packet: Box<dyn Packet>) {
        packet.run();
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
        }
    }

    fn new_shared(&self) -> Self::SharedWorker {
        self.queue.stealer()
    }

    fn run_epoch(&mut self) {
        LOCAL.set(self as *mut Self);
        self.objs = 0;
        self.slots = 0;
        self.ne_slots = 0;
        let group = self.group.upgrade().unwrap();
        // trace objects
        loop {
            'poll: loop {
                let mut executed_packets = false;
                // Drain local queue
                while let Some(p) = self.queue.pop() {
                    executed_packets = true;
                    self.run_packet(p);
                }
                // Steal from global queue
                match self.global.queue.steal() {
                    Steal::Success(p) => {
                        executed_packets = true;
                        self.run_packet(p);
                    }
                    Steal::Retry => continue 'poll,
                    _ => {}
                }
                // Steal from other workers
                for stealer in &*group.workers {
                    match stealer.steal() {
                        Steal::Success(p) => {
                            executed_packets = true;
                            self.run_packet(p);
                            break;
                        }
                        Steal::Retry => continue 'poll,
                        _ => {}
                    }
                }
                // If there was no packet to execute, break
                if !executed_packets {
                    break;
                }
            }
            // sleep
            let mut yielded = GLOBAL.temp_yield.lock().unwrap();
            *yielded += 1;
            GLOBAL.yielded.fetch_add(1, Ordering::SeqCst);
            if group.workers.len() == *yielded {
                // notify all workers we are done
                self.global.cvar.notify_all();
                break;
            }
            yielded = self.global.cvar.wait(yielded).unwrap();
            if group.workers.len() == *yielded {
                // finish the current epoch
                break;
            }
            *yielded -= 1;
            GLOBAL.yielded.fetch_sub(1, Ordering::SeqCst);
        }
        assert!(self.queue.is_empty());
        let global = &self.global;
        global.objs.fetch_add(self.objs, Ordering::SeqCst);
        global.edges.fetch_add(self.slots, Ordering::SeqCst);
        global.ne_edges.fetch_add(self.ne_slots, Ordering::SeqCst);
    }
}
