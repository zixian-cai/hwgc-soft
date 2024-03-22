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
};

type DistGCMsg = u64;

static MARKED_OBJECTS: AtomicU64 = AtomicU64::new(0);
static SLOTS: AtomicU64 = AtomicU64::new(0);
static NON_EMPTY_SLOTS: AtomicU64 = AtomicU64::new(0);
static SENDS: AtomicU64 = AtomicU64::new(0);
static PARKED_THREADS: AtomicUsize = AtomicUsize::new(0);

const LOG_NUM_TREADS: usize = 5;
const NUM_THREADS: usize = 1 << LOG_NUM_TREADS;
// we spread cache lines (2^6 = 64B) across four memory channels
const OWNER_SHIFT: usize = 22;

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
    fn new() -> ForwardQueue {
        ForwardQueue {
            queue: UnsafeCell::new(vec![Slot::from_raw(0 as _); 1 << 15]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn next(v: usize) -> usize {
        (v + 1) & ((1 << 15) - 1)
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
struct State {
    yielded: AtomicUsize,
    monitor: (Mutex<()>, Condvar),
    workers: [PerWorkerState; NUM_THREADS],
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
        }
    }
    unsafe fn run<O>(&mut self, mark_sense: u8)
    where
        O: ObjectModel,
    {
        // info!("Thread {} started", self.id);
        loop {
            // if self.scan_queue.len() != 0 {
            // println!(
            //     "Thread {} A {} {}",
            //     self.id,
            //     self.scan_queue.len(),
            //     self.counters.marked_objects
            // );
            // }
            while let Some(slot) = self.scan_queue.pop_front() {
                let o = slot.load().unwrap();
                debug_assert_eq!(get_owner_thread(o.raw()), self.id);
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
                            // if self.state.workers[owner].slept.load(Ordering::Relaxed) {
                            //     self.state.workers[owner].slept.store(false, Ordering::Relaxed);
                            //     // self.state.workers[owner].monitor.1.notify_one();
                            // }
                            // self.state.workers[owner].monitor.1.notify_one();
                        }
                    })
                }
            }
            // println!("Thread {} B {}", self.id, self.scan_queue.len());
            // Drain forward queues
            for i in 0..NUM_THREADS {
                if i == self.id {
                    continue;
                }
                while let Some(child) = self.queues[i][self.id].deq() {
                    self.scan_queue.push_back(child);
                }
            }
            std::thread::yield_now();
            // if self.scan_queue.len() != 0 {
            // println!("Thread {} C {}", self.id, self.scan_queue.len());
            // }
            // Terminate?
            if self.scan_queue.is_empty() {
                // let all_sending_queues_empty = self.queues[self.id].iter().all(|q| q.is_empty());
                // let all_receive_queues_empty =
                //     self.queues.iter().all(|row| row[self.id].is_empty());
                // println!(
                //     "Thread {} D sending_queues={:?}",
                //     self.id,
                //     self.queues[self.id]
                //         .iter()
                //         .enumerate()
                //         .filter(|(i, q)| !q.is_empty())
                //         .map(|(i, q)| i)
                //         .collect::<Vec<_>>()
                // );
                // println!(
                //     "Thread {} D receive_queues={:?}",
                //     self.id,
                //     self.queues
                //         .iter()
                //         .enumerate()
                //         .filter(|(i, r)| !r[self.id].is_empty())
                //         .map(|(i, q)| i)
                //         .collect::<Vec<_>>()
                // );
                // if all_sending_queues_empty && all_receive_queues_empty {
                //     // println!("Thread {} terminated", self.id);
                //     break;
                // }
                // let mut yielded = self.state.yielded.fetch_add(1, Ordering::Relaxed);
                // self.state.workers[self.id]
                //     .slept
                //     .store(true, Ordering::SeqCst);
                // let mut guard = self.state.workers[self.id].monitor.0.lock().unwrap();
                // loop {
                //     guard = self.state.workers[self.id].monitor.1.wait(guard);
                //     if self.scan_queue.is_empty()
                //         || self.queues.iter().all(|row| row[self.id].is_empty())
                //     {
                //         break;
                //     }
                // }
                // self.state.workers[self.id]
                //     .slept
                //     .store(false, Ordering::SeqCst);
                // self.state.yielded.fetch_sub(1, Ordering::Relaxed);

                // let mut g = self.state.monitor.0.lock().unwrap();
                // let mut guard = self.state.workers[self.id].monitor.0.lock().unwrap();
                // self.state.workers[self.id]
                //     .slept
                //     .store(true, Ordering::SeqCst);
                // let yielded = self.state.yielded.fetch_add(1, Ordering::SeqCst);
                // if yielded == NUM_THREADS - 1 {
                //     //
                //     // println!("Thread {} Terminate", self.id);
                //     for i in 0..NUM_THREADS {
                //         self.state.workers[i].monitor.1.notify_one();
                //     }
                //     return;
                // }
                // std::mem::drop(g);
                // // println!("Thread {} Sleep", self.id);
                // // loop {
                // // println!("Thread {} w", self.id);
                // guard = self.state.workers[self.id].monitor.1.wait(guard).unwrap();
                // // println!("Thread {} s", self.id);
                // // if self.state.yielded.load(Ordering::SeqCst) == NUM_THREADS {
                // //     println!("Thread {} Terminate", self.id);
                // //     return;
                // // }

                // // println!(
                // //     "Thread {} receive_queues={:?}",
                // //     self.id,
                // //     self.queues
                // //         .iter()
                // //         .enumerate()
                // //         .filter(|(i, r)| !r[self.id].is_empty())
                // //         .map(|(i, q)| i)
                // //         .collect::<Vec<_>>()
                // // );
                // // if self.queues.iter().all(|row| row[self.id].is_empty()) {
                // //     break;
                // // }
                // // }
                // if self.state.yielded.load(Ordering::SeqCst) == NUM_THREADS {
                //     // println!("Thread {} Terminate", self.id);
                //     return;
                // }
                // // println!("Thread {} Awake", self.id);
                // self.state.workers[self.id]
                //     .slept
                //     .store(false, Ordering::SeqCst);
                // self.state.yielded.fetch_sub(1, Ordering::Relaxed);

                if !self.state.workers[self.id].slept.load(Ordering::Relaxed) {
                    self.state.workers[self.id]
                        .slept
                        .store(true, Ordering::SeqCst);
                    let yielded = self.state.yielded.fetch_add(1, Ordering::SeqCst);
                    if yielded == NUM_THREADS - 1 {
                        // println!("Thread {} Terminate", self.id);
                        return;
                    }
                    // println!("Thread {} Sleep {}", self.id, yielded);
                } else {
                    if self.state.yielded.load(Ordering::SeqCst) == NUM_THREADS {
                        // println!("Thread {} Terminate", self.id);
                        return;
                    }
                }
            } else {
                // self.state.workers[self.id]
                //     .slept
                //     .store(false, Ordering::SeqCst);

                if self.state.workers[self.id].slept.load(Ordering::Relaxed) {
                    self.state.workers[self.id]
                        .slept
                        .store(false, Ordering::SeqCst);
                    self.state.yielded.fetch_sub(1, Ordering::SeqCst);
                }
            }
            std::thread::yield_now();
        }
    }
}

pub(super) unsafe fn transitive_closure_distributed_node_objref<O: ObjectModel>(
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
            if owner == 0 {
                queues[1][owner].enq(Slot::from_raw(root as *const u64 as *mut u64));
            } else {
                queues[0][owner].enq(Slot::from_raw(root as *const u64 as *mut u64));
            }
        }
    }

    // let thread_join_handles: Vec<std::thread::JoinHandle<_>> = threads
    //     .iter_mut()
    //     .map(|t| std::thread::spawn(move || t.run::<O>(mark_sense)))
    //     .collect();

    std::thread::scope(|s| {
        for t in threads.iter_mut() {
            s.spawn(|| t.run::<O>(mark_sense));
        }
    });

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
