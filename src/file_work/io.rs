//! Устройство IO-драйвера: [`IoDriver`], [`PushErr`], [`block_on`] и
//! низкоуровневые хелперы операций.
//!
//! Модуль не знает о конкретных объектах работы: файловые операции живут в
//! [`crate::file_work::file`], операции с директориями — в
//! [`crate::file_work::dir`]. Здесь то, что нужно обоим: пул потоков, ошибка
//! [`PushErr`], ожидание операции и мини-экзекьютор [`block_on`].
//!
//! Драйвер поддерживает два стиля вызова:
//!
//! * блокирующий — операции вида `read_file`/`create_dir` выполняются в
//!   вызывающем потоке. Файловые операции при этом всё равно асинхронные:
//!   `Proactor` создаётся и живёт только на время вызова, поток лишь ждёт
//!   завершения в [`Proactor::poll`];
//! * асинхронный — `spawn_*`: задача уходит в пул драйвера, наружу
//!   возвращается [`Future`], который можно ждать через [`block_on`].

use std::fmt;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use affinitypool::{Builder, Threadpool};
use compio::BufResult;
use compio::driver::{OpCode, OwnedFd, Proactor, PushEntry};

/// Размер чанка, которым драйвер читает файл целиком, по умолчанию.
pub const DEFAULT_CHUNK: usize = 64 * 1024;

/// Ошибка операции IO-драйвера.
#[derive(Debug)]
pub enum PushErr {
    /// Не удалось создать `Proactor`, зарегистрировать в нём дескриптор или
    /// опросить его ([`Proactor::new`], [`Proactor::attach`], [`Proactor::poll`]).
    Driver(io::Error),
    /// Не удалось открыть файл или директорию.
    Open(io::Error),
    /// Ошибка во время чтения файла ([`compio::driver::op::ReadAt`]).
    Read(io::Error),
    /// Ошибка во время записи файла ([`compio::driver::op::WriteAt`]).
    Write(io::Error),
    /// Драйверу не удалось записать ни одного байта (файл закрыт на запись).
    WriteZero,
    /// Не удалось создать директорию.
    Create(io::Error),
    /// Не удалось удалить директорию.
    Remove(io::Error),
    /// Не удалось прочитать содержимое директории.
    List(io::Error),
    /// Недопустимое имя вложенного объекта: пустое, абсолютное, содержащее
    /// компоненты `.`/`..` или разделители пути.
    BadName(String),
    /// Не удалось явно закрыть файл ([`compio::driver::op::CloseFile`]).
    Close(io::Error),
    /// Объект уже закрыт либо его дескриптором владеют другие операции.
    Closed,
}

impl PushErr {
    /// Внутренняя [`io::Error`], если у ошибки есть.
    pub const fn io_error(&self) -> Option<&io::Error> {
        match self {
            Self::Driver(err)
            | Self::Open(err)
            | Self::Read(err)
            | Self::Write(err)
            | Self::Create(err)
            | Self::Remove(err)
            | Self::List(err)
            | Self::Close(err) => Some(err),
            Self::WriteZero | Self::BadName(_) | Self::Closed => None,
        }
    }
}

impl fmt::Display for PushErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver(err) => write!(f, "ошибка io-драйвера: {err}"),
            Self::Open(err) => write!(f, "не удалось открыть файл или директорию: {err}"),
            Self::Read(err) => write!(f, "ошибка чтения: {err}"),
            Self::Write(err) => write!(f, "ошибка записи: {err}"),
            Self::WriteZero => write!(f, "драйвер не смог записать ни одного байта"),
            Self::Create(err) => write!(f, "не удалось создать директорию: {err}"),
            Self::Remove(err) => write!(f, "не удалось удалить директорию: {err}"),
            Self::List(err) => write!(f, "не удалось прочитать директорию: {err}"),
            Self::BadName(name) => write!(f, "недопустимое имя вложенного объекта: {name:?}"),
            Self::Close(err) => write!(f, "не удалось закрыть файл: {err}"),
            Self::Closed => write!(f, "объект уже закрыт или дескриптор разделяется"),
        }
    }
}

impl std::error::Error for PushErr {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Driver(err)
            | Self::Open(err)
            | Self::Read(err)
            | Self::Write(err)
            | Self::Create(err)
            | Self::Remove(err)
            | Self::List(err)
            | Self::Close(err) => Some(err),
            Self::WriteZero | Self::BadName(_) | Self::Closed => None,
        }
    }
}

/// Настройки IO-драйвера.
#[derive(Debug, Clone)]
pub struct IoDriverConfig {
    /// Число потоков пула. `None` — значение по умолчанию `affinitypool`
    /// (2 потока, либо по одному на ядро при [`thread_per_core`](Self::thread_per_core)).
    pub num_threads: Option<usize>,
    /// Запускать по потоку на ядро, если [`num_threads`](Self::num_threads)
    /// не задан.
    pub thread_per_core: bool,
    /// Размер стека потока. `None` — стандартный размер из `std::thread`.
    pub thread_stack_size: Option<usize>,
    /// Число шардов очереди пула.
    pub shards: usize,
    /// Размер чанка, которым читается файл целиком.
    pub chunk_size: usize,
}

impl Default for IoDriverConfig {
    fn default() -> Self {
        Self {
            num_threads: None,
            thread_per_core: true,
            thread_stack_size: None,
            shards: 4,
            chunk_size: DEFAULT_CHUNK,
        }
    }
}

