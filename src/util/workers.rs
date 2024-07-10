use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Barrier, BarrierWaitResult, Condvar, Mutex, Weak,
};

use crossbeam::queue::SegQueue;

struct Monitor {
    cvar: Condvar,
    lock: Mutex<usize>,
    epoch: AtomicUsize,
    finish: AtomicBool,
    barrier: Barrier,
}

impl Monitor {
    fn new(num_workers: usize) -> Self {
        Self {
            cvar: Condvar::new(),
            lock: Mutex::new(0),
            epoch: AtomicUsize::new(0),
            finish: AtomicBool::new(false),
            barrier: Barrier::new(num_workers),
        }
    }
}

pub struct WorkerGroup<W: Worker> {
    monitor: Arc<Monitor>,
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
    pub workers: Vec<W::SharedWorker>,
    local_workers: Mutex<Option<Vec<W>>>,
}

impl<W: Worker> WorkerGroup<W> {
    pub fn new(num_workers: usize) -> Arc<Self> {
        Arc::new_cyclic(|w| {
            let mut workers = vec![];
            let mut shared = vec![];
            for i in 0..num_workers {
                let worker = W::new(i, w.clone());
                shared.push(worker.new_shared());
                workers.push(worker);
            }
            Self {
                monitor: Arc::new(Monitor::new(num_workers)),
                handles: Mutex::new(Vec::new()),
                workers: shared,
                local_workers: Mutex::new(Some(workers)),
            }
        })
    }

    /// Barrier synchronization
    #[allow(unused)]
    pub fn sync(&self) -> BarrierWaitResult {
        self.monitor.barrier.wait()
    }

    /// Spawn workers
    pub fn spawn(&self) {
        let mut handles = self.handles.lock().unwrap();
        let workers = self.local_workers.lock().unwrap().take().unwrap();
        for mut worker in workers.into_iter() {
            let monitor = self.monitor.clone();
            let handle = std::thread::spawn(move || {
                loop {
                    // Wait for GC request
                    {
                        let mut epoch = monitor.lock.lock().unwrap();
                        while *epoch == monitor.epoch.load(Ordering::SeqCst)
                            && !monitor.finish.load(Ordering::SeqCst)
                        {
                            epoch = monitor.cvar.wait(epoch).unwrap();
                        }
                        if monitor.finish.load(Ordering::SeqCst) {
                            return;
                        }
                    }
                    // Do GC
                    worker.run_epoch();
                    // Final sync
                    {
                        if monitor.barrier.wait().is_leader() {
                            let mut epoch = monitor.lock.lock().unwrap();
                            *epoch = monitor.epoch.load(Ordering::SeqCst);
                            monitor.cvar.notify_all();
                        }
                        monitor.barrier.wait();
                    }
                }
            });
            handles.push(handle);
        }
    }

    /// Wake up the workers to run an GC epoch
    pub fn run_epoch(&self) {
        // Wake up workers
        let mut epoch = self.monitor.lock.lock().unwrap();
        self.monitor.epoch.fetch_add(1, Ordering::SeqCst);
        self.monitor.cvar.notify_all();
        // Wait for workers to finish
        while *epoch != self.monitor.epoch.load(Ordering::SeqCst) {
            epoch = self.monitor.cvar.wait(epoch).unwrap();
        }
    }

    /// Terminate workers
    pub fn finish(&self) {
        // Notify workers to finish
        let guard = self.monitor.lock.lock().unwrap();
        self.monitor.finish.store(true, Ordering::SeqCst);
        self.monitor.cvar.notify_all();
        std::mem::drop(guard);
        // Wait for workers to finish
        let mut handles = self.handles.lock().unwrap();
        let handles = std::mem::take::<Vec<_>>(&mut handles);
        for handle in handles {
            handle.join().unwrap();
        }
        // Reset monitor
        self.monitor.finish.store(false, Ordering::SeqCst);
        self.monitor.epoch.store(0, Ordering::SeqCst);
        *self.monitor.lock.lock().unwrap() = 0;
    }
}

/// Private thread-local worker data
pub trait Worker: Send + 'static + Sized {
    /// The shared worker data
    type SharedWorker: Send + Sync + 'static;

    /// Create a new worker
    fn new(id: usize, group: Weak<WorkerGroup<Self>>) -> Self;
    /// Create a new shared worker
    fn new_shared(&self) -> Self::SharedWorker;
    /// Run an GC epoch
    fn run_epoch(&mut self);
}

const YIELD_TIMER: bool = false;
static mut NOW: Option<std::time::Instant> = None;
static YIELD: SegQueue<f32> = SegQueue::new();

pub fn thread_done() {
    if YIELD_TIMER {
        let elapsed = unsafe { NOW.as_ref().unwrap().elapsed().as_micros() as f32 / 1000.0 };
        YIELD.push(elapsed);
    }
}

pub fn start_epoch() {
    if YIELD_TIMER {
        unsafe { NOW = Some(std::time::Instant::now()) }
    }
}

pub fn end_epoch() {
    if YIELD_TIMER {
        let elapsed = unsafe { NOW.as_ref().unwrap().elapsed().as_micros() as f32 / 1000.0 };
        let mut per_thread_elapsed = vec![];
        while let Some(e) = YIELD.pop() {
            per_thread_elapsed.push(e);
        }
        println!("Elapsed: {:.3} ms, {:.3?}", elapsed, per_thread_elapsed);
    }
}
