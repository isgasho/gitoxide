use crate::{
    pack,
    pack::data::EntrySlice,
    pack::tree::{Item, Tree},
};
use git_features::{interruptible::is_interrupted, parallel, parallel::in_parallel_if, progress::Progress};
use quick_error::quick_error;

mod resolve;

quick_error! {
    #[derive(Debug)]
    pub enum Error {
        ZlibInflate(err: crate::zlib::Error, msg: &'static str) {
            display("{}", msg)
            source(err)
        }
        ResolveFailed(pack_offset: u64) {
            display("The resolver failed to obtain the pack entry bytes for the entry at {}", pack_offset)
        }
        Inspect(err: Box<dyn std::error::Error + Send + Sync>) {
            display("One of the object inspectors failed")
            source(&**err)
            from()
        }
        Interrupted {
            display("Interrupted by user")
        }
    }
}

pub struct Context<'a, S> {
    pub entry: &'a pack::data::Entry,
    pub entry_end: u64,
    pub decompressed: &'a [u8],
    pub state: &'a mut S,
    pub level: u16,
}

impl<T> Tree<T>
where
    T: Default + Send,
{
    #[allow(clippy::too_many_arguments)]
    pub fn traverse<F, P, MBFN, S, E>(
        mut self,
        should_run_in_parallel: impl FnOnce() -> bool,
        resolve: F,
        progress: P,
        thread_limit: Option<usize>,
        pack_entries_end: u64,
        new_thread_state: impl Fn() -> S + Send + Sync,
        inspect_object: MBFN,
    ) -> Result<Vec<Item<T>>, Error>
    where
        F: for<'r> Fn(EntrySlice, &'r mut Vec<u8>) -> Option<()> + Send + Sync,
        P: Progress + Send,
        MBFN: Fn(&mut T, &mut <P as Progress>::SubProgress, Context<'_, S>) -> Result<(), E> + Send + Sync,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.pack_entries_end = Some(pack_entries_end);
        let (chunk_size, thread_limit, _) = parallel::optimize_chunk_size_and_thread_limit(1, None, thread_limit, None);
        let progress = parking_lot::Mutex::new(progress);

        // SAFETY: We are owning 'self', and it's the UnsafeCell which we are supposed to use requiring unsafe on every access now.
        #[allow(unsafe_code)]
        let num_objects = unsafe { (*self.items.get()).len() } as u32;
        in_parallel_if(
            should_run_in_parallel,
            self.iter_root_chunks(chunk_size),
            thread_limit,
            |thread_index| {
                (
                    Vec::<u8>::with_capacity(4096),
                    progress.lock().add_child(format!("thread {}", thread_index)),
                    new_thread_state(),
                )
            },
            |root_nodes, state| resolve::deltas(root_nodes, state, &resolve, &inspect_object),
            Reducer::new(num_objects, &progress),
        )?;
        Ok(self.into_items())
    }
}

pub(crate) struct Reducer<'a, P> {
    item_count: usize,
    progress: &'a parking_lot::Mutex<P>,
    start: std::time::Instant,
}

impl<'a, P> Reducer<'a, P>
where
    P: Progress,
{
    pub fn new(num_objects: u32, progress: &'a parking_lot::Mutex<P>) -> Self {
        progress.lock().init(Some(num_objects), Some("objects"));
        Reducer {
            item_count: 0,
            progress,
            start: std::time::Instant::now(),
        }
    }
}

impl<'a, P> parallel::Reducer for Reducer<'a, P>
where
    P: Progress,
{
    type Input = Result<usize, Error>;
    type Output = ();
    type Error = Error;

    fn feed(&mut self, input: Self::Input) -> Result<(), Self::Error> {
        let input = input?;
        self.item_count += input;
        self.progress.lock().set(self.item_count as u32);
        if is_interrupted() {
            return Err(Error::Interrupted);
        }
        Ok(())
    }

    fn finalize(self) -> Result<Self::Output, Self::Error> {
        self.progress
            .lock()
            .show_throughput(self.start, self.item_count as u32, "objects");
        Ok(())
    }
}