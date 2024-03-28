use smallvec::SmallVec;

use super::TracingStats;
use crate::util::fake_forwarding::TO_SPACE;
use crate::util::tracer::Tracer;
use crate::util::typed_obj::{Object, Slot};
use crate::util::workers::WorkerGroup;
use crate::util::wp::{Packet, WPWorker, GLOBAL};
use crate::{ObjectModel, TraceArgs};
use std::ops::Range;
use std::{
    marker::PhantomData,
    sync::{atomic::Ordering, Arc},
};

static mut ROOTS: Option<*const [u64]> = None;

const SMALL_VEC_SIZE: usize = 32;
const NUM_QUEUES: usize = 2;

struct TracePacket<O: ObjectModel> {
    slots: [SmallVec<[Slot; SMALL_VEC_SIZE]>; NUM_QUEUES],
    next_slots: [SmallVec<[Slot; SMALL_VEC_SIZE]>; NUM_QUEUES],
    next_len: usize,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> TracePacket<O> {
    fn new(slots: [SmallVec<[Slot; SMALL_VEC_SIZE]>; NUM_QUEUES]) -> Self {
        Self {
            slots,
            next_slots: Default::default(),
            next_len: 0,
            _p: PhantomData,
        }
    }

    fn flush(&mut self, local: &mut WPWorker) {
        if self.next_len > 0 {
            assert_eq!(
                self.next_len,
                self.next_slots.iter().map(|s| s.len()).sum::<usize>()
            );
            let next_slots = std::mem::take(&mut self.next_slots);
            let next = TracePacket::<O>::new(next_slots);
            local.add(Box::new(next));
            self.next_len = 0;
        }
    }

    fn get_queue_index(o: Object) -> usize {
        if cfg!(feature = "no_space_dispatch") {
            debug_assert!(o.space_id() == 0x2 || o.space_id() == 0x6);
            (o.space_id() != 0x2) as usize
        } else {
            0
        }
    }

    fn scan_object(&mut self, o: Object, local: &mut WPWorker, mark_state: u8, cap: usize) {
        local.objs += 1;
        o.scan::<O, _>(|s| {
            let Some(c) = s.load() else {
                local.slots += 1;
                return;
            };
            if c.is_marked(mark_state) {
                local.slots += 1;
                local.ne_slots += 1;
                return;
            }
            let index = Self::get_queue_index(c);
            let next_slots = &mut self.next_slots[index];
            if next_slots.len() >= SMALL_VEC_SIZE {
                next_slots.reserve(cap - SMALL_VEC_SIZE);
            }
            next_slots.push(s);
            self.next_len += 1;
            if self.next_len >= cap {
                self.flush(local);
            }
        });
    }

    fn trace_mark_object(&mut self, o: Object, local: &mut WPWorker, mark_state: u8, cap: usize) {
        debug_assert!(o.space_id() == 0x6);
        let marked = if cfg!(feature = "relaxed_mark") {
            o.mark_relaxed(mark_state)
        } else {
            o.mark(mark_state)
        };
        if marked {
            self.scan_object(o, local, mark_state, cap)
        }
    }

    fn trace_forward_object(
        &mut self,
        slot: Slot,
        o: Object,
        local: &mut WPWorker,
        mark_state: u8,
        cap: usize,
    ) {
        debug_assert!(o.space_id() == 0x2);
        if cfg!(feature = "atomic_free_farwarding") {
            if o.is_forwarded_or_being_forwarded(mark_state) {
                slot.volatile_store(o);
                return;
            }
            o.set_as_forwarded(mark_state);
        } else {
            let old_state = o.attempt_to_forward(mark_state);
            if old_state.is_forwarded_or_being_forwarded() {
                let fwd = o.spin_and_get_farwarded_object(mark_state);
                slot.volatile_store(fwd);
                return;
            }
        }
        // copy
        let _farwarded = local.copy.copy_object::<O>(o);
        slot.volatile_store(o);
        o.set_as_forwarded(mark_state);
        local.copied_objects += 1;
        // Add complexity: touch every byte
        let size = o.size::<O>();
        for i in 8..size {
            unsafe {
                let ptr = (o.raw() as *mut u8).add(i);
                let v = std::ptr::read_volatile(ptr);
                std::ptr::write_volatile(ptr, v);
            }
        }
        // scan
        o.mark_relaxed(mark_state);
        self.scan_object(o, local, mark_state, cap);
    }

    fn trace_object_generic(
        &mut self,
        slot: Slot,
        o: Object,
        local: &mut WPWorker,
        mark_state: u8,
        cap: usize,
    ) {
        if cfg!(feature = "forwarding") && o.space_id() == 0x2 {
            self.trace_forward_object(slot, o, local, mark_state, cap)
        } else {
            self.trace_mark_object(o, local, mark_state, cap)
        }
    }
}

impl<O: ObjectModel> Packet for TracePacket<O> {
    fn run(&mut self, local: &mut WPWorker) {
        local.packets += 1;
        let capacity = GLOBAL.cap();
        let mark_state = local.global.mark_state();
        let mut slots = std::mem::take(&mut self.slots);
        for i in 0..NUM_QUEUES {
            local.slots += slots[i].len() as u64;
            local.ne_slots += slots[i].len() as u64;
        }
        if cfg!(feature = "no_space_dispatch") {
            for slot in std::mem::take(&mut slots[0]) {
                let o = slot.load().unwrap();
                self.trace_forward_object(slot, o, local, mark_state, capacity);
            }
            for slot in std::mem::take(&mut slots[1]) {
                let o = slot.load().unwrap();
                self.trace_mark_object(o, local, mark_state, capacity);
            }
        } else {
            for buf in slots {
                for slot in buf {
                    let o = slot.load().unwrap();
                    self.trace_object_generic(slot, o, local, mark_state, capacity);
                }
            }
        }
        self.flush(local);
    }
}

struct ScanRoots<O: ObjectModel> {
    range: Range<usize>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> ScanRoots<O> {
    fn new(range: Range<usize>) -> Self {
        ScanRoots {
            range,
            _p: PhantomData,
        }
    }
}

impl<O: ObjectModel> Packet for ScanRoots<O> {
    fn run(&mut self, local: &mut WPWorker) {
        let capacity = GLOBAL.cap();
        let mut slots: [SmallVec<[Slot; SMALL_VEC_SIZE]>; NUM_QUEUES] = Default::default();
        let mut count = 0;
        let Some(roots) = (unsafe { ROOTS }) else {
            unreachable!()
        };
        let roots = unsafe { &*roots };
        for root in &roots[self.range.clone()] {
            let slot = Slot::from_raw(root as *const u64 as *mut u64);
            let Some(o) = slot.load() else {
                local.slots += 1;
                continue;
            };
            let index = TracePacket::<O>::get_queue_index(o);
            let buf = &mut slots[index];
            if buf.len() >= SMALL_VEC_SIZE {
                buf.reserve(capacity - SMALL_VEC_SIZE);
            }
            buf.push(slot);
            count += 1;
            if count >= capacity {
                let packet = TracePacket::<O>::new(std::mem::take(&mut slots));
                local.add(Box::new(packet));
                count = 0;
            }
        }
        if count > 0 {
            let packet = TracePacket::<O>::new(std::mem::take(&mut slots));
            local.add(Box::new(packet));
        }
    }
}

struct WPEdgeSlotTracer<O: ObjectModel> {
    group: Arc<WorkerGroup<WPWorker>>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for WPEdgeSlotTracer<O> {
    fn startup(&self) {
        info!("Use {} worker threads.", self.group.workers.len());
        self.group.spawn();
    }

    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats {
        GLOBAL.reset();
        GLOBAL.mark_state.store(mark_sense, Ordering::SeqCst);
        TO_SPACE.reset();
        // Create initial root scanning packets
        let roots = object_model.roots();
        let roots_len = roots.len();
        unsafe { ROOTS = Some(roots) };
        let num_workers = self.group.workers.len();
        for id in 0..num_workers {
            let range = (roots_len * id) / num_workers..(roots_len * (id + 1)) / num_workers;
            let packet = ScanRoots::<O>::new(range);
            GLOBAL.queue.push(Box::new(packet));
        }
        // Wake up workers
        self.group.run_epoch();
        GLOBAL.get_stats()
    }

    fn teardown(&self) {
        self.group.finish();
    }
}

impl<O: ObjectModel> WPEdgeSlotTracer<O> {
    pub fn new(num_workers: usize) -> Self {
        Self {
            group: WorkerGroup::new(num_workers),
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
    GLOBAL.set_cap(args.wp_capacity);
    Box::new(WPEdgeSlotTracer::<O>::new(threads))
}
