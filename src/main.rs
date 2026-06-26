use allocative::Allocative;
use core::ffi::c_void;
use std::cmp;
use std::num::NonZero;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use threadpool::ThreadPool;
use typed_arena::Arena;
use vec_string::*;
use windows::core as win_core;
use zxc_write::find_proccess::*;
use zxc_write::log::{Logger, LoggerRes};
use zxc_write::mem::*;
use zxc_write::privilege::enable_privilege_one;
use zxc_write::utils::SendablePtr;
use zxc_write::utils::*;

fn wait_close() {
    println!("CLOSE...");
    let mut buffer: String = String::new();
    let _ = std::io::stdin().read_line(&mut buffer);
}

#[hotpath::measure]
fn extract_str(dwprocessid: u32, log: OptionLog) -> Result<ExtractStrResult, ()> {
    let extract_ascii_strings_fn =
        |buf, size, base_ptr| unsafe { extract_ascii_strings(buf, size, base_ptr, 10, None) };
    let extract_unicode_strings_fn =
        |buf, size, base_ptr| unsafe { extract_unicode_strings(buf, size, base_ptr, 10, None) };

    let mut processors = [extract_ascii_strings_fn, extract_unicode_strings_fn];

    let result: Result<ExtractResult<ExtractStr>, _> = {
        scan_process_processors_lossy_gen(
            dwprocessid,
            &mut processors,
            16,              // start_cap
            101704332083002, // начать с NULL
            None,
            log,
        )
    };

    // Извлечь плоский список всех строк (объединяя ascii+unicode из всех регионов)
    if let Ok(extract_result) = result {
        let all_ascii: Vec<_> = extract_result
            .iter()
            .filter_map(|opt| Some(opt))
            .flat_map(|per_proc| per_proc.into_iter().nth(0)) // ascii — первый обработчик
            .flatten()
            .collect();
        let all_unicode: Vec<_> = extract_result
            .iter()
            .filter_map(|opt| Some(opt))
            .flat_map(|per_proc| per_proc.into_iter().nth(1)) // unicode — второй обработчик
            .flatten()
            .collect();
        let res: ExtractStrResult = ExtractStrResult {
            ascii: vec_flat2_owned_xz(all_ascii),
            unicode: vec_flat2_owned_xz(all_unicode),
        };
        Ok(res)
    } else {
        Err(())
    }
}

#[hotpath::measure]
fn extract_str_dyn_mem_lossy(
    dwprocessid: u32,
    log: OptionLog,
    min_len: usize,
    max_len: Option<usize>,
    all_trace_bytes: Arc<AtomicUsize>,
) -> Result<ExtractStrResult, ()> {
    let extract_ascii_strings_fn = |buf, size, base_ptr, min_len, max_len| unsafe {
        extract_ascii_strings_lossy(buf, size, base_ptr, min_len, max_len)
    };
    let extract_unicode_strings_fn = |buf, size, base_ptr, min_len, max_len| unsafe {
        extract_unicode_strings_lossy(buf, size, base_ptr, min_len, max_len)
    };

    let scan_res;
    hotpath::measure_block!("scan_dynamic_mem in extract_str_dyn_mem", {
        scan_res = scan_dynamic_mem(
            dwprocessid,
            1,
            500_000_000,
            101_704_332_083_002,
            None,
            log.clone(),
            all_trace_bytes,
        ); //тут кажется может проблема быть
    }); //cold меньше процента

    if scan_res.is_err() {
        return Err(());
    }
    let scan_res = scan_res.unwrap();
    let mut processors: Vec<DynProcessors<ExtractStr>> = vec![
        Box::new(extract_ascii_strings_fn) as DynProcessors<ExtractStr>,
        Box::new(extract_unicode_strings_fn) as DynProcessors<ExtractStr>,
    ];
    let result;
    hotpath::measure_block!("scan_process_processors_mbi in extract_str_dyn_mem", {
        result = scan_process_processors_mbi::<ExtractStr>(
            dwprocessid,
            processors.as_mut(),
            50000,
            scan_res,
            log.clone(),
            min_len,
            max_len,
        );
    }); //hot
    result.ok().map_or(Err(()), |mut extract_result| {
        /*
        let all_ascii: Vec<_> = extract_result
            .iter()
            .filter_map(Some)
            .flat_map(|per_proc| per_proc.iter().next())
            .flatten()
            .collect();
        */
        let mut all_ascii_arena = Arena::with_capacity(extract_result.len());
        extract_result.iter_mut().for_each(|item| {
            if let Some(x) = item {
                {
                    let get_vec: Vec<_> = make_vec_to_borrow_arena(x)
                        .iter()
                        .flat_map(|per_proc| per_proc.first())
                        .cloned()
                        .collect();
                    all_ascii_arena.alloc(get_vec);
                }
            }
        });
        /*
        let all_unicode: Vec<_> = extract_result
            .iter()
            .filter_map(Some)
            .flat_map(|per_proc| per_proc.iter().nth(1))
            .flatten()
            .collect();
        */
        let mut all_unicode_arena = Arena::with_capacity(extract_result.len());
        extract_result.iter_mut().for_each(|item| {
            if let Some(x) = item {
                {
                    let get_vec: Vec<_> = make_vec_to_borrow_arena(x)
                        .iter()
                        .flat_map(|per_proc| per_proc.get(1))
                        .cloned()
                        .collect();
                    all_unicode_arena.alloc(get_vec);
                }
            }
        });

        let arena_ascii_borrow: Vec<_> = all_ascii_arena
            .iter_mut()
            .map(|x| x as &Vec<ExtractStr>)
            .collect();
        let arena_unicode_borrow: Vec<_> = all_unicode_arena
            .iter_mut()
            .map(|x| x as &Vec<ExtractStr>)
            .collect();
        let log_deref = log.deref();
        if let Some(x) = log_deref {
            let guard = x.lock();
            if let Ok(ok_guard) = guard {
                let len_unicode = arena_unicode_borrow.len();
                let len_ascii = arena_ascii_borrow.len();
                ok_guard.untrack_info(move || {
                    format!(
                        "arena unicode len: {} , arena_ascii: {}",
                        len_unicode, len_ascii
                    )
                });
            }
        }
        let res = ExtractStrResult {
            ascii: vec_flat2_owned_xz(arena_ascii_borrow),
            unicode: vec_flat2_owned_xz(arena_unicode_borrow),
        };
        Ok(res)
    })
}

