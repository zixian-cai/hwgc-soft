use super::{trace_object, TracingStats};
use crate::{
    util::typed_obj::{Object, Slot},
    ObjectModel,
};
use crossbeam::channel::{unbounded, Receiver, Sender};
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier, Condvar, Mutex,
    },
    time::Instant,
};

type DistGCMsg = u64;

static MARKED_OBJECTS: AtomicU64 = AtomicU64::new(0);
static SLOTS: AtomicU64 = AtomicU64::new(0);
static NON_EMPTY_SLOTS: AtomicU64 = AtomicU64::new(0);
static SENDS: AtomicU64 = AtomicU64::new(0);
static PARKED_THREADS: AtomicUsize = AtomicUsize::new(0);

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

struct State {
    yielded: AtomicUsize,
    gc_finished: AtomicBool,
    workers: [PerWorkerState; NUM_THREADS],
    start_time: Instant,
}

impl Default for State {
    fn default() -> Self {
        Self {
            yielded: AtomicUsize::new(0),
            gc_finished: AtomicBool::new(false),
            workers: Default::default(),
            start_time: Instant::now(),
        }
    }
}

#[derive(Default)]
struct Counters {
    marked_objects: u64,
    slots: u64,
    non_empty_slots: u64,
}

struct DistGCThread {
    id: usize,
    // receiver: Receiver<DistGCMsg>,
    // senders: Vec<Sender<DistGCMsg>>,
    queues: Arc<Vec<Vec<ForwardQueue>>>,
    scan_queue: VecDeque<Slot>,
    counters: Counters,
    state: Arc<State>,
    slots: u64,
    iter_slots: u64,
}

impl DistGCThread {
    fn new(id: usize, queues: Arc<Vec<Vec<ForwardQueue>>>, state: Arc<State>) -> DistGCThread {
        DistGCThread {
            id,
            queues,
            state,
            // receiver,
            // senders: senders.to_vec(),
            scan_queue: VecDeque::new(),
            counters: Counters::default(),
            slots: 0,
            iter_slots: 0,
        }
    }

    fn sleep(&mut self) -> bool {
        // println!("Thread {} try lock", self.id);
        let mut lock = self.state.workers[self.id].monitor.0.lock().unwrap();
        // println!("Thread {} locked", self.id);
        loop {
            if self.state.gc_finished.load(Ordering::SeqCst) {
                // println!("Thread {} Exit", self.id);
                return true;
            }
            let recv_queues_empty = self.queues.iter().all(|r| r[self.id].is_empty());
            if !recv_queues_empty {
                break;
            }
            if !self.state.workers[self.id].slept.load(Ordering::SeqCst) {
                self.state.workers[self.id]
                    .slept
                    .store(true, Ordering::SeqCst);
                let old = self.state.yielded.fetch_add(1, Ordering::Relaxed);
                let ms = self.state.start_time.elapsed().as_micros() as f64 / 1000.0;
                println!(
                    "[{:.3}] Thread {} Sleep total={} slots={}",
                    ms,
                    self.id,
                    old + 1,
                    self.slots
                );
                self.slots = 0;
                if old + 1 == NUM_THREADS {
                    self.state.gc_finished.store(true, Ordering::SeqCst);
                    for i in 0..NUM_THREADS {
                        if i != self.id {
                            let _lock = self.state.workers[i].monitor.0.lock().unwrap();
                            self.state.workers[i].monitor.1.notify_one();
                        }
                    }
                    // println!("Thread {} Exit", self.id);
                    return true;
                }
            }
            // println!("Thread {} (wait start)", self.id);
            lock = self.state.workers[self.id].monitor.1.wait(lock).unwrap();
            // println!("Thread {} (wait end)", self.id);
        }
        if self.state.workers[self.id].slept.load(Ordering::SeqCst) {
            self.state.workers[self.id]
                .slept
                .store(false, Ordering::SeqCst);
            let old = self.state.yielded.fetch_sub(1, Ordering::Relaxed);
            // println!("Thread {} Awake total={}", self.id, old - 1);
        }
        // println!("Thread {} unlock", self.id);
        false
    }

