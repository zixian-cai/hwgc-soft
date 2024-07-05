pub struct Stack<T> {
    data: Vec<Vec<T>>,
    cache: Option<Vec<T>>,
}

impl<T> Stack<T> {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            cache: None,
        }
    }

    fn alloc(&mut self) -> Vec<T> {
        if let Some(cache) = self.cache.take() {
            cache
        } else {
            Vec::with_capacity(Self::segment_size())
        }
    }

    const fn segment_size() -> usize {
        4096 / std::mem::size_of::<T>()
    }

    #[inline(always)]
    pub fn push(&mut self, item: T) {
        if self.data.is_empty() {
            let new_buf = self.alloc();
            self.data.push(new_buf);
        }
        self.data.last_mut().unwrap().push(item);
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        if self.data.is_empty() {
            return None;
        }
        let item = self.data.last_mut().unwrap().pop();
        if self.data.last().unwrap().is_empty() {
            let buf = self.data.pop();
            self.cache = buf;
        }
        item
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