/// Собрать пул потоков под настройки драйвера.
fn build_pool(cfg: &IoDriverConfig) -> Threadpool {
    let mut builder = Builder::new()
        .shards(cfg.shards)
        .thread_per_core(cfg.thread_per_core);
    // worker_threads перекрывает thread_per_core, поэтому ставим его последним
    if let Some(num_threads) = cfg.num_threads {
        builder = builder.worker_threads(num_threads);
    }
    if let Some(thread_stack_size) = cfg.thread_stack_size {
        builder = builder.thread_stack_size(thread_stack_size);
    }
    builder.build()
}

/// IO-драйвер: пул потоков, задачи которого живут со своим `Proactor`.
///
/// [`Proactor`] не является `Send`, поэтому он никогда не покидает поток, в
/// котором был создан: в блокирующем API — вызывающий поток, в async API —
/// поток пула.
///
/// Операции с файлами (`read_file`, `write_file`, `spawn_*`) описаны в
/// [`crate::file_work::file`], с директориями (`create_dir`, `list_dir`,
/// `spawn_*`) — в [`crate::file_work::dir`].
pub struct IoDriver {
    pool: Arc<Threadpool>,
    chunk_size: usize,
}

impl fmt::Debug for IoDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoDriver")
            .field("chunk_size", &self.chunk_size)
            .finish_non_exhaustive()
    }
}

impl Clone for IoDriver {
    fn clone(&self) -> Self {
        Self {
            pool: Arc::clone(&self.pool),
            chunk_size: self.chunk_size,
        }
    }
}

impl Default for IoDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl IoDriver {
    /// Создать драйвер с настройками по умолчанию.
    pub fn new() -> Self {
        Self::with_config(IoDriverConfig::default())
    }

    /// Создать драйвер с заданными настройками.
    pub fn with_config(cfg: IoDriverConfig) -> Self {
        let chunk_size = cfg.chunk_size.max(1);
        let pool = Arc::new(build_pool(&cfg));
        Self { pool, chunk_size }
    }

    /// Размер чанка, которым драйвер читает файл целиком.
    pub const fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Пул потоков драйвера.
    pub fn pool(&self) -> &Threadpool {
        &self.pool
    }

    /// Запустить произвольную блокирующую задачу в пуле драйвера.
    ///
    /// Возвращает [`Future`], который разрешится, когда задача выполнится.
    pub fn spawn<F, R>(&self, f: F) -> impl Future<Output = R> + Send + 'static
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.pool.spawn(f)
    }
}

/// Мини-экзекьютор для [`Future`] драйвера: ждёт результат прямо здесь.
///
/// Реализован через `thread::park`/`unpark`, поэтому подходит только для
/// будущих, которые просыпаются из другого потока (например, возвращённых
/// `spawn_*`). Вызывать из воркера пула драйвера нельзя — можно упереться в
/// переполнение пула.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// [`Wake`], будящий поток через [`std::thread::Thread::unpark`].
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Запустить операцию и дождаться её завершения.
pub(crate) fn push_and_wait<O: OpCode + 'static>(
    driver: &mut Proactor,
    op: O,
) -> Result<BufResult<usize, O>, PushErr> {
    match driver.push(op) {
        PushEntry::Ready(res) => Ok(res),
        PushEntry::Pending(mut user_data) => loop {
            driver.poll(None).map_err(PushErr::Driver)?;
            match driver.pop(user_data) {
                PushEntry::Pending(key) => user_data = key,
                PushEntry::Ready(res) => return Ok(res),
            }
        },
    }
}

/// Дескриптор файла в сыром виде, который умеет ждать `Asyncify`.
#[cfg(windows)]
pub(crate) fn file_to_raw(file: std::fs::File) -> usize {
    use std::os::windows::io::IntoRawHandle;

    file.into_raw_handle() as usize
}

/// Обратная операция к [`file_to_raw`].
///
/// # Safety
/// `raw` должен быть получен из [`file_to_raw`] и не должен быть закрыт.
#[cfg(windows)]
pub(crate) unsafe fn raw_to_owned(raw: usize) -> OwnedFd {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};

    OwnedFd::File(unsafe { OwnedHandle::from_raw_handle(raw as _) })
}

/// Дескриптор файла в сыром виде, который умеет ждать `Asyncify`.
#[cfg(unix)]
pub(crate) fn file_to_raw(file: std::fs::File) -> usize {
    use std::os::unix::io::IntoRawFd;

    file.into_raw_fd() as usize
}

/// Обратная операция к [`file_to_raw`].
///
/// # Safety
/// `raw` должен быть получен из [`file_to_raw`] и не должен быть закрыт.
#[cfg(unix)]
pub(crate) unsafe fn raw_to_owned(raw: usize) -> OwnedFd {
    use std::os::unix::io::FromRawFd;

    unsafe { OwnedFd::from_raw_fd(raw as _) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_returns_ready_output() {
        let value = block_on(async { 6 * 7 });
        assert_eq!(
            value, 42,
            "block_on должен вернуть результат готового future"
        );
    }

    #[test]
    fn push_err_keeps_io_error_only_where_it_wraps_one() {
        let closed = PushErr::Closed;
        assert!(
            closed.io_error().is_none(),
            "Closed не должен нести внутреннюю io::Error"
        );
        let bad_name = PushErr::BadName("..".to_string());
        assert!(
            bad_name.io_error().is_none(),
            "BadName не должен нести внутреннюю io::Error"
        );
        let create = PushErr::Create(io::Error::from(io::ErrorKind::AlreadyExists));
        assert!(
            create.io_error().is_some(),
            "Create должен нести внутреннюю io::Error"
        );
    }
}
