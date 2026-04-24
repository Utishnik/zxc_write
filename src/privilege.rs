use windows::Win32::Foundation::{CloseHandle, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HRESULT, PCWSTR, Result};

//todo для множества привилегий / для не только текущего процесса
pub fn enable_privilege_one(privilege_name: &str) -> Result<()> {
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?;
    }
    let _guard = HandleGuard(token);

    let mut luid = LUID::default();
    let name_wide: Vec<u16> = privilege_name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        LookupPrivilegeValueW(None, PCWSTR(name_wide.as_ptr()), &mut luid)?;
    }

    let tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };

    unsafe {
        AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)?;
    }

    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        let hr = HRESULT::from_win32(ERROR_NOT_ALL_ASSIGNED.0);
        return Err(windows::core::Error::new(
            hr,
            "Привилегия отсутствует в токене",
        ));
    }

    Ok(())
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let close_res = CloseHandle(self.0);
                if close_res.is_err() {
                    println!("[LOG] HandleGuard Drop CloseHandle Err");
                    //todo использовать какие нибудь tiny log и тд
                }
            }
        }
    }
}
