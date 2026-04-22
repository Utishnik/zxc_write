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
use windows::core::{PCWSTR, PWSTR};
type LPCVOID = *const core::ffi::c_void;
use core::marker::PhantomData;

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
