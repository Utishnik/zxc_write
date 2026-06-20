//https://github.com/FreerGit/ring-log/blob/main/src/lib.rs#L29

use super::ui_utils::*;
use crossfire::mpsc;
use std::any::Any;
use std::cell::Cell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::thread::JoinHandle;

const CHAN_SIZE: usize = 1024;

static INFO_MSG: LazyLock<String> = LazyLock::new(|| write_green("[INFO]"));
static ERR_MSG: LazyLock<String> = LazyLock::new(|| write_red("[ERROR]"));
static DBG_MSG: LazyLock<String> = LazyLock::new(|| write_cyan("[DEBUG]"));
static WARN_MSG: LazyLock<String> = LazyLock::new(|| write_yellow("[WARN]"));

#[derive(Clone)]
pub enum LogTo {
    Ephemeral,
    File,
    InRam,
}

struct LogEntry {
    closure: Box<dyn FnOnce() -> String + Send>,
    log_to: LogTo,
}

pub struct Logger {
    /// `Option` чтобы `shutdown()` мог закрыть канал, дропнув MTx.
    sx: Cell<Option<crossfire::MTx<mpsc::Array<LogEntry>>>>,
    file: Option<File>,
    log_to: LogTo,
    with_time: bool,
    shutdown: Arc<AtomicBool>,
    mem_ptr: Arc<RwLock<Vec<String>>>,
    /// Считает только сообщения "в полёте" — после `send` и до обработки writer-ом.
    sender_cnt: Arc<AtomicUsize>,
    /// Хэндл writer-потока чтобы корректно его дождаться в `shutdown`.
    thread_handle: Option<JoinHandle<()>>,
}

/// Оставлено для совместимости с публичным API. Внутренне больше не используется —
/// writer-поток сам декрементирует счётчик после обработки сообщения.
#[derive(Debug)]
pub struct SenderCntGuard(pub Arc<AtomicUsize>);

