use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32, Module32First, Module32Next, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::core::Error;

#[derive(Debug)]
pub struct ModuleInfo {
    pub name: String,
    pub mod_base_addr: *mut u8,
    pub mod_base_size: u32,
    pub path: String,
}
#[derive(Debug)]
pub enum SnapshotErr {
    Err(Error),
    InvalidHandleValue,
}

const DEFAULT_CAP_MODULES_INFO: usize = 16;
pub fn list_modules(pid: u32) -> Result<Vec<ModuleInfo>, SnapshotErr> {
    let mut ret: Vec<ModuleInfo> = Vec::with_capacity(DEFAULT_CAP_MODULES_INFO);
    unsafe {
        let flags = TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32;
        let snapshot = CreateToolhelp32Snapshot(flags, pid);
        if let Err(e) = snapshot {
            return Err(SnapshotErr::Err(e));
        }
        let snapshot = snapshot.unwrap_unchecked();

        if snapshot == INVALID_HANDLE_VALUE {
            println!("Failed to snapshot modules for PID {}", pid);
            return Err(SnapshotErr::InvalidHandleValue);
        }

        let mut entry = MODULEENTRY32 {
            dwSize: std::mem::size_of::<MODULEENTRY32>() as u32,
            ..MODULEENTRY32::default()
        };

        if Module32First(snapshot, &mut entry).is_ok() {
            loop {
                let name = std::ffi::CStr::from_ptr(entry.szModule.as_ptr())
                    .to_string_lossy()
                    .into_owned();

                let path = std::ffi::CStr::from_ptr(entry.szExePath.as_ptr())
                    .to_string_lossy()
                    .into_owned();

                println!(
                    "Module: {:<20} | Base: {:p} | Size: {:>8} | {}",
                    name.trim_end_matches('\0'),
                    entry.modBaseAddr,
                    entry.modBaseSize,
                    path.trim_end_matches('\0')
                );

                let name_fmt = name.trim_end_matches('\0').to_string();
                let path_fmt = path.trim_end_matches('\0').to_string();

                let module_info: ModuleInfo = ModuleInfo {
                    name: name_fmt,
                    mod_base_addr: entry.modBaseAddr,
                    mod_base_size: entry.modBaseSize,
                    path: path_fmt,
                };

                ret.push(module_info);

                if Module32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _: windows::core::Result<()> = CloseHandle(snapshot);
    }
    Ok(ret)
}
