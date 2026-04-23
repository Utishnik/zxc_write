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

            dbg!("VirtualQueryEx FAILED\n");
            dbg!("Return code: {}\n", result);
            dbg!(format!("Last error: {} (0x{:08X})\n", error.0, error.0));
            dbg!("Error message: {}\n", error_msg.clone());

            virtual_query_ex_err_debug(error.0);
            return Err(VirtualQueryErr::VirtualQueryExErr(VirtualQueryExErr {
                ret_code: result,
                last_err: error.0,
                err_msg: error_msg,
            }));
        }
    }

    dbg!(format!("VirtualQueryEx SUCCESS\n"));
    dbg!(format!("Base Address: {:p}\n", mbi.BaseAddress));
    dbg!(format!("Region Size: 0x{:X} bytes\n", mbi.RegionSize));
    dbg!(format!("State: 0x{:X}\n", mbi.State.0));
    dbg!(format!("Protect: 0x{:X}\n", mbi.Protect.0));
    dbg!(format!("Type: 0x{:X}\n", mbi.Type.0));

    Ok(mbi)
}

pub fn virtual_query_ex_err_debug(last_err: u32) {
    match last_err {
        val if val == ERROR_ACCESS_DENIED.0 => {
            dbg!("ERROR_ACCESS_DENIED\n");
        }
        val if val == ERROR_INVALID_PARAMETER.0 => {
            dbg!("ERROR_INVALID_PARAMETER\n");
        }
        val if val == ERROR_INVALID_HANDLE.0 => {
            dbg!("ERROR_INVALID_HANDLE\n");
        }
        val if val == ERROR_PARTIAL_COPY.0 => {
            dbg!("ERROR_PARTIAL_COPY\n");
        }
        val => {
            dbg!("другая {}\n", val);
        }
    }
}

pub mod check_mbi {
    use windows::Win32::System::Memory::*;
    pub fn describe_memory_region(mbi: &MEMORY_BASIC_INFORMATION) -> String {
        // Описание состояния страницы
        let state_str = match mbi.State {
            MEM_COMMIT => "COMMIT",
            MEM_RESERVE => "RESERVE",
            MEM_FREE => "FREE",
            other => return format!("UNKNOWN_STATE(0x{:X})", other.0),
        };

        // Для свободной памяти тип и защита не определены
        if mbi.State == MEM_FREE {
            return format!("{} (свободная область)", state_str);
        }

        // Для зарезервированной памяти тип может быть указан, но защита обычно 0
        let type_str = match mbi.Type {
            MEM_IMAGE => "IMAGE (образ EXE/DLL)",
            MEM_MAPPED => "MAPPED (отображённый файл)",
            MEM_PRIVATE => "PRIVATE (куча/стек)",
            val if val.0 == 0 => "NOT SPECIFIED", // для MEM_RESERVE иногда 0
            other => return format!("{} - UNKNOWN_TYPE(0x{:X})", state_str, other.0),
        };

        // Для зарезервированной памяти защиты нет (Protect = 0)
        let protect_str = if mbi.State == MEM_RESERVE {
            "no access (reserved)".to_string()
        } else if mbi.State == MEM_COMMIT {
            describe_page_protection(mbi.Protect)
        } else {
            "N/A".to_string()
        };

        format!("{} - {} - [{}]", state_str, type_str, protect_str)
    }

    pub fn describe_page_protection(protect: PAGE_PROTECTION_FLAGS) -> String {
        let mut desc = String::new();

        // Основные права
        let basic = match protect
            & !(PAGE_GUARD
                | PAGE_NOCACHE
                | PAGE_WRITECOMBINE
                | PAGE_TARGETS_INVALID
                | PAGE_TARGETS_NO_UPDATE)
        {
            PAGE_NOACCESS => "NOACCESS",
            PAGE_READONLY => "READONLY",
            PAGE_READWRITE => "READWRITE",
            PAGE_WRITECOPY => "WRITECOPY",
            PAGE_EXECUTE => "EXECUTE",
            PAGE_EXECUTE_READ => "EXECUTE_READ",
            PAGE_EXECUTE_READWRITE => "EXECUTE_READWRITE",
            PAGE_EXECUTE_WRITECOPY => "EXECUTE_WRITECOPY",
            _ => "UNKNOWN_PROTECT",
        };
        desc.push_str(basic);

        // Дополнительные флаги
        if protect.0 & PAGE_GUARD.0 != 0 {
            desc.push_str(" | GUARD");
        }
        if protect.0 & PAGE_NOCACHE.0 != 0 {
            desc.push_str(" | NOCACHE");
        }
        if protect.0 & PAGE_WRITECOMBINE.0 != 0 {
            desc.push_str(" | WRITECOMBINE");
        }
        if protect.0 & PAGE_TARGETS_INVALID.0 != 0 {
            desc.push_str(" | TARGETS_INVALID");
        }
        if protect.0 & PAGE_TARGETS_NO_UPDATE.0 != 0 {
            desc.push_str(" | TARGETS_NO_UPDATE");
        }

        if desc.is_empty() {
            "no flags".to_string()
        } else {
            desc
        }
    }
}
