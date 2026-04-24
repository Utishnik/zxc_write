use std::mem;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, MAX_PATH};
use windows::Win32::System::ProcessStatus::{
    EnumProcessModules, EnumProcessModulesEx, GetModuleFileNameExW, LIST_MODULES_ALL,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
use windows::core::Result;

pub fn list_modules_standard(pid: u32) -> Result<Option<Vec<String>>> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)? };
    let _guard = HandleGuard(handle);

    let mut bytes_needed: u32 = 0;
    unsafe {
        EnumProcessModules(handle, std::ptr::null_mut(), 0, &mut bytes_needed)?;
    }

    if bytes_needed == 0 {
        println!("[LOG] Нет модулей");
        return Ok(None);
    }

    let count = (bytes_needed as usize) / mem::size_of::<HMODULE>();
    let mut modules: Vec<HMODULE> = vec![HMODULE(std::ptr::null_mut()); count];

    unsafe {
        EnumProcessModules(
            handle,
            modules.as_mut_ptr(),
            bytes_needed,
            &mut bytes_needed,
        )?;
    }
    let mut ret_vec: Vec<String> = Vec::with_capacity(bytes_needed as usize);
    for &hmod in &modules {
        let mut path_buf = vec![0_u16; MAX_PATH as usize];
        let len = unsafe { GetModuleFileNameExW(Some(handle), Some(hmod), &mut path_buf) };
        if len > 0 {
            let str = String::from_utf16_lossy(&path_buf[..len as usize]);
            ret_vec.push(str);
        } else {
            println!("[LOG] модуль без названия");
        }
    }

    Ok(Some(ret_vec))
}

use windows::Win32::System::ProcessStatus::ENUM_PROCESS_MODULES_EX_FLAGS;
pub fn list_modules_ex(
    pid: u32,
    flag: ENUM_PROCESS_MODULES_EX_FLAGS,
) -> Result<Option<Vec<String>>> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)? };
    let _guard = HandleGuard(handle);

    let mut bytes_needed: u32 = 0;
    unsafe {
        EnumProcessModulesEx(handle, std::ptr::null_mut(), 0, &mut bytes_needed, flag)?;
    }

    if bytes_needed == 0 {
        println!("[LOG] Нет модулей");
        return Ok(None);
    }

    let count = (bytes_needed as usize) / mem::size_of::<HMODULE>();
    let mut modules: Vec<HMODULE> = vec![HMODULE(std::ptr::null_mut()); count];

    unsafe {
        EnumProcessModulesEx(
            handle,
            modules.as_mut_ptr(),
            bytes_needed,
            &mut bytes_needed,
            flag,
        )?;
    }
    let mut ret_vec: Vec<String> = Vec::with_capacity(bytes_needed as usize);

    for &hmod in &modules {
        let mut path_buf = vec![0_u16; MAX_PATH as usize];
        let len = unsafe { GetModuleFileNameExW(Some(handle), Some(hmod), &mut path_buf) };
        if len > 0 {
            let str = String::from_utf16_lossy(&path_buf[..len as usize]);
            ret_vec.push(str);
        } else {
            println!("[LOG] модуль без названия");
        }
    }

    Ok(Some(ret_vec))
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
