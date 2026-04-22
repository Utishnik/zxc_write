use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_PARTIAL_COPY, GetLastError, HANDLE, LUID,
};

use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, VirtualQueryEx};

use windows::Win32::System::Diagnostics::Debug::{
    FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

use windows::Win32::Security::{
    GetTokenInformation, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_INFORMATION_CLASS, TOKEN_PRIVILEGES, TOKEN_QUERY,
};

use crate::utils::*;
use std::ptr::null_mut;
use windows::core::{Error, PCWSTR, PWSTR};
type LPCVOID = *const core::ffi::c_void;
use core::marker::PhantomData;

///# Safety
pub unsafe fn get_last_error_as_string() -> String {
    // 1. GetLastError() возвращает обёртку WIN32_ERROR, извлекаем u32 через .0
    unsafe {
        let error_code = GetLastError().0;
        let buffer: PWSTR = PWSTR(std::ptr::null_mut());

        let chars_copied = FormatMessageW(
            // 2. Добавляем IGNORE_INSERTS, чтобы не падать на сообщениях с %1, %2 и т.д.
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            None,
            error_code,
            0,
            buffer,              // 3. Передаём сырой указатель, а не ссылку на Vec
            buffer.len() as u32, // 4. Размер буфера передаётся отдельным параметром
            None,
        );
        let _phantom: PhantomData<&mut u16> = PhantomData::<&mut u16>;

        if chars_copied > 0 {
            // FormatMessageW не включает null-terminator в возвращаемую длину
            let cast_mut = mut_ptr_cast_slice::<u16>(buffer.0, chars_copied as usize, _phantom);
            String::from_utf16_lossy(&cast_mut[..chars_copied as usize])
                .trim()
                .to_string()
        } else {
            format!("Unknown error code: {}", error_code)
        }
    }
}

#[derive(Debug)]
pub enum VirtualQueryErrCode {
    ErrorAccessDenied,
    ErrorInvalidParameter,
    ErrorInvalidHandle,
    ErrorPartialCopy,
}

#[derive(Debug)]
pub struct VirtualQueryExErr {
    pub ret_code: usize,
    pub last_err: u32,
    pub err_msg: String,
}

#[derive(Debug)]
pub enum VirtualQueryErr {
    Handle(String),
    CloseHandle(Error),
    VirtualQueryExErr(VirtualQueryExErr),
    GetProcessId(String),
    OpenProcess(Error, String),
}

///# Safety
pub unsafe fn virtual_query_with_diagnostics(
    handle: HANDLE,
    address: LPCVOID,
) -> Result<MEMORY_BASIC_INFORMATION, VirtualQueryErr> {
    println!("=== VirtualQueryEx Diagnostic ===");
    println!("Handle: {:?}", handle);
    println!("Address: {:p}", address);

    if handle.is_invalid() {
        return Err(VirtualQueryErr::Handle(
            "Handle is invalid (NULL or INVALID_HANDLE_VALUE)".to_string(),
        ));
    }

    // Проверка, что handle валидный, пытаясь открыть тот же процесс по PID
    let pid;
    unsafe {
        pid = GetProcessId(handle);
    }
    if pid == 0 {
        return Err(VirtualQueryErr::GetProcessId(format!(
            "Failed to get process ID: {}",
            unsafe { get_last_error_as_string() }
        )));
    }
    unsafe {
        let test_handle = OpenProcess(PROCESS_QUERY_INFORMATION, false, pid);
        match test_handle {
            Ok(test_handle) => {
                let close_res = CloseHandle(test_handle);
                if let Err(e) = close_res {
                    return Err(VirtualQueryErr::CloseHandle(e));
                }
            }
            Err(e) => {
                println!("[DEBUG] test_handle err code: {}", e);
                return Err(VirtualQueryErr::OpenProcess(
                    e,
                    format!("Handle validation failed: {}", get_last_error_as_string()),
                ));
            }
        }
    }

    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();

    println!("MBI size: {} bytes", size);

    unsafe {
        let result = VirtualQueryEx(handle, Some(address), &mut mbi, size);

        if result == 0 {
            let error = GetLastError();
            let error_msg = get_last_error_as_string();

            dbg!("VirtualQueryEx FAILED");
            dbg!("Return code: {}", result);
            dbg!(format!("Last error: {} (0x{:08X})", error.0, error.0));
            dbg!("Error message: {}", error_msg.clone());

            virtual_query_ex_err_debug(error.0);
            return Err(VirtualQueryErr::VirtualQueryExErr(VirtualQueryExErr {
                ret_code: result,
                last_err: error.0,
                err_msg: error_msg,
            }));
        }
    }

    dbg!(format!("VirtualQueryEx SUCCESS"));
    dbg!(format!("Base Address: {:p}", mbi.BaseAddress));
    dbg!(format!("Region Size: 0x{:X} bytes", mbi.RegionSize));
    dbg!(format!("State: 0x{:X}", mbi.State.0));
    dbg!(format!("Protect: 0x{:X}", mbi.Protect.0));
    dbg!(format!("Type: 0x{:X}", mbi.Type.0));

    Ok(mbi)
}

pub fn virtual_query_ex_err_debug(last_err: u32) {
    match last_err {
        val if val == ERROR_ACCESS_DENIED.0 => {
            dbg!("ERROR_ACCESS_DENIED");
        }
        val if val == ERROR_INVALID_PARAMETER.0 => {
            dbg!("ERROR_INVALID_PARAMETER");
        }
        val if val == ERROR_INVALID_HANDLE.0 => {
            dbg!("ERROR_INVALID_HANDLE");
        }
        val if val == ERROR_PARTIAL_COPY.0 => {
            dbg!("ERROR_PARTIAL_COPY");
        }
        val => {
            dbg!("другая {}", val);
        }
    }
}
