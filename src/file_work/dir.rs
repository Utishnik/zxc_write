//! Директории: [`DirEntry`], [`DirSession`] и операции [`IoDriver`].
//!
//! В отличие от файлов, у директорий в `compio` нет overlapped-операций:
//! `create_dir`/`remove_dir`/`read_dir` — это всегда блокирующие вызовы
//! `std::fs`. Поэтому:
//!
//! * [`IoDriver`] выполняет их прямо в вызывающем потоке, а в `spawn_*` —
//!   в пуле драйвера;
//! * [`DirSession`] не держит `Proactor`: ей нужен только дескриптор
//!   директории, который она держит открытым до [`DirSession::close`] или
//!   [`Drop`].
//!
//! У сессии есть обе версии каждой операции:
//!
//! * синхронные — [`DirSession::list`], [`DirSession::create_dir`],
//!   [`DirSession::remove_dir`], [`DirSession::close`]: выполняются
//!   сразу, в вызывающем потоке;
//! * асинхронные — [`DirSession::spawn_list`],
//!   [`DirSession::spawn_create_dir`], [`DirSession::spawn_remove_dir`],
//!   [`DirSession::spawn_close`], плюс [`IoDriver::spawn_open_dir`]:
//!   задача уходит в пул `affinitypool`, наружу возвращается [`Future`],
//!   который ждётся через [`block_on`](crate::file_work::block_on).
//!
//! Пул сессия берёт у драйвера, с которым открыта ([`IoDriver::open_dir`]),
//! и разделяет его с другими сессиями: пул создаётся один раз на драйвер.
//!
//! RAII-контракт [`DirSession`] совпадает с [`crate::file_work::file::FileSession`]:
//! дескриптор забирается ровно в одном из двух мест (`close()` или `Drop`),
//! поэтому двойного закрытия не бывает, а после закрытия любая операция
//! возвращает [`PushErr::Closed`].

use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::io;
use std::path::{Component, Path, PathBuf};

use compio::driver::{OwnedFd, SharedFd};

#[cfg(unix)]
use super::io::file_to_raw;
use super::io::{IoDriver, PushErr, raw_to_owned};

/// Запись о содержимом директории.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Имя файла или директории без родительского пути.
    pub name: OsString,
    /// Полный путь: директория сессии + [`name`](Self::name).
    pub path: PathBuf,
    /// Это директория (для симлинков — не раскрывается).
    pub is_dir: bool,
    /// Размер в байтах; для директории зависит от файловой системы.
    pub len: u64,
}

/// Открытая директория: путь + дескриптор + пул для async-операций.
///
/// RAII-контракт:
/// * [`DirSession::close`] / [`DirSession::spawn_close`] — явное закрытие
///   дескриптора;
/// * [`Drop`] — гарантирует закрытие, даже если `close()` не вызывался (и
///   ничего не делает, если директория уже закрыта: дескриптор забирается
///   ровно в одном из двух мест, поэтому двойного закрытия нет);
/// * после закрытия любая операция возвращает [`PushErr::Closed`] вместо
///   работы с чужим дескриптором.
///
/// Сессия запоминает путь при открытии и работает только внутри него:
/// вложенные объекты задаются именем ([`DirSession::create_dir`]),
/// абсолютные пути, `..` и разделители отклоняются с [`PushErr::BadName`].
///
/// Синхронные операции (`list`, `create_dir`, `remove_dir`, `close`)
/// выполняются в вызывающем потоке, асинхронные (`spawn_*`) — в пуле
/// `affinitypool`, унаследованном от драйвера, с которым открыта сессия.
pub struct DirSession {
    /// Пул драйвера для `spawn_*`-операций: разделяется с другими
    /// сессиями, создаётся один раз на драйвер.
    driver: IoDriver,
    path: PathBuf,
    /// [`None`] после [`DirSession::close`] / [`DirSession::spawn_close`].
    fd: Option<SharedFd<OwnedFd>>,
}

