use crossbeam::queue::SegQueue;
use once_cell::sync::Lazy;
use ws_deque::{self, Stealer, Stolen, Worker};

use super::TracingStats;
use crate::util::tracer::Tracer;
use crate::util::typed_obj::Slot;
use crate::util::workers::WorkerGroup;
use crate::util::workers::{end_epoch, start_epoch, thread_done};
use crate::{ObjectModel, TraceArgs};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize};
use std::sync::Weak;
use std::time::Duration;
use std::{
    marker::PhantomData,
    sync::{atomic::Ordering, Arc},
};

static mut ROOTS: Option<*const [u64]> = None;

pub struct GlobalContext {
    pub root_segments: SegQueue<Range<usize>>,
    pub mark_state: AtomicU8,
    pub objs: AtomicU64,
    pub edges: AtomicU64,
    pub ne_edges: AtomicU64,
}

impl GlobalContext {
    pub fn new() -> Self {
        Self {
            root_segments: SegQueue::new(),
            mark_state: AtomicU8::new(0),
            objs: AtomicU64::new(0),
            edges: AtomicU64::new(0),
            ne_edges: AtomicU64::new(0),
        }
    }

    pub fn mark_state(&self) -> u8 {
        self.mark_state.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.objs.store(0, Ordering::SeqCst);
        self.edges.store(0, Ordering::SeqCst);
        self.ne_edges.store(0, Ordering::SeqCst);
    }
}

pub static GLOBAL: Lazy<Arc<GlobalContext>> = Lazy::new(|| Arc::new(GlobalContext::new()));

pub struct ParTracingWorker<O: ObjectModel> {
    id: usize,
    queue: Worker<Slot>,
    stealer: Stealer<Slot>,
    global: Arc<GlobalContext>,
    group: Weak<WorkerGroup<Self>>,
    objs: u64,
    slots: u64,
    ne_slots: u64,
    _p: PhantomData<O>,
}

static TERMINATION_COUNT: AtomicUsize = AtomicUsize::new(0);

impl<O: ObjectModel> ParTracingWorker<O> {
    #[inline(always)]
    fn peek(&self, group: &WorkerGroup<Self>) -> bool {
        for w in group.workers.iter() {
            if !w.is_empty() {
                return true;
            }
        }
        false
    }

    fn offer_termination(&self, group: &WorkerGroup<Self>) -> bool {
        TERMINATION_COUNT.fetch_add(1, Ordering::SeqCst);
        const HARD_SPINS: usize = 4096;
        const YIELDS_BEFORE_SLEEP: usize = 5000;
        const SPIN_TO_YIELD_RATIO: usize = 10;

        let mut yield_count = 0usize;
        let mut hard_spin_count = 0usize;
        let mut hard_spin_limit = (HARD_SPINS >> SPIN_TO_YIELD_RATIO).max(1);

        let hard_spin_start = hard_spin_limit;

        let num_workers = group.workers.len();

        loop {
            if TERMINATION_COUNT.load(Ordering::Relaxed) == num_workers {
                return true;
            }
            if yield_count <= YIELDS_BEFORE_SLEEP {
                yield_count += 1;
                if hard_spin_count > SPIN_TO_YIELD_RATIO {
                    std::thread::yield_now();
                    hard_spin_count = 0;
                    hard_spin_limit = hard_spin_start;
                } else {
                    hard_spin_limit = usize::max(2 * hard_spin_limit, HARD_SPINS);
                    for _ in 0..hard_spin_limit {
                        unsafe { std::arch::asm!("nop") }
                    }
                    hard_spin_count += 1;
                }
            } else {
                yield_count = 0;
                std::thread::sleep(Duration::from_millis(1));
            }
            if self.peek(group) {
                TERMINATION_COUNT.fetch_sub(1, Ordering::SeqCst);
                return false;
            }
        }
    }
}

impl<O: ObjectModel> crate::util::workers::Worker for ParTracingWorker<O> {
    type SharedWorker = Stealer<Slot>;

