//! Работа с файлами и директориями поверх IO-драйвера.
//!
//! Модули разделены по предмету:
//!
//! * [`io`] — устройство драйвера: [`io::IoDriver`] (пул потоков, настройки,
//!   [`io::block_on`]) и [`io::PushErr`];
//! * [`mod@file`] — файлы: [`file::OpenMode`], [`file::FileSession`] и операции
//!   [`io::IoDriver::read_file`], [`io::IoDriver::write_file`],
//!   [`io::IoDriver::append_file`] (плюс `spawn_*`);
//! * [`dir`] — директории: [`dir::DirEntry`], [`dir::DirSession`] и операции
//!   [`io::IoDriver::open_dir`], [`io::IoDriver::create_dir`],
//!   [`io::IoDriver::remove_dir`], [`io::IoDriver::list_dir`] (плюс
//!   `spawn_*`).
//!
//! У каждой директорной операции есть две версии: синхронная (в вызывающем
//! потоке) и `spawn_*` — задача уходит в пул `affinitypool` и возвращает
//! [`Future`], который ждётся через [`block_on`]. У
//! [`dir::DirSession`] обе версии есть у всего: `list`/`create_dir`/`remove_dir`/`close` и
//! `spawn_list`/`spawn_create_dir`/`spawn_remove_dir`/`spawn_close`,
//! плюс открытие — [`io::IoDriver::open_dir`] и
//! [`io::IoDriver::spawn_open_dir`].
//!
//! Обе сессии — RAII-структуры с одинаковым контрактом:
//!
//! * [`file::FileSession::close`] / [`dir::DirSession::close`] — явное
//!   закрытие, его можно вызывать повторно;
//! * [`Drop`] — закрывает ресурс, если `close()` не вызывали; дескриптор
//!   забирается ровно в одном из двух мест, поэтому двойного закрытия не
//!   бывает;
//! * после закрытия любая операция возвращает [`io::PushErr::Closed`] вместо
//!   работы с чужим дескриптором.

pub mod dir;
pub mod file;
pub mod find_write;
pub mod io;

#[cfg(test)]
mod testing;

pub use dir::{DirEntry, DirSession};
pub use file::{FileSession, OpenMode};
pub use io::{DEFAULT_CHUNK, IoDriver, IoDriverConfig, PushErr, block_on};
