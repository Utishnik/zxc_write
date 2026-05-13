use core::ffi::c_void;
use std::cmp;
use std::num::NonZero;
use std::ops::Deref;
use std::os::raw::c_double;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use threadpool::ThreadPool;
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

fn extract_str_dyn_mem(dwprocessid: u32, log: OptionLog) -> Result<ExtractStrResult, ()> {
    let extract_ascii_strings_fn =
        |buf, size, base_ptr| unsafe { extract_ascii_strings(buf, size, base_ptr, 5, None) };
    let extract_unicode_strings_fn =
        |buf, size, base_ptr| unsafe { extract_unicode_strings(buf, size, base_ptr, 5, None) };
    let scan_res = scan_dynamic_mem(dwprocessid, 48, 500_000_000, 101_704_332_083_002, None);

    if let Err(_) = scan_res {
        return Err(());
    }
    let scan_res = scan_res.unwrap();
    let mut processors = [extract_ascii_strings_fn, extract_unicode_strings_fn];
    let result = scan_process_processors_mbi(dwprocessid, &mut processors, 50000, scan_res, log);
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

fn find_strings(dwprocessid: u32, log: OptionLog) -> Result<ExtractStrResult, ()> {
    let strs = extract_str(dwprocessid, log);
    if let Err(e) = strs {
        println!("[DEBUG] strs Err: {:?}", e);
        return Err(());
    }
    Ok(strs.unwrap())
}

fn get_base_addr_assci(find_res: &ExtractStrResult) -> Vec<*const c_void> {
    find_res
        .ascii
        .iter()
        .map(|x| x.base_addr as *const c_void)
        .collect::<Vec<_>>()
}

fn get_base_addr_unicode(find_res: &ExtractStrResult) -> Vec<*const c_void> {
    find_res
        .unicode
        .iter()
        .map(|x| x.base_addr as *const c_void)
        .collect::<Vec<_>>()
}

#[derive(Clone)]
struct BaseAddrRes {
    pub assci: Vec<*const c_void>,
    pub unicode: Vec<*const c_void>,
}

unsafe fn get_base_addr_assci_send<T>(find_res: &ExtractStrResult) -> Vec<SendablePtr<T>>
//where T: Clone,
{
    find_res
        .ascii
        .iter()
        .map(|x| SendablePtr(x.base_addr as *const T))
        .collect::<Vec<_>>()
}

unsafe fn get_base_addr_unicode_send<T>(find_res: &ExtractStrResult) -> Vec<SendablePtr<T>>
//where T: Clone,
{
    find_res
        .unicode
        .iter()
        .map(|x| SendablePtr(x.base_addr as *const T))
        .collect::<Vec<_>>()
}

#[derive(Clone)]
struct BaseAddrResSend<T>
//where T: Clone,
{
    pub assci: Vec<SendablePtr<T>>,
    pub unicode: Vec<SendablePtr<T>>,
}

fn get_base_addr_all(find_res: &ExtractStrResult) -> BaseAddrRes {
    let res_ascii = get_base_addr_assci(find_res);
    let res_unicode = get_base_addr_unicode(find_res);
    BaseAddrRes {
        assci: res_ascii,
        unicode: res_unicode,
    }
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

#[derive(Clone)]
struct ScanStrRes {
    pub finds_ascii: Vec<String>,
    pub finds_unicode: Vec<String>,
}

#[derive(Clone)]
struct ScanStrAllRes {
    pub ssr: ScanStrRes,
    pub finds_addr: BaseAddrRes,
}

#[derive(Clone)]
struct ScanStrAllResSend<T>
//where T: Clone,
{
    pub ssr: ScanStrRes,
    pub finds_addr: BaseAddrResSend<T>,
}

unsafe fn get_childs_dyn_pat_cvoid(
    pid: u32,
    pat: String,
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
        let cnt_pids = pids_vec.len();
        let mut ret: Vec<ScanStrAllResSend<SendableCvoidPtrMut>> = Vec::with_capacity(cnt_pids);
        let ret_raw_ptr = ret.as_mut_ptr();
        let ret_ptr = SendablePtrMut::<ScanStrAllResSend<SendableCvoidPtrMut>>(ret_raw_ptr);

        println!("{}", names.vec_string(DEFAULT_FORMAT_RULE));
        println!("CNT Pids:  {}", cnt_pids);
        let avb_p = std::thread::available_parallelism().unwrap_or(NonZero::new(8).unwrap());
        let pool = ThreadPool::new(/*cnt_pids*/ avb_p.get());
        let an_atomic = Arc::new(AtomicUsize::new(0));
        let cnt_job = cmp::max(cnt_pids / avb_p, 1);
        println!("cnt job {}", cnt_job);
        let jobs_vec = jobs_disp(cnt_job, pids_vec);
        let mut vec_cur: usize = 0;
        for jobs in jobs_vec.into_iter() {
            let l = jobs.len();
            println!("{} jb len", l);
            let len_job = jobs.clone().len();
            let an_atomic = an_atomic.clone();
            let vec_cur_copy = vec_cur;
            let pat_clone = pat.clone();
            let ret_ptr_clone = ret_ptr.clone();
            let log_clone = log.clone();
            pool.execute(move || {
                for item in jobs.into_iter().enumerate() {
                    let a = item.0;
                    println!("{} jb start", a);
                    let pool_log_clone = log_clone.clone();
                    let find_res = extract_str_dyn_mem(item.1, pool_log_clone);
                    if find_res.is_err() {
                        println!("find strings failed: None");
                        return;
                    }
                    let find_res = find_res.unwrap();

                    #[allow(clippy::search_is_some)]
                    let finds_uc: Vec<String> = find_res
                        .unicode
                        .iter()
                        .filter(|x| x.str.find(&pat_clone).is_some())
                        .map(|x| x.str.clone())
                        .collect();
                    #[allow(clippy::search_is_some)]
                    let finds_ascii: Vec<String> = find_res
                        .ascii
                        .iter()
                        .filter(|x| x.str.find(&pat_clone).is_some())
                        .map(|x| x.str.clone())
                        .collect();

                    let finds_addr =
                        unsafe { get_base_addr_all_send::<SendableCvoidPtrMut>(&find_res) };
                    #[cfg(debug_assertions)]
                    {
                        println!(
                            "finds unicode: {}",
                            finds_uc.vec_string(DEFAULT_FORMAT_RULE)
                        );
                        println!(
                            "finds ascii: {}",
                            finds_ascii.vec_string(DEFAULT_FORMAT_RULE)
                        );
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
                            finds_ascii,
                            finds_unicode: finds_uc,
                        };
                    }
                    an_atomic.fetch_add(1, Ordering::Relaxed);
                }
            });

            //println!("Ascii:\t{}", find_res.ascii.vec_string(DEFAULT_FORMAT_RULE));
            /*
            println!(
                "Unicode:\t{}",
                find_res.unicode.vec_string(DEFAULT_FORMAT_RULE)
            );
            */
            vec_cur += len_job;
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
        Ok(ret)
    } else {
        unreachable!();
    }
}

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
        println!("{}", names.vec_string(DEFAULT_FORMAT_RULE));
        let cnt_pids = pids_vec.len();
        let pool = ThreadPool::new(cnt_pids);
        let an_atomic = Arc::new(AtomicUsize::new(0));
        let avb_p = std::thread::available_parallelism().unwrap_or(NonZero::new(8).unwrap());
        let cnt_job = cmp::max(cnt_pids / avb_p, 1);
        let mut vec_cur: usize = 0;
        let mut ret: Vec<ScanStrAllResSend<SendableCvoidPtrMut>> = Vec::with_capacity(cnt_pids);
        let ret_raw_ptr = ret.as_mut_ptr();
        let ret_ptr = SendablePtrMut::<ScanStrAllResSend<SendableCvoidPtrMut>>(ret_raw_ptr);

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
                        println!("Ascii:\t{}", find_res.ascii.vec_string(DEFAULT_FORMAT_RULE));
                        println!(
                            "Unicode:\t{}",
                            find_res.unicode.vec_string(DEFAULT_FORMAT_RULE)
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
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
        Ok(ret)
    } else {
        unreachable!();
    }
}

