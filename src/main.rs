use zxc_write::find_proccess::*;
use zxc_write::mem::*;

fn find_strings(dwprocessid: u32) {
    let strs = scan_process_strings(dwprocessid);
    if strs.is_none() {
        println!("find strings failed: None");
        return;
    }
    let strs = strs.unwrap();
    println!("{:?}", strs);
}

fn main() {
    //find_strings();
    let fnd_name = find_process_by_name("Firefox");
    if let Err(e) = fnd_name {
        println!("Error: {:?}", e);
        return;
    }
    if let Ok(pid) = fnd_name {
        println!("Pid: {}", pid);
    }
}
