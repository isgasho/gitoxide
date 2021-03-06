pub struct EagerIter<I: Iterator> {
    receiver: std::sync::mpsc::Receiver<Vec<I::Item>>,
    chunk: Option<std::vec::IntoIter<I::Item>>,
    size_hint: (usize, Option<usize>),
}

impl<I> EagerIter<I>
where
    I: Iterator + Send + 'static,
    <I as Iterator>::Item: Send,
{
    pub fn new(iter: I, chunk_size: usize, chunks_in_flight: usize) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel(chunks_in_flight);
        let size_hint = iter.size_hint();
        assert!(chunk_size > 0, "non-zero chunk size is needed");

        std::thread::spawn(move || {
            let mut out = Vec::with_capacity(chunk_size);
            for item in iter {
                out.push(item);
                if out.len() == chunk_size {
                    if sender.send(out).is_err() {
                        return;
                    }
                    out = Vec::with_capacity(chunk_size);
                }
            }
            if !out.is_empty() {
                sender.send(out).ok();
            }
        });
        EagerIter {
            receiver,
            chunk: None,
            size_hint,
        }
    }

    fn fill_buf_and_pop(&mut self) -> Option<I::Item> {
        self.chunk = self.receiver.recv().ok().map(|v| {
            assert!(!v.is_empty());
            v.into_iter()
        });
        self.chunk.as_mut().and_then(|c| c.next())
    }
}

impl<I> Iterator for EagerIter<I>
where
    I: Iterator + Send + 'static,
    <I as Iterator>::Item: Send,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self.chunk.as_mut() {
            Some(chunk) => chunk.next().or_else(|| self.fill_buf_and_pop()),
            None => self.fill_buf_and_pop(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.size_hint
    }
}

pub enum EagerIterIf<I: Iterator> {
    Eager(EagerIter<I>),
    OnDemand(I),
}

impl<I> EagerIterIf<I>
where
    I: Iterator + Send + 'static,
    <I as Iterator>::Item: Send,
{
    pub fn new(condition: impl FnOnce() -> bool, iter: I, chunk_size: usize, chunks_in_flight: usize) -> Self {
        if condition() {
            EagerIterIf::Eager(EagerIter::new(iter, chunk_size, chunks_in_flight))
        } else {
            EagerIterIf::OnDemand(iter)
        }
    }
}
impl<I> Iterator for EagerIterIf<I>
where
    I: Iterator + Send + 'static,
    <I as Iterator>::Item: Send,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            EagerIterIf::OnDemand(i) => i.next(),
            EagerIterIf::Eager(i) => i.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            EagerIterIf::OnDemand(i) => i.size_hint(),
            EagerIterIf::Eager(i) => i.size_hint(),
        }
    }
}
