use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use wp::{Object, Slot};

use super::{trace_object, trace_object_atomic, TracingStats};
use crate::{ObjectModel, OpenJDKObjectModel};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Barrier, Condvar, Mutex,
};

type O = OpenJDKObjectModel<false>;

trait ObjectOps {
    fn get(&self) -> u64;
    fn scan_object<F: FnMut(Slot)>(&self, mut f: F) {
        O::scan_object(self.get(), |edge, repeat| {
            for i in 0..repeat {
                let ptr = edge.wrapping_add(i as usize);
                f(Slot(ptr));
            }
        })
    }
    fn mark(&self) -> bool {
        trace_object_atomic(self.get(), unsafe { MARK_STATE })
    }
}

impl ObjectOps for Object {
    fn get(&self) -> u64 {
        self.0
    }
}

// #[thread_local]
// pub static MARK_QUEUE: wp::LocalQueue<Slot> = wp::LocalQueue::new(|slot| {
//     // for slot in slots {
//     SLOTS.fetch_add(1, Ordering::SeqCst);
//     if let Some(o) = slot.load() {
//         if o.mark() {
//             MARKED_OBJECTS.fetch_add(1, Ordering::SeqCst);
//             o.scan_object(|s| MARK_QUEUE.push(s));
//         }
//     } else {
//         NON_EMPTY_SLOTS.fetch_add(1, Ordering::SeqCst);
//     }
//     // }
// });

fn run_worker(local: &Worker<Slot>, stealers: &[Stealer<Slot>]) {
    let (mut mo, mut s, mut nes) = (0, 0, 0);
    let mut process_slot = |slot: Slot| {
        s += 1;
        if let Some(o) = slot.load() {
            if o.mark() {
                mo += 1;
                o.scan_object(|s| local.push(s));
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
        for stealer in stealers {
            match stealer.steal() {
                Steal::Success(slot) => {
                    process_slot(slot);
                    continue 'outer;
                }
                Steal::Retry => continue 'outer,
                _ => {}
            }
        }
        // Steal from global
        match INJECTOR.steal_batch_and_pop(&local) {
            Steal::Success(slot) => {
                process_slot(slot);
                continue 'outer;
            }
            Steal::Retry => continue 'outer,
            _ => {}
        }
        break;
    }
    assert!(local.is_empty());
    SLOTS.fetch_add(s, Ordering::SeqCst);
    MARKED_OBJECTS.fetch_add(mo, Ordering::SeqCst);
    NON_EMPTY_SLOTS.fetch_add(nes, Ordering::SeqCst);
}

static mut MARK_STATE: u8 = 0;
static mut STEALERS: Vec<Stealer<Slot>> = Vec::new();
static INJECTOR: Lazy<Injector<Slot>> = Lazy::new(|| Injector::new());

const N_WORKERS: usize = 2;
static CVAR: Condvar = Condvar::new();
static LOCK: Mutex<usize> = Mutex::new(0);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static BARRIER: Lazy<Barrier> = Lazy::new(|| Barrier::new(N_WORKERS));

pub fn prologue() {
    let mut workers = vec![];
    for _ in 0..N_WORKERS {
        let worker = Worker::<Slot>::new_lifo();
        unsafe { STEALERS.push(worker.stealer()) };
        workers.push(worker);
    }
    let mut handles = vec![];
    for w in workers {
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
            run_worker(&w, stealer);
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
    // Create initial work
    for root in object_model.roots() {
        INJECTOR.push(Slot(root as *const u64 as *mut u64));
    }
    // Wake up workers
    {
        let mut epoch = LOCK.lock().unwrap();
        EPOCH.fetch_add(1, Ordering::SeqCst);
        CVAR.notify_all();
        while *epoch != EPOCH.load(Ordering::SeqCst) {
            epoch = CVAR.wait(epoch).unwrap();
        }
    }
    assert!(INJECTOR.is_empty());
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