impl Drop for SenderCntGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
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
    fn builder(
        log_op: Option<LoggerFileOptions>,
        log_in_ram: Option<Arc<Mutex<Vec<String>>>>,
    ) -> Result<Self, std::io::Error> {
        let (sx, rx) = mpsc::bounded_blocking::<LogEntry>(CHAN_SIZE);

        let shutdown_flag = Arc::new(AtomicBool::new(false));
        // `shutdown_flag` остаётся в публичной структуре Logger на случай
        // если внешний код захочет проверить состояние, но writer-поток больше
        // его не использует — выход определяется Err от закрытого канала.
        let _shutdown_flag_for_logger = shutdown_flag.clone();
        let buf_ram: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(Vec::new())); //rwlock
        let thread_clone = buf_ram.clone();
        let in_ram = log_in_ram.is_some();
        let sender_cnt = Arc::new(AtomicUsize::new(0));
        let sender_cnt_clone = sender_cnt.clone();

        let thread_handle = std::thread::spawn(move || {
            let file = log_op.map(Self::open_log_file);
            let checked_file: Option<File> = if let Some(ref x) = file
                && x.is_err()
            {
                println!("[ERROR FILE]"); //TODO
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
                    // Канал закрыт со стороны Logger::shutdown() — корректный выход.
                    Err(_) => break,
                    Ok(entry) => {
                        let mut message = (entry.closure)();

                        match entry.log_to {
                            LogTo::File => {
                                if file.as_mut().is_some() {
                                    message.push('\n');
                                    let f = unsafe { file.as_mut().unwrap_unchecked() };
                                    if let Err(e) = f.write_all(message.as_bytes()) {
                                        eprintln!("[LOGGER] write_all failed: {:?}", e);
                                    }
                                    if let Err(e) = f.flush() {
                                        eprintln!("[LOGGER] flush failed: {:?}", e);
                                    }
                                }
                            }
                            LogTo::Ephemeral => println!("{}", message),
                            LogTo::InRam => {
                                if in_ram {
                                    let guard = &mut thread_clone.write();
                                    match guard {
                                        Ok(guard) => {
                                            guard.push(message);
                                        }
                                        Err(e) => {
                                            println!("Logger отравлен: {:?}", e);
                                        }
                                    }
                                }
                            }
                        };

                        // Сообщение обработано — уменьшаем счётчик "в полёте".
                        sender_cnt_clone.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }

            // Дренаж внутреннего буфера в переданный Vec — после остановки приёма.
            let r_guard = thread_clone.read();
            match r_guard {
                Ok(ref r_guard) => {
                    if in_ram {
                        match log_in_ram {
                            Some(x) => {
                                let guard = x.lock();
                                match guard {
                                    Ok(mut guard) => {
                                        for item in r_guard.iter() {
                                            guard.push(item.clone());
                                        }
                                    }
                                    Err(e) => {
                                        println!("Logger отравлен: {:?}", e);
                                    }
                                }
                            }
                            // in_ram == true ⇔ log_in_ram.is_some(), так что сюда
                            // попасть не должны. Пишем в stderr вместо паники,
                            // чтобы не валить процесс из-за внутреннего бага.
                            None => {
                                eprintln!(
                                    "[LOGGER] internal inconsistency: in_ram=true but log_in_ram=None"
                                );
                            }
                        }
                    }
                }
                Err(ref e) => {
                    println!("Logger отравлен: {:?}", e);
                }
            }
            drop(r_guard);
            drop(thread_clone);
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
        let mem_ptr = buf_ram;
        Ok(Self {
            sx: Cell::new(Some(sx)),
            file,
            log_to: log_op.map_or(LogTo::Ephemeral, |_| LogTo::File),
            with_time: false,
            shutdown: shutdown_flag,
            mem_ptr,
            sender_cnt,
            thread_handle: Some(thread_handle),
        })
    }

    fn open_log_file(op: LoggerFileOptions) -> Result<File, std::io::Error> {
        File::options()
            .write(true)
            .append(op.append_mode)
            .create(true)
            .open(op.path)
    }

    /// Общая отправка сообщения в канал. Увеличивает `sender_cnt` ТОЛЬКО после
    /// успешного `send` — writer-поток сам уменьшит его после обработки.
    #[track_caller]
    fn send_entry(&self, entry: LogEntry) {
        unsafe {
            let sx_opt = cell_borrow(&self.sx);
            match sx_opt.as_ref() {
                Some(sx) => match sx.send(entry) {
                    Ok(_) => {
                        self.sender_cnt.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => panic!("Logger thread died :("),
                },
                None => panic!("Logger already shut down"),
            }
        }
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
        self.send_entry(entry);
    }

    pub const fn with_time(mut self, time: bool) -> Self {
        self.with_time = time;
        self
    }

    /// Корректно останавливает writer-поток:
    /// 1) ждёт обработки всех сообщений в полёте;
    /// 2) закрывает канал (дропает `MTx`) — writer получает `Err` и выходит;
    /// 3) дожидается реального завершения writer-потока через `JoinHandle`.
    pub fn shutdown(&mut self) -> Result<(), std::io::Error> {
        // Сигнализируем о намерении остановиться.
        self.shutdown.store(true, Ordering::Release);

        // Ждём, пока writer обработает всё, что уже отправлено.
        while self.sender_cnt.load(Ordering::Relaxed) != 0 {
            std::thread::yield_now();
        }

        // Закрываем канал: take() заменяет содержимое Cell на None,
        // оригинальный MTx дропается → writer.recv() вернёт Err и break.
        let _ = self.sx.get_mut().take();

        // Дожидаемся реального завершения writer-потока.
        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(()) => {}
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Logger writer thread panicked",
                    ));
                }
            }
        }

        if let Some(ref file) = self.file {
            file.sync_all()?;
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

    //untrack
    #[track_caller]
    fn untrack_log<F, T>(&self, level: String, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let tt = self.with_time;
        let entry = LogEntry {
            closure: Box::new(move || {
                let time = match tt {
                    true => format!(
                        "{}",
                        chrono::offset::Local::now().format("%Y-%m-%d %H:%M:%S ")
                    ),
                    false => String::new(),
                };
                let message = f();
                format!("{} {} {}", time, level, message.as_ref())
            }),
            log_to: self.log_to.clone(),
        };
        self.send_entry(entry);
    }

    #[track_caller]
    pub fn untrack_info<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let info = LazyLock::force(&INFO_MSG).clone(); //бля честно залупа вышла но похуя
        self.untrack_log(info, f);
    }

    #[track_caller]
    pub fn untrack_error<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let err = LazyLock::force(&ERR_MSG).clone();
        self.untrack_log(err, f);
    }

    #[track_caller]
    pub fn untrack_debug<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let dbg = LazyLock::force(&DBG_MSG).clone();
        self.untrack_log(dbg, f);
    }

    #[track_caller]
    pub fn untrack_warning<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let warn = LazyLock::force(&WARN_MSG).clone();
        self.untrack_log(warn, f);
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
        log_in_ram: Option<Arc<Mutex<Vec<String>>>>,
    ) -> LoggerRes<Result<Self, std::io::Error>> {
        let result = std::panic::catch_unwind(|| Self::builder(log_op, log_in_ram));
        match result {
            Ok(ok) => LoggerRes::Ok(ok),
            Err(e) => LoggerRes::Panic(e),
        }
    }
}

//TODO TEST ADD
