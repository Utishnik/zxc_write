use core::ffi::c_void;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use threadpool::ThreadPool;
use vec_string::*;
use zxc_write::find_proccess::*;
use zxc_write::mem::*;
use zxc_write::privilege::enable_privilege_one;
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

fn get_childs_dyn(pid: u32) {
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
                    let find_res = extract_str_dyn_mem(item);
                    if find_res.is_err() {
                        println!("find strings failed: None");
                        wait_close();
                        return;
                    }
                    let find_res = find_res.unwrap();

                    let finds_uc: Vec<String> = find_res
                        .unicode
                        .iter()
                        .filter(|x| x.str.find("Багровели").is_some())
                        .map(|x| x.str.clone())
                        .collect();
                    let finds_ascii: Vec<String> = find_res
                        .ascii
                        .iter()
                        .filter(|x| x.str.find("Багровели").is_some())
                        .map(|x| x.str.clone())
                        .collect();

                    println!(
                        "finds unicode vk: {}",
                        finds_uc.vec_string(DEFAULT_FORMAT_RULE)
                    );
                    println!(
                        "finds ascii vk: {}",
                        finds_ascii.vec_string(DEFAULT_FORMAT_RULE)
                    );
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
                    println!("Ascii:\t{}", find_res.ascii.vec_string(DEFAULT_FORMAT_RULE));
                    println!(
                        "Unicode:\t{}",
                        find_res.unicode.vec_string(DEFAULT_FORMAT_RULE)
                    );
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
    get_childs_dyn(pid);
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
