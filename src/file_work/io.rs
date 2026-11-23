//! IO-драйвер поверх `compio::driver::Proactor` (IOCP на Windows) и пула
//! потоков `affinitypool`.
//!
//! Драйвер поддерживает два стиля вызова:
//!
//! * блокирующий — [`IoDriver::read_file`], [`IoDriver::write_file`],
//!   [`IoDriver::append_file`]: выполняются в вызывающем потоке, `Proactor`
//!   создаётся и живёт только на время вызова. Сами операции асинхронные,
//!   поток лишь ждёт завершения в [`Proactor::poll`];
//! * асинхронный — [`IoDriver::spawn_read_file`], [`IoDriver::spawn_write_file`]
//!   и [`IoDriver::spawn_append_file`]: задача уходит в пул драйвера, наружу
//!   возвращается [`Future`], который можно ждать через [`block_on`].
//!
//! Если операций с файлом несколько, удобнее [`FileSession`]: открывает файл
//! один раз и умеет читать/писать по смещению ([`FileSession::read_at`],
//! [`FileSession::write_at`]).

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use affinitypool::{Builder, Threadpool};
use compio::BufResult;
use compio::buf::IntoInner;
use compio::driver::{
    AsRawFd, OpCode, OwnedFd, Proactor, PushEntry, SharedFd,
    op::{CloseFile, ReadAt, WriteAt},
};

/// Размер чанка, которым драйвер читает файл целиком, по умолчанию.
pub const DEFAULT_CHUNK: usize = 64 * 1024;

/// Ошибка операции IO-драйвера.
#[derive(Debug)]
pub enum PushErr {
    /// Не удалось создать `Proactor`, зарегистрировать в нём дескриптор или
    /// опросить его ([`Proactor::new`], [`Proactor::attach`], [`Proactor::poll`]).
    Driver(io::Error),
    /// Не удалось открыть файл.
    Open(io::Error),
    /// Ошибка во время чтения ([`ReadAt`]).
    Read(io::Error),
    /// Ошибка во время записи ([`WriteAt`]).
    Write(io::Error),
    /// Драйверу не удалось записать ни одного байта (файл закрыт на запись).
    WriteZero,
    /// Не удалось явно закрыть файл ([`CloseFile`]).
    Close(io::Error),
    /// Файл уже закрыт либо его дескриптором владеют другие операции.
    Closed,
}

impl PushErr {
    /// Внутренняя [`io::Error`], если у ошибки есть.
    pub const fn io_error(&self) -> Option<&io::Error> {
        match self {
            Self::Driver(err) | Self::Open(err) | Self::Read(err) | Self::Write(err)
            | Self::Close(err) => Some(err),
            Self::WriteZero | Self::Closed => None,
        }
    }
}

impl fmt::Display for PushErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver(err) => write!(f, "ошибка io-драйвера: {err}"),
            Self::Open(err) => write!(f, "не удалось открыть файл: {err}"),
            Self::Read(err) => write!(f, "ошибка чтения: {err}"),
            Self::Write(err) => write!(f, "ошибка записи: {err}"),
            Self::WriteZero => write!(f, "драйвер не смог записать ни одного байта"),
            Self::Close(err) => write!(f, "не удалось закрыть файл: {err}"),
            Self::Closed => write!(f, "файл уже закрыт или дескриптор разделяется"),
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
            | Self::Close(err) => Some(err),
            Self::WriteZero | Self::Closed => None,
        }
    }
}

