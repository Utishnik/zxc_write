use std::{ffi::c_void, ptr};

use crate::error_hand::*;
use windows::{
    Win32::{
        Foundation::*,
        System::{
            Diagnostics::{Debug::ReadProcessMemory, ToolHelp::*},
            LibraryLoader::*,
            Memory::*,
            Threading::*,
        },
    },
    core::Error,
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

impl std::fmt::Display for ExtractStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Str: {}", self.str)
    }
}

///# Safety
///
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
                #[expect(clippy::missing_panics_doc, reason = "infallible")]
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
const unsafe fn buf_get_u8(buf: *const u8, idx: usize) -> (u8, u8) {
    unsafe {
        let as_char1 = *buf.add(idx);
        let as_char2 = *buf.add(idx + 1);
        (as_char1, as_char2)
    }
}

pub const fn ascii_to_char(high: u8, low: u8) -> Option<char> {
    let code_point = ((high as u16) << 8) | (low as u16);
    if let Some(c) = char::from_u32(code_point as u32) {
        return Some(c);
    }
    None
}

/// # Safety
/// при валидных inputs
pub unsafe fn get_u8_to_buf(
    buf: *const u8,
    last_idx: usize,
    idx_start: usize,
    step: usize,
) -> Vec<(u8, u8)> {
    let mut ret: Vec<(u8, u8)> = Vec::with_capacity((last_idx - idx_start) / step);
    for i in (idx_start..last_idx).step_by(step) {
        unsafe {
            let (inner_as_char1, inner_as_char2) = buf_get_u8(buf, i);
            ret.push((inner_as_char1, inner_as_char2));
        }
    }
    ret
}

/// # Safety
/// при валидных inputs
pub unsafe fn extract_unicode_strings(
    buf: *const u8,
    size: usize,
    base_ptr: *mut core::ffi::c_void,
    min_len: usize,
    max_len: usize,
) -> Vec<ExtractStr> {
    let mut extract_res: Vec<ExtractStr> = Vec::with_capacity(size / min_len);
    let mut i = 0;
    unsafe {
        while i < size - 1 {
            let as_char1 = *buf.add(i);
            let as_char2 = *buf.add(i + 1);
            if check_unicode(as_char1, as_char2) {
                let mut wstr: String = String::default();
                let mut last_val_j: usize = 0;
                let get: Vec<(u8, u8)> = get_u8_to_buf(buf, size - 1, i, 2);

                for j in get
                    .iter()
                    .filter(|&x| -> bool { check_unicode(x.0, x.1) })
                    .enumerate()
                {
                    let empty_c = ' ';
                    let tern_nil = '\0';
                    let (high, low) = *j.1;
                    let as_char = ascii_to_char(high, low);
                    if let Some(c) = as_char {
                        if c == tern_nil {
                            break;
                        }
                        wstr.push(c);
                    } else {
                        wstr.push(empty_c);
                    }
                    last_val_j = j.0 + i;
                }
                if wstr.len() >= min_len && wstr.len() <= max_len {
                    extract_res.push(ExtractStr {
                        base_addr: base_ptr.add(i),
                        str: wstr,
                    });
                }
                i += last_val_j - 2;
            }
            i += 2;
        }
    }
    extract_res
}

#[derive(Debug)]
pub struct StringCfg {
    pub min_len: usize,
    pub max_len: Option<usize>,
}

impl Default for StringCfg {
    fn default() -> Self {
        Self {
            min_len: 5,
            max_len: None,
        }
    }
}

#[derive(Debug)]
pub struct ExtractResult {
    pub ascii: Vec<ExtractStr>,
    pub unicode: Vec<ExtractStr>,
}

#[derive(Debug)]
pub struct VirtualQueryExErr {
    pub old: Result<MEMORY_BASIC_INFORMATION, VirtualQueryErr>,
    pub new: Result<MEMORY_BASIC_INFORMATION, VirtualQueryErr>,
}

#[derive(Debug)]
pub enum ScanProcessStringsError {
    ReadProcessMemory,
    OpenProcess(Error),
    VirtualQueryEx(VirtualQueryExErr),
    VirtualQueryExNonePtr,
}

