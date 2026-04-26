use core::ffi::c_void;
use vec_string::*;
use zxc_write::find_proccess::*;
use zxc_write::mem::*;
use zxc_write::privilege::enable_privilege_one;

fn wait_close() {
    let mut buffer: String = String::new();
    let _ = std::io::stdin().read_line(&mut buffer);
}

fn extract_str(dwprocessid: u32) {
    let cfg_ascii = StringCfg::default(); // min_len = 4, max_len = None
    let cfg_unicode = StringCfg {
        min_len: 5,
        max_len: Some(25),
    };

    let mut ascii_processor = |buf_ptr: *mut c_void, size: usize, base: *const c_void| unsafe {
        extract_ascii_strings(
            buf_ptr as *const u8,
            size,
            base,
            cfg_ascii.min_len,
            cfg_ascii.max_len,
        )
    };
    let mut unicode_processor = |buf_ptr: *mut c_void, size: usize, base: *const c_void| unsafe {
        extract_unicode_strings(
            buf_ptr as *const u8,
            size,
            base,
            cfg_unicode.min_len,
            cfg_unicode.max_len,
        )
    };

    let mut processors = [ascii_processor, unicode_processor];

    let result: Result<ExtractResult<ExtractStr>, _> = scan_process_strings_lossy(
        dwprocessid,
        &mut processors,
        16,   // start_cap
        None, // начать с NULL
    );

    // Извлечь плоский список всех строк (объединяя ascii+unicode из всех регионов)
    if let Ok(extract_result) = result {
        let all_ascii: Vec<_> = extract_result
            .into_iter()
            .filter_map(|opt| opt)
            .flat_map(|per_proc| per_proc.into_iter().nth(0)) // ascii — первый обработчик
            .flatten()
            .collect();
        let all_unicode: Vec<_>;
    }
}

fn find_strings(dwprocessid: u32) -> Result<ExtractStrResult, ()> {
    let strs = scan_process_strings_lossy(dwprocessid, true);
    if let Err(e) = strs {
        println!("[DEBUG] strs Err: {:?}", e);
        return Err(());
    }
    let strs = strs.unwrap();
    if strs.is_none() {
        println!("[DEBUG] find_strings strs is none");
        return Err(());
    }
    Ok(strs.unwrap())
}

fn get_childs(pid: u32) {
    let childs = get_child_processes(pid);
    if let Err(e) = childs {
        println!("[ERROR] get_childs {:?}", e);
    } else if let Ok(ok) = childs {
        let mut pids_vec: Vec<u32> = Vec::new();
        let names = ok
            .iter()
            .map(|x| {
                pids_vec.push(x.0);
                format!("name exe {}\tpid: {}", x.1.clone(), x.0)
            })
            .collect::<Vec<String>>();
        println!("{}", names.vec_string(DEFAULT_FORMAT_RULE));
        for &item in pids_vec.iter().rev() {
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
        }
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
    get_childs(pid);
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