    unsafe fn trace_object<O: ObjectModel>(
        &mut self,
        o: Object,
        mark_sense: u8,
        notify_threads: &mut Vec<bool>,
    ) {
        if trace_object(o.raw(), mark_sense) {
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
                    self.scan_queue.push_back(s);
                } else {
                    self.queues[self.id][owner].enq(s);
                    notify_threads[owner] = true;
                }
            })
        }
    }

    unsafe fn process_slot<O: ObjectModel>(
        &mut self,
        slot: Slot,
        mark_sense: u8,
        notify_threads: &mut Vec<bool>,
    ) {
        let o = slot.load().unwrap();
        debug_assert_eq!(get_owner_thread(o.raw()), self.id);
        self.slots += 1;
        self.iter_slots += 1;
        self.trace_object::<O>(o, mark_sense, notify_threads);
    }

    unsafe fn run<O>(&mut self, mark_sense: u8)
    where
        O: ObjectModel,
    {
        // let ms = self.state.start_time.elapsed().as_micros() as f64 / 1000.0;
        // println!("[{:.3}] Thread {} Start", ms, self.id);
        // info!("Thread {} started", self.id);
        let mut notify_threads = vec![false; NUM_THREADS];
        loop {
            self.iter_slots = 0;

            for i in 0..NUM_THREADS {
                // let q = &self.queues[i][self.id];
                while let Some(slot) = self.queues[i][self.id].deq() {
                    // self.scan_queue.push_back(slot);
                    self.process_slot::<O>(slot, mark_sense, &mut notify_threads);
                }
            }

            while let Some(slot) = self.scan_queue.pop_front() {
                self.process_slot::<O>(slot, mark_sense, &mut notify_threads);
            }

            for i in 0..NUM_THREADS {
                if !notify_threads[i] {
                    continue;
                }
                notify_threads[i] = false;
                if self.state.workers[i].slept.load(Ordering::SeqCst) {
                    let _lock = self.state.workers[i].monitor.0.lock().unwrap();
                    if self.state.workers[i].slept.load(Ordering::SeqCst) {
                        self.state.workers[i].monitor.1.notify_one();
                    }
                }
            }
            // let ms = self.state.start_time.elapsed().as_micros() as f64 / 1000.0;
            // println!("[{:.3}] thread terminate", ms,);

            // Terminate?
            if self.scan_queue.is_empty() {
                let send_queues_empty = self.queues[self.id].iter().all(|q| q.is_empty());
                let recv_queues_empty = self.queues.iter().all(|r| r[self.id].is_empty());
                if recv_queues_empty && send_queues_empty {
                    if self.sleep() {
                        return;
                    }
                }
            }
        }
    }
}

pub(super) unsafe fn transitive_closure_distributed_node_objref_impl<O: ObjectModel>(
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    // Node-ObjRef enqueuing
    MARKED_OBJECTS.store(0, Ordering::SeqCst);
    SLOTS.store(0, Ordering::SeqCst);
    NON_EMPTY_SLOTS.store(0, Ordering::SeqCst);
    SENDS.store(0, Ordering::SeqCst);

    // let mut senders: Vec<Sender<DistGCMsg>> = vec![];
    // let mut receivers: Vec<Receiver<DistGCMsg>> = vec![];
    let mut queues: Vec<Vec<ForwardQueue>> = vec![];

    for i in 0..NUM_THREADS {
        let mut row = vec![];
        for j in 0..NUM_THREADS {
            row.push(ForwardQueue::new());
        }
        queues.push(row);
    }
    let queues = Arc::new(queues);
    let state = Arc::new(State::default());

    let mut threads = (0..NUM_THREADS)
        .map(|id| DistGCThread::new(id, queues.clone(), Arc::clone(&state)))
        .collect::<Vec<_>>();

    let ms = state.start_time.elapsed().as_micros() as f64 / 1000.0;
    println!("[{:.3}] roots start", ms);
    for root in object_model.roots() {
        let o = *root;
        if cfg!(feature = "detailed_stats") {
            SLOTS.fetch_add(1, Ordering::Relaxed);
            if o != 0 {
                NON_EMPTY_SLOTS.fetch_add(1, Ordering::Relaxed);
            }
        }
        if o != 0 {
            let owner = get_owner_thread(o);
            queues[0][owner].enq(Slot::from_raw(root as *const u64 as *mut u64));
        }
    }
    let ms = state.start_time.elapsed().as_micros() as f64 / 1000.0;
    println!("[{:.3}] tracing start", ms);

    // let thread_join_handles: Vec<std::thread::JoinHandle<_>> = threads
    //     .iter_mut()
    //     .map(|t| std::thread::spawn(move || t.run::<O>(mark_sense)))
    //     .collect();

    std::thread::scope(|s| {
        for t in threads.iter_mut() {
            s.spawn(|| t.run::<O>(mark_sense));
        }
    });

    let ms = state.start_time.elapsed().as_micros() as f64 / 1000.0;
    println!("[{:.3}] tracing finish", ms);

    // for h in thread_join_handles {
    //     h.join().unwrap();
    // }

    // let sends = threads.iter().map(|t| t.counters.marked_objects);
    let marked_objects = threads.iter().map(|t| t.counters.marked_objects).sum();
    let slots = threads.iter().map(|t| t.counters.slots).sum();
    let non_empty_slots = threads.iter().map(|t| t.counters.non_empty_slots).sum();

    TracingStats {
        marked_objects,
        slots,
        non_empty_slots,
        sends: 0,
        ..Default::default()
    }
}

pub(super) unsafe fn transitive_closure_distributed_node_objref<O: ObjectModel>(
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    let t = Instant::now();
    let r = transitive_closure_distributed_node_objref_impl(mark_sense, object_model);
    let ms = t.elapsed().as_micros() as f64 / 1000.0;
    println!("[{:.3}] all finish", ms);
    r
}
