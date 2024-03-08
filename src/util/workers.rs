use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Barrier, BarrierWaitResult, Condvar, Mutex,
    },
};

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

pub struct WorkerGroup {
    num_workers: usize,
    monitor: Arc<Monitor>,
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
    global: RefCell<Option<Arc<dyn Context>>>,
}

unsafe impl Send for WorkerGroup {}
unsafe impl Sync for WorkerGroup {}

impl WorkerGroup {
    pub fn new(num_workers: usize) -> Self {
        Self {
            num_workers,
            monitor: Arc::new(Monitor::new(num_workers)),
            handles: Mutex::new(Vec::new()),
            global: RefCell::new(None),
        }
    }

    pub fn sync(&self) -> BarrierWaitResult {
        self.monitor.barrier.wait()
    }

    // Spawn workers
    pub fn spawn<W: Worker>(self: &Arc<Self>, global: &Arc<W::Global>) {
        self.global.replace(Some(global.clone()));
        let mut handles = self.handles.lock().unwrap();
        let mut workers = vec![];
        let mut shared = vec![];
        for i in 0..self.num_workers {
            let worker = W::new(i, self.clone(), global.clone());
            shared.push(worker.new_shared());
            workers.push(worker);
        }
        let shared_arc = Arc::new(shared);
        for (_i, mut worker) in workers.into_iter().enumerate() {
            let monitor = self.monitor.clone();
            let shared = shared_arc.clone();
            let handle = std::thread::spawn(move || {
                worker.init(shared);
                loop {
                    // Wait for GC request
                    {
                        let mut epoch = monitor.lock.lock().unwrap();
                        while *epoch == monitor.epoch.load(Ordering::SeqCst) {
                            epoch = monitor.cvar.wait(epoch).unwrap();
                            if monitor.finish.load(Ordering::SeqCst) {
                                return;
                            }
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

    // Run an GC epoch
    pub fn run_epoch(&self) {
        self.global.borrow().as_ref().unwrap().reset();
        // Wake up workers
        let mut epoch = self.monitor.lock.lock().unwrap();
        self.monitor.epoch.fetch_add(1, Ordering::SeqCst);
        self.monitor.cvar.notify_all();
        // Wait for workers to finish
        while *epoch != self.monitor.epoch.load(Ordering::SeqCst) {
            epoch = self.monitor.cvar.wait(epoch).unwrap();
        }
    }

    // Terminate workers
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

pub trait Worker: Send + 'static {
    type Global: Send + Sync + Context + 'static;
    type SharedWorker: Send + Sync + 'static;
    fn new(id: usize, group: Arc<WorkerGroup>, global: Arc<Self::Global>) -> Self;
    fn new_shared(&self) -> Self::SharedWorker;
    fn init(&mut self, workers: Arc<Vec<Self::SharedWorker>>);
    fn run_epoch(&mut self);
}

pub trait Context {
    fn reset(&self) {}
}
