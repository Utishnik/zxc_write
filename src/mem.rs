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

#[inline(always)]
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

#[inline(always)]
pub fn is_printable_char(c: u8) -> bool {
    (0x20..0x7E).contains(&c)
}

#[inline(always)]
pub const fn is_null_term_ascii(c: u8) -> bool {
    c == 0x00
}

#[inline(always)]
pub const fn is_null_term_unicode(c: char) -> bool {
    c == '\0'
}

#[inline(always)]
fn is_ascii_utf16le(c1: u8, c2: u8) -> bool {
    is_printable_char(c1) && is_null_term_ascii(c2)
}

#[inline(always)]
pub fn is_printable_utf16le(c1: u8, c2: u8) -> bool {
    matches!(u16::from_le_bytes([c1, c2]),
        0x0020..=0x007E |      // Basic Latin
        0x00A1..=0x024F |      // Latin Extended
        0x0400..=0x04FF |      // Cyrillic
        0x2000..=0x206F |      // General Punctuation
        0x3000..=0x303F        // CJK Punctuation
    )
}

pub const fn is_null_utf16le(c1: u8, c2: u8) -> bool {
    c1 == 0x00 && c2 == 0x00
}

#[derive(Debug, Default, Clone)]
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
    buf: *const c_void,
    size: usize,
    base_ptr: *const core::ffi::c_void,
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
                let as_char = (*(buf as *const u8).add(i)) as char;
                cur_char = as_char;
                cur.push(as_char);
            } else {
                #[expect(clippy::missing_panics_doc, reason = "infallible")]
                if (cur.len() >= min_len && (max_len_some && cur.len() >= max_len.unwrap()))
                    || cur.len() >= min_len && (max_len_some && is_null_term_ascii(cur_char as u8))
                {
                    extract_res.push(ExtractStr {
                        base_addr: base_ptr.add(i - cur.len()) as _,
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
                base_addr: base_ptr.add(size - cur.len()) as _,
                str: cur.clone(),
            });
        }
    }
    extract_res
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
    buf: *const c_void,
    size: usize,
    base_ptr: *const c_void,
    min_len: usize,
    max_len: Option<usize>,
) -> Vec<ExtractStr> {
    let mut extract_res: Vec<ExtractStr> = Vec::with_capacity(size / 2 / min_len);
    let mut cur: String = String::default();
    let mut start_offset: usize = 0;
    let max_len_some: bool = max_len.is_some();
    let mut i: usize = 0;
    while i + 1 < size {
        let c1 = unsafe { *(buf as *const u8).add(i) };
        let c2 = unsafe { *(buf as *const u8).add(i + 1) };

        if is_null_utf16le(c1, c2) {
            // Null terminator — конец строки
            if cur.len() >= min_len {
                let fits_max = if max_len_some {
                    cur.len() <= max_len.unwrap()
                } else {
                    true
                };
                if fits_max {
                    extract_res.push(ExtractStr {
                        base_addr: (base_ptr as usize + start_offset) as *mut c_void,
                        str: cur.clone(),
                    });
                }
            }
            cur.clear();
        } else if is_printable_utf16le(c1, c2) {
            // Валидный printable символ UTF-16LE
            if cur.is_empty() {
                start_offset = i;
            }
            let code_unit = u16::from_le_bytes([c1, c2]);
            if let Some(ch) = char::from_u32(code_unit as u32) {
                cur.push(ch);
            }
            // Принудительный пуш если достигли max_len
            if max_len_some && cur.len() >= max_len.unwrap() {
                extract_res.push(ExtractStr {
                    base_addr: (base_ptr as usize + start_offset) as *mut c_void,
                    str: cur.clone(),
                });
                cur.clear();
            }
        } else {
            // Мусор — сбрасываем
            cur.clear();
        }

        i += 2;
    }

    // Хвост (если данные закончились без null-terminator)
    if cur.len() >= min_len {
        let fits_max = if max_len_some {
            cur.len() <= max_len.unwrap()
        } else {
            true
        };
        if fits_max {
            extract_res.push(ExtractStr {
                base_addr: (base_ptr as usize + start_offset) as *mut c_void,
                str: cur,
            });
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
pub struct ExtractStrResult {
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

pub fn open_read_process(dwprocessid: u32) -> Result<HANDLE, Error> {
    unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
            false,
            dwprocessid,
        )
    }
}

pub type ExtractResult<T> = Vec<Option<Vec<Vec<T>>>>;

#[test]
fn test() {
    let mut test: ExtractResult<u8> = (0..10).map(|_| Some(Vec::with_capacity(10))).collect();
    let p = vec![vec![1_u8]];
    test.push(Some(p));

    let _: &u8 = test
        .last()
        .unwrap()
        .clone()
        .unwrap()
        .first()
        .unwrap()
        .first()
        .unwrap();
}

/// # Panics
/// если неверный конфиг
#[must_use]
pub fn scan_process_processors<F, T>(
    dwprocessid: u32,
    processors: &[fn(*mut c_void, usize, *const c_void) -> Vec<T>],
    start_cap: usize,
    stard_addr: Option<*const c_void>,
) -> Result<ExtractResult<T>, ScanProcessStringsError> {
    //PROCESS_QUERY_INFORMATION
    println!("[DEBUG] PID SCAN:\t{dwprocessid}");
    let mut accumulator: ExtractResult<T> = Vec::with_capacity(start_cap);

    let h_process = open_read_process(dwprocessid);
    match h_process {
        Ok(ok) => {
            let mbi: *mut MEMORY_BASIC_INFORMATION = ptr::null_mut();
            let addr: Option<*const c_void> = stard_addr;
            let mut vqe =
                unsafe { VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
            use crate::error_hand::check_mbi::*;
            while vqe != 0 {
                vqe =
                    unsafe { VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
                let get_protect = unsafe { (*mbi).Protect };
                let get_state = unsafe { (*mbi).State };
                let reg_size = unsafe { (*mbi).RegionSize };
                if get_state == MEM_COMMIT && is_readable(get_protect) {
                    let base_addr: *mut c_void = unsafe { (*mbi).BaseAddress };
                    let mut buffer: Vec<u8> = Vec::with_capacity(reg_size);
                    let ptr_buf: *mut c_void = buffer.as_mut_ptr() as *mut c_void;
                    let byte_read: Option<*mut usize> = Some(ptr::null_mut());
                    unsafe {
                        if ReadProcessMemory(ok, base_addr, ptr_buf, reg_size, byte_read).is_ok() {
                            let res = byte_read.map_or_else(
                                || None,
                                |x| {
                                    let mut ret: Vec<Vec<T>> = (0..processors.len())
                                        .map(|_| Vec::with_capacity(start_cap))
                                        .collect();

                                    processors.iter().for_each(|item| {
                                        ret.push(item(ptr_buf, *x, base_addr));
                                    });

                                    Some(ret)
                                },
                            );
                            accumulator.push(res);
                        } else {
                            return Err(ScanProcessStringsError::ReadProcessMemory);
                        }
                    }
                }
            }
            if vqe == 0 {
                //todo это не коректно всегда null будет
                if !mbi.is_null() {
                    let dbg_dmr = describe_memory_region(unsafe { &*mbi });
                    println!("[DEBUG] {}", dbg_dmr);
                } else {
                    let err = unsafe { get_last_error_as_string_array() };
                    println!("last err: {}", err);
                    println!("mbi is null ptr");
                }

                let mut err_ret = std::mem::MaybeUninit::<VirtualQueryExErr>::uninit();
                use std::ptr::addr_of_mut;
                let old_ptr_mut = unsafe { addr_of_mut!((*err_ret.as_mut_ptr()).old) };
                let new_ptr_mut = unsafe { addr_of_mut!((*err_ret.as_mut_ptr()).new) };

                //check старый hand
                {
                    let h_process = h_process.unwrap(); //безопасно потому что у нас выше и если не там ошибка то ScanProcessStringsError::VirtualQueryEx
                    let _: usize = unsafe {
                        VirtualQueryEx(ok, addr, mbi, size_of::<MEMORY_BASIC_INFORMATION>())
                    };
                    if let Some(x) = addr {
                        unsafe {
                            *old_ptr_mut = virtual_query_with_diagnostics(h_process, x);
                        }
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
                        unsafe {
                            let _: usize = VirtualQueryEx(
                                ok,
                                addr,
                                mbi,
                                size_of::<MEMORY_BASIC_INFORMATION>(),
                            );
                        }
                        if let Some(x) = addr {
                            unsafe {
                                *new_ptr_mut = virtual_query_with_diagnostics(err_hand, x);
                            }
                        } else {
                            println!("[DEBUG] None ptr new hand\n");
                            return Err(ScanProcessStringsError::VirtualQueryExNonePtr);
                        }
                    }
                }
                let initialized = unsafe { err_ret.assume_init() };
                Err(ScanProcessStringsError::VirtualQueryEx(initialized))
            } else {
                Ok(accumulator)
            }
        }
        Err(e) => Err(ScanProcessStringsError::OpenProcess(e)),
    }
}

/// Регион памяти (уже прочитанный)
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    pub buf: Vec<u8>,
    pub read: usize,
    pub mbi: MEMORY_BASIC_INFORMATION,
}

#[must_use]
pub fn scan_process_processors_mbi<F, T>(
    dwprocessid: u32,
    processors: &mut [F],
    start_cap: usize,
    rpmr: Vec<ReadProcessMemoryResult>, //ахуеное название
) -> Result<ExtractResult<T>, ScanProcessStringsError>
where
    F: FnMut(*const c_void, usize, *const c_void) -> Vec<T>,
{
    //PROCESS_QUERY_INFORMATION
    println!("[DEBUG] PID SCAN:\t{dwprocessid}");
    let h_process = open_read_process(dwprocessid);
    match h_process {
        Ok(_) => {
            let mut accumulator: ExtractResult<T> = Vec::with_capacity(start_cap);
            for item in rpmr.iter() {
                let buf = &item.buf;
                let read = item.read;
                let base_addr = item.mbi.BaseAddress;
                let mut ret: Vec<Vec<T>> = (0..processors.len())
                    .map(|_| Vec::with_capacity(start_cap))
                    .collect();
                processors.iter_mut().for_each(|item| {
                    ret.push(item(buf.as_ptr() as _, read, base_addr));
                });
                accumulator.push(Some(ret)); //всегда some так как scan_dynamic_mem фильтрует
            }
            Ok(accumulator)
        }
        Err(e) => Err(ScanProcessStringsError::OpenProcess(e)),
    }
}

use crate::utils::HandleGuard;
pub fn scan_dynamic_mem_custom_filter<F: Fn(MEMORY_BASIC_INFORMATION) -> bool>(
    pid: u32,
    jmp_len: usize,
    max_cap: usize,
    filter: Option<F>,
) -> windows::core::Result<Vec<ReadProcessMemoryResult>> {
    let mut ret: Vec<ReadProcessMemoryResult> = Vec::new();
    let h_process =
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)? };
    let _guard = HandleGuard(h_process);
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

    Ok(ret)
}

pub fn scan_dynamic_mem(
    pid: u32,
    jmp_len: usize,
    max_cap: usize,
    max_addr_offset: usize,
) -> windows::core::Result<Vec<ReadProcessMemoryResult>> {
    let mut ret: Vec<ReadProcessMemoryResult> = Vec::new();
    let h_process =
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)? };
    let _guard = HandleGuard(h_process);
    let mut addr: *const std::ffi::c_void = std::ptr::null();
    let mut start_addr: *const c_void = std::ptr::null();

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
        if start_addr.is_null() {
            start_addr = addr;
        } else {
            let check_addr = start_addr as usize;
            let addr = addr as usize;
            let res = addr.checked_sub(check_addr);
            if let Some(x) = res
                && x > max_addr_offset
            {
                break;
            }
        }

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

    Ok(ret)
}

