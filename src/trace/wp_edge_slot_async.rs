use futures::future::BoxFuture;
use tokio::task::JoinSet;

use super::TracingStats;
use crate::util::fake_forwarding::{LocalAllocator, TO_SPACE};
use crate::util::tracer::Tracer;
use crate::util::typed_obj::{Object, Slot};
use crate::{ObjectModel, TraceArgs};
use std::marker::PhantomData;
use std::ops::Range;

const CAP: usize = 1024;

#[thread_local]
static mut LOCAL_COPY_ALLOCATOR: LocalAllocator = LocalAllocator::new();

struct TransitiveClosure {
    set: JoinSet<()>,
    mark_state: u8,
    next_slots: Vec<Slot>,
}

impl TransitiveClosure {
    fn new(mark_state: u8) -> Self {
        Self {
            set: JoinSet::new(),
            mark_state,
            next_slots: vec![],
        }
    }

    fn flush<O: ObjectModel>(&mut self) {
        if !self.next_slots.is_empty() {
            self.set.spawn(process_slots_rec::<O>(
                std::mem::take(&mut self.next_slots),
                self.mark_state,
            ));
        }
    }

    fn scan_object<O: ObjectModel>(&mut self, o: Object) {
        o.scan::<O, _>(|s| {
            let Some(c) = s.load() else { return };
            if c.is_marked(self.mark_state) {
                return;
            }
            if self.next_slots.is_empty() {
                self.next_slots.reserve(CAP);
            }
            self.next_slots.push(s);
            if self.next_slots.len() >= CAP {
                self.flush::<O>();
            }
        });
    }

    fn trace_mark_object<O: ObjectModel>(&mut self, o: Object) {
        if o.mark(self.mark_state) {
            self.scan_object::<O>(o)
        }
    }

    fn trace_forward_object<O: ObjectModel>(&mut self, slot: Slot, o: Object) {
        let old_state = o.attempt_to_forward(self.mark_state);
        if old_state.is_forwarded_or_being_forwarded() {
            let fwd = o.spin_and_get_farwarded_object(self.mark_state);
            slot.volatile_store(fwd);
            return;
        }
        // copy
        let _farwarded = unsafe { LOCAL_COPY_ALLOCATOR.copy_object::<O>(o) };
        slot.volatile_store(o);
        o.set_as_forwarded(self.mark_state);
        // scan
        o.mark_relaxed(self.mark_state);
        self.scan_object::<O>(o);
    }

    fn trace_object<O: ObjectModel>(&mut self, slot: Slot, o: Object) {
        if cfg!(feature = "forwarding") && o.space_id() == 0x2 {
            self.trace_forward_object::<O>(slot, o)
        } else {
            self.trace_mark_object::<O>(o)
        }
    }
}

fn process_slots_rec<O: ObjectModel>(slots: Vec<Slot>, mark_state: u8) -> BoxFuture<'static, ()> {
    Box::pin(process_slots::<O>(slots, mark_state))
}

async fn process_slots<O: ObjectModel>(slots: Vec<Slot>, mark_state: u8) {
    let mut closure = TransitiveClosure::new(mark_state);
    for slot in slots {
        let o = slot.load().unwrap();
        closure.trace_object::<O>(slot, o);
    }
    closure.flush::<O>();
    while let Some(_) = closure.set.join_next().await {}
}

async fn scan_roots<O: ObjectModel>(roots: &[u64], range: Range<usize>, mark_state: u8) {
    let mut tasks = tokio::task::JoinSet::new();
    let mut buf = vec![];
    for root in &roots[range.clone()] {
        let slot = Slot::from_raw(root as *const u64 as *mut u64);
        if slot.load().is_none() {
            continue;
        }
        if buf.is_empty() {
            buf.reserve(CAP);
        }
        buf.push(slot);
        if buf.len() >= CAP {
            tasks.spawn(process_slots::<O>(std::mem::take(&mut buf), mark_state));
        }
    }
    if !buf.is_empty() {
        tasks.spawn(process_slots::<O>(std::mem::take(&mut buf), mark_state));
    }
    while let Some(_) = tasks.join_next().await {}
}

struct WPEdgeSlotAsyncTracer<O: ObjectModel> {
    rt: tokio::runtime::Runtime,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for WPEdgeSlotAsyncTracer<O> {
    fn startup(&self) {}

    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats {
        TO_SPACE.reset();
        // Create initial root scanning packets
        let roots = object_model.roots();
        let roots_len = roots.len();
        let num_workers = 32;
        self.rt.block_on(async move {
            let mut tasks = vec![];
            for id in 0..num_workers {
                let range = (roots_len * id) / num_workers..(roots_len * (id + 1)) / num_workers;
                tasks.push(scan_roots::<O>(roots, range.clone(), mark_sense));
            }
            futures::future::join_all(tasks).await;
        });
        TracingStats {
            ..Default::default()
        }
    }

    fn teardown(&self) {}
}

impl<O: ObjectModel> WPEdgeSlotAsyncTracer<O> {
    pub fn new(_num_workers: usize) -> Self {
        Self {
            rt: tokio::runtime::Runtime::new().unwrap(),
            _p: PhantomData,
        }
    }
}

pub fn create_tracer<O: ObjectModel>(args: &TraceArgs) -> Box<dyn Tracer<O>> {
    let threads = if cfg!(feature = "single_thread") {
        1
    } else {
        args.threads
    };
    Box::new(WPEdgeSlotAsyncTracer::<O>::new(threads))
}
