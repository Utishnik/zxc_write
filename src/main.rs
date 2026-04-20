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
}