    fn new(id: usize, group: Weak<WorkerGroup<Self>>) -> Self {
        let (worker, stealer) = ws_deque::new(cfg!(feature = "deque_overflow"));
        Self {
            id,
            queue: worker,
            stealer,
            group,
            global: GLOBAL.clone(),
            objs: 0,
            slots: 0,
            ne_slots: 0,
            _p: PhantomData,
        }
    }

    fn new_shared(&self) -> Self::SharedWorker {
        self.stealer.clone()
    }

    fn run_epoch(&mut self) {
        self.objs = 0;
        self.slots = 0;
        self.ne_slots = 0;
        let group = self.group.upgrade().unwrap();
        let mark_state = self.global.mark_state();
        // scan roots
        let roots = unsafe { &*ROOTS.unwrap() };
        while let Some(mut range) = GLOBAL.root_segments.pop() {
            while let Some(root) = roots.get(range.start) {
                let slot = Slot::from_raw(root as *const u64 as *mut u64);
                self.queue.push(slot);
                range.start += 1;
            }
        }
        // trace objects
        macro_rules! process_slot {
            ($slot: expr) => {{
                if cfg!(feature = "detailed_stats") {
                    self.slots += 1;
                }
                if let Some(o) = $slot.load() {
                    if o.mark(mark_state) {
                        if cfg!(feature = "detailed_stats") {
                            self.objs += 1;
                        }
                        o.scan::<O, _>(|s| self.queue.push(s));
                    }
                } else {
                    if cfg!(feature = "detailed_stats") {
                        self.ne_slots += 1;
                    }
                }
            }};
        }
        'outer: loop {
            // Drain local queue
            if cfg!(feature = "deque_bulk_pop") {
                while let Some((slots, n)) = self.queue.pop_bulk::<64>() {
                    for i in 0..n {
                        let s = unsafe { slots[i].assume_init() };
                        process_slot!(s);
                    }
                }
            } else {
                while let Some(slot) = self.queue.pop() {
                    process_slot!(slot);
                }
            }
            // Steal from other workers
            let mut retry = false;
            for (i, stealer) in group.workers.iter().enumerate() {
                if i == self.id {
                    continue;
                }
                match stealer.steal() {
                    Stolen::Data(slot) => {
                        process_slot!(slot);
                        continue 'outer;
                    }
                    Stolen::Abort => {
                        retry = true;
                        continue;
                    }
                    _ => {}
                }
            }
            if retry {
                continue;
            }
            // Termination check
            if self.queue.is_empty() && self.offer_termination(&group) {
                break;
            }
        }
        thread_done();
        // assert!(self.queue.is_empty());
        let global = &self.global;
        global.objs.fetch_add(self.objs, Ordering::SeqCst);
        global.edges.fetch_add(self.slots, Ordering::SeqCst);
        global.ne_edges.fetch_add(self.ne_slots, Ordering::SeqCst);
    }
}

struct ParEdgeSlotTracer<O: ObjectModel> {
    group: Arc<WorkerGroup<ParTracingWorker<O>>>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for ParEdgeSlotTracer<O> {
    fn startup(&self) {
        println!(
            "[ParEdgeSlot2] Use {} worker threads.",
            self.group.workers.len()
        );
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
        TERMINATION_COUNT.store(0, Ordering::SeqCst);
        start_epoch();
        self.group.run_epoch();
        end_epoch();
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

impl<O: ObjectModel> ParEdgeSlotTracer<O> {
    pub fn new(mut num_workers: usize) -> Self {
        if let Ok(x) = std::env::var("THREADS") {
            num_workers = x.parse().unwrap();
        }
        Self {
            group: WorkerGroup::new(num_workers),
            _p: PhantomData,
        }
    }
}

pub fn create_tracer<O: ObjectModel>(args: &TraceArgs) -> Box<dyn Tracer<O>> {
    Box::new(ParEdgeSlotTracer::<O>::new(args.threads))
}
