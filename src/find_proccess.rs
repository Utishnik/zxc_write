use vec_string::*;
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

#[derive(Debug)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

impl std::fmt::Display for ProcessInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "(pid: {};parent_pid: {};name: {})",
            self.pid, self.parent_pid, self.name
        )
    }
}

/// # Errors
/// возвращает ошибку если не удалось закрыть handle, если не нашел процесс, если snapshot вернул ошибку
pub fn get_all_processes_detailed() -> Result<Vec<ProcessInfo>, FindProccesError> {
    let mut list = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if let Err(e) = snapshot {
            return Err(FindProccesError::Snapshot(e));
        } else if let Ok(snapshot) = snapshot {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;

            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let name = String::from_utf16_lossy(&entry.szExeFile)
                        .trim_end_matches('\0')
                        .to_string();
                    list.push(ProcessInfo {
                        pid: entry.th32ProcessID,
                        parent_pid: entry.th32ParentProcessID,
                        name,
                    });
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let ch = CloseHandle(snapshot);
            if let Err(e) = ch {
                return Err(FindProccesError::CloseHndl(e));
            }
        }
    }
    Ok(list)
}

///u32 - pid / String - exe_name
pub fn get_child_processes(parent_pid: u32) -> Result<Vec<(u32, String)>, Error> {
    let mut children = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if let Err(e) = snapshot {
            return Err(e);
        }
        let snapshot = snapshot.unwrap_unchecked(); //safe !!!
        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..PROCESSENTRY32::default()
        };

        if Process32First(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32ParentProcessID == parent_pid {
                    let exe_name = std::ffi::CStr::from_ptr(entry.szExeFile.as_ptr())
                        .to_string_lossy() // Потеря для не-UTF8 символов
                        .into_owned();
                    children.push((
                        entry.th32ProcessID,
                        exe_name.trim_end_matches('\0').to_string(),
                    ));
                }
                if Process32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _: windows::core::Result<()> = CloseHandle(snapshot);
    }
    Ok(children)
}

#[test]
fn test_get_all_processes_detailed() {
    let f = get_all_processes_detailed();
    if let Err(e) = f {
        println!("{:?}", e);
    } else if let Ok(ok) = f {
        let fmt = ok.vec_string(DEFAULT_FORMAT_RULE);
        println!("{}", fmt);
    }
}

#[test]
fn test_find_pid() {
    let fnd_name = find_process_by_name("firefox.exe");
    if let Err(e) = fnd_name {
        println!("Error: {:?}", e);
        return;
    }
    if let Ok(pid) = fnd_name {
        println!("Pid: {}", pid);
    }
}