impl std::fmt::Debug for DirSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirSession")
            .field("path", &self.path)
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl DirSession {
    /// Открыта ли директория.
    pub const fn is_open(&self) -> bool {
        self.fd.is_some()
    }

    /// Путь директории, запомненный при открытии.
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Проверить, что сессия ещё открыта.
    const fn ensure_open(&self) -> Result<(), PushErr> {
        if self.is_open() {
            Ok(())
        } else {
            Err(PushErr::Closed)
        }
    }

    /// Прочитать содержимое директории, отсортированное по имени.
    ///
    /// # Errors
    /// [`PushErr::List`] если директорию прочитать не удалось,
    /// [`PushErr::Closed`] — если сессия закрыта.
    pub fn list(&self) -> Result<Vec<DirEntry>, PushErr> {
        self.ensure_open()?;
        read_dir(&self.path)
    }

    /// Создать вложенную директорию с именем `name`.
    ///
    /// # Errors
    /// [`PushErr::BadName`] если `name` не является одним обычным компонентом
    /// пути (пусто, `.`, `..`, абсолютный путь, разделители),[`PushErr::Create`] — если создать не удалось,
    /// [`PushErr::Closed`] — если сессия закрыта.
    pub fn create_dir(&self, name: impl AsRef<OsStr>) -> Result<(), PushErr> {
        self.ensure_open()?;
        let path = self.child(name.as_ref())?;
        std::fs::create_dir(&path).map_err(PushErr::Create)
    }

    /// Удалить вложенную директорию `name` (она должна быть пустой).
    ///
    /// # Errors
    /// [`PushErr::BadName`] если `name` не является одним обычным компонентом
    /// пути, [`PushErr::Remove`] — если удалить не удалось (в том числе
    /// директория не пуста), [`PushErr::Closed`] — если сессия закрыта.
    pub fn remove_dir(&self, name: impl AsRef<OsStr>) -> Result<(), PushErr> {
        self.ensure_open()?;
        let path = self.child(name.as_ref())?;
        std::fs::remove_dir(&path).map_err(PushErr::Remove)
    }

    /// Закрыть директорию: дескриптор освобождается здесь, либо при [`Drop`].
    ///
    /// Функцию можно вызывать повторно: повторное закрытие (и любая операция
    /// после закрытия) возвращает [`PushErr::Closed`], а дескриптор при этом
    /// не закрывается второй раз.
    ///
    /// # Errors
    /// [`PushErr::Closed`], если директория уже закрыта или дескриптором
    /// владеют другие операции.
    pub fn close(&mut self) -> Result<(), PushErr> {
        let Some(fd) = self.fd.take() else {
            return Err(PushErr::Closed);
        };
        match fd.try_unwrap() {
            // OwnedFd закрывается здесь (RAII), второй раз — нигде
            Ok(_owned) => Ok(()),
            Err(fd) => {
                // дескриптором владеют другие операции: возвращаем его,
                // чтобы закрыл Drop, и не притворяемся, что закрыли
                self.fd = Some(fd);
                Err(PushErr::Closed)
            }
        }
    }

    /// Собрать путь вложенного объекта внутри сессии.
    ///
    /// # Errors
    /// [`PushErr::BadName`], если `name` позволяет выйти за пределы сессии.
    fn child(&self, name: &OsStr) -> Result<PathBuf, PushErr> {
        let raw = name.to_string_lossy();
        // Ровно один обычный компонент: так отсекаются "", ".", "..",
        // абсолютные пути, "C:..." и всё, что содержит разделители.
        let mut parts = Path::new(name).components();
        let only_normal = matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none()
            && !raw.contains('/')
            && !raw.contains('\\');
        if only_normal {
            Ok(self.path.join(name))
        } else {
            Err(PushErr::BadName(raw.into_owned()))
        }
    }

    /// Прочитать содержимое директории в пуле драйвера.
    ///
    /// Проверка на закрытие выполняется в момент вызова, сама работа — в
    /// пуле `affinitypool`. Ждать результат через
    /// [`block_on`](crate::file_work::block_on).
    ///
    /// # Errors
    /// Те же, что и у [`DirSession::list`].
    pub fn spawn_list(
        &self,
    ) -> impl Future<Output = Result<Vec<DirEntry>, PushErr>> + Send + 'static {
        let path = self.ensure_open().map(|()| self.path.clone());
        self.driver.pool().spawn(move || match path {
            Ok(path) => read_dir(&path),
            Err(err) => Err(err),
        })
    }

    /// Создать вложенную директорию в пуле драйвера.
    ///
    /// Имя проверяется внутри сессии (см. [`DirSession::create_dir`]) в
    /// момент вызова, само создание
    /// — в пуле `affinitypool`.
    ///
    /// # Errors
    /// Те же, что и у [`DirSession::create_dir`].
    pub fn spawn_create_dir(
        &self,
        name: impl AsRef<OsStr>,
    ) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        let path = self.ensure_open().and_then(|()| self.child(name.as_ref()));
        self.driver.pool().spawn(move || match path {
            Ok(path) => std::fs::create_dir(&path).map_err(PushErr::Create),
            Err(err) => Err(err),
        })
    }

    /// Удалить вложенную директорию в пуле драйвера.
    ///
    /// Имя проверяется внутри сессии (см. [`DirSession::remove_dir`]) в
    /// момент вызова, само удаление
    /// — в пуле `affinitypool`.
    ///
    /// # Errors
    /// Те же, что и у [`DirSession::remove_dir`].
    pub fn spawn_remove_dir(
        &self,
        name: impl AsRef<OsStr>,
    ) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        let path = self.ensure_open().and_then(|()| self.child(name.as_ref()));
        self.driver.pool().spawn(move || match path {
            Ok(path) => std::fs::remove_dir(&path).map_err(PushErr::Remove),
            Err(err) => Err(err),
        })
    }

    /// Закрыть директорию в пуле драйвера.
    ///
    /// Дескриптор изымается сразу в момент вызова (состояние сессии
    /// меняется немедленно и видно без ожидания), а само закрытие уходит в
    /// пул `affinitypool`. Повторный вызов — и любая операция после —
    /// возвращает [`PushErr::Closed`], дескриптор второй раз не закрывается.
    ///
    /// # Errors
    /// [`PushErr::Closed`], если директория уже закрыта или дескриптором
    /// владеют другие операции.
    pub fn spawn_close(&mut self) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        let taken = match self.fd.take() {
            Some(fd) => match fd.try_unwrap() {
                Ok(owned) => Ok(owned),
                Err(fd) => {
                    // дескриптором владеют другие операции: возвращаем его,
                    // чтобы закрыл Drop, и не притворяемся, что закрыли
                    self.fd = Some(fd);
                    Err(PushErr::Closed)
                }
            },
            None => Err(PushErr::Closed),
        };
        self.driver.pool().spawn(move || match taken {
            Ok(owned) => {
                // закрытие (CloseHandle) происходит здесь, в пуле
                drop(owned);
                Ok(())
            }
            Err(err) => Err(err),
        })
    }
}

