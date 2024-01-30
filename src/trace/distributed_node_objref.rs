use super::{trace_object, TracingStats};
use crate::ObjectModel;
use crossbeam::channel::{unbounded, Receiver, Sender};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier,
    },
};

type DistGCMsg = u64;

static MARKED_OBJECTS: AtomicU64 = AtomicU64::new(0);
static SLOTS: AtomicU64 = AtomicU64::new(0);
static NON_EMPTY_SLOTS: AtomicU64 = AtomicU64::new(0);
static SENDS: AtomicU64 = AtomicU64::new(0);
static PARKED_THREADS: AtomicUsize = AtomicUsize::new(0);

const LOG_NUM_TREADS: usize = 3;
const NUM_THREADS: usize = 1 << LOG_NUM_TREADS;
// we spread cache lines (2^6 = 64B) across four memory channels
const OWNER_SHIFT: usize = 6;

fn get_owner_thread(o: u64) -> usize {
    let mask = ((NUM_THREADS - 1) << OWNER_SHIFT) as u64;
    ((o & mask) >> OWNER_SHIFT) as usize
}

struct DistGCThread {
    id: usize,
    receiver: Receiver<DistGCMsg>,
    senders: Vec<Sender<DistGCMsg>>,
    scan_queue: VecDeque<u64>,
    barrier: Arc<Barrier>,
}

impl DistGCThread {
    fn new(
        id: usize,
        receiver: Receiver<DistGCMsg>,
        senders: &[Sender<DistGCMsg>],
        barrier: Arc<Barrier>,
    ) -> DistGCThread {
        DistGCThread {
            id,
            receiver,
            senders: senders.to_vec(),
            scan_queue: VecDeque::new(),
            barrier,
        }
    }

    unsafe fn run<O>(&mut self, mark_sense: u8)
    where
        O: ObjectModel,
    {
        info!("Thread {} started", self.id);
        loop {
            while let Some(o) = self.scan_queue.pop_front() {
                debug_assert_eq!(get_owner_thread(o), self.id);
                O::scan_object(o, |edge| {
                    let child = *edge;
                    if cfg!(feature = "detailed_stats") {
                        SLOTS.fetch_add(1, Ordering::Relaxed);
                    }
                    if child != 0 {
                        if cfg!(feature = "detailed_stats") {
                            NON_EMPTY_SLOTS.fetch_add(1, Ordering::Relaxed);
                        }
                        let owner = get_owner_thread(child);
                        if owner == self.id {
                            if trace_object(child, mark_sense) {
                                if cfg!(feature = "detailed_stats") {
                                    MARKED_OBJECTS.fetch_add(1, Ordering::Relaxed);
                                }
                                self.scan_queue.push_back(child);
                            }
                        } else {
                            // trace!("{} -> {} {}", self.id, owner, child);
                            if cfg!(feature = "detailed_stats") {
                                SENDS.fetch_add(1, Ordering::Relaxed);
                            }
                            self.senders[owner].send(child).unwrap();
                        }
                    }
                });
            }
            if self.receiver.is_empty() {
                info!("Thread {} entering barrier", self.id);
                self.barrier.wait();
                if self.receiver.is_empty() {
                    PARKED_THREADS.fetch_add(1, Ordering::SeqCst);
                }
                let wait = self.barrier.wait();
                if PARKED_THREADS.load(Ordering::SeqCst) == NUM_THREADS {
                    info!("Thread {} exiting", self.id);
                    break;
                }
                if wait.is_leader() {
                    // Leader reset the counter
                    PARKED_THREADS.store(0, Ordering::SeqCst);
                } else {
                    // Others wait for the counter to be reset
                    while PARKED_THREADS.load(Ordering::SeqCst) != 0 {}
                }
            } else {
                let child = self.receiver.recv().unwrap();
                if trace_object(child, mark_sense) {
                    if cfg!(feature = "detailed_stats") {
                        MARKED_OBJECTS.fetch_add(1, Ordering::Relaxed);
                    }
                    self.scan_queue.push_back(child);
                }
            }
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

    let mut senders: Vec<Sender<DistGCMsg>> = vec![];
    let mut receivers: Vec<Receiver<DistGCMsg>> = vec![];

    for _ in 0..NUM_THREADS {
        let (s, r) = unbounded();
        senders.push(s);
        receivers.push(r);
    }
    let barrier = Arc::new(Barrier::new(NUM_THREADS));

    let threads = receivers
        .into_iter()
        .enumerate()
        .map(|(id, r)| DistGCThread::new(id, r, &senders, Arc::clone(&barrier)));

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
            senders[owner].send(o).unwrap();
        }
    }

    let thread_join_handles: Vec<std::thread::JoinHandle<_>> = threads
        .map(|mut t| std::thread::spawn(move || t.run::<O>(mark_sense)))
        .collect();

    for h in thread_join_handles {
        h.join().unwrap();
    }

    let sends = SENDS.load(Ordering::SeqCst);
    let marked_objects = MARKED_OBJECTS.load(Ordering::SeqCst);
    let slots = SLOTS.load(Ordering::SeqCst);
    let non_empty_slots = NON_EMPTY_SLOTS.load(Ordering::SeqCst);

    TracingStats {
        marked_objects,
        slots,
        non_empty_slots,
        sends,
    }
}
