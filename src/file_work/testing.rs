//! Общие хелперы для тестов [`crate::file_work`]: пути тестовых объектов,
//! уборка за собой и (на Windows) проверка того, что дескриптор действительно
//! закрыт.

use std::io;
use std::path::{Path, PathBuf};

/// Путь к тестовому объекту: уникален для процесса и вида теста.
pub(crate) fn test_path(kind: &str, name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zxc_write_{kind}_{}_{name}", std::process::id()))
}

/// Убрать за собой тестовый файл: файла могло и не быть.
pub(crate) fn cleanup_file(path: &Path) {
    let _: io::Result<()> = std::fs::remove_file(path);
}

/// Убрать за собой тестовую директорию со всем содержимым.
pub(crate) fn cleanup_dir(path: &Path) {
    let _: io::Result<()> = std::fs::remove_dir_all(path);
}

/// Попытка открыть файл (`dir == false`) или директорию (`dir == true`) без
/// прав на совместное использование: удаётся, только если других открытых
/// дескрипторов на объект нет.
#[cfg(windows)]
pub(crate) fn try_open_exclusive(path: &Path, dir: bool) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, OPEN_EXISTING,
        },
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut flags = FILE_ATTRIBUTE_NORMAL;
    if dir {
        // без этого флага CreateFileW не открывает директорию
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    // SAFETY: wide — nul-терминированная строка, остальные указатели пустые
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            0, // share mode: никто больше не может держать объект открытым
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    let opened = handle != INVALID_HANDLE_VALUE;
    if opened {
        // SAFETY: handle получен из CreateFileW и никому не передавался
        unsafe { CloseHandle(handle) };
    }
    opened
}

/// То же, что [`try_open_exclusive`], но с повторами: объект могут временно
/// держать посторонние процессы (индексатор, антивирус).
#[cfg(windows)]
pub(crate) fn wait_until_free(path: &Path, dir: bool) -> bool {
    for _ in 0..50 {
        if try_open_exclusive(path, dir) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}