pub unsafe fn open_read_process(dwprocessid: u32) -> Result<HANDLE, Error> {
    unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
            false,
            dwprocessid,
        )
    }
}

/// # Panics
/// если неверный конфиг
#[must_use]
pub fn scan_process_strings(
    dwprocessid: u32,
) -> Result<Option<ExtractResult>, ScanProcessStringsError> {
    //PROCESS_QUERY_INFORMATION
    let cfg_ascii: StringCfg = StringCfg::default();
    let cfg_unicode: StringCfg = StringCfg {
        min_len: 5,
        max_len: Some(25),
    };
    unsafe {
        let h_process = open_read_process(dwprocessid);
        match h_process {
            Ok(ok) => {
                let mbi: *mut MEMORY_BASIC_INFORMATION = ptr::null_mut();
                let addr: Option<*const c_void> = None;
                let vqe = VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>());
                if vqe != 0 {
                    let get_protect = (*mbi).Protect;
                    let get_state = (*mbi).State;
                    let reg_size = (*mbi).RegionSize;
                    if get_state == MEM_COMMIT && is_readable(get_protect) {
                        let base_addr: *mut c_void = (*mbi).BaseAddress;
                        let mut buffer: Vec<u8> = Vec::with_capacity(reg_size);
                        let ptr_buf: *mut c_void = buffer.as_mut_ptr() as *mut c_void;
                        let reg_size: usize = (*mbi).RegionSize;
                        let byte_read: Option<*mut usize> = Some(ptr::null_mut());
                        if ReadProcessMemory(ok, base_addr, ptr_buf, reg_size, byte_read).is_ok() {
                            let res = byte_read.map_or_else(
                                || None,
                                |x| {
                                    let extract_ascii_str = extract_ascii_strings(
                                        ptr_buf as *const u8,
                                        *x,
                                        base_addr,
                                        cfg_ascii.min_len,
                                        cfg_ascii.max_len,
                                    );
                                    let extract_unicode_str = extract_unicode_strings(
                                        ptr_buf as *const u8,
                                        *x,
                                        base_addr,
                                        cfg_unicode.min_len,
                                        cfg_unicode.max_len.unwrap(),
                                    );
                                    let ret: ExtractResult = ExtractResult {
                                        ascii: extract_ascii_str,
                                        unicode: extract_unicode_str,
                                    };
                                    Some(ret)
                                },
                            );
                            return Ok(res);
                        } else {
                            return Err(ScanProcessStringsError::ReadProcessMemory);
                        }
                    }
                } else {
                    let mut err_ret = std::mem::MaybeUninit::<VirtualQueryExErr>::uninit();
                    use std::ptr::addr_of_mut;
                    let old_ptr_mut = addr_of_mut!((*err_ret.as_mut_ptr()).old);
                    let new_ptr_mut = addr_of_mut!((*err_ret.as_mut_ptr()).new);

                    //check старый hand
                    {
                        let h_process = h_process.unwrap(); //безопасно потому что у нас выше и если не там ошибка то ScanProcessStringsError::VirtualQueryEx
                        let vqe =
                            VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>());
                        if let Some(x) = addr {
                            *old_ptr_mut = virtual_query_with_diagnostics(h_process, x);
                        } else {
                            return Err(ScanProcessStringsError::VirtualQueryExNonePtr);
                        }
                    }
                    //check new open process
                    {
                        let err_hand = open_read_process(dwprocessid);
                        if let Err(e) = err_hand {
                            println!("[DEBUG] find_strings err_hand Err: {:?}", e);
                        } else if let Ok(ok) = err_hand {
                            let err_hand = ok;
                            let vqe = VirtualQueryEx(
                                ok,
                                addr,
                                mbi,
                                size_of::<MEMORY_BASIC_INFORMATION>(),
                            );
                            if let Some(x) = addr {
                                *new_ptr_mut = virtual_query_with_diagnostics(err_hand, x);
                            } else {
                                return Err(ScanProcessStringsError::VirtualQueryExNonePtr);
                            }
                        }
                    }
                    let initialized = err_ret.assume_init();
                    return Err(ScanProcessStringsError::VirtualQueryEx(initialized));
                }
            }
            Err(e) => {
                return Err(ScanProcessStringsError::OpenProcess(e));
            }
        }
    }
    unreachable!();
}
