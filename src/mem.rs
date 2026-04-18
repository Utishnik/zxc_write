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

pub const fn is_null_term_ascii(c: u8) -> bool {
    c == 0x00
}

pub const fn is_null_term_unicode(c: char) -> bool {
    c == '\0'
}

#[derive(Debug, Default)]
pub struct ExtractStr {
    pub base_addr: *mut core::ffi::c_void,
    pub str: String,
}
///# Safety
pub unsafe fn extract_ascii_strings(
    buf: *const u8,
    size: usize,
    base_ptr: *mut core::ffi::c_void,
    min_len: usize,
    max_len: Option<usize>,
) -> Vec<ExtractStr> {
    let mut extract_res: Vec<ExtractStr> = Vec::with_capacity(size / min_len);
    let mut cur: String = String::default();
    let mut cur_char: char = char::default();
    let max_len_some: bool = max_len.is_some();
    for i in 0..size {
        unsafe {
            if is_printable_char(buf.add(i) as u8) {
                let as_char = (*buf.add(i)) as char;
                cur_char = as_char;
                cur.push(as_char);
            } else {
                if (cur.len() >= min_len && (max_len_some && cur.len() >= max_len.unwrap()))
                    || cur.len() >= min_len && (max_len_some && is_null_term_ascii(cur_char as u8))
                {
                    extract_res.push(ExtractStr {
                        base_addr: base_ptr.add(i - cur.len()),
                        str: cur.clone(),
                    });
                }
                cur.clear();
            }
        }
    }
    unsafe {
        if cur.len() >= min_len {
            extract_res.push(ExtractStr {
                base_addr: base_ptr.add(size - cur.len()),
                str: cur.clone(),
            });
        }
    }
    extract_res
}

#[inline(always)]
fn check_unicode(c1: u8, c2: u8) -> bool {
    is_printable_char(c1) && is_null_term_ascii(c2)
}

#[inline(always)]
unsafe fn buf_get_u8(buf: *const u8, idx: usize) -> (u8, u8) {
    unsafe {
        let as_char1 = (*buf.add(idx)) as u8;
        let as_char2 = (*buf.add(idx + 1)) as u8;
        (as_char1, as_char2)
    }
}

pub unsafe fn extract_unicode_strings(
    buf: *const u8,
    size: usize,
    base_ptr: *mut core::ffi::c_void,
    min_len: usize,
    max_len: Option<usize>,
) {
    let mut extract_res: Vec<ExtractStr> = Vec::with_capacity(size / min_len);
    unsafe {
        for i in (0..size - 1).step_by(2) {
            let as_char1 = (*buf.add(i)) as u8;
            let as_char2 = (*buf.add(i + 1)) as u8;
            if check_unicode(as_char1, as_char2) {
                let mut wstr: String = String::default();
                for j in (i..size - 1).into_iter().filter(|x| -> bool {
                    let inner_as_char1 = (*buf.add(*x)) as u8;
                    let inner_as_char2 = (*buf.add(*x + 1)) as u8;
                    check_unicode(inner_as_char1, inner_as_char2)
                }) {}
            }
        }
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