fn find_strings(dwprocessid: u32, log: OptionLog) -> Result<ExtractStrResult, ()> {
    let strs = extract_str(dwprocessid, log);
    if let Err(e) = strs {
        println!("[DEBUG] strs Err: {:?}", e);
        return Err(());
    }
    Ok(strs.unwrap())
}

fn get_base_addr_assci(find_res: &ExtractStrResult) -> AddrResObj {
    find_res
        .ascii
        .iter()
        .map(|x| x.base_addr as *const c_void)
        .collect()
}

fn get_base_addr_unicode(find_res: &ExtractStrResult) -> AddrResObj {
    find_res
        .unicode
        .iter()
        .map(|x| x.base_addr as *const c_void)
        .collect()
}

#[derive(Clone)]
struct BaseAddrRes {
    pub assci: AddrResObj,
    pub unicode: AddrResObj,
}

impl BaseAddrRes {
    pub fn with_capacity(&mut self, cap_assci: usize, cap_unicode: usize) {
        self.assci = AddrResObj::with_capacity(cap_assci);
        self.unicode = AddrResObj::with_capacity(cap_unicode);
    }
}

unsafe fn get_base_addr_assci_send<T>(find_res: &ExtractStrResult) -> AddrResSendObj<T>
//where T: Clone,
{
    find_res
        .ascii
        .iter()
        .map(|x| SendablePtr(x.base_addr as *const T))
        .collect()
}

unsafe fn get_base_addr_unicode_send<T>(find_res: &ExtractStrResult) -> AddrResSendObj<T>
//where T: Clone,
{
    find_res
        .unicode
        .iter()
        .map(|x| SendablePtr(x.base_addr as *const T))
        .collect()
}

unsafe fn get_base_addr_all_send<T>(find_res: &ExtractStrResult) -> BaseAddrResSend<T>
//where T: Clone,
{
    let res_ascii = unsafe { get_base_addr_assci_send(find_res) };
    let res_unicode = unsafe { get_base_addr_unicode_send(find_res) };
    BaseAddrResSend::<T> {
        assci: res_ascii,
        unicode: res_unicode,
    }
}

unsafe fn get_base_addr_assci_send_pat_filter<T>(
    find_res: &ExtractStrResult,
    pat: impl Fn(&String) -> (bool, usize),
) -> AddrPosResSendObj<T>
//where T: Clone,
{
    /*
    find_res
        .ascii
        .iter()
        .filter(|&x| pat(&x.str))
        .map(|x| SendablePtr(x.base_addr as *const T))
        .collect::<Vec<_>>()
    */
    find_res
        .ascii
        .iter()
        .map(|x| (x, pat(&x.str)))
        .filter(|x| x.1.0)
        .map(|x| (SendablePtr(x.0.base_addr as *const T), x.1.1))
        .collect()
}

unsafe fn get_base_addr_unicode_send_pat_filter<T>(
    find_res: &ExtractStrResult,
    pat: impl Fn(&String) -> (bool, usize),
) -> AddrPosResSendObj<T>
//where T: Clone,
{
    /*
    find_res
        .unicode
        .iter()
        .filter(|&x| pat(&x.str).0)
        .map(|x| SendablePtr(x.base_addr as *const T))
        .collect::<Vec<_>>()
        */
    find_res
        .unicode
        .iter()
        .map(|x| (x, pat(&x.str)))
        .filter(|x| x.1.0)
        .map(|x| (SendablePtr(x.0.base_addr as *const T), x.1.1))
        .collect()
}

unsafe fn get_base_addr_all_send_pat_filter<T>(
    find_res: &ExtractStrResult,
    pat: impl Fn(&String) -> (bool, usize) + Clone,
) -> BaseAddrResSendPos<T>
//where T: Clone,
{
    let res_ascii = unsafe { get_base_addr_assci_send_pat_filter(find_res, pat.clone()) };
    let res_unicode = unsafe { get_base_addr_unicode_send_pat_filter(find_res, pat) };
    BaseAddrResSendPos::<T> {
        assci: res_ascii,
        unicode: res_unicode,
    }
}

#[derive(Clone)]
struct BaseAddrResSendPos<T>
//where T: Clone,
{
    pub assci: AddrPosResSendObj<T>,
    pub unicode: AddrPosResSendObj<T>,
}

#[derive(Clone)]
struct BaseAddrResSend<T>
//where T: Clone,
{
    pub assci: AddrResSendObj<T>,
    pub unicode: AddrResSendObj<T>,
}

impl<T> BaseAddrResSend<T> {
    pub fn with_capacity(&mut self, cap_assci: usize, cap_unicode: usize) {
        self.assci = AddrResSendObj::with_capacity(cap_assci);
        self.unicode = AddrResSendObj::with_capacity(cap_unicode);
    }
}

impl<T> BaseAddrResSendPos<T> {
    pub fn with_capacity(&mut self, cap_assci: usize, cap_unicode: usize) {
        self.assci = AddrPosResSendObj::with_capacity(cap_assci);
        self.unicode = AddrPosResSendObj::with_capacity(cap_unicode);
    }
}

fn get_base_addr_all(find_res: &ExtractStrResult) -> BaseAddrRes {
    let res_ascii = get_base_addr_assci(find_res);
    let res_unicode = get_base_addr_unicode(find_res);
    BaseAddrRes {
        assci: res_ascii,
        unicode: res_unicode,
    }
}

#[derive(Clone)]
struct ScanStrRes {
    pub finds_ascii: FindPatResObj<String>,
    pub finds_unicode: FindPatResObj<String>,
}

impl ScanStrRes {
    fn with_capacity(&mut self, cap_ascii: usize, cap_unicode: usize) {
        self.finds_ascii = FindPatResObj::with_capacity(cap_ascii);
        self.finds_unicode = FindPatResObj::with_capacity(cap_unicode);
    }
}

#[derive(Clone)]
struct ScanStrAllResCvoid {
    pub ssr: ScanStrRes,
    pub finds_addr: BaseAddrRes,
}

#[derive(Clone)]
struct ScanStrAllResSendPos<T>
//where T: Clone,
{
    pub ssr: ScanStrRes, //ub!
    pub finds_addr: BaseAddrResSendPos<T>,
}

#[derive(Clone)]
struct ScanStrAllResSend<T>
//where T: Clone,
{
    pub ssr: ScanStrRes, //ub!
    pub finds_addr: BaseAddrResSend<T>,
}