/// Режим открытия файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenMode {
    /// Только чтение.
    #[default]
    Read,
    /// Запись с созданием и обрезкой файла.
    Write,
    /// Запись в конец файла (создаёт, если нет).
    ///
    /// Смещение вычисляется по `std::fs::metadata` в момент открытия, поэтому
    /// при конкурентной дозаписи из нескольких процессов записи могут
    /// наложиться.
    Append,
    /// Чтение и запись (файл создаётся, если нет).
    ReadWrite,
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

    /// Прочитать файл целиком (в вызывающем потоке).
    ///
    /// # Errors
    /// [`PushErr::Open`], если файл не открылся, [`PushErr::Read`] или
    /// [`PushErr::Driver`] — при ошибке чтения.
    pub fn read_file(&self, path: impl AsRef<Path>) -> Result<Vec<u8>, PushErr> {
        read_file_sync(path.as_ref(), self.chunk_size)
    }

    /// Создать файл и записать в него `data` целиком (обрезая прежнее
    /// содержимое), в вызывающем потоке.
    ///
    /// # Errors
    /// [`PushErr::Open`], [`PushErr::Write`], [`PushErr::Close`] или
    /// [`PushErr::Driver`].
    pub fn write_file(
        &self,
        path: impl AsRef<Path>,
        data: impl Into<Vec<u8>>,
    ) -> Result<usize, PushErr> {
        write_file_sync(path.as_ref(), data.into(), OpenMode::Write)
    }

    /// Дописать `data` в конец файла, в вызывающем потоке.
    ///
    /// # Errors
    /// [`PushErr::Open`], [`PushErr::Write`], [`PushErr::Close`] или
    /// [`PushErr::Driver`].
    pub fn append_file(
        &self,
        path: impl AsRef<Path>,
        data: impl Into<Vec<u8>>,
    ) -> Result<usize, PushErr> {
        write_file_sync(path.as_ref(), data.into(), OpenMode::Append)
    }

    /// Прочитать файл целиком в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::read_file`].
    pub fn spawn_read_file(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<Vec<u8>, PushErr>> + Send + 'static {
        let chunk_size = self.chunk_size;
        self.pool.spawn(move || read_file_sync(&path, chunk_size))
    }

    /// Создать файл и записать в него `data` целиком в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::write_file`].
    pub fn spawn_write_file(
        &self,
        path: PathBuf,
        data: impl Into<Vec<u8>>,
    ) -> impl Future<Output = Result<usize, PushErr>> + Send + 'static {
        let data = data.into();
        self.pool
            .spawn(move || write_file_sync(&path, data, OpenMode::Write))
    }

    /// Дописать `data` в конец файла в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::append_file`].
    pub fn spawn_append_file(
        &self,
        path: PathBuf,
        data: impl Into<Vec<u8>>,
    ) -> impl Future<Output = Result<usize, PushErr>> + Send + 'static {
        let data = data.into();
        self.pool
            .spawn(move || write_file_sync(&path, data, OpenMode::Append))
    }
}

/// Открытый файл: один `Proactor` + один дескриптор.
///
/// Тип не является `Send`: им можно пользоваться только в том потоке, где он
/// был открыт.
pub struct FileSession {
    driver: Proactor,
    fd: SharedFd<OwnedFd>,
}

impl fmt::Debug for FileSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSession").finish_non_exhaustive()
    }
}

impl FileSession {
    /// Открыть файл в заданном режиме.
    ///
    /// # Errors
    /// [`PushErr::Open`], если файл не открылся, [`PushErr::Driver`] — если
    /// не удалось создать `Proactor` или привязать к нему дескриптор.
    pub fn open(path: impl AsRef<Path>, mode: OpenMode) -> Result<Self, PushErr> {
        let mut driver = Proactor::new().map_err(PushErr::Driver)?;
        let fd = open_fd(&mut driver, path.as_ref(), mode)?;
        let fd = SharedFd::new(fd);
        driver.attach(fd.as_raw_fd()).map_err(PushErr::Driver)?;
        Ok(Self { driver, fd })
    }

