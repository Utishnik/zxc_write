//https://github.com/FreerGit/ring-log/blob/main/src/lib.rs#L29

use super::ui_utils::*;
use crossfire::mpsc;
use std::any::Any;
use std::cell::Cell;
use std::fs::File;
use std::io::Error;
use std::io::Write;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

const CHAN_SIZE: usize = 1024;

static INFO_MSG: LazyLock<String> = LazyLock::new(|| write_green("[INFO]"));
static ERR_MSG: LazyLock<String> = LazyLock::new(|| write_red("[ERROR]"));
static DBG_MSG: LazyLock<String> = LazyLock::new(|| write_cyan("[DEBUG]"));
static WARN_MSG: LazyLock<String> = LazyLock::new(|| write_yellow("[WARN]"));

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

const unsafe fn cell_borrow<T>(cell: &Cell<T>) -> &T {
    let ptr = cell.as_ptr() as *const T;
    unsafe { ptr.as_ref_unchecked() }
}

impl Logger {
    fn builder(log_op: Option<LoggerFileOptions>) -> Result<Self, std::io::Error> {
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

    #[track_caller]
    fn log<F, T>(&self, level: String, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let tt = self.with_time;
        let location = std::panic::Location::caller();
        let entry = LogEntry {
            closure: Box::new(move || {
                let file_line = format!(
                    "file: {} line: {} column: {}",
                    location.file(),
                    location.line(),
                    location.column()
                );
                let time = match tt {
                    true => format!(
                        "{}",
                        chrono::offset::Local::now().format("%Y-%m-%d %H:%M:%S ")
                    ),
                    false => String::new(),
                };
                let message = f();
                format!("{}{} {} {}", time, file_line, level, message.as_ref())
            }),
            log_to: self.log_to.clone(),
        };

        unsafe {
            match cell_borrow(&self.sx).send(entry) {
                Ok(_) => (),
                Err(_) => panic!("Logger thread died :("),
            }
        }
    }
    pub const fn with_time(mut self, time: bool) -> Self {
        self.with_time = time;
        self
    }

    /// Waits until all messages are logged
    fn shutdown(&self) -> Result<(), std::io::Error> {
        self.shutdown.store(true, Ordering::Release);
        unsafe {
            while !cell_borrow(&self.sx).is_disconnected() {
                std::thread::yield_now();
            }
        }

        if let Some(ref file) = self.file {
            file.sync_all()?
        }
        Ok(())
    }
    #[track_caller]
    pub fn info<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let info = LazyLock::force(&INFO_MSG).clone(); //бля честно залупа вышла но похуя
        self.log(info, f);
    }

    #[track_caller]
    pub fn error<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let err = LazyLock::force(&ERR_MSG).clone();
        self.log(err, f);
    }
    #[track_caller]
    pub fn debug<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let dbg = LazyLock::force(&DBG_MSG).clone();
        self.log(dbg, f);
    }

    #[track_caller]
    pub fn warning<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let warn = LazyLock::force(&WARN_MSG).clone();
        self.log(warn, f);
    }
}

pub enum LoggerRes<T> {
    Panic(Box<dyn Any + Send>),
    Ok(T),
}

//public no panic
impl Logger {
    pub fn safe_builder(
        log_op: Option<LoggerFileOptions>,
    ) -> LoggerRes<Result<Logger, std::io::Error>> {
        let result = std::panic::catch_unwind(|| Self::builder(log_op));
        match result {
            Ok(ok) => LoggerRes::Ok(ok),
            Err(e) => LoggerRes::Panic(e),
        }
    }
}
