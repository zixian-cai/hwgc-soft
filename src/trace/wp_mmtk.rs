use crossbeam::deque::{Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use wp::Slot;

use super::TracingStats;
use crate::util::ObjectOps;
use crate::ObjectModel;
use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier, Condvar, Mutex,
    },
};

const N_WORKERS: usize = 32;

static CVAR: Condvar = Condvar::new();
static LOCK: Mutex<usize> = Mutex::new(0);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static BARRIER: Lazy<Barrier> = Lazy::new(|| Barrier::new(N_WORKERS));

static mut MARK_STATE: u8 = 0;
static mut ROOTS: Option<*const [u64]> = None;

#[thread_local]
static mut LOCAL_OBJS: u64 = 0;
#[thread_local]
static mut LOCAL_EDGES: u64 = 0;
#[thread_local]
static mut LOCAL_NE_EDGES: u64 = 0;

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

    fn run(&mut self, local: &Worker<TracePacket<O>>) {
        let slots = std::mem::take(&mut self.slots);
        for slot in slots {
            unsafe { LOCAL_EDGES += 1 };
            if let Some(o) = slot.load() {
                if o.mark(unsafe { MARK_STATE }) {
                    unsafe { LOCAL_OBJS += 1 };
                    o.scan_object::<O, _>(|s| {
                        if self.next_slots.is_empty() {
                            self.next_slots.reserve(Self::CAP);
                        }
                        self.next_slots.push(s);
                        if self.next_slots.len() >= Self::CAP {
                            self.flush(&local);
                        }
                    });
                }
            } else {
                unsafe { LOCAL_NE_EDGES += 1 };
            }
        }
        self.flush(&local);
    }
}

fn run_worker<O: ObjectModel>(
    id: usize,
    local: &Worker<TracePacket<O>>,
    stealers: &[Stealer<TracePacket<O>>],
) {
    unsafe {
        LOCAL_NE_EDGES = 0;
        LOCAL_EDGES = 0;
        LOCAL_OBJS = 0
    };
    // scan roots
    if let Some(roots) = unsafe { ROOTS } {
        let roots = unsafe { &*roots };
        let range = (roots.len() * id) / N_WORKERS..(roots.len() * (id + 1)) / N_WORKERS;
        let mut buf = vec![];
        for root in &roots[range] {
            let slot = Slot(root as *const u64 as *mut u64);
            if buf.is_empty() {
                buf.reserve(TracePacket::<O>::CAP);
            }
            buf.push(slot);
            if buf.len() >= TracePacket::<O>::CAP {
                let packet = TracePacket::<O>::new(buf);
                local.push(packet);
                buf = vec![];
            }
        }
        if !buf.is_empty() {
            let packet = TracePacket::<O>::new(buf);
            local.push(packet);
        }
    }
    BARRIER.wait();
    // trace objects
    'outer: loop {
        // Drain local queue
        while let Some(mut p) = local.pop() {
            p.run(local);
        }
        // Steal from other workers
        for stealer in stealers {
            match stealer.steal() {
                Steal::Success(mut p) => {
                    p.run(local);
                    continue 'outer;
                }
                Steal::Retry => continue 'outer,
                _ => {}
            }
        }
        break;
    }
    assert!(local.is_empty());
    SLOTS.fetch_add(unsafe { LOCAL_EDGES }, Ordering::SeqCst);
    MARKED_OBJECTS.fetch_add(unsafe { LOCAL_OBJS }, Ordering::SeqCst);
    NON_EMPTY_SLOTS.fetch_add(unsafe { LOCAL_NE_EDGES }, Ordering::SeqCst);
}

pub fn prologue<O: ObjectModel>() {
    let mut workers = vec![];
    let mut stealers = vec![];
    for _ in 0..N_WORKERS {
        let worker = Worker::<TracePacket<O>>::new_lifo();
        stealers.push(worker.stealer());
        workers.push(worker);
    }
    let stealer_arc = Arc::new(stealers);
    let mut handles = vec![];
    for (i, w) in workers.into_iter().enumerate() {
        let stealers = stealer_arc.clone();
        let handle = std::thread::spawn(move || loop {
            // wait for request
            {
                let mut epoch = LOCK.lock().unwrap();
                while *epoch == EPOCH.load(Ordering::SeqCst) {
                    epoch = CVAR.wait(epoch).unwrap();
                }
            }
            // Do GC
            run_worker::<O>(i, &w, &stealers);
            // Update epoch
            {
                if BARRIER.wait().is_leader() {
                    let mut epoch = LOCK.lock().unwrap();
                    *epoch = EPOCH.load(Ordering::SeqCst);
                    CVAR.notify_all();
                }
                BARRIER.wait();
            }
        });
        handles.push(handle);
    }
}

pub(super) unsafe fn transitive_closure<O: ObjectModel>(
    mark_sense: u8,
    object_model: &O,
) -> TracingStats {
    unsafe { MARK_STATE = mark_sense };
    MARKED_OBJECTS.store(0, Ordering::SeqCst);
    SLOTS.store(0, Ordering::SeqCst);
    NON_EMPTY_SLOTS.store(0, Ordering::SeqCst);
    // Get roots
    unsafe { ROOTS = Some(object_model.roots()) };
    // Wake up workers
    let mut epoch = LOCK.lock().unwrap();
    EPOCH.fetch_add(1, Ordering::SeqCst);
    CVAR.notify_all();
    while *epoch != EPOCH.load(Ordering::SeqCst) {
        epoch = CVAR.wait(epoch).unwrap();
    }
    TracingStats {
        marked_objects: MARKED_OBJECTS.load(Ordering::SeqCst),
        slots: SLOTS.load(Ordering::SeqCst),
        non_empty_slots: NON_EMPTY_SLOTS.load(Ordering::SeqCst),
        sends: 0,
    }
}

static MARKED_OBJECTS: AtomicU64 = AtomicU64::new(0);
static SLOTS: AtomicU64 = AtomicU64::new(0);
static NON_EMPTY_SLOTS: AtomicU64 = AtomicU64::new(0);
