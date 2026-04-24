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

#[derive(Debug)]
pub struct ReadProcessMemoryResult {
    pub mbi: MEMORY_BASIC_INFORMATION,
    pub read: usize,
    pub buf: Vec<u8>,
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
fn is_ascii_utf16le(c1: u8, c2: u8) -> bool {
    is_printable_char(c1) && is_null_term_ascii(c2)
}

use windows::Win32::System::Memory::{
    PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_READWRITE, PAGE_WRITECOPY,
};
pub fn is_readwrite(protect: PAGE_PROTECTION_FLAGS) -> bool {
    protect == PAGE_READWRITE
        || protect == PAGE_EXECUTE_READWRITE
        || protect == PAGE_WRITECOPY
        || protect == PAGE_EXECUTE_WRITECOPY
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
            if is_ascii_utf16le(as_char1, as_char2) {
                let mut wstr: String = String::default();
                let mut last_val_j: usize = 0;
                let get: Vec<(u8, u8)> = get_u8_to_buf(buf, size - 1, i, 2);

                for j in get
                    .iter()
                    .filter(|&x| -> bool { is_ascii_utf16le(x.0, x.1) })
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

                let l = wstr.len();
                if l < 100 && l > 7 {
                    println!("{wstr}");
                }
                if wstr.len() >= min_len && wstr.len() <= max_len {
                    extract_res.push(ExtractStr {
                        base_addr: base_ptr.add(i),
                        str: wstr,
                    });
                }
                if last_val_j < 2 {
                    continue;
                }
                let add = i.checked_add(last_val_j - 2);
                if add.is_none() {
                    continue;
                }
                let add = add.unwrap_unchecked();

                i += add;
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

// TODO ! МЕНЬШЕ UNSAFE
/// # Panics
/// если неверный конфиг
#[must_use]
pub fn scan_process_strings(
    dwprocessid: u32,
) -> Result<Option<ExtractResult>, ScanProcessStringsError> {
    //PROCESS_QUERY_INFORMATION
    println!("[DEBUG] PID SCAN:\t{dwprocessid}");
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
                let mut vqe = VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>());
                use crate::error_hand::check_mbi::*;
                while vqe != 0 {
                    vqe = VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>());
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
                }
                if vqe == 0 {
                    //todo это не коректно всегда null будет
                    if !mbi.is_null() {
                        let dbg_dmr = describe_memory_region(&*mbi);
                        println!("[DEBUG] {}", dbg_dmr);
                    } else {
                        let err = get_last_error_as_string_array();
                        println!("last err: {}", err);
                        println!("mbi is null ptr");
                    }

                    let mut err_ret = std::mem::MaybeUninit::<VirtualQueryExErr>::uninit();
                    use std::ptr::addr_of_mut;
                    let old_ptr_mut = addr_of_mut!((*err_ret.as_mut_ptr()).old);
                    let new_ptr_mut = addr_of_mut!((*err_ret.as_mut_ptr()).new);

                    //check старый hand
                    {
                        let h_process = h_process.unwrap(); //безопасно потому что у нас выше и если не там ошибка то ScanProcessStringsError::VirtualQueryEx
                        let _: usize =
                            VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>());
                        if let Some(x) = addr {
                            *old_ptr_mut = virtual_query_with_diagnostics(h_process, x);
                        } else {
                            println!("[DEBUG] None ptr старый hand");
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
                            let _: usize = VirtualQueryEx(
                                ok,
                                addr,
                                mbi,
                                size_of::<MEMORY_BASIC_INFORMATION>(),
                            );
                            if let Some(x) = addr {
                                *new_ptr_mut = virtual_query_with_diagnostics(err_hand, x);
                            } else {
                                println!("[DEBUG] None ptr new hand\n");
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

pub fn scan_dynamic_mem<F: Fn(MEMORY_BASIC_INFORMATION) -> bool>(
    pid: u32,
    jmp_len: usize,
    max_cap: usize,
    filter: Option<F>,
) -> windows::core::Result<Vec<ReadProcessMemoryResult>> {
    let mut ret: Vec<ReadProcessMemoryResult> = Vec::new();
    let h_process =
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)? };

    let mut addr: *const std::ffi::c_void = std::ptr::null();

    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let result = unsafe {
            VirtualQueryEx(
                h_process,
                Some(addr),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };

        if result == 0 {
            let err = unsafe { windows::Win32::Foundation::GetLastError() };
            if err == ERROR_INVALID_ADDRESS {
                break;
            }
            // Пропускаем ошибку, двигаемся вперёд
            addr = ((addr as usize) + jmp_len) as *const _;
            continue;
        }

        let next = (mbi.BaseAddress as usize) + mbi.RegionSize;

        // ФИЛЬТР: только динамическая память (куча/стек), не модули, не маппинги
        let is_dynamic =
            mbi.State == MEM_COMMIT && mbi.Type == MEM_PRIVATE && is_readwrite(mbi.Protect);

        if is_dynamic {
            let size = mbi.RegionSize.min(max_cap);
            let mut buf = vec![0_u8; size];
            let mut read = 0_usize;
            if let Some(ref x) = filter
                && !x(mbi)
            {
                addr = next as *const _; //skip
                continue;
            }
            let ok = unsafe {
                ReadProcessMemory(
                    h_process,
                    mbi.BaseAddress,
                    buf.as_mut_ptr() as *mut _,
                    size,
                    Some(&mut read),
                )
                .is_ok()
            };

            if ok && read > 0 {
                ret.push(ReadProcessMemoryResult { mbi, read, buf });
            }
        }

        addr = next as *const _;
    }

    unsafe {
        CloseHandle(h_process).ok();
    }
    Ok(ret)
}

/// # Panics
/// если неверный конфиг
#[doc = "не прирывается при нулевом VirtualQueryEx и не закоммиченной/не is_readable"]
///# Errors
///
pub fn scan_process_strings_lossy(
    dwprocessid: u32,
    vqe_ignore: bool,
) -> Result<Option<ExtractResult>, ScanProcessStringsError> {
    let cfg_ascii = StringCfg::default();
    let cfg_unicode = StringCfg {
        min_len: 5,
        max_len: Some(25),
    };

    println!("[DEBUG] PID SCAN:\t{dwprocessid}");

    let h_process =
        unsafe { open_read_process(dwprocessid).map_err(ScanProcessStringsError::OpenProcess)? };

    let mut addr: *const c_void = std::ptr::null();
    let mut all_ascii = Vec::new();
    let mut all_unicode = Vec::new();

    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();

        let result = unsafe {
            VirtualQueryEx(
                h_process,
                Some(addr),
                &mut mbi,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };

        if result == 0 && !vqe_ignore {
            let err = unsafe { GetLastError() };
            if err == ERROR_INVALID_ADDRESS {
                println!("[DEBUG] Конец адресного пространства: {:p}", addr);
            } else {
                println!("[DEBUG] VirtualQueryEx ошибка: {:?}, addr: {:p}", err, addr);
            }
            break; // Выходим
        } else if result == 0 {
            let err = unsafe { GetLastError() };
            println!("[LOG] scan_process_strings_lossy: {:?}", err);
        }

        let base_addr = mbi.BaseAddress;
        let reg_size = mbi.RegionSize;
        let protect = mbi.Protect;
        let state = mbi.State;

        if state != MEM_COMMIT || !is_readable(protect) {
            let next = unsafe { base_addr.add(reg_size) };
            if next.is_null() || next <= addr as _ {
                break;
            }
            addr = next as *const c_void;
            continue;
        }

        // Защита от огромных регионов
        const MAX_REGION_SIZE: usize = 100 * 1024 * 1024;
        let read_size = if reg_size > MAX_REGION_SIZE {
            MAX_REGION_SIZE
        } else {
            reg_size
        };

        let mut buffer = vec![0_u8; read_size];
        let mut bytes_read = 0_usize;
        let read_ok = unsafe {
            ReadProcessMemory(
                h_process,
                base_addr,
                buffer.as_mut_ptr() as *mut c_void,
                read_size,
                Some(&mut bytes_read),
            )
            .is_ok()
        };

        if read_ok && bytes_read > 0 {
            let ascii = unsafe {
                extract_ascii_strings(
                    buffer.as_ptr(),
                    bytes_read,
                    base_addr,
                    cfg_ascii.min_len,
                    cfg_ascii.max_len,
                )
            };
            let unicode = unsafe {
                extract_unicode_strings(
                    buffer.as_ptr(),
                    bytes_read,
                    base_addr,
                    cfg_unicode.min_len,
                    cfg_unicode.max_len.unwrap_or(usize::MAX),
                )
            };

            all_ascii.extend(ascii);
            all_unicode.extend(unicode);
        }
        // Если ReadProcessMemory не сработал — просто пропускаем регион и идём дальше

        // Переходим к следующему региону
        let next = unsafe { base_addr.add(reg_size) };
        if next.is_null() || next <= addr as _ {
            break;
        }
        addr = next as *const c_void;
    }

    unsafe {
        CloseHandle(h_process).ok();
    }

    if all_ascii.is_empty() && all_unicode.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ExtractResult {
            ascii: all_ascii,
            unicode: all_unicode,
        }))
    }
}
