//! Файлы: [`OpenMode`], [`FileSession`] и файловые операции [`IoDriver`].
//!
//! [`FileSession`] — RAII-структура: файл закрывается явно в
//! [`FileSession::close`] и автоматически при [`Drop`], причём дескриптор
//! забирается ровно в одном из этих двух мест, поэтому двойного закрытия не
//! бывает. После закрытия любая операция возвращает [`PushErr::Closed`].
//!
//! Если операций с файлом несколько, удобнее [`FileSession`]: открывает файл
//! один раз и умеет читать/писать по смещению ([`FileSession::read_at`],
//! [`FileSession::write_at`]).

use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};

use compio::BufResult;
use compio::buf::IntoInner;
use compio::driver::{
    AsRawFd, OwnedFd, Proactor, SharedFd,
    op::{CloseFile, ReadAt, WriteAt},
};

use super::io::{IoDriver, PushErr, file_to_raw, push_and_wait, raw_to_owned};

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

/// Открытый файл: один `Proactor` + один дескриптор.
///
/// RAII-контракт:
/// * [`FileSession::close`] — явное закрытие через операцию [`CloseFile`];
/// * [`Drop`] — гарантирует закрытие дескриптора, даже если `close()` не
///   вызывался (и ничего не делает, если файл уже закрыт: дескриптор
///   забирается ровно в одном из двух мест, поэтому двойного закрытия нет);
/// * после закрытия любая операция возвращает [`PushErr::Closed`] вместо
///   работы с чужим дескриптором.
///
/// Тип не является `Send`: им можно пользоваться только в том потоке, где он
/// был открыт.
pub struct FileSession {
    driver: Proactor,
    /// [`None`] после [`FileSession::close`].
    fd: Option<SharedFd<OwnedFd>>,
}

