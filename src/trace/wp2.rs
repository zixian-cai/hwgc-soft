use crossbeam::deque::{Steal, Stealer, Worker};
use once_cell::sync::Lazy;

use super::TracingStats;
use crate::util::typed_obj::Slot;
use crate::ObjectModel;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Barrier, Condvar, Mutex,
};

const N_WORKERS: usize = 32;

static CVAR: Condvar = Condvar::new();
static LOCK: Mutex<usize> = Mutex::new(0);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static BARRIER: Lazy<Barrier> = Lazy::new(|| Barrier::new(N_WORKERS));

static mut MARK_STATE: u8 = 0;
static mut STEALERS: Vec<Stealer<Slot>> = Vec::new();
static mut ROOTS: Option<*const [u64]> = None;

fn run_worker<O: ObjectModel>(id: usize, local: &Worker<Slot>, stealers: &[Stealer<Slot>]) {
    // scan roots
    if let Some(roots) = unsafe { ROOTS } {
        let roots = unsafe { &*roots };
        let range = (roots.len() * id) / N_WORKERS..(roots.len() * (id + 1)) / N_WORKERS;
        for root in &roots[range] {
            let slot = Slot::from_raw(root as *const u64 as *mut u64);
            local.push(slot);
        }
    }
    BARRIER.wait();
    // trace objects
    let (mut mo, mut s, mut nes) = (0, 0, 0);
    let mut process_slot = |slot: Slot| {
        s += 1;
        if let Some(o) = slot.load() {
            if o.mark(unsafe { MARK_STATE }) {
                mo += 1;
                o.scan::<O, _>(|s| local.push(s));
            }
        } else {
            nes += 1;
        }
    };
    'outer: loop {
        // Drain local queue
        while let Some(slot) = local.pop() {
            process_slot(slot);
        }
        // Steal from other workers
        let mut retry = false;
        for stealer in stealers {
            match stealer.steal_batch_and_pop(local) {
                Steal::Success(slot) => {
                    process_slot(slot);
                    continue 'outer;
                }
                Steal::Retry => {
                    retry = true;
                    continue;
                }
                _ => {}
            }
        }
        if retry {
            continue;
        }
        break;
    }
    assert!(local.is_empty());
    SLOTS.fetch_add(s, Ordering::SeqCst);
    MARKED_OBJECTS.fetch_add(mo, Ordering::SeqCst);
    NON_EMPTY_SLOTS.fetch_add(nes, Ordering::SeqCst);
}

pub fn prologue<O: ObjectModel>() {
    let mut workers = vec![];
    for _ in 0..N_WORKERS {
        let worker = Worker::<Slot>::new_lifo();
        unsafe { STEALERS.push(worker.stealer()) };
        workers.push(worker);
    }
    let mut handles = vec![];
    for (i, w) in workers.into_iter().enumerate() {
        let stealer = unsafe { &STEALERS };
        let handle = std::thread::spawn(move || loop {
            // wait for request
            {
                let mut epoch = LOCK.lock().unwrap();
                while *epoch == EPOCH.load(Ordering::SeqCst) {
                    epoch = CVAR.wait(epoch).unwrap();
                }
            }
            // Do GC
            run_worker::<O>(i, &w, stealer);
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
        shape_cache_stats: Default::default(),
    }
}

static MARKED_OBJECTS: AtomicU64 = AtomicU64::new(0);
static SLOTS: AtomicU64 = AtomicU64::new(0);
static NON_EMPTY_SLOTS: AtomicU64 = AtomicU64::new(0);