impl<T> ScanStrAllResSendPos<T> {
    pub fn with_capacity(&mut self, cap_ssr: usize, cap_finds_addr: usize) {
        self.finds_addr.with_capacity(cap_ssr, cap_ssr);
        self.ssr.with_capacity(cap_finds_addr, cap_finds_addr);
    }
}

#[hotpath::measure]
unsafe fn get_childs_dyn_pat_cvoid(
    pid: u32,
    pat: String,
    log: Arc<Option<Mutex<Logger>>>,
    min_len: usize,
    max_len: Option<usize>,
) -> Result<Vec<ScanStrAllResSendPos<SendableCvoidPtrMut>>, win_core::Error> {
    let childs = get_child_processes(pid);
    if let Err(e) = childs {
        println!("[ERROR] get_childs {:?}", e);
        Err(e)
    } else if let Ok(ok) = childs {
        let mut pids_vec: Vec<u32> = Vec::new();
        pids_vec.push(pid);
        let names = ok
            .iter()
            .map(|x| {
                pids_vec.push(x.0);
                format!("name exe {}\tpid: {}", x.1.clone(), x.0)
            })
            .collect::<Vec<String>>();
        let cnt_pids = pids_vec.len();
        let mut ret: Vec<ScanStrAllResSendPos<SendableCvoidPtrMut>> = Vec::with_capacity(cnt_pids);
        let ret_raw_ptr = ret.as_mut_ptr();
        let ret_ptr = SendablePtrMut::<ScanStrAllResSendPos<SendableCvoidPtrMut>>(ret_raw_ptr);

        println!("{}", VecString::vec_string(&names, DEFAULT_FORMAT_RULE));
        println!("CNT Pids:  {}", cnt_pids);
        let avb_p = std::thread::available_parallelism().unwrap_or(NonZero::new(8).unwrap());
        let pool = ThreadPool::new(/*cnt_pids*/ avb_p.get());
        let an_atomic = Arc::new(AtomicUsize::new(0));
        let cnt_job = cmp::max(cnt_pids / avb_p, 1);
        println!("cnt job {}", cnt_job);
        let jobs_vec = jobs_disp(cnt_job, pids_vec);
        let mut vec_cur: usize = 0;
        let mut ret_len = 0;
        let all_bytes_trace: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        for jobs in jobs_vec.into_iter() {
            let len_job = jobs.clone().len();
            let an_atomic = an_atomic.clone();
            let all_bytes_trace = all_bytes_trace.clone();
            let vec_cur_copy = vec_cur;
            let pat_clone = pat.clone();
            let ret_ptr_clone = ret_ptr.clone();
            let log_clone = log.clone();
            pool.execute(move || {
                for item in jobs.into_iter().enumerate() {
                    let pool_log_clone = log_clone.clone();
                    let find_res = extract_str_dyn_mem_lossy(
                        item.1,
                        pool_log_clone.clone(),
                        min_len,
                        max_len,
                        all_bytes_trace.clone(),
                    );
                    if find_res.is_err() {
                        println!("find strings failed: None");
                        return;
                    }
                    let find_res = find_res.unwrap();

                    //от memchr конечно профита нет но пофиг
                    #[allow(clippy::search_is_some)]
                    let finds_uc: Vec<String> = find_res
                        .unicode
                        .iter()
                        .filter(|x| {
                            memchr::memmem::find(x.str.as_bytes(), pat_clone.as_bytes()).is_some()
                        })
                        .map(|x| x.str.clone())
                        .collect();
                    #[allow(clippy::search_is_some)]
                    let finds_ascii: Vec<String> = find_res
                        .ascii
                        .iter()
                        .filter(|x| {
                            memchr::memmem::find(x.str.as_bytes(), pat_clone.as_bytes()).is_some()
                        })
                        .map(|x| x.str.clone())
                        .collect();

                    let finds_addr = unsafe {
                        get_base_addr_all_send_pat_filter::<SendableCvoidPtrMut>(&find_res, |x| {
                            let find = memchr::memmem::find(x.as_bytes(), pat_clone.as_bytes());
                            if let Some(x) = find {
                                (true, x)
                            } else {
                                (false, 0)
                            }
                        })
                    };
                    //бля адресса нефильтрую
                    //#[cfg(debug_assertions)]
                    {
                        let deref_log = pool_log_clone.deref().as_ref();
                        if let Some(x) = deref_log
                            && let Ok(guard) = x.lock()
                        {
                            let clone_finds_ascii = finds_ascii.clone();
                            guard.untrack_info(move || {
                                format!(
                                    "finds ascii: len: {}",
                                    VecString::vec_string(&clone_finds_ascii, DEFAULT_FORMAT_RULE)
                                        .len(),
                                )
                            });

                            let clone_finds_uc = finds_uc.clone();
                            guard.untrack_info(move || {
                                format!(
                                    "finds unicode: len: {}",
                                    VecString::vec_string(&clone_finds_uc, DEFAULT_FORMAT_RULE)
                                        .len(),
                                )
                            });
                            /*
                            let clone_finds_ascii = finds_ascii.clone();
                            guard.untrack_info(move || {
                                format!(
                                    "Ascii:\t{}",
                                    VecString::vec_string(&clone_finds_ascii, DEFAULT_FORMAT_RULE),
                                )
                            });
                            let clone_finds_uc = finds_uc.clone();
                            guard.untrack_info(move || {
                                format!(
                                    "Unicode:\t{}",
                                    VecString::vec_string(&clone_finds_uc, DEFAULT_FORMAT_RULE),
                                )
                            });
                            */
                        }
                    }
                    unsafe {
                        let ret_ptr = ret_ptr_clone.clone();
                        //раст не дает перемещать ptr
                        //мы создаем указатель внутри/если делать снаружи и писать что то типа (*ret_ptr).0 то ошибка что *mut
                        //нельзя перемещать
                        //потому что блять типо поле мы захватаем а не весь тип а поле 0 как раз у нас нихуя не send это *mut
                        let inner = ret_ptr.0.add(vec_cur_copy + item.0);
                        (*inner).finds_addr = finds_addr;
                        (*inner).ssr = ScanStrRes {
                            finds_ascii: FindPatResObj(finds_ascii),
                            finds_unicode: FindPatResObj(finds_uc),
                        };
                        //#[cfg(debug_assertions)]
                        // (*inner).finds_addr.assci.iter().for_each(|x|println!("addres ascii: {:p}",x.0));
                    }
                    an_atomic.fetch_add(1, Ordering::Relaxed);
                }
            });

            //println!("Ascii:\t{}", VecString::vec_string(&find_res.ascii, DEFAULT_FORMAT_RULE));
            /*
            println!(
                "Unicode:\t{}",
                VecString::vec_string(&find_res.unicode, DEFAULT_FORMAT_RULE)
            );
            */
            vec_cur += len_job;
            ret_len += len_job;
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
        unsafe {
            ret.set_len(ret_len);
        }
        println!("log");
        let _ = shutdown_logger(log);
        Ok(ret)
    } else {
        unreachable!();
    }
}
pub struct ScanStrAllResSendArena<T> {
    pub finds_ascii: FindPatsResArenaObj<String>,
    pub finds_unicode: FindPatsResArenaObj<String>,
    pub finds_addr: Arena<BaseAddrResSendPos<T>>,
}

pub struct ScanStrAllResPosSendArena<T> {
    pub finds_ascii: FindPatsResArenaObj<String>,
    pub finds_unicode: FindPatsResArenaObj<String>,
    pub finds_addr: Arena<BaseAddrResSend<T>>,
    pub find_ascii_res_pos_arena: Arena<FindPatResObj<PosFindObj>>,
    pub find_uc_res_pos_arena: Arena<FindPatResObj<PosFindObj>>,
}

#[repr(transparent)]
#[derive(Clone)]
pub struct FindPatResObj<T>(pub Vec<T>);

#[repr(transparent)]
pub struct FindPatsResArenaObj<T>(pub Arena<FindPatResObj<T>>);

#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PosFindObj(pub Option<usize>);

impl<T> FindPatResObj<T> {
    pub fn new() -> Self {
        FindPatResObj(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> Self {
        FindPatResObj(Vec::with_capacity(cap))
    }
}

impl<T> Default for FindPatResObj<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FromIterator<T> for FindPatResObj<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        FindPatResObj(iter.into_iter().collect())
    }
}

impl<T> Deref for FindPatResObj<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> DerefMut for FindPatResObj<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> Deref for FindPatsResArenaObj<T> {
    type Target = Arena<FindPatResObj<T>>;
    fn deref(&self) -> &Arena<FindPatResObj<T>> {
        &self.0
    }
}

impl<T> DerefMut for FindPatsResArenaObj<T> {
    fn deref_mut(&mut self) -> &mut Arena<FindPatResObj<T>> {
        &mut self.0
    }
}

impl Deref for PosFindObj {
    type Target = Option<usize>;
    fn deref(&self) -> &Option<usize> {
        &self.0
    }
}

impl DerefMut for PosFindObj {
    fn deref_mut(&mut self) -> &mut Option<usize> {
        &mut self.0
    }
}

impl<T: core::fmt::Display> VecString for FindPatResObj<T> {
    fn vec_string(&self, format_rule: FormatRuleFn) -> String {
        VecString::vec_string(&self.0, format_rule)
    }
}

// ── адреса (raw *const c_void) ──

#[repr(transparent)]
#[derive(Clone)]
pub struct AddrResObj(pub Vec<*const c_void>);

impl AddrResObj {
    pub fn new() -> Self {
        AddrResObj(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> Self {
        AddrResObj(Vec::with_capacity(cap))
    }
}

impl Default for AddrResObj {
    fn default() -> Self {
        Self::new()
    }
}

impl FromIterator<*const c_void> for AddrResObj {
    fn from_iter<I: IntoIterator<Item = *const c_void>>(iter: I) -> Self {
        AddrResObj(iter.into_iter().collect())
    }
}

impl Deref for AddrResObj {
    type Target = Vec<*const c_void>;
    fn deref(&self) -> &Vec<*const c_void> {
        &self.0
    }
}

impl DerefMut for AddrResObj {
    fn deref_mut(&mut self) -> &mut Vec<*const c_void> {
        &mut self.0
    }
}

// ── адреса (send SendablePtr<T>) ──

#[repr(transparent)]
#[derive(Clone)]
pub struct AddrResSendObj<T>(pub Vec<SendablePtr<T>>);

impl<T> AddrResSendObj<T> {
    pub fn new() -> Self {
        AddrResSendObj(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> Self {
        AddrResSendObj(Vec::with_capacity(cap))
    }
}

impl<T> Default for AddrResSendObj<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FromIterator<SendablePtr<T>> for AddrResSendObj<T> {
    fn from_iter<I: IntoIterator<Item = SendablePtr<T>>>(iter: I) -> Self {
        AddrResSendObj(iter.into_iter().collect())
    }
}

impl<T> Deref for AddrResSendObj<T> {
    type Target = Vec<SendablePtr<T>>;
    fn deref(&self) -> &Vec<SendablePtr<T>> {
        &self.0
    }
}

impl<T> DerefMut for AddrResSendObj<T> {
    fn deref_mut(&mut self) -> &mut Vec<SendablePtr<T>> {
        &mut self.0
    }
}

// ── адреса + позиция (send SendablePtr<T> + usize) ──

#[repr(transparent)]
#[derive(Clone)]
pub struct AddrPosResSendObj<T>(pub Vec<(SendablePtr<T>, usize)>);

impl<T> AddrPosResSendObj<T> {
    pub fn new() -> Self {
        AddrPosResSendObj(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> Self {
        AddrPosResSendObj(Vec::with_capacity(cap))
    }
}

impl<T> Default for AddrPosResSendObj<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FromIterator<(SendablePtr<T>, usize)> for AddrPosResSendObj<T> {
    fn from_iter<I: IntoIterator<Item = (SendablePtr<T>, usize)>>(iter: I) -> Self {
        AddrPosResSendObj(iter.into_iter().collect())
    }
}

impl<T> Deref for AddrPosResSendObj<T> {
    type Target = Vec<(SendablePtr<T>, usize)>;
    fn deref(&self) -> &Vec<(SendablePtr<T>, usize)> {
        &self.0
    }
}

impl<T> DerefMut for AddrPosResSendObj<T> {
    fn deref_mut(&mut self) -> &mut Vec<(SendablePtr<T>, usize)> {
        &mut self.0
    }
}

#[hotpath::measure]
unsafe fn get_childs_dyn_pats_cvoid(
    pid: u32,
    pats: Box<[String]>,
    log: Arc<Option<Mutex<Logger>>>,
    min_len: usize,
    max_len: Option<usize>,
) -> Result<Vec<ScanStrAllResPosSendArena<SendableCvoidPtrMut>>, win_core::Error> {
    let childs = get_child_processes(pid);
    if let Err(e) = childs {
        println!("[ERROR] get_childs {:?}", e);
        Err(e)
    } else if let Ok(ok) = childs {
        let mut pids_vec: Vec<u32> = Vec::new();
        pids_vec.push(pid);
        let names = ok
            .iter()
            .map(|x| {
                pids_vec.push(x.0);
                format!("name exe {}\tpid: {}", x.1.clone(), x.0)
            })
            .collect::<Vec<String>>();
        let cnt_pids = pids_vec.len();
        let mut ret = Vec::with_capacity(cnt_pids);
        let ret_raw_ptr = ret.as_mut_ptr();
        let ret_ptr = SendablePtrMut::<ScanStrAllResPosSendArena<SendableCvoidPtrMut>>(ret_raw_ptr);

        println!("{}", VecString::vec_string(&names, DEFAULT_FORMAT_RULE));
        println!("CNT Pids:  {}", cnt_pids);
        let avb_p = std::thread::available_parallelism().unwrap_or(NonZero::new(8).unwrap());
        let pool = ThreadPool::new(/*cnt_pids*/ avb_p.get());
        let an_atomic = Arc::new(AtomicUsize::new(0));
        let all_trace = Arc::new(AtomicUsize::new(0));
        let cnt_job = cmp::max(cnt_pids / avb_p, 1);
        println!("cnt job {}", cnt_job);
        let jobs_vec = jobs_disp(cnt_job, pids_vec);
        let mut vec_cur = 0;
        let mut ret_len = 0;
        //let len_pats = pats.len();
        for jobs in jobs_vec.into_iter() {
            let len_job = jobs.clone().len();
            let an_atomic = an_atomic.clone();
            let all_trace = all_trace.clone();
            let vec_cur_copy = vec_cur;
            let pat_clone = pats.clone();
            let ret_ptr_clone = ret_ptr.clone();
            let log_clone = log.clone();
            pool.execute(move || {
                for item in jobs.into_iter().enumerate() {
                    let pool_log_clone = log_clone.clone();
                    let find_res = extract_str_dyn_mem_lossy(
                        item.1,
                        pool_log_clone.clone(),
                        min_len,
                        max_len,
                        all_trace.clone(),
                    );
                    if find_res.is_err() {
                        let deref_log = pool_log_clone.deref().as_ref();
                        if let Some(x) = deref_log
                            && let Ok(guard) = x.lock()
                        {
                            guard.untrack_error(|| "find strings failed: None");
                        }
                        return;
                    }
                    let find_res = find_res.unwrap();

                    let finds_uc_arena: FindPatsResArenaObj<String> =
                        FindPatsResArenaObj(Arena::new());
                    let finds_ascii_arena: FindPatsResArenaObj<String> =
                        FindPatsResArenaObj(Arena::new());
                    let finds_addr_arena = Arena::new();
                    let find_ascii_res_pos_arena = Arena::new();
                    let find_uc_res_pos_arena = Arena::new();

                    //от memchr конечно профита нет но пофиг
                    #[allow(clippy::search_is_some)]
                    for item in pat_clone.iter() {
                        let mut find_uc_res_pos: FindPatResObj<PosFindObj> =
                            FindPatResObj::with_capacity(find_res.unicode.len() / 2);
                        let finds_uc: FindPatResObj<String> = FindPatResObj(
                            find_res
                                .unicode
                                .iter()
                                .filter(|x| {
                                    let find =
                                        memchr::memmem::find(x.str.as_bytes(), item.as_bytes());
                                    if let Some(x) = find {
                                        find_uc_res_pos.push(PosFindObj(Some(x)));
                                        return true;
                                    }
                                    find_uc_res_pos.push(PosFindObj(None));
                                    false
                                })
                                .map(|x| x.str.clone())
                                .collect(),
                        );
                        let mut find_ascii_res_pos: FindPatResObj<PosFindObj> =
                            FindPatResObj::with_capacity(find_res.unicode.len() / 2);
                        #[allow(clippy::search_is_some)]
                        let finds_ascii: FindPatResObj<String> = find_res
                            .ascii
                            .iter()
                            .filter(|x| {
                                let find = memchr::memmem::find(x.str.as_bytes(), item.as_bytes());
                                if let Some(x) = find {
                                    find_ascii_res_pos.push(PosFindObj(Some(x)));
                                    return true;
                                }
                                find_ascii_res_pos.push(PosFindObj(None));
                                false
                            })
                            .map(|x| x.str.clone())
                            .collect();

                        //#[cfg(debug_assertions)]
                        let finds_ascii_clone = finds_ascii.clone();
                        let finds_uc_clone = finds_uc.clone();

                        finds_ascii_arena.alloc(finds_ascii);
                        finds_uc_arena.alloc(finds_uc);
                        find_ascii_res_pos_arena.alloc(find_ascii_res_pos);
                        find_uc_res_pos_arena.alloc(find_uc_res_pos);

                        let finds_addr =
                            unsafe { get_base_addr_all_send::<SendableCvoidPtrMut>(&find_res) };
                        finds_addr_arena.alloc(finds_addr);
                        //бля адресса нефильтрую
                        //#[cfg(debug_assertions)]
                        {
                            let deref_log = pool_log_clone.deref().as_ref();
                            if let Some(x) = deref_log
                                && let Ok(guard) = x.lock()
                            {
                                let clone_finds_ascii = finds_ascii_clone.clone();
                                guard.untrack_info(move || {
                                    format!(
                                        "finds ascii: len: {}",
                                        VecString::vec_string(
                                            &clone_finds_ascii,
                                            DEFAULT_FORMAT_RULE
                                        )
                                        .len(),
                                    )
                                });

                                let clone_finds_uc = finds_uc_clone.clone();
                                guard.untrack_info(move || {
                                    format!(
                                        "finds unicode: len: {}",
                                        VecString::vec_string(&clone_finds_uc, DEFAULT_FORMAT_RULE)
                                            .len(),
                                    )
                                });
                                let an_atomic_clone = an_atomic.clone();
                                guard.untrack_info(move || {
                                    format!("number: {}", an_atomic_clone.load(Ordering::Relaxed))
                                });
                                /*
                                let clone_finds_ascii = finds_ascii.clone();
                                guard.untrack_info(move || {
                                    format!(
                                        "Ascii:\t{}",
                                        VecString::vec_string(&clone_finds_ascii, DEFAULT_FORMAT_RULE),
                                    )
                                });
                                let clone_finds_uc = finds_uc.clone();
                                guard.untrack_info(move || {
                                    format!(
                                        "Unicode:\t{}",
                                        VecString::vec_string(&clone_finds_uc, DEFAULT_FORMAT_RULE),
                                    )
                                });
                                */
                            }
                        }
                    }
                    unsafe {
                        let ret_ptr = ret_ptr_clone.clone();
                        //раст не дает перемещать ptr
                        //мы создаем указатель внутри/если делать снаружи и писать что то типа (*ret_ptr).0 то ошибка что *mut
                        //нельзя перемещать
                        //потому что блять типо поле мы захватаем а не весь тип а поле 0 как раз у нас нихуя не send это *mut
                        let inner = ret_ptr.0.add(vec_cur_copy + item.0);
                        (*inner).finds_addr = finds_addr_arena;
                        (*inner).finds_ascii = finds_ascii_arena;
                        (*inner).finds_unicode = finds_uc_arena;
                        (*inner).find_ascii_res_pos_arena = find_ascii_res_pos_arena;
                        (*inner).find_uc_res_pos_arena = find_uc_res_pos_arena;

                        //#[cfg(debug_assertions)]
                        // (*inner).finds_addr.assci.iter().for_each(|x|println!("addres ascii: {:p}",x.0));
                    }
                    an_atomic.fetch_add(1, Ordering::Relaxed);
                }
            });

            //println!("Ascii:\t{}", VecString::vec_string(&find_res.ascii, DEFAULT_FORMAT_RULE));
            /*
            println!(
                "Unicode:\t{}",
                VecString::vec_string(&find_res.unicode, DEFAULT_FORMAT_RULE)
            );
            */
            vec_cur += len_job;
            ret_len += len_job;
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
        unsafe {
            ret.set_len(ret_len);
        }
        println!("log");
        let _ = shutdown_logger(log);
        Ok(ret)
    } else {
        unreachable!();
    }
}

#[hotpath::measure]
unsafe fn get_childs_cvoid(
    pid: u32,
    log: Arc<Option<Mutex<Logger>>>,
) -> Result<Vec<ScanStrAllResSend<SendableCvoidPtrMut>>, win_core::Error> {
    let childs = get_child_processes(pid);
    if let Err(e) = childs {
        println!("[ERROR] get_childs {:?}", e);
        Err(e)
    } else if let Ok(ok) = childs {
        let mut pids_vec: Vec<u32> = Vec::new();
        pids_vec.push(pid);
        let names = ok
            .iter()
            .map(|x| {
                pids_vec.push(x.0);
                format!("name exe {}\tpid: {}", x.1.clone(), x.0)
            })
            .collect::<Vec<String>>();
        println!("{}", VecString::vec_string(&names, DEFAULT_FORMAT_RULE));
        let cnt_pids = pids_vec.len();
        let pool = ThreadPool::new(cnt_pids);
        let an_atomic = Arc::new(AtomicUsize::new(0));
        let avb_p = std::thread::available_parallelism().unwrap_or(NonZero::new(8).unwrap());
        let cnt_job = cmp::max(cnt_pids / avb_p, 1);
        let mut vec_cur: usize = 0;
        let mut ret: Vec<ScanStrAllResSend<SendableCvoidPtrMut>> = Vec::with_capacity(cnt_pids);
        let ret_raw_ptr = ret.as_mut_ptr();
        let ret_ptr = SendablePtrMut::<ScanStrAllResSend<SendableCvoidPtrMut>>(ret_raw_ptr);
        let mut ret_len = 0;

        let jobs_vec = jobs_disp(cnt_job, pids_vec);
        for jobs in jobs_vec.into_iter() {
            let len_job = jobs.clone().len();
            let vec_cur_copy = vec_cur;
            let an_atomic = an_atomic.clone();
            let ret_ptr_clone = ret_ptr.clone();
            let log_clone = log.clone();
            pool.execute(move || {
                for item in jobs.into_iter().enumerate() {
                    let pool_log_clone = log_clone.clone();
                    let find_res = find_strings(item.1, pool_log_clone);
                    if find_res.is_err() {
                        println!("find strings failed: None");
                        return;
                    }
                    let find_res = find_res.unwrap();
                    let finds_addr =
                        unsafe { get_base_addr_all_send::<SendableCvoidPtrMut>(&find_res) };
                    let finds_ascii = find_res.ascii.iter().map(|x| x.str.clone()).collect();
                    let finds_unicode = find_res.unicode.iter().map(|x| x.str.clone()).collect();
                    #[cfg(debug_assertions)]
                    {
                        println!(
                            "Ascii:\t{}\tlen: {}",
                            VecString::vec_string(&find_res.ascii, DEFAULT_FORMAT_RULE),
                            VecString::vec_string(&find_res.ascii, DEFAULT_FORMAT_RULE).len()
                        );
                        println!(
                            "Unicode:\t{}\tlen: {}",
                            VecString::vec_string(&find_res.unicode, DEFAULT_FORMAT_RULE),
                            VecString::vec_string(&find_res.unicode, DEFAULT_FORMAT_RULE).len(),
                        );
                    }
                    unsafe {
                        let ret_ptr = ret_ptr_clone.clone();
                        let inner = ret_ptr.0.add(vec_cur_copy + item.0);
                        (*inner).finds_addr = finds_addr;
                        (*inner).ssr = ScanStrRes {
                            finds_ascii,
                            finds_unicode,
                        };
                    }
                    an_atomic.fetch_add(1, Ordering::Relaxed);
                }
            });
            vec_cur += len_job;
            ret_len += len_job;
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
        unsafe {
            ret.set_len(ret_len);
        }
        let _ = shutdown_logger(log);
        Ok(ret)
    } else {
        unreachable!();
    }
}

fn shutdown_logger(log: Arc<Option<Mutex<Logger>>>) -> Result<(), ()> {
    if let Some(x) = log.deref() {
        let guard = x.lock();
        match guard {
            Ok(mut ok_guard) => {
                if ok_guard.shutdown().is_err() {
                    #[cfg(debug_assertions)]
                    println!("ошибка закрытия логгера");
                    return Err(());
                }
            }
            Err(ref e) => {
                #[cfg(debug_assertions)]
                println!("Logger отравлен: {:?}", e);
                return Err(());
            }
        }
    }
    Ok(())
}

pub struct MorePatsExtractRes {
    pub extract_assci_str: Vec<Vec<FindPatResObj<String>>>,
    pub extract_unicode_str: Vec<Vec<FindPatResObj<String>>>,
    pub extract_addr_only_ascii: Vec<AddrResSendObj<SendableCvoidPtrMut>>,
    pub extract_addr_only_unicode: Vec<AddrResSendObj<SendableCvoidPtrMut>>,
    pub extract_find_pos_ascii: Vec<Vec<FindPatResObj<PosFindObj>>>,
    pub extract_find_pos_unicode: Vec<Vec<FindPatResObj<PosFindObj>>>,
}

pub struct MorePatsExtractResPos {
    pub extract_assci_str: Vec<Vec<Vec<String>>>,
    pub extract_unicode_str: Vec<Vec<Vec<String>>>,
    pub extract_addr_only_ascii: Vec<Vec<(SendablePtr<SendableCvoidPtrMut>, usize)>>,
    pub extract_addr_only_unicode: Vec<Vec<(SendablePtr<SendableCvoidPtrMut>, usize)>>,
}

//надо сделать еще одну версию которая не будет смешивать
//<Vec<(SendablePtr<SendableCvoidPtrMut>, usize)> usize а usize как отдельная арена
#[hotpath::measure]
fn more_pats(
    log: Arc<Option<Mutex<Logger>>>,
    pid: u32,
) -> windows::core::Result<MorePatsExtractRes> {
    let res_dyn_pats;
    unsafe {
        res_dyn_pats = get_childs_dyn_pats_cvoid(
            pid,
            Box::new(["russh".to_string(), "hotpath".to_string()]),
            log,
            1,
            Some(14000),
        );
    }
    if let Err(e) = res_dyn_pats {
        println!("[ERROR] {:?}", e);
        wait_close();
        return Err(e);
    }
    //todo rayon
    let mut res_dyn_pats = res_dyn_pats.unwrap();
    let extract_assci_str: Vec<_> = res_dyn_pats
        .iter_mut()
        .map(|x| make_vec_to_borrow_arena(&mut x.finds_ascii))
        .collect();
    let extract_unicode_str: Vec<_> = res_dyn_pats
        .iter_mut()
        .map(|x| make_vec_to_borrow_arena(&mut x.finds_unicode))
        .collect();
    let extract_addr: Vec<_> = res_dyn_pats
        .iter_mut()
        .map(|x| make_vec_to_borrow_arena(&mut x.finds_addr))
        .collect();
    let extract_find_pos_ascii: Vec<_> = res_dyn_pats
        .iter_mut()
        .map(|x| make_vec_to_borrow_arena(&mut x.find_ascii_res_pos_arena))
        .collect();
    let extract_find_pos_unicode: Vec<_> = res_dyn_pats
        .iter_mut()
        .map(|x| make_vec_to_borrow_arena(&mut x.find_uc_res_pos_arena))
        .collect();
    drop(res_dyn_pats);

    let extract_addr_only_ascii: Vec<_> = extract_addr
        .iter()
        .flatten()
        .map(|x| x.assci.clone())
        .collect();
    let extract_addr_only_unicode: Vec<_> = extract_addr
        .iter()
        .flatten()
        .map(|x| x.unicode.clone())
        .collect();

    Ok(MorePatsExtractRes {
        extract_assci_str,
        extract_unicode_str,
        extract_addr_only_ascii,
        extract_addr_only_unicode,
        extract_find_pos_ascii,
        extract_find_pos_unicode,
    })
}

fn more_pats_run(name: &str) {
    let build_log = Logger::safe_builder(None, None);
    let unwrap = match build_log {
        LoggerRes::Ok(ok) => ok.ok(),
        LoggerRes::Panic(_) => {
            println!("[ERROR] Logger отвалился");
            None
        }
    };
    let log = unwrap.map_or_else(|| Arc::new(None), |x| Arc::new(Some(Mutex::new(x))));
    let privilege_res = enable_privilege_one("SeDebugPrivilege");
    if let Err(e) = privilege_res {
        println!("Error: {:?}", e);
        wait_close();
    }
    let fnd_name = find_process_by_name(name);
    if let Err(e) = fnd_name {
        println!("Error: {:?}", e);
        wait_close();
        return;
    }
    let pid = fnd_name.unwrap();
    println!("[DEBUG] pid: {}", pid);
    let res = more_pats(log, pid);
    if let Err(e) = res {
        println!("[ERROR] {:?}", e);
        wait_close();
        return;
    }
    let res = res.unwrap();
    //#[cfg(debug_assertions)]
    println!(
        "addres cnt ascii: {} unicode: {}",
        res.extract_addr_only_ascii.len(),
        res.extract_addr_only_unicode.len()
    );

    // #[cfg(debug_assertions)]
    {
        println!("address print:");
        let fmt_assci_addr: Vec<_> = res
            .extract_addr_only_ascii
            .iter()
            .map(|x| {
                x.iter()
                    .map(|y| {
                        if y.0.is_null() {
                            "".to_string()
                        } else {
                            format!("{:p}", y.0)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let fmt_unicode_addr: Vec<_> = res
            .extract_addr_only_unicode
            .iter()
            .map(|x| {
                x.iter()
                    .map(|y| {
                        if y.0.is_null() {
                            "".to_string()
                        } else {
                            format!("{:p}", y.0)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let extract_ascii_pos_iter = res.extract_find_pos_ascii.iter();
        let extract_unicode_pos_iter = res.extract_find_pos_unicode.iter();
        let str_assci_addr = VecString::vec_string(
            &fmt_assci_addr.iter().flatten().collect::<Vec<_>>(),
            DEFAULT_FORMAT_RULE,
        );
        let str_unicode_addr = VecString::vec_string(
            &fmt_unicode_addr.iter().flatten().collect::<Vec<_>>(),
            DEFAULT_FORMAT_RULE,
        );
        println!("ASSCI ADDR:  {}", str_assci_addr);
        println!("UNICODE ADDR:  {}", str_unicode_addr);
        println!("\n\nstrs: ");

        for (item, pos) in res
            .extract_assci_str
            .iter()
            .flatten()
            .zip(extract_ascii_pos_iter.flatten())
        {
            if item.len() < 64 {
                println!(
                    "ascii: {}",
                    VecString::vec_string(item, DEFAULT_FORMAT_RULE)
                );
            } else {
                let pos_unwrap = pos.iter().find_map(|p| p.0).unwrap_or_default();
                let format_rule = |val: &str, index: usize, len: usize| -> String {
                    if val.len() < 64 {
                        if index == 0 {
                            format!("[{}", val)
                        } else if index != len - 1 {
                            format!(", {}", val)
                        } else {
                            format!(", {}]", val)
                        }
                    } else {
                        let val = format!("[{}", val.get(pos_unwrap..=63).unwrap_or_default());
                        if index == 0 {
                            format!("[{}", val)
                        } else if index != len - 1 {
                            format!(", {}", val)
                        } else {
                            format!(", {}]", val)
                        }
                    }
                };
                println!(
                    "ascii: {} ...",
                    VecStringFn::vec_string(
                        &item
                            .iter()
                            .map(|x| x.get((pos_unwrap)..63).unwrap_or_default())
                            .collect::<Vec<_>>(),
                        format_rule
                    )
                );
            }
        }
        for (item, pos) in res
            .extract_assci_str
            .iter()
            .flatten()
            .zip(extract_unicode_pos_iter.flatten())
        {
            if item.len() < 64 {
                println!(
                    "unicode: {}",
                    VecString::vec_string(item, DEFAULT_FORMAT_RULE)
                );
            } else {
                let pos_unwrap = pos.iter().find_map(|p| p.0).unwrap_or_default();
                let format_rule = |val: &str, index: usize, len: usize| -> String {
                    if val.len() < 64 {
                        if index == 0 {
                            format!("[{}", val)
                        } else if index != len - 1 {
                            format!(", {}", val)
                        } else {
                            format!(", {}]", val)
                        }
                    } else {
                        let val = format!("[{}", val.get(pos_unwrap..=63).unwrap_or_default());
                        if index == 0 {
                            format!("[{}", val)
                        } else if index != len - 1 {
                            format!(", {}", val)
                        } else {
                            format!(", {}]", val)
                        }
                    }
                };

                println!(
                    "unicode: {} ...",
                    VecStringFn::vec_string(
                        &item
                            .iter()
                            .map(|x| x.get((pos_unwrap)..63).unwrap_or_default())
                            .collect::<Vec<_>>(),
                        format_rule
                    )
                );
            }
        }
    }
}

#[test]
fn more_pats_test() {
    more_pats_run("firefox.exe");
}

#[hotpath::main]
fn main() {
    /*
    let build_log = Logger::safe_builder(None, None);
    let unwrap = match build_log {
        LoggerRes::Ok(ok) => ok.ok(),
        LoggerRes::Panic(_) => {
            println!("[ERROR] Logger отвалился");
            None
        }
    };
    let log = unwrap.map_or_else(|| Arc::new(None), |x| Arc::new(Some(Mutex::new(x))));

    //find_strings();
    let privilege_res = enable_privilege_one("SeDebugPrivilege");
    if let Err(e) = privilege_res {
        println!("Error: {:?}", e);
        wait_close();
    }
    let fnd_name = find_process_by_name("firefox.exe");
    if let Err(e) = fnd_name {
        println!("Error: {:?}", e);
        wait_close();
        return;
    }
    let pid = fnd_name.unwrap();
    println!("[DEBUG] pid: {}", pid);
    unsafe {
        let res_dyn_pat: Result<Vec<ScanStrAllResSend<SendableCvoidPtrMut>>, win_core::Error> =
            get_childs_dyn_pat_cvoid(pid, "russh".to_string(), log, 1, Some(14000));
        if let Err(e) = res_dyn_pat {
            println!("[ERROR] {:?}", e);
            wait_close();
            return;
        }
        let res_dyn_pat = res_dyn_pat.unwrap();
        //todo use rayon
        let addr_only_assci: Vec<_> = res_dyn_pat
            .iter()
            .flat_map(|x| x.finds_addr.assci.clone())
            .collect();
        let addr_only_unicode: Vec<_> = res_dyn_pat
            .iter()
            .flat_map(|x| x.finds_addr.unicode.clone())
            .collect();
        let extract_ascii_str: Vec<_> = res_dyn_pat
            .iter()
            .flat_map(|x| x.ssr.finds_ascii.clone())
            .collect();
        let extract_unicode_str: Vec<_> = res_dyn_pat
            .iter()
            .flat_map(|x| x.ssr.finds_unicode.clone())
            .collect();
        drop(res_dyn_pat);

        //#[cfg(debug_assertions)]
        println!(
            "addres cnt ascii: {} unicode: {}",
            addr_only_assci.len(),
            addr_only_unicode.len()
        );
        // #[cfg(debug_assertions)]
        {
            println!("address print:");
            let fmt_assci_addr: Vec<_> = addr_only_assci
                .iter()
                .map(|x| {
                    if x.0.is_null() {
                        "".to_string()
                    } else {
                        format!("{:p}", x.0)
                    }
                })
                .collect();
            let fmt_unicode_addr: Vec<_> = addr_only_unicode
                .iter()
                .map(|x| {
                    if x.0.is_null() {
                        "".to_string()
                    } else {
                        format!("{:p}", x.0)
                    }
                })
                .collect();
            let str_assci_addr = fmt_assci_addr.vec_string(DEFAULT_FORMAT_RULE);
            let str_unicode_addr = fmt_unicode_addr.vec_string(DEFAULT_FORMAT_RULE);
            println!("ASSCI ADDR:  {}", str_assci_addr);
            println!("UNICODE ADDR:  {}", str_unicode_addr);
            println!("\n\nstrs: ");
            for item in extract_ascii_str.iter() {
                if item.len() < 64 {
                    println!("ascii: {}", item);
                } else {
                    println!("ascii: {} ...", item.get(0..63).unwrap_or_default());
                }
            }
            for item in extract_unicode_str.iter() {
                if item.len() < 64 {
                    println!("unicode: {}", item);
                } else {
                    println!("unicode: {} ...", item.get(0..63).unwrap_or_default());
                }
            }
        }
        let addr_tuple: Vec<_> = addr_only_assci.into_iter().zip(addr_only_unicode).collect();
    };
    unsafe {
        //let _: Result<Vec<ScanStrAllResSend::<SendableCvoidPtrMut>>, win_core::Error> =
        //get_childs_cvoid(pid);
    }

    std::thread::sleep(std::time::Duration::from_millis(1000));
    */
    println!("нажми enter для теста more pats");
    wait_close();
    more_pats_run("firefox.exe");
    wait_close();
}