impl Drop for DirSession {
    fn drop(&mut self) {
        // RAII: отпускаем дескриптор — его закроет последний SharedFd.
        // Драйвер не участвует: падение не должно зависеть от его состояния.
        // Двойное закрытие исключено: fd забирается либо здесь, либо в
        // DirSession::close() / DirSession::spawn_close().
        drop(self.fd.take());
    }
}

impl IoDriver {
    /// Открыть директорию в вызывающем потоке, привязав к сессии пул
    /// драйвера.
    ///
    /// Пул нужен сессии для [`DirSession::spawn_list`] и остальных
    /// `spawn_*`-операций; он разделяется со всеми сессиями этого же
    /// драйвера и создаётся один раз.
    ///
    /// # Errors
    /// [`PushErr::Open`], если пути нет, он ведёт не к директории или
    /// дескриптор открыть не удалось.
    pub fn open_dir(&self, path: impl AsRef<Path>) -> Result<DirSession, PushErr> {
        let path = path.as_ref().to_path_buf();
        ensure_dir(&path)?;
        let fd = SharedFd::new(open_dir_fd(&path)?);
        Ok(DirSession {
            driver: self.clone(),
            path,
            fd: Some(fd),
        })
    }

    /// Открыть директорию в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::open_dir`].
    pub fn spawn_open_dir(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<DirSession, PushErr>> + Send + 'static {
        let driver = self.clone();
        self.pool().spawn(move || driver.open_dir(&path))
    }

    /// Создать директорию, родитель должен существовать (в вызывающем
    /// потоке).
    ///
    /// # Errors
    /// [`PushErr::Create`], если создать не удалось (нет родителя, уже
    /// существует и т.п.).
    pub fn create_dir(&self, path: impl AsRef<Path>) -> Result<(), PushErr> {
        create_dir_sync(path.as_ref(), false)
    }