fn main() {
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
            get_childs_dyn_pat_cvoid(pid, "zxc".to_string(), log.clone());
        if let Err(e) = res_dyn_pat {
            println!("[ERROR] {:?}", e);
            wait_close();
            return;
        }
        let res_dyn_pat = res_dyn_pat.unwrap();
        let addr_only_assci: Vec<_> = res_dyn_pat
            .iter()
            .flat_map(|x| x.finds_addr.assci.clone())
            .collect();
        let addr_only_unicode: Vec<_> = res_dyn_pat
            .iter()
            .flat_map(|x| x.finds_addr.unicode.clone())
            .collect();
        //#[cfg(debug_assertions)]
        {
            let fmt_assci_addr: Vec<_> = addr_only_assci
                .iter()
                .map(|x| format!("{:p}", x.0))
                .collect();
            let fmt_unicode_addr: Vec<_> = addr_only_unicode
                .iter()
                .map(|x| format!("{:p}", x.0))
                .collect();
            let str_assci_addr = fmt_assci_addr.vec_string(DEFAULT_FORMAT_RULE);
            let str_unicode_addr = fmt_unicode_addr.vec_string(DEFAULT_FORMAT_RULE);
            println!("ASSCI ADDR:  {}", str_assci_addr);
            println!("UNICODE ADDR:  {}", str_unicode_addr);
        }
    };
    unsafe {
        //let _: Result<Vec<ScanStrAllResSend::<SendableCvoidPtrMut>>, win_core::Error> =
        //get_childs_cvoid(pid);
    }
    wait_close();
    let find_res = find_strings(pid, log);
    if find_res.is_err() {
        println!("find strings failed: None");
        wait_close();
        return;
    }
    let find_res = find_res.unwrap();
    println!("Ascii:\t{}", find_res.ascii.vec_string(DEFAULT_FORMAT_RULE));
    println!(
        "Unicode:\t{}",
        find_res.unicode.vec_string(DEFAULT_FORMAT_RULE)
    );
    wait_close();
}
