use crate::trace::TracingStats;
use crate::util::workers::WorkerGroup;
use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::sync::{Condvar, Mutex, Weak};

use super::fake_forwarding::LocalAllocator;

pub trait Packet: Send {
    fn run(&mut self);
}

pub struct Buckets {
    pub prepare: Bucket,
    pub closure: Bucket,
}

pub struct Bucket {
    count: AtomicUsize,
    pub name: &'static str,
    is_open: AtomicBool,
    queue: crossbeam::queue::SegQueue<Box<dyn Packet>>,
}

impl Bucket {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            count: AtomicUsize::new(0),
            is_open: AtomicBool::new(false),
            queue: crossbeam::queue::SegQueue::new(),
        }
    }

    pub fn open(&self) {
        info!("[{:.3}ms] Opening bucket {}", GLOBAL.elapsed(), self.name);
        self.is_open.store(true, Ordering::SeqCst);
        while let Some(p) = self.queue.pop() {
            GLOBAL.active_queue.push(p);
        }
        if GLOBAL.yielded.load(Ordering::SeqCst) > 0 {
            GLOBAL.cvar.notify_all();
        }
    }

    pub fn close(&self) {
        self.is_open.store(false, Ordering::SeqCst);
    }

    pub fn is_open(&self) -> bool {
        self.is_open.load(Ordering::SeqCst)
    }

    pub fn push(&self, packet: Box<dyn Packet>) {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.queue.push(packet);
    }
}

pub struct GlobalContext {
    pub active_queue: Injector<Box<dyn Packet>>,
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
    pub start_time: UnsafeCell<std::time::Instant>,
    cvar: Condvar,
    temp_yield: Mutex<usize>,
    yielded: AtomicUsize,
    pub buckets: Buckets,
    pub total_busy_us: AtomicUsize,
}

impl GlobalContext {
    pub fn new() -> Self {
        Self {
            active_queue: Injector::new(),
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
            start_time: UnsafeCell::new(std::time::Instant::now()),
            cvar: Condvar::new(),
            temp_yield: Mutex::new(0),
            yielded: AtomicUsize::new(0),
            buckets: Buckets {
                prepare: Bucket::new("prepare"),
                closure: Bucket::new("closure"),
            },
            total_busy_us: AtomicUsize::new(0),
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

    pub fn elapsed(&self) -> f32 {
        let t = unsafe { &*self.start_time.get() };
        t.elapsed().as_millis_f32()
    }

    pub fn reset(&self) {
        let mut yielded = GLOBAL.temp_yield.lock().unwrap();
        *yielded = 0;
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
        self.copied_objects.store(0, Ordering::SeqCst);
        self.packets.store(0, Ordering::SeqCst);
        self.total_run_time_us.store(0, Ordering::SeqCst);
        self.parked.store(0, Ordering::SeqCst);
        unsafe {
            *self.start_time.get() = std::time::Instant::now();
        }
        *self.monitor.0.lock().unwrap() = false;
        self.yielded.store(0, Ordering::SeqCst);
        self.buckets.prepare.close();
        self.buckets.closure.close();
        self.total_busy_us.store(0, Ordering::SeqCst);
    }

    pub fn get_stats(&self) -> TracingStats {
        // println!(
        //     "total_busy_us {}",
        //     self.total_busy_us.load(Ordering::SeqCst)
        // );
        TracingStats {
            marked_objects: self.objs.load(Ordering::SeqCst),
            slots: self.edges.load(Ordering::SeqCst),
            non_empty_slots: self.ne_edges.load(Ordering::SeqCst),
            copied_objects: self.copied_objects.load(Ordering::SeqCst),
            packets: self.packets.load(Ordering::SeqCst),
            total_run_time_us: self.total_run_time_us.load(Ordering::SeqCst),
            total_busy_time_us: self.total_busy_us.load(Ordering::SeqCst) as _,
            ..Default::default()
        }
    }
}

unsafe impl Sync for GlobalContext {}

pub static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

#[thread_local]
static mut LOCAL: *mut WPWorker = ptr::null_mut();

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
    pub fn spawn<P: Packet + 'static>(&self, bucket: usize, packet: P) {
        let bucket = if bucket == 0 {
            &self.global.buckets.prepare
        } else {
            &self.global.buckets.closure
        };
        if bucket.is_open() {
            // println!("spawn: Bucket is open {}", bucket.name);
            bucket.count.fetch_add(1, Ordering::SeqCst);
            self.queue.push(Box::new(packet));
            if GLOBAL.yielded.load(Ordering::SeqCst) > 0 {
                self.global.cvar.notify_one();
            }
        } else {
            bucket.push(Box::new(packet));
        }
    }

    pub fn current() -> &'static mut WPWorker {
        unsafe { &mut *LOCAL }
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
            queue: if cfg!(feature = "fifo") {
                Worker::new_fifo()
            } else {
                Worker::new_lifo()
            },
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
        unsafe { LOCAL = self as *mut Self };
        self.copy.reset();
        self.objs = 0;
        self.slots = 0;
        self.ne_slots = 0;
        self.copied_objects = 0;
        self.packets = 0;
        let group = self.group.upgrade().unwrap();
        let t = std::time::Instant::now();
        // trace objects
        loop {
            let x = std::time::Instant::now();
            'outer: loop {
                let mut executed_packets = false;
                // Drain local queue
                while let Some(p) = self.queue.pop() {
                    executed_packets = true;
                    self.run_packet(p);
                }
                // Steal from global queue
                match self.global.active_queue.steal() {
                    Steal::Success(p) => {
                        executed_packets = true;
                        self.run_packet(p);
                    }
                    Steal::Retry => continue 'outer,
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
                        Steal::Retry => continue 'outer,
                        _ => {}
                    }
                }
                if executed_packets {
                    continue 'outer;
                }
                break;
            }
            let elapsed = x.elapsed().as_micros();
            self.global
                .total_busy_us
                .fetch_add(elapsed as usize, Ordering::Relaxed);

            // sleep
            let mut yielded = GLOBAL.temp_yield.lock().unwrap();
            *yielded += 1;
            GLOBAL.yielded.fetch_add(1, Ordering::SeqCst);
            // println!(
            //     "[{:.3}ms] Worker #{} yield {}",
            //     GLOBAL.elapsed(),
            //     self._id,
            //     *yielded
            // );
            if group.workers.len() == *yielded {
                if self.global.buckets.closure.is_open() {
                    self.global.cvar.notify_all();
                    break;
                } else {
                    self.global.buckets.closure.open();
                    self.global.cvar.notify_all();
                    *yielded -= 1;
                    GLOBAL.yielded.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
            }
            yielded = self.global.cvar.wait(yielded).unwrap();
            // println!("[{:.3}ms] Worker #{} wake", GLOBAL.elapsed(), self._id);
            if group.workers.len() == *yielded {
                break;
            }
            *yielded -= 1;
            GLOBAL.yielded.fetch_sub(1, Ordering::SeqCst);
        }

        // println!("Worker #{} exit", self._id);
        // println!("[{:.3}ms] Worker #{} exit", GLOBAL.elapsed(), self._id);
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
