use core::marker::PhantomData;
use windows::Win32::Foundation::*;

///# Safety
/// no alias и тд трубования для гарантий
pub const unsafe fn mut_ptr_cast_slice<T>(
    ptr: *mut T,
    len: usize,
    _phantom: PhantomData<&mut T>,
) -> &mut [T] {
    unsafe { std::slice::from_raw_parts_mut(ptr, len) }
}

///# Safety
/// no alias и тд трубования для гарантий
pub const unsafe fn ptr_cast_slice<T>(
    ptr: *mut T,
    len: usize,
    _phantom: PhantomData<&mut T>,
) -> &[T] {
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

#[derive(Debug)]
pub struct HandleGuard(pub HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let close_res = CloseHandle(self.0);
                if close_res.is_err() {
                    println!("[LOG] HandleGuard Drop CloseHandle Err");
                    //todo использовать какие нибудь tiny log и тд
                }
            }
        }
    }
}
