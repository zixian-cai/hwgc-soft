pub struct Stack<T> {
    data: Vec<Vec<T>>,
}

impl<T> Stack<T> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    const fn segment_size() -> usize {
        4096 / std::mem::size_of::<T>()
    }

    #[inline(always)]
    pub fn push(&mut self, item: T) {
        if self.data.is_empty() {
            self.data.push(Vec::with_capacity(Self::segment_size()));
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
            self.data.pop();
        }
        item
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
