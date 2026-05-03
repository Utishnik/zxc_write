use core::ffi::c_void;
use std::num::NonZero;
use std::os::raw::c_double;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use threadpool::ThreadPool;
use vec_string::*;
use windows::core as win_core;
use zxc_write::find_proccess::*;
use zxc_write::mem::*;
use zxc_write::privilege::enable_privilege_one;
use zxc_write::utils::SendablePtr;
use zxc_write::utils::*;

fn wait_close() {
    let mut buffer: String = String::new();
    let _ = std::io::stdin().read_line(&mut buffer);
}

fn extract_str(dwprocessid: u32) -> Result<ExtractStrResult, ()> {
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

fn extract_str_dyn_mem(dwprocessid: u32) -> Result<ExtractStrResult, ()> {
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
    let result = scan_process_processors_mbi(dwprocessid, &mut processors, 50000, scan_res);
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

fn find_strings(dwprocessid: u32) -> Result<ExtractStrResult, ()> {
    let strs = extract_str(dwprocessid);
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

unsafe fn get_childs_dyn_pat(
    pid: u32,
    pat: String,
) -> Result<Vec<ScanStrAllResSend<SendableCvoidPtrMut>>, win_core::Error> {
    let childs = get_child_processes(pid);
    if let Err(e) = childs {
        println!("[ERROR] get_childs {:?}", e);
        return Err(e);
    } else if let Ok(ok) = childs {
        let mut pids_vec: Vec<u32> = Vec::new();
        let cnt_pids = pids_vec.len();
        pids_vec.push(pid);
        let names = ok
            .iter()
            .map(|x| {
                pids_vec.push(x.0);
                format!("name exe {}\tpid: {}", x.1.clone(), x.0)
            })
            .collect::<Vec<String>>();
        let mut ret: Vec<ScanStrAllResSend<SendableCvoidPtrMut>> = Vec::with_capacity(cnt_pids);
        let ret_raw_ptr = ret.as_mut_ptr();
        let ret_ptr = SendablePtrMut::<ScanStrAllResSend<SendableCvoidPtrMut>>(ret_raw_ptr);

        println!("{}", names.vec_string(DEFAULT_FORMAT_RULE));
        let pool = ThreadPool::new(cnt_pids);
        let an_atomic = Arc::new(AtomicUsize::new(0));
        let avb_p = std::thread::available_parallelism().unwrap_or(NonZero::new(8).unwrap());
        let cnt_job = cnt_pids / avb_p;
        let jobs_vec = jobs_disp(cnt_job, pids_vec);
        let vec_cur: usize = 0;
        for jobs in jobs_vec.into_iter() {
            let an_atomic = an_atomic.clone();
            let vec_cur_copy = vec_cur.clone();
            let pat_clone = pat.clone();
            let ret_ptr_clone = ret_ptr.clone();
            pool.execute(move || {
                for item in jobs {
                    let find_res = extract_str_dyn_mem(item);
                    if find_res.is_err() {
                        println!("find strings failed: None");
                        wait_close();
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
                        let inner = ret_ptr.0;
                        (*inner).finds_addr = finds_addr;

                        /*
                        r.push(ScanStrAllResSend {
                            ssr: ScanStrRes {
                                finds_ascii,
                                finds_unicode: finds_uc,
                            },
                            finds_addr,
                        });*/
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
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
        Ok(ret)
    } else {
        unreachable!();
    }
}

fn get_childs(pid: u32) {
    let childs = get_child_processes(pid);
    if let Err(e) = childs {
        println!("[ERROR] get_childs {:?}", e);
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
        let cnt_job = cnt_pids / avb_p;
        let jobs_vec = jobs_disp(cnt_job, pids_vec);
        for jobs in jobs_vec.into_iter() {
            let an_atomic = an_atomic.clone();
            pool.execute(move || {
                for item in jobs {
                    let find_res = find_strings(item);
                    if find_res.is_err() {
                        println!("find strings failed: None");
                        wait_close();
                        return;
                    }
                    let find_res = find_res.unwrap();
                    #[cfg(debug_assertions)]
                    {
                        println!("Ascii:\t{}", find_res.ascii.vec_string(DEFAULT_FORMAT_RULE));
                        println!(
                            "Unicode:\t{}",
                            find_res.unicode.vec_string(DEFAULT_FORMAT_RULE)
                        );
                    }
                    an_atomic.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        while let load = an_atomic.load(Ordering::Relaxed)
            && load != cnt_pids
        {}
    } else {
        unreachable!();
    }
}

fn main() {
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
    unsafe { get_childs_dyn_pat(pid, "zxc".to_string()) };
    return; //
    let find_res = find_strings(pid);
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