    /// Создать директорию со всеми родителями (в вызывающем потоке).
    ///
    /// Уже существующая директория не считается ошибкой.
    ///
    /// # Errors
    /// [`PushErr::Create`], если создать не удалось (на пути лежит файл и
    /// т.п.).
    pub fn create_dir_all(&self, path: impl AsRef<Path>) -> Result<(), PushErr> {
        create_dir_sync(path.as_ref(), true)
    }

    /// Удалить пустую директорию (в вызывающем потоке).
    ///
    /// # Errors
    /// [`PushErr::Remove`], если удалить не удалось (не пуста, нет родителя,
    /// уже удалена и т.п.).
    pub fn remove_dir(&self, path: impl AsRef<Path>) -> Result<(), PushErr> {
        std::fs::remove_dir(path.as_ref()).map_err(PushErr::Remove)
    }

    /// Удалить директорию вместе со всем содержимым (в вызывающем потоке).
    ///
    /// # Errors
    /// [`PushErr::Remove`], если удалить не удалось.
    pub fn remove_dir_all(&self, path: impl AsRef<Path>) -> Result<(), PushErr> {
        std::fs::remove_dir_all(path.as_ref()).map_err(PushErr::Remove)
    }

    /// Прочитать содержимое директории, отсортированное по имени (в
    /// вызывающем потоке).
    ///
    /// # Errors
    /// [`PushErr::List`], если директорию прочитать не удалось.
    pub fn list_dir(&self, path: impl AsRef<Path>) -> Result<Vec<DirEntry>, PushErr> {
        read_dir(path.as_ref())
    }

    /// Создать директорию в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::create_dir`].
    pub fn spawn_create_dir(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        self.pool().spawn(move || create_dir_sync(&path, false))
    }

    /// Создать директорию со всеми родителями в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::create_dir_all`].
    pub fn spawn_create_dir_all(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        self.pool().spawn(move || create_dir_sync(&path, true))
    }

    /// Удалить пустую директорию в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::remove_dir`].
    pub fn spawn_remove_dir(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        self.pool()
            .spawn(move || std::fs::remove_dir(&path).map_err(PushErr::Remove))
    }

    /// Удалить директорию со всем содержимым в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::remove_dir_all`].
    pub fn spawn_remove_dir_all(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<(), PushErr>> + Send + 'static {
        self.pool()
            .spawn(move || std::fs::remove_dir_all(&path).map_err(PushErr::Remove))
    }

    /// Прочитать содержимое директории в пуле драйвера.
    ///
    /// # Errors
    /// Те же, что и у [`IoDriver::list_dir`].
    pub fn spawn_list_dir(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<Vec<DirEntry>, PushErr>> + Send + 'static {
        self.pool().spawn(move || read_dir(&path))
    }
}

/// Создать директорию (создаёт родителей, если `recursive`).
fn create_dir_sync(path: &Path, recursive: bool) -> Result<(), PushErr> {
    let res = if recursive {
        std::fs::create_dir_all(path)
    } else {
        std::fs::create_dir(path)
    };
    res.map_err(PushErr::Create)
}