    /// Прочитать не больше `len` байт, начиная с `offset`.
    ///
    /// Пустой вектор означает, что файл на этом смещении кончился.
    ///
    /// # Errors
    /// [`PushErr::Read`] при ошибке чтения, [`PushErr::Driver`] — при ошибке
    /// драйвера.
    pub fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, PushErr> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let op = ReadAt::new(self.fd.clone(), offset, Vec::with_capacity(len));
        let BufResult(res, op) = push_and_wait(&mut self.driver, op)?;
        let n = res.map_err(PushErr::Read)?;
        let mut buf = op.into_inner();
        debug_assert!(n <= buf.capacity(), "драйвер прочитал больше буфера");
        // SAFETY: драйвер пишет не больше capacity буфера, поэтому байты 0..n
        // гарантированно инициализированы
        unsafe { buf.set_len(n) };
        Ok(buf)
    }

    /// Прочитать файл целиком чанками по `chunk` байт.
    ///
    /// # Errors
    /// [`PushErr::Read`] при ошибке чтения, [`PushErr::Driver`] — при ошибке
    /// драйвера.
    pub fn read_all(&mut self, chunk: usize) -> Result<Vec<u8>, PushErr> {
        let chunk = chunk.max(1);
        let fd = self.fd.clone();
        let mut out: Vec<u8> = Vec::with_capacity(chunk);
        let mut buf: Vec<u8> = Vec::new();
        let mut offset: u64 = 0;
        loop {
            if buf.capacity() < chunk {
                buf = Vec::with_capacity(chunk);
            }
            let op = ReadAt::new(fd.clone(), offset, std::mem::take(&mut buf));
            let BufResult(res, op) = push_and_wait(&mut self.driver, op)?;
            let n = res.map_err(PushErr::Read)?;
            buf = op.into_inner();
            if n == 0 {
                break;
            }
            debug_assert!(n <= buf.capacity(), "драйвер прочитал больше буфера");
            // SAFETY: буфер перед операцией имел capacity >= chunk >= n, а
            // драйвер заполнил байты 0..n
            unsafe { buf.set_len(n) };
            out.append(&mut buf);
            offset += n as u64;
        }
        Ok(out)
    }

    /// Записать `data` целиком, начиная с `offset`.
    ///
    /// Возвращает число записанных байт (всегда равно длине `data`, кроме
    /// ошибки).
    ///
    /// # Errors
    /// [`PushErr::Write`] при ошибке записи, [`PushErr::WriteZero`], если
    /// драйвер не смог записать ни одного байта, [`PushErr::Driver`] — при
    /// ошибке драйвера.
    pub fn write_at(&mut self, offset: u64, data: impl Into<Vec<u8>>) -> Result<usize, PushErr> {
        let mut data = data.into();
        let total = data.len();
        let mut offset = offset;
        while !data.is_empty() {
            let op = WriteAt::new(self.fd.clone(), offset, data);
            let BufResult(res, op) = push_and_wait(&mut self.driver, op)?;
            let n = res.map_err(PushErr::Write)?;
            data = op.into_inner();
            if n == 0 {
                return Err(PushErr::WriteZero);
            }
            if n >= data.len() {
                break;
            }
            // частичная запись: оставляем в буфере только ещё не записанное
            let rest = data.split_off(n);
            data = rest;
            offset += n as u64;
        }
        Ok(total)
    }

    /// Закрыть файл явно через [`CloseFile`].
    ///
    /// Если [`FileSession`] просто отпустить без вызова этой функции,
    /// дескриптор всё равно будет закрыт при удалении последней ссылки
    /// [`SharedFd`].
    ///
    /// # Errors
    /// [`PushErr::Closed`], если дескриптором владеют другие операции,
    /// [`PushErr::Close`] или [`PushErr::Driver`] — при ошибке закрытия.
    pub fn close(self) -> Result<(), PushErr> {
        let Self { mut driver, fd } = self;
        let owned = fd.try_unwrap().map_err(|_| PushErr::Closed)?;
        let BufResult(res, _) = push_and_wait(&mut driver, CloseFile::new(owned))?;
        res.map(|_| ()).map_err(PushErr::Close)
    }
}

/// Мини-экзекьютор для [`Future`] драйвера: ждёт результат прямо здесь.
///
/// Реализован через `thread::park`/`unpark`, поэтому подходит только для
/// будущих, которые просыпаются из другого потока (например, возвращённых
/// [`IoDriver::spawn_read_file`]). Вызывать из воркера пула драйвера нельзя —
/// можно упереться в переполнение пула.
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
fn push_and_wait<O: OpCode + 'static>(
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

/// Открыть файл и привязать его дескриптор к драйверу.
fn open_fd(driver: &mut Proactor, path: &Path, mode: OpenMode) -> Result<OwnedFd, PushErr> {
    use compio::driver::op::Asyncify;

    let mut opts = std::fs::OpenOptions::new();
    match mode {
        OpenMode::Read => {
            opts.read(true);
        }
        OpenMode::Write => {
            opts.write(true).create(true).truncate(true);
        }
        OpenMode::Append => {
            opts.write(true).create(true);
        }
        OpenMode::ReadWrite => {
            opts.read(true).write(true).create(true);
        }
    }
    apply_platform_flags(&mut opts);

    let path = path.to_path_buf();
    let op = Asyncify::new(move || BufResult(opts.open(&path).map(file_to_raw), ()));
    let BufResult(res, _) = push_and_wait(driver, op)?;
    let raw = res.map_err(PushErr::Open)?;
    // SAFETY: raw получен из file_to_raw и никуда не передавался
    Ok(unsafe { raw_to_owned(raw) })
}

/// Дескриптор файла в сыром виде, который умеет ждать `Asyncify`.
#[cfg(windows)]
fn file_to_raw(file: std::fs::File) -> usize {
    use std::os::windows::io::IntoRawHandle;

    file.into_raw_handle() as usize
}

/// Обратная операция к [`file_to_raw`].
///
/// # Safety
/// `raw` должен быть получен из [`file_to_raw`] и не должен быть закрыт.
#[cfg(windows)]
unsafe fn raw_to_owned(raw: usize) -> OwnedFd {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};

    OwnedFd::File(unsafe { OwnedHandle::from_raw_handle(raw as _) })
}

/// Дескриптор файла в сыром виде, который умеет ждать `Asyncify`.
#[cfg(unix)]
fn file_to_raw(file: std::fs::File) -> usize {
    use std::os::unix::io::IntoRawFd;

    file.into_raw_fd() as usize
}

