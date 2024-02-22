use std::sync::Mutex;

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Slot(pub *mut u64);

unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

impl Slot {
    pub fn load(&self) -> Option<Object> {
        let v = unsafe { *self.0 };
        if v == 0 {
            None
        } else {
            Some(Object(v))
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Object(pub u64);

pub struct LocalQueue<T> {
    data: Mutex<Vec<T>>,
    handler: fn(&T),
}

impl<T> LocalQueue<T> {
    pub const fn new(handler: fn(&T)) -> Self {
        LocalQueue {
            data: Mutex::new(Vec::new()),
            handler,
        }
    }

    pub fn push(&self, item: T) {
        self.data.lock().unwrap().push(item);
    }

    pub fn pop(&self) -> Option<T> {
        self.data.lock().unwrap().pop()
    }

    pub fn consume(&self) {
        while let Some(data) = self.pop() {
            (self.handler)(&data);
        }
    }
}