/// Прочитать содержимое директории, отсортированное по имени.
fn read_dir(path: &Path) -> Result<Vec<DirEntry>, PushErr> {
    let entries = std::fs::read_dir(path).map_err(PushErr::List)?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(PushErr::List)?;
        let is_dir = entry.file_type().map_err(PushErr::List)?.is_dir();
        let len = entry.metadata().map_err(PushErr::List)?.len();
        out.push(DirEntry {
            name: entry.file_name(),
            path: entry.path(),
            is_dir,
            len,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Проверить, что путь существует и ведёт к директории.
fn ensure_dir(path: &Path) -> Result<(), PushErr> {
    let meta = std::fs::metadata(path).map_err(PushErr::Open)?;
    if meta.is_dir() {
        Ok(())
    } else {
        Err(PushErr::Open(io::Error::new(
            io::ErrorKind::NotADirectory,
            "путь ведёт не к директории",
        )))
    }
}

/// Открыть директорию и вернуть владеющий дескриптор.
fn open_dir_fd(path: &Path) -> Result<OwnedFd, PushErr> {
    open_dir_raw(path).map_err(PushErr::Open)
}

/// Открыть директорию в сыром виде.
#[cfg(windows)]
fn open_dir_raw(path: &Path) -> io::Result<OwnedFd> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // share mode максимально разрешённый: сама сессия должна уметь читать
    // и создавать внутри директории, а ограничение накладывает сам дескриптор
    // // SAFETY: wide — nul-терминированная строка, остальные указатели пустые
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            // без BACKUP_SEMANTICS директорию открыть нельзя
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: handle получен из CreateFileW и никому не передавался
    Ok(unsafe { raw_to_owned(handle as usize) })
}

/// Открыть директорию в сыром виде.
#[cfg(unix)]
fn open_dir_raw(path: &Path) -> io::Result<OwnedFd> {
    // на unix директория открывается обычным O_RDONLY
    let file = std::fs::File::open(path)?;
    // SAFETY: raw получен из file_to_raw и никуда не передавался
    Ok(unsafe { raw_to_owned(file_to_raw(file)) })
}

#[cfg(test)]
mod tests {
    use super::super::io::block_on;
    use super::super::testing::{cleanup_dir, cleanup_file, test_path};
    use super::*;

    #[cfg(windows)]
    use super::super::testing::{try_open_exclusive, wait_until_free};

    #[test]
    fn create_list_remove_roundtrip() {
        let root = test_path("dir", "roundtrip");
        cleanup_dir(&root);
        let driver = IoDriver::new();

        driver
            .create_dir_all(&root)
            .expect("create_dir_all должен создать директорию");
        assert!(root.is_dir(), "директория должна существовать");

        // файл внутри, чтобы list увидел и файл, и директорию
        let file = root.join("note.txt");
        driver
            .write_file(&file, &b"dir"[..])
            .expect("write_file должен записать файл");
        let sub = root.join("sub");
        driver
            .create_dir(&sub)
            .expect("create_dir должен создать поддиректорию");

        let entries = driver
            .list_dir(&root)
            .expect("list_dir должен прочитать содержимое");
        assert_eq!(entries.len(), 2, "должно быть ровно два элемента");
        assert_eq!(
            entries[0].name,
            OsString::from("note.txt"),
            "содержимое отсортировано по имени: note.txt раньше sub"
        );
        assert!(!entries[0].is_dir, "note.txt — файл");
        assert_eq!(entries[0].path, file, "путь элемента должен быть полным");
        assert_eq!(entries[0].len, 3, "размер файла — 3 байта");
        assert_eq!(
            entries[1].name,
            OsString::from("sub"),
            "второй элемент — sub"
        );
        assert!(entries[1].is_dir, "sub — директория");

        driver
            .remove_dir(&sub)
            .expect("remove_dir должен удалить пустую поддиректорию");
        assert!(!sub.exists(), "поддиректория должна исчезнуть");

        driver
            .remove_dir_all(&root)
            .expect("remove_dir_all должен удалить директорию с файлом");
        assert!(!root.exists(), "корневая директория должна исчезнуть");

        cleanup_dir(&root);
    }

    #[test]
    fn async_dir_ops() {
        let root = test_path("dir", "async");
        cleanup_dir(&root);
        let driver = IoDriver::new();

        block_on(driver.spawn_create_dir_all(root.clone()))
            .expect("spawn_create_dir_all должен создать директорию");
        block_on(driver.spawn_create_dir(root.join("child")))
            .expect("spawn_create_dir должен создать поддиректорию");

        let entries = block_on(driver.spawn_list_dir(root.clone()))
            .expect("spawn_list_dir должен прочитать содержимое");
        assert_eq!(entries.len(), 1, "должна быть одна поддиректория");
        assert!(entries[0].is_dir, "child — директория");
        assert_eq!(
            entries[0].name,
            OsString::from("child"),
            "имя поддиректории"
        );

        block_on(driver.spawn_remove_dir(root.join("child")))
            .expect("spawn_remove_dir должен удалить пустую поддиректорию");
        block_on(driver.spawn_remove_dir_all(root.clone()))
            .expect("spawn_remove_dir_all должен удалить директорию");
        assert!(!root.exists(), "директория должна исчезнуть");

        cleanup_dir(&root);
    }

    #[test]
    fn missing_dir_is_open_list_or_create_error() {
        let driver = IoDriver::new();
        let missing = test_path("dir", "missing");
        cleanup_dir(&missing);

        let err = driver
            .list_dir(&missing)
            .expect_err("list_dir несуществующей директории должно падать");
        assert!(
            matches!(err, PushErr::List(_)),
            "ожидалась ошибка List, получено {err:?}"
        );

        let err = driver
            .create_dir(missing.join("nested").join("deep"))
            .expect_err("create_dir без родителя должно падать");
        assert!(
            matches!(err, PushErr::Create(_)),
            "ожидалась ошибка Create, получено {err:?}"
        );

        let err = driver
            .open_dir(&missing)
            .expect_err("открытие несуществующей директории должно падать");
        assert!(
            matches!(err, PushErr::Open(_)),
            "ожидалась ошибка Open, получено {err:?}"
        );

        // путь есть, но это файл, а не директория
        let file = test_path("dir", "not_a_dir");
        cleanup_file(&file);
        driver
            .write_file(&file, &b"x"[..])
            .expect("write_file должен записать файл");
        let err = driver
            .open_dir(&file)
            .expect_err("открытие файла как директории должно падать");
        assert!(
            matches!(err, PushErr::Open(_)),
            "ожидалась ошибка Open, получено {err:?}"
        );

        cleanup_file(&file);
        cleanup_dir(&missing);
    }

    /// Работа после закрытия и повторное закрытие не должны трогать
    /// чужой/закрытый дескриптор.
    #[test]
    fn dir_session_use_after_close_is_error() {
        let root = test_path("dir", "use_after_close");
        cleanup_dir(&root);
        let driver = IoDriver::new();
        driver
            .create_dir_all(&root)
            .expect("create_dir_all должен создать директорию");

        let mut session = driver
            .open_dir(&root)
            .expect("open_dir должен открыть директорию");
        assert!(session.is_open(), "после open сессия должна быть открытой");
        assert_eq!(
            session.path(),
            root.as_path(),
            "path() должен отдавать путь, с которым открыли"
        );
        let entries = session.list().expect("list должен работать до закрытия");
        assert!(entries.is_empty(), "новая директория должна быть пустой");
        session
            .create_dir("child")
            .expect("create_dir должен работать до закрытия");

        session.close().expect("первое close должно пройти");
        assert!(
            !session.is_open(),
            "после close сессия должна быть закрытой"
        );

        let err = session.list().expect_err("list после close должен падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );

        let err = session
            .create_dir("other")
            .expect_err("create_dir после close должен падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );

        let err = session
            .remove_dir("child")
            .expect_err("remove_dir после close должен падать");
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

        cleanup_dir(&root);
    }

    /// Имя вложенного объекта не должно позволять выйти за пределы сессии.
    #[test]
    fn dir_session_rejects_bad_names() {
        let root = test_path("dir", "bad_names");
        cleanup_dir(&root);
        let driver = IoDriver::new();
        driver
            .create_dir_all(&root)
            .expect("create_dir_all должен создать директорию");

        let mut session = driver
            .open_dir(&root)
            .expect("open_dir должен открыть директорию");
        for bad in ["", ".", "..", "a/b", "a\\b", "/abs"] {
            let err = session
                .create_dir(bad)
                .expect_err("недопустимое имя должно отклоняться");
            assert!(
                matches!(err, PushErr::BadName(_)),
                "имя {bad:?} должно быть отклонено, получено {err:?}"
            );
            let err = session
                .remove_dir(bad)
                .expect_err("недопустимое имя должно отклоняться");
            assert!(
                matches!(err, PushErr::BadName(_)),
                "имя {bad:?} должно быть отклонено, получено {err:?}"
            );
        }

        session
            .create_dir("ok")
            .expect("обычное имя должно проходить проверку");
        assert!(root.join("ok").is_dir(), "директория ok должна создаться");

        session.close().expect("close должен пройти");
        cleanup_dir(&root);
    }

    /// RAII: живая сессия занимает директорию, а close() и Drop отпускают её.
    #[cfg(windows)]
    #[test]
    fn dir_session_raii_closes_handle() {
        let root = test_path("dir", "raii");
        cleanup_dir(&root);
        let driver = IoDriver::new();
        driver
            .create_dir_all(&root)
            .expect("create_dir_all должен создать директорию");

        // путь 1: явное закрытие
        let mut session = driver
            .open_dir(&root)
            .expect("open_dir должен открыть директорию");
        assert!(
            !try_open_exclusive(&root, true),
            "директория должна быть занята живой сессией"
        );
        session.close().expect("close должен закрыть директорию");
        assert!(
            wait_until_free(&root, true),
            "после close директория должна быть полностью свободна"
        );

        // путь 2: закрытие через Drop
        let session = driver
            .open_dir(&root)
            .expect("open_dir должен открыть директорию");
        assert!(
            !try_open_exclusive(&root, true),
            "директория должна быть занята живой сессией"
        );
        drop(session);
        assert!(
            wait_until_free(&root, true),
            "после Drop директория должна быть полностью свободна"
        );

        // путь 3: асинхронное закрытие в пуле
        let mut session = driver
            .open_dir(&root)
            .expect("open_dir должен открыть директорию");
        assert!(
            !try_open_exclusive(&root, true),
            "директория должна быть занята живой сессией"
        );
        block_on(session.spawn_close()).expect("spawn_close должен закрыть директорию");
        assert!(
            !session.is_open(),
            "после spawn_close сессия должна быть закрытой"
        );
        assert!(
            wait_until_free(&root, true),
            "после spawn_close директория должна быть полностью свободна"
        );

        cleanup_dir(&root);
    }

    /// Полный async-цикл сессии: открытие, работа и закрытие — в пуле
    /// драйвера.
    #[test]
    fn async_dir_session_ops() {
        let root = test_path("dir", "session_async");
        cleanup_dir(&root);
        let driver = IoDriver::new();
        driver
            .create_dir_all(&root)
            .expect("create_dir_all должен создать директорию");

        let mut session = block_on(driver.spawn_open_dir(root.clone()))
            .expect("spawn_open_dir должен открыть директорию");
        assert!(
            session.is_open(),
            "после async open сессия должна быть открытой"
        );
        assert_eq!(
            session.path(),
            root.as_path(),
            "path() должен отдавать путь, с которым открыли"
        );

        let entries =
            block_on(session.spawn_list()).expect("spawn_list должен прочитать пустую директорию");
        assert!(entries.is_empty(), "новая директория должна быть пустой");

        block_on(session.spawn_create_dir("child"))
            .expect("spawn_create_dir должен создать поддиректорию");
        let entries =
            block_on(session.spawn_list()).expect("spawn_list должен увидеть новую поддиректорию");
        assert_eq!(entries.len(), 1, "должна быть одна поддиректория");
        assert!(entries[0].is_dir, "child — директория");
        assert_eq!(
            entries[0].name,
            OsString::from("child"),
            "имя поддиректории"
        );

        block_on(session.spawn_remove_dir("child"))
            .expect("spawn_remove_dir должен удалить пустую поддиректорию");
        let entries =
            block_on(session.spawn_list()).expect("spawn_list должен увидеть пустую директорию");
        assert!(entries.is_empty(), "поддиректория должна исчезнуть");

        // ошибки тоже приходят через future
        let err = block_on(session.spawn_create_dir(".."))
            .expect_err("недопустимое имя должно отклоняться");
        assert!(
            matches!(err, PushErr::BadName(_)),
            "ожидалась ошибка BadName, получено {err:?}"
        );

        block_on(session.spawn_close()).expect("spawn_close должен закрыть директорию");
        assert!(
            !session.is_open(),
            "после spawn_close сессия должна быть закрытой"
        );
        let err =
            block_on(session.spawn_list()).expect_err("async list после закрытия должен падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );
        let err = block_on(session.spawn_close()).expect_err("повторный spawn_close должен падать");
        assert!(
            matches!(err, PushErr::Closed),
            "ожидалась ошибка Closed, получено {err:?}"
        );

        cleanup_dir(&root);
    }

    /// Async-открытие несуществующей директории отдаёт ошибку через future.
    #[test]
    fn async_spawn_open_dir_is_open_error() {
        let driver = IoDriver::new();
        let missing = test_path("dir", "async_missing");
        cleanup_dir(&missing);

        let err = block_on(driver.spawn_open_dir(missing.clone()))
            .expect_err("async открытие несуществующей директории должно падать");
        assert!(
            matches!(err, PushErr::Open(_)),
            "ожидалась ошибка Open, получено {err:?}"
        );

        cleanup_dir(&missing);
    }
}
