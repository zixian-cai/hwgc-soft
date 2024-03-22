use super::TracingStats;
use crate::{
    util::{tracer::Tracer, typed_obj::Slot, workers::WorkerGroup},
    ObjectModel, TraceArgs,
};
use crossbeam::queue::SegQueue;
use once_cell::sync::Lazy;
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    marker::PhantomData,
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    time::Instant,
};

const LOG_NUM_TREADS: usize = 0;
const NUM_THREADS: usize = 1 << LOG_NUM_TREADS;
// we spread cache lines (2^6 = 64B) across four memory channels
const OWNER_SHIFT: usize = 15;

fn get_owner_thread(o: u64) -> usize {
    let mask = ((NUM_THREADS - 1) << OWNER_SHIFT) as u64;
    ((o & mask) >> OWNER_SHIFT) as usize
}

struct ForwardQueue {
    queue: UnsafeCell<Vec<Slot>>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Send for ForwardQueue {}
unsafe impl Sync for ForwardQueue {}

impl ForwardQueue {
    const LOG_SIZE: usize = if NUM_THREADS == 1 { 21 } else { 15 };
    fn new() -> ForwardQueue {
        ForwardQueue {
            queue: UnsafeCell::new(vec![Slot::from_raw(0 as _); 1 << Self::LOG_SIZE]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    const fn next(v: usize) -> usize {
        (v + 1) & ((1 << Self::LOG_SIZE) - 1)
    }

    fn is_empty(&self) -> bool {
        self.head.load(Ordering::Relaxed) == self.tail.load(Ordering::Relaxed)
    }

    fn enq(&self, slot: Slot) {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        if ForwardQueue::next(tail) != head {
            unsafe {
                (*self.queue.get())[tail] = slot;
            }
            self.tail.store(ForwardQueue::next(tail), Ordering::Relaxed);
        } else {
            panic!("Queue full");
        }
    }

    fn deq(&self) -> Option<Slot> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        if head == tail {
            None
        } else {
            let slot = unsafe { (*self.queue.get())[head] };
            self.head.store(ForwardQueue::next(head), Ordering::Relaxed);
            Some(slot)
        }
    }
}

#[derive(Default)]
struct PerWorkerState {
    slept: AtomicBool,
    monitor: (Mutex<()>, Condvar),
}

#[derive(Default)]
struct Counters {
    marked_objects: u64,
    slots: u64,
    non_empty_slots: u64,
}

impl Counters {
    fn reset(&mut self) {
        self.marked_objects = 0;
        self.slots = 0;
        self.non_empty_slots = 0;
    }

    fn flush(&mut self) {
        let global = &*GLOBAL;
        global.objs.fetch_add(self.marked_objects, Ordering::SeqCst);
        global.edges.fetch_add(self.slots, Ordering::SeqCst);
        global
            .ne_edges
            .fetch_add(self.non_empty_slots, Ordering::SeqCst);
    }
}

pub struct TracingWorker<O: ObjectModel> {
    id: usize,
    queue: VecDeque<Slot>,
    notify_threads: Vec<bool>,
    global: Arc<GlobalContext>,
    counters: Counters,
    slots: usize,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> TracingWorker<O> {
    fn sleep(&mut self) -> bool {
        // println!("Thread {} try lock", self.id);
        let mut lock = self.global.workers[self.id].monitor.0.lock().unwrap();
        // println!("Thread {} locked", self.id);
        loop {
            if self.global.gc_finished.load(Ordering::SeqCst) {
                // println!("Thread {} Exit", self.id);
                return true;
            }
            let recv_queues_empty = self.global.queues.iter().all(|r| r[self.id].is_empty());
            if !recv_queues_empty {
                break;
            }
            if !self.global.workers[self.id].slept.load(Ordering::SeqCst) {
                self.global.workers[self.id]
                    .slept
                    .store(true, Ordering::SeqCst);
                let old = self.global.yielded.fetch_add(1, Ordering::Relaxed);
                let ms = self.global.elapsed();
                println!(
                    "[{:.3}] Thread {} Sleep total={} slots={}",
                    ms,
                    self.id,
                    old + 1,
                    self.slots
                );
                self.slots = 0;
                if old + 1 == NUM_THREADS {
                    self.global.gc_finished.store(true, Ordering::SeqCst);
                    for i in 0..NUM_THREADS {
                        if i != self.id {
                            let _lock = self.global.workers[i].monitor.0.lock().unwrap();
                            self.global.workers[i].monitor.1.notify_one();
                        }
                    }
                    // println!("Thread {} Exit", self.id);
                    return true;
                }
            }
            // println!("Thread {} (wait start)", self.id);
            lock = self.global.workers[self.id].monitor.1.wait(lock).unwrap();
            // println!("Thread {} (wait end)", self.id);
        }
        if self.global.workers[self.id].slept.load(Ordering::SeqCst) {
            self.global.workers[self.id]
                .slept
                .store(false, Ordering::SeqCst);
            let _old = self.global.yielded.fetch_sub(1, Ordering::Relaxed);
            // println!("Thread {} Awake total={}", self.id, old - 1);
        }
        // println!("Thread {} unlock", self.id);
        false
    }

    fn process_slot(&mut self, slot: Slot, mark_sense: u8) {
        let o = slot.load().unwrap();
        debug_assert_eq!(get_owner_thread(o.raw()), self.id);
        self.slots += 1;
        // self.iter_slots += 1;
        if o.marked_relaxed(mark_sense) {
            self.counters.marked_objects += 1;
            o.scan::<O, _>(|s| {
                self.counters.slots += 1;
                let Some(child) = s.load() else {
                    return;
                };
                self.counters.non_empty_slots += 1;
                if child.is_marked(mark_sense) {
                    return;
                }
                let owner = get_owner_thread(child.raw());
                if owner == self.id {
                    self.queue.push_back(s);
                } else {
                    self.global.queues[self.id][owner].enq(s);
                    self.notify_threads[owner] = true;
                }
            })
        }
    }
}

impl<O: ObjectModel> crate::util::workers::Worker for TracingWorker<O> {
    type SharedWorker = ();

    fn new(id: usize, _group: Weak<WorkerGroup<Self>>) -> Self {
        Self {
            id,
            notify_threads: vec![false; GLOBAL.workers.len()],
            queue: VecDeque::with_capacity(1 << 15),
            global: GLOBAL.clone(),
            counters: Default::default(),
            slots: 0,
            _p: PhantomData,
        }
    }

    fn new_shared(&self) -> Self::SharedWorker {
        ()
    }

    fn run_epoch(&mut self) {
        self.counters.reset();
        let mark_state = self.global.mark_state();
        // scan roots
        let roots = unsafe { &*ROOTS.unwrap() };
        while let Some(mut range) = GLOBAL.root_segments.pop() {
            while let Some(root) = roots.get(range.start) {
                let slot = Slot::from_raw(root as *const u64 as *mut u64);
                if let Some(c) = slot.load() {
                    let owner = get_owner_thread(c.raw());
                    if owner == self.id {
                        self.queue.push_back(slot);
                    } else {
                        self.global.queues[self.id][owner].enq(slot);
                        self.notify_threads[owner] = true;
                    }
                } else {
                    self.counters.slots += 1;
                }
                range.start += 1;
            }
        }
        // trace objects
        loop {
            for i in 0..NUM_THREADS {
                while let Some(slot) = self.global.queues[i][self.id].deq() {
                    // self.queue.push_back(slot);
                    self.process_slot(slot, mark_state);
                }
            }

            while let Some(slot) = self.queue.pop_front() {
                self.process_slot(slot, mark_state);
            }

            for i in 0..NUM_THREADS {
                if !self.notify_threads[i] {
                    continue;
                }
                self.notify_threads[i] = false;
                if self.global.workers[i].slept.load(Ordering::SeqCst) {
                    let _lock = self.global.workers[i].monitor.0.lock().unwrap();
                    if self.global.workers[i].slept.load(Ordering::SeqCst) {
                        self.global.workers[i].monitor.1.notify_one();
                    }
                }
            }
            // let ms = self.state.start_time.elapsed().as_micros() as f64 / 1000.0;
            // println!("[{:.3}] thread terminate", ms,);

            // Terminate?
            if self.queue.is_empty() {
                let send_queues_empty = self.global.queues[self.id].iter().all(|q| q.is_empty());
                let recv_queues_empty = self.global.queues.iter().all(|r| r[self.id].is_empty());
                if recv_queues_empty && send_queues_empty {
                    if self.sleep() {
                        break;
                    }
                }
            }
        }

        self.counters.flush();
    }
}

struct GlobalContext {
    root_segments: SegQueue<Range<usize>>,
    mark_state: AtomicU8,
    objs: AtomicU64,
    edges: AtomicU64,
    ne_edges: AtomicU64,
    queues: Vec<Vec<ForwardQueue>>,
    yielded: AtomicUsize,
    gc_finished: AtomicBool,
    workers: [PerWorkerState; NUM_THREADS],
    start_time: UnsafeCell<Instant>,
}

impl GlobalContext {
    fn new() -> Self {
        Self {
            root_segments: SegQueue::new(),
            mark_state: AtomicU8::new(0),
            objs: AtomicU64::new(0),
            edges: AtomicU64::new(0),
            ne_edges: AtomicU64::new(0),
            queues: (0..NUM_THREADS)
                .map(|_| (0..NUM_THREADS).map(|_| ForwardQueue::new()).collect())
                .collect(),
            yielded: AtomicUsize::new(0),
            gc_finished: AtomicBool::new(false),
            workers: Default::default(),
            start_time: UnsafeCell::new(Instant::now()),
        }
    }

    fn elapsed(&self) -> f64 {
        unsafe { *self.start_time.get() }.elapsed().as_micros() as f64 / 1000.0
    }

    pub fn mark_state(&self) -> u8 {
        self.mark_state.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
        for i in 0..NUM_THREADS {
            GLOBAL.workers[i].slept.store(false, Ordering::SeqCst);
        }
        self.gc_finished.store(false, Ordering::SeqCst);
        self.yielded.store(0, Ordering::SeqCst);
        unsafe { *GLOBAL.start_time.get() = Instant::now() };
    }
}

unsafe impl Send for GlobalContext {}
unsafe impl Sync for GlobalContext {}

static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

static mut ROOTS: Option<*const [u64]> = None;

struct DistEdgeSlotTracer<O: ObjectModel> {
    group: Arc<WorkerGroup<TracingWorker<O>>>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for DistEdgeSlotTracer<O> {
    fn startup(&self) {
        info!("Use {} worker threads.", self.group.workers.len());
        self.group.spawn();
    }

    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats {
        GLOBAL.reset();
        GLOBAL.mark_state.store(mark_sense, Ordering::SeqCst);
        // Create initial root scanning tasks
        let roots = object_model.roots();
        let roots_len = roots.len();
        unsafe { ROOTS = Some(roots) };
        let num_segments = self.group.workers.len() * 2;
        for id in 0..num_segments {
            let range = (roots_len * id) / num_segments..(roots_len * (id + 1)) / num_segments;
            GLOBAL.root_segments.push(range);
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

impl<O: ObjectModel> DistEdgeSlotTracer<O> {
    pub fn new(num_workers: usize) -> Self {
        Self {
            group: WorkerGroup::new(num_workers),
            _p: PhantomData,
        }
    }
}

pub fn create_tracer<O: ObjectModel>(_args: &TraceArgs) -> Box<dyn Tracer<O>> {
    Box::new(DistEdgeSlotTracer::<O>::new(NUM_THREADS))
}
