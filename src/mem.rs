use std::ptr;

use windows::{
    Win32::Foundation::*, Win32::System::Diagnostics::ToolHelp::*, Win32::System::LibraryLoader::*,
    Win32::System::Memory::*, Win32::System::Threading::*, core::PCSTR, core::PCWSTR,
};

pub fn is_readable(protect: PAGE_PROTECTION_FLAGS) -> bool {
    let f = protect
        & (PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY);
    f.0 != 0
}

pub fn is_printable_char(c: u8) -> bool {
    (0x20..0x7E).contains(&c)
}

pub unsafe fn extract_strings(
    buf: *const u8,
    size: usize,
    base_ptr: *mut core::ffi::c_void,
    min_len: usize,
) {
    let mut cur: String = String::default();
    for i in 0..size {
        unsafe { if is_printable_char(buf.offset(i as isize) as u8) {} }
    }
}

fn scan_process_strings(dwprocessid: u32) -> bool {
    //PROCESS_QUERY_INFORMATION
    unsafe {
        let hProcess = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
            false,
            dwprocessid,
        );
        match hProcess {
            Ok(ok) => {
                let mbi: *mut MEMORY_BASIC_INFORMATION = ptr::null_mut();
                let mut addr: Option<*const core::ffi::c_void> = None;
                while let ret =
                    VirtualQueryEx(ok, addr.clone(), mbi, size_of::<MEMORY_BASIC_INFORMATION>())
                {
                    let get_protect = (*mbi).Protect;
                    let get_state = (*mbi).State;
                    let reg_size = (*mbi).RegionSize;
                    if get_state == MEM_COMMIT && is_readable(get_protect) {
                        let mut buffer: Vec<u8> = Vec::with_capacity(reg_size);
                        let mut byte_read: usize = 0;
                    }
                }
            }
            Err(e) => {
                println!("OpenProcess failed: {}", e);
            }
        }
    }
    false
}