/// Обратная операция к [`file_to_raw`].
///
/// # Safety
/// `raw` должен быть получен из [`file_to_raw`] и не должен быть закрыт.
#[cfg(unix)]
unsafe fn raw_to_owned(raw: usize) -> OwnedFd {
    use std::os::unix::io::FromRawFd;

    unsafe { OwnedFd::from_raw_fd(raw as _) }
}

/// Плагин свойств открытия файла, зависящих от платформы.
#[cfg(windows)]
fn apply_platform_flags(opts: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

    // IOCP принимает на порт только дескрипторы, открытые с этим флагом
    opts.attributes(FILE_FLAG_OVERLAPPED);
}

/// Плагин свойств открытия файла, зависящих от платформы.
#[cfg(unix)]
fn apply_platform_flags(_opts: &mut std::fs::OpenOptions) {}

/// Прочитать файл целиком в текущем потоке.
fn read_file_sync(path: &Path, chunk: usize) -> Result<Vec<u8>, PushErr> {
    let mut session = FileSession::open(path, OpenMode::Read)?;
    let data = session.read_all(chunk);
    with_close(data, session)
}

/// Записать файл в текущем потоке: создание/дозапись в зависимости от режима.
fn write_file_sync(path: &Path, data: Vec<u8>, mode: OpenMode) -> Result<usize, PushErr> {
    // для дозаписи смещение считаем до открытия: дальше файл не трогаем
    let offset = if mode == OpenMode::Append {
        file_len(path)
    } else {
        0
    };
    let mut session = FileSession::open(path, mode)?;
    let written = session.write_at(offset, data);
    with_close(written, session)
}

/// Закрыть файл и вернуть результат основной операции: ошибка закрытия
/// интересна только если основная операция прошла нормально.
fn with_close<T>(res: Result<T, PushErr>, session: FileSession) -> Result<T, PushErr> {
    let closed = session.close();
    res.and_then(|value| closed.map(|()| value))
}

/// Размер файла, 0 если файла нет.
fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zxc_write_io_{}_{name}", std::process::id()))
    }

    /// Убрать за собой: файла могло и не быть.
    fn cleanup(path: &Path) {
        let _: io::Result<()> = std::fs::remove_file(path);
    }

    #[test]
    fn read_write_append_roundtrip() {
        let path = test_path("roundtrip");
        cleanup(&path);
        let driver = IoDriver::new();

        let written = driver
            .write_file(&path, &b"hello driver"[..])
            .expect("write_file должен записать файл");
        assert_eq!(written, 12, "write_file должен вернуть длину данных");

        let data = driver
            .read_file(&path)
            .expect("read_file должен прочитать файл");
        assert_eq!(
            data,
            b"hello driver".to_vec(),
            "содержимое должно совпасть"
        );

        driver
            .append_file(&path, &b"!"[..])
            .expect("append_file должен дописать файл");
        let data = driver
            .read_file(&path)
            .expect("read_file должен прочитать файл после дозаписи");
        assert_eq!(
            data,
            b"hello driver!".to_vec(),
            "дозапись должна идти в конец файла"
        );

        cleanup(&path);
    }

    #[test]
    fn async_read_and_session() {
        let path = test_path("async");
        cleanup(&path);
        let driver = IoDriver::new();
        driver
            .write_file(&path, &b"0123456789"[..])
            .expect("write_file должен записать файл");

        let fut = driver.spawn_read_file(path.clone());
        let data = block_on(fut).expect("spawn_read_file должен прочитать файл");
        assert_eq!(
            data,
            b"0123456789".to_vec(),
            "async чтение должно вернуть то же содержимое"
        );

        let mut session =
            FileSession::open(&path, OpenMode::Read).expect("FileSession::open должен открыть файл");
        let part = session
            .read_at(0, 4)
            .expect("read_at должен прочитать кусок");
        assert_eq!(part, b"0123".to_vec(), "read_at(0, 4)");
        let tail = session.read_at(7, 100).expect("read_at за концом файла");
        assert_eq!(
            tail,
            b"789".to_vec(),
            "read_at за концом файла отдаёт остаток"
        );
        let all = session
            .read_all(3)
            .expect("read_all должен прочитать файл целиком");
        assert_eq!(
            all,
            b"0123456789".to_vec(),
            "read_all должен читать до конца файла"
        );
        session.close().expect("close должен закрыть файл");

        cleanup(&path);
    }

    #[test]
    fn missing_file_is_open_error() {
        let driver = IoDriver::new();
        let missing = std::env::temp_dir().join("zxc_write_io_no_such_file_zxc");
        let err = driver
            .read_file(&missing)
            .expect_err("чтение несуществующего файла должно падать");
        assert!(
            matches!(err, PushErr::Open(_)),
            "ожидалась ошибка открытия, получено {err:?}"
        );
    }
}