macro_rules! gen_dispatch {
    ($($f:ty),*) => {
        pub enum ProcessorsDispatch{
            $(f),*
        }
    };
}

/// Универсальный обход памяти процесса с извлечением данных.
/// Принимает срез замыканий, каждое из которых вызывается для каждого читабельного региона.
/// Возвращает `Vec<Option<Vec<Vec<T>>>>` — по одному `Option` на регион,
/// внутри `Some` лежит результат каждого обработчика (`Vec<T>` на обработчик).
pub fn scan_process_processors_lossy_gen<T, F>(
    dwprocessid: u32,
    processors: &mut [F],
    start_cap: usize,
    start_addr: Option<*const c_void>,
) -> Result<ExtractResult<T>, ScanProcessStringsError>
where
    F: FnMut(*const c_void, usize, *const c_void) -> Vec<T>,
{
    println!("[DEBUG] PID SCAN:\t{dwprocessid}");

    let h_process = open_read_process(dwprocessid).map_err(ScanProcessStringsError::OpenProcess)?;

    let mut accumulator: ExtractResult<T> = Vec::with_capacity(start_cap);
    let mut addr = start_addr.unwrap_or(ptr::null());

    loop {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };

        let result = unsafe {
            VirtualQueryEx(
                h_process,
                Some(addr),
                &mut mbi,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };

        if result == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_INVALID_ADDRESS {
                println!("[DEBUG] Конец адресного пространства: {:p}", addr);
            } else {
                eprintln!("[DEBUG] VirtualQueryEx ошибка: {:?}, addr: {:p}", err, addr);
            }
            break;
        }

        let base_addr = mbi.BaseAddress;
        let reg_size = mbi.RegionSize;
        let protect = mbi.Protect;
        let state = mbi.State;

        if state != MEM_COMMIT || !is_readable(protect) {
            addr = unsafe { base_addr.add(reg_size) };
            continue;
        }

        // Защита от слишком больших регионов
        const MAX_REGION_SIZE: usize = 100 * 1024 * 1024;
        let read_size = if reg_size > MAX_REGION_SIZE {
            MAX_REGION_SIZE
        } else {
            reg_size
        };

        let mut buffer: Vec<u8> = vec![0u8; read_size];
        let mut bytes_read: usize = 0;

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
            let region_results: Vec<Vec<T>> = processors
                .iter_mut()
                .map(|proc| proc(buffer.as_mut_ptr() as *mut c_void, bytes_read, base_addr))
                .collect();
            accumulator.push(Some(region_results));
        } else {
            // Если не прочитали, всё равно сохраняем None для этого региона?
            // В исходной логике просто пропускали, здесь кладём None, чтобы сохранить число регионов.
            accumulator.push(None);
        }

        addr = unsafe { base_addr.add(reg_size) };
    }

    unsafe {
        CloseHandle(h_process).ok();
    }

    Ok(accumulator)
}
