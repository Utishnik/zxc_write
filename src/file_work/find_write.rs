use compio::driver::OpCode;
use std::io;
use std::num::NonZeroUsize;

use compio_flexible_dispatch::{Dispatcher, DispatcherBuilder};

pub fn build_dispatch(nthreads: NonZeroUsize) -> io::Result<Dispatcher> {
    DispatcherBuilder::new()
        .worker_threads(nthreads)
        .thread_names(|x| (x + 1).to_string())
        .build()
}
