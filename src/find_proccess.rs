use windows::{
    Win32::{Foundation::*, System::Diagnostics::ToolHelp::*},
    core::Error,
};

#[derive(Debug)]
pub enum FindProccesError {
    Snapshot(Error),
    CloseHndl(Error),
    NotFind,
}

/// # Errors
/// возвращает ошибку если не удалось закрыть handle, если не нашел процесс, если snapshot вернул ошибку
pub fn find_process_by_name(process_name: &str) -> Result<u32, FindProccesError> {
    // Конвертируем искомое имя в wide string
    let target_wide: Vec<u16> = process_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if let Ok(snapshot) = snapshot {
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    // Сравниваем имена
                    let mut found = true;
                    for (i, &ch) in target_wide.iter().enumerate() {
                        if entry.szExeFile[i] != ch {
                            found = false;
                            break;
                        }
                        if ch == 0 {
                            break;
                        }
                    }

                    if found {
                        let pid = entry.th32ProcessID;
                        let ch = CloseHandle(snapshot);
                        if let Err(e) = ch {
                            return Err(FindProccesError::CloseHndl(e));
                        }

                        return Ok(pid);
                    }

                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let ch = CloseHandle(snapshot);
            if let Err(e) = ch {
                return Err(FindProccesError::CloseHndl(e));
            }
            Err(FindProccesError::NotFind)
        } else if let Err(e) = snapshot {
            Err(FindProccesError::Snapshot(e))
        } else {
            unreachable!();
        }
    }
}