impl fmt::Debug for FileSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSession")
            .field("open", &self.is_open())
            .finish_non_exhaustive()
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
        Ok(Self {
            driver,
            fd: Some(fd),
        })
    }

    /// Открыт ли файл.
    pub const fn is_open(&self) -> bool {
        self.fd.is_some()
    }

    /// Дескриптор для очередной операции.
    ///
    /// Возвращает [`PushErr::Closed`], если файл уже закрыт: так операции не
    /// могут получить чужой или закрытый дескриптор.
    fn fd(&self) -> Result<SharedFd<OwnedFd>, PushErr> {
        self.fd.clone().ok_or(PushErr::Closed)
    }

    /// Прочитать не больше `len` байт, начиная с `offset`.
    ///
    /// Пустой вектор означает, что файл на этом смещении кончился.
    ///
    /// # Errors
    /// [`PushErr::Read`] при ошибке чтения, [`PushErr::Closed`] если файл
    /// закрыт, [`PushErr::Driver`] — при ошибке драйвера.
    pub fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, PushErr> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let op = ReadAt::new(self.fd()?, offset, Vec::with_capacity(len));
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
    /// [`PushErr::Read`] при ошибке чтения, [`PushErr::Closed`] если файл
    /// закрыт, [`PushErr::Driver`] — при ошибке драйвера.
    pub fn read_all(&mut self, chunk: usize) -> Result<Vec<u8>, PushErr> {
        let chunk = chunk.max(1);
        let fd = self.fd()?;
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
    /// драйвер не смог записать ни одного байта, [`PushErr::Closed`] если файл
    /// закрыт, [`PushErr::Driver`] — при ошибке драйвера.
    pub fn write_at(&mut self, offset: u64, data: impl Into<Vec<u8>>) -> Result<usize, PushErr> {
        let mut data = data.into();
        let total = data.len();
        let mut offset = offset;
        while !data.is_empty() {
            let op = WriteAt::new(self.fd()?, offset, data);
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
    /// Функцию можно вызывать повторно: повторное закрытие (и любая операция
    /// после закрытия) возвращает [`PushErr::Closed`], а дескриптор при этом
    /// не закрывается второй раз. Если `close()` не вызывать, файл закроет
    /// [`Drop`].
    ///
    /// # Errors
    /// [`PushErr::Closed`], если файл уже закрыт или дескриптором владеют
    /// другие операции, [`PushErr::Close`] — если `CloseHandle` не прошёл,
    /// [`PushErr::Driver`] — если драйвер упал во время операции (в этом
    /// случае дескриптор уже перемещён в драйвер и может не быть закрыт).
    pub fn close(&mut self) -> Result<(), PushErr> {
        let Some(fd) = self.fd.take() else {
            return Err(PushErr::Closed);
        };
        let owned = match fd.try_unwrap() {
            Ok(owned) => owned,
            Err(fd) => {
                // дескриптором владеют другие операции: возвращаем его,
                // чтобы закрыл Drop, и не притворяемся, что закрыли
                self.fd = Some(fd);
                return Err(PushErr::Closed);
            }
        };
        let BufResult(res, _) = push_and_wait(&mut self.driver, CloseFile::new(owned))?;
        res.map(|_| ()).map_err(PushErr::Close)
    }
}

impl Drop for FileSession {
    fn drop(&mut self) {
        // RAII: отпускаем дескриптор — его закроет последний SharedFd.
        // Намеренно не операцией CloseFile через драйвер: падение не должно
        // зависеть от состояния драйвера и не должно уметь возвращать ошибку.
        // Двойное закрытие исключено: fd забирается либо здесь, либо в
        // FileSession::close().
        drop(self.fd.take());
    }
}

impl IoDriver {
    /// Прочитать файл целиком (в вызывающем потоке).
    ///
    /// # Errors
    /// [`PushErr::Open`], если файл не открылся, [`PushErr::Read`] или
    /// [`PushErr::Driver`] — при ошибке чтения.
    pub fn read_file(&self, path: impl AsRef<Path>) -> Result<Vec<u8>, PushErr> {
        read_file_sync(path.as_ref(), self.chunk_size())
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
        let chunk_size = self.chunk_size();
        self.pool().spawn(move || read_file_sync(&path, chunk_size))
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
        self.pool()
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
        self.pool()
            .spawn(move || write_file_sync(&path, data, OpenMode::Append))
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
    with_close(data, &mut session)
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
    with_close(written, &mut session)
}

/// Закрыть файл и вернуть результат основной операции: ошибка закрытия
/// интересна только если основная операция прошла нормально.
fn with_close<T>(res: Result<T, PushErr>, session: &mut FileSession) -> Result<T, PushErr> {
    let closed = session.close();
    res.and_then(|value| closed.map(|()| value))
}

/// Размер файла, 0 если файла нет.
fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

#[cfg(test)]
mod tests {
    use super::super::io::block_on;
    use super::super::testing::{cleanup_file, test_path};
    use super::*;

    #[cfg(windows)]
    use super::super::testing::{try_open_exclusive, wait_until_free};

    #[test]
    fn read_write_append_roundtrip() {
        let path = test_path("file", "roundtrip");
        cleanup_file(&path);
        let driver = IoDriver::new();

        let written = driver
            .write_file(&path, &b"hello driver"[..])
            .expect("write_file должен записать файл");
        assert_eq!(written, 12, "write_file должен вернуть длину данных");

        let data = driver
            .read_file(&path)
            .expect("read_file должен прочитать файл");
        assert_eq!(data, b"hello driver".to_vec(), "содержимое должно совпасть");

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

        cleanup_file(&path);
    }

    #[test]
    fn async_read_and_session() {
        let path = test_path("file", "async");
        cleanup_file(&path);
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

        let mut session = FileSession::open(&path, OpenMode::Read)
            .expect("FileSession::open должен открыть файл");
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

        cleanup_file(&path);
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

    /// Работа после закрытия и повторное закрытие не должны трогать
    /// чужой/закрытый дескриптор.
    #[test]
    fn session_use_after_close_is_error() {
        let path = test_path("file", "use_after_close");
        cleanup_file(&path);
        let driver = IoDriver::new();
        driver
            .write_file(&path, &b"data"[..])
            .expect("write_file должен записать файл");

        let mut session = FileSession::open(&path, OpenMode::Read)
            .expect("FileSession::open должен открыть файл");
        assert!(session.is_open(), "после open сессия должна быть открытой");
        let part = session
            .read_at(0, 4)
            .expect("read_at должен работать до закрытия");
        assert_eq!(part, b"data".to_vec(), "read_at(0, 4)");

        session.close().expect("первое close должно пройти");
        assert!(
            !session.is_open(),
            "после close сессия должна быть закрытой"
        );

        let err = session
            .read_at(0, 1)
            .expect_err("чтение после close должно падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );

        let err = session
            .write_at(0, &b"x"[..])
            .expect_err("запись после close должна падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );

        // повторное закрытие не должно закрывать дескриптор второй раз
        let err = session.close().expect_err("повторное close должно падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );

        cleanup_file(&path);
    }

    /// RAII: живая сессия занимает файл, а close() и Drop отпускают его.
    #[cfg(windows)]
    #[test]
    fn session_raii_closes_handle() {
        let path = test_path("file", "raii");
        cleanup_file(&path);
        let driver = IoDriver::new();
        driver
            .write_file(&path, &b"x"[..])
            .expect("write_file должен записать файл");

        // путь 1: явное закрытие
        let mut session = FileSession::open(&path, OpenMode::Read)
            .expect("FileSession::open должен открыть файл");
        assert!(
            !try_open_exclusive(&path, false),
            "файл должен быть занят живой сессией"
        );
        session.close().expect("close должен закрыть файл");
        assert!(
            wait_until_free(&path, false),
            "после close файл должен быть полностью свободен"
        );

        // путь 2: закрытие через Drop
        let session = FileSession::open(&path, OpenMode::Read)
            .expect("FileSession::open должен открыть файл");
        assert!(
            !try_open_exclusive(&path, false),
            "файл должен быть занят живой сессией"
        );
        drop(session);
        assert!(
            wait_until_free(&path, false),
            "после Drop файл должен быть полностью свободен"
        );

        cleanup_file(&path);
    }
}
