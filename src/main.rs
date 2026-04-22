use vec_string::*;
use zxc_write::find_proccess::*;
use zxc_write::mem::*;

fn find_strings(dwprocessid: u32) -> Result<ExtractResult, ()> {
    let strs = scan_process_strings(dwprocessid);
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

fn main() {
    //find_strings();
    let fnd_name = find_process_by_name("firefox.exe");
    if let Err(e) = fnd_name {
        println!("Error: {:?}", e);
        return;
    }
    let pid = fnd_name.unwrap();
    println!("[DEBUG] pid: {}", pid);
    let find_res = find_strings(pid);
    if find_res.is_err() {
        println!("find strings failed: None");
        return;
    }
    let find_res = find_res.unwrap();
    println!("Ascii:\t{}", find_res.ascii.vec_string(DEFAULT_FORMAT_RULE));
    println!(
        "Unicode:\t{}",
        find_res.unicode.vec_string(DEFAULT_FORMAT_RULE)
    );
}
