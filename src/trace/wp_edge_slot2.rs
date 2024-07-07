use super::TracingStats;
use crate::util::fake_forwarding::TO_SPACE;
use crate::util::side_mark_table::SideMarkTable;
use crate::util::tracer::Tracer;
use crate::util::typed_obj::{Object, Slot};
use crate::util::workers::WorkerGroup;
use crate::util::wp2::{Packet, WPWorker, GLOBAL};
use crate::{ObjectModel, TraceArgs};
use std::arch::asm;
use std::ops::Range;
use std::sync::LazyLock;
use std::{
    marker::PhantomData,
    sync::{atomic::Ordering, Arc},
};

static mut ROOTS: Option<*const [u64]> = None;

static SIDE_MARK_TABLE_IX: LazyLock<SideMarkTable> = LazyLock::new(|| SideMarkTable::new(8 << 30));

struct MarkTableZeroingPacket {
    range: Range<usize>,
}

impl Packet for MarkTableZeroingPacket {
    fn run(&mut self) {
        SIDE_MARK_TABLE_IX.bulk_zero(self.range.clone());
    }
}

struct TracePacket<O: ObjectModel> {
    slots: Vec<Slot>,
    next_slots: Vec<Slot>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> TracePacket<O> {
    fn new(slots: Vec<Slot>) -> Self {
        Self {
            slots,
            next_slots: Vec::new(),
            _p: PhantomData,
        }
    }

    fn flush(&mut self, local: &mut WPWorker) {
        if !self.next_slots.is_empty() {
            let next = TracePacket::<O>::new(std::mem::take(&mut self.next_slots));
            local.spawn(1, next);
        }
    }

    fn scan_object(&mut self, o: Object, local: &mut WPWorker, mark_state: u8, cap: usize) {
        if cfg!(feature = "detailed_stats") {
            local.objs += 1;
        }
        o.scan::<O, _>(|s| {
            let Some(c) = s.load() else { return };
            if c.is_marked(mark_state) {
                return;
            }
            if self.next_slots.is_empty() {
                self.next_slots.reserve(cap);
            }
            self.next_slots.push(s);
            if self.next_slots.len() >= cap {
                self.flush(local);
            }
        });
    }

    fn trace_mark_object(&mut self, o: Object, local: &mut WPWorker, mark_state: u8, cap: usize) {
        let marked = if o.space_id() == 0x2 {
            o.mark_relaxed(mark_state);
            SIDE_MARK_TABLE_IX.mark(o)
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
        let old_state = o.attempt_to_forward(mark_state);
        if old_state.is_forwarded_or_being_forwarded() {
            let fwd = o.spin_and_get_farwarded_object(mark_state);
            slot.volatile_store(fwd);
            return;
        }
        // copy
        let _farwarded = local.copy.copy_object::<O>(o);
        slot.volatile_store(o);
        o.set_as_forwarded(mark_state);
        // scan
        o.mark_relaxed(mark_state);
        self.scan_object(o, local, mark_state, cap);
    }

    fn trace_object(
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
    fn run(&mut self) {
        let local = WPWorker::current();
        let capacity = GLOBAL.cap();
        let mark_state = local.global.mark_state();
        for slot in std::mem::take(&mut self.slots) {
            if cfg!(feature = "detailed_stats") {
                local.slots += 1;
            }
            if let Some(o) = slot.load() {
                self.trace_object(slot, o, local, mark_state, capacity);
            } else {
                if cfg!(feature = "detailed_stats") {
                    local.ne_slots += 1;
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
    fn run(&mut self) {
        let local = WPWorker::current();
        let capacity = GLOBAL.cap();
        let mut buf = vec![];
        let Some(roots) = (unsafe { ROOTS }) else {
            unreachable!()
        };
        let roots = unsafe { &*roots };
        for root in &roots[self.range.clone()] {
            let slot = Slot::from_raw(root as *const u64 as *mut u64);
            if cfg!(feature = "slower_root_scanning") {
                for _ in 0..4096 {
                    unsafe { asm!("nop") };
                }
            }
            if slot.load().is_none() {
                if cfg!(feature = "detailed_stats") {
                    local.slots += 1;
                }
                continue;
            }
            if buf.is_empty() {
                buf.reserve(capacity);
            }
            buf.push(slot);
            if buf.len() >= capacity {
                let packet = TracePacket::<O>::new(buf);
                local.spawn(1, packet);
                buf = vec![];
            }
        }
        if !buf.is_empty() {
            let packet = TracePacket::<O>::new(buf);
            local.spawn(1, packet);
        }
    }
}

struct WPEdgeSlotTracer<O: ObjectModel> {
    group: Arc<WorkerGroup<WPWorker>>,
    _p: PhantomData<O>,
}

impl<O: ObjectModel> Tracer<O> for WPEdgeSlotTracer<O> {
    fn startup(&self) {
        println!(
            "[WPEdgeSlot2] Use {} worker threads.",
            self.group.workers.len()
        );
        self.group.spawn();
    }

    fn trace(&self, mark_sense: u8, object_model: &O) -> TracingStats {
        GLOBAL.reset();
        GLOBAL.mark_state.store(mark_sense, Ordering::SeqCst);
        TO_SPACE.reset();
        // Create fake mark table zeroing packet
        let entries = SIDE_MARK_TABLE_IX.entries();
        let chunk_size = entries / self.group.workers.len();
        for i in (0..entries).step_by(chunk_size) {
            let range = i..(i + chunk_size).min(entries);
            let packet = MarkTableZeroingPacket { range };
            GLOBAL.buckets.prepare.push(Box::new(packet));
        }
        // Create initial root scanning packets
        let roots = object_model.roots();
        let roots_len = roots.len();
        unsafe { ROOTS = Some(roots) };
        let num_workers = self.group.workers.len();
        for id in 0..num_workers {
            let range = (roots_len * id) / num_workers..(roots_len * (id + 1)) / num_workers;
            let packet = ScanRoots::<O>::new(range);
            GLOBAL.buckets.prepare.push(Box::new(packet));
        }
        GLOBAL.buckets.prepare.open();
        // Wake up workers
        self.group.run_epoch();
        GLOBAL.get_stats()
    }

    fn teardown(&self) {
        self.group.finish();
    }
}

impl<O: ObjectModel> WPEdgeSlotTracer<O> {
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
    let threads = if cfg!(feature = "single_thread") {
        1
    } else {
        args.threads
    };
    GLOBAL.set_cap(args.wp_capacity);
    Box::new(WPEdgeSlotTracer::<O>::new(threads))
}
