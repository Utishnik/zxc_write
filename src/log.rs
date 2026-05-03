use crossfire::mpsc;
use getrandom::fill;
use std::cell::Cell;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const CHAN_SIZE: usize = 1024;

#[derive(Clone)]
pub enum LogTo {
    Ephemeral,
    File,
}
struct LogEntry {
    closure: Box<dyn FnOnce() -> String + Send>,
    log_to: LogTo,
}

pub struct Logger {
    sx: Cell<crossfire::MTx<mpsc::Array<LogEntry>>>,
    file: Option<File>,
    log_to: LogTo,
    with_time: bool,
    shutdown: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug)]
pub struct LoggerFileOptions {
    pub path: &'static str,
    pub append_mode: bool,
}

impl Logger {
    pub fn builder(log_op: Option<LoggerFileOptions>) -> Result<Self, std::io::Error> {
        let (sx, rx) = mpsc::bounded_blocking::<LogEntry>(CHAN_SIZE);

        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = shutdown_flag.clone();
        std::thread::spawn(move || {
            let file = log_op.map(Self::open_log_file);
            let checked_file: Option<File> = if let Some(ref x) = file
                && x.is_err()
            {
                println!("[ERROR FILE]");
                return;
            } else if let Some(x) = file
                && x.is_ok()
            {
                Some(unsafe { x.unwrap_unchecked() })
            } else {
                None
            };
            let mut file = checked_file;

            loop {
                match rx.recv() {
                    Err(_) => {
                        if shutdown_flag_clone.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Ok(entry) => {
                        let mut message = (entry.closure)();

                        match entry.log_to {
                            LogTo::File => {
                                if file.as_mut().is_some() {
                                    message.push('\n');
                                    let f = unsafe { file.as_mut().unwrap_unchecked() };
                                    f.write_all(message.as_bytes()).unwrap();
                                    f.flush().unwrap();
                                }
                            }
                            LogTo::Ephemeral => println!("{}", message),
                        };
                    }
                }
            }
        });

        let file = if let Some(x) = log_op {
            let res = Self::open_log_file(x);
            match res {
                Ok(ok) => Some(ok),
                Err(e) => return Err(e),
            }
        } else {
            None
        };

        Ok(Self {
            sx: Cell::new(sx),
            file,
            log_to: log_op.map_or(LogTo::Ephemeral, |_| LogTo::File),
            with_time: false,
            shutdown: shutdown_flag,
        })
    }
    fn open_log_file(op: LoggerFileOptions) -> Result<File, std::io::Error> {
        File::options()
            .write(true)
            .append(op.append_mode)
            .create(true)
            .open(op.path)
    }
}
