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

pub fn vec_flat2_borrow<T>(vec2: &Vec<Vec<T>>) -> Vec<T>
where
    T: Clone,
{
    vec2.iter().flat_map(|x| x).map(|x| x.clone()).collect()
}

pub fn vec_flat2_owned<T>(vec2: Vec<Vec<T>>) -> Vec<T> {
    vec2.into_iter().flat_map(|x| x).collect()
}

pub fn vec_flat2_owned_xz<T>(vec2: Vec<&Vec<T>>) -> Vec<T>
where
    T: Clone,
{
    vec2.into_iter()
        .flat_map(|x| x)
        .map(|x| x.clone())
        .collect()
}

#[must_use]
pub fn jobs_disp<T>(jobs_vec_len: usize, work_vec: Vec<T>) -> Vec<Vec<T>>
where
    T: Clone,
{
    let mut jobs_vec: Vec<Vec<T>> = Vec::with_capacity(jobs_vec_len);
    let mut cur_job: Vec<T> = Vec::new();
    let mut i = 0;
    for item in work_vec.iter() {
        if i >= jobs_vec_len {
            jobs_vec.push(cur_job.clone());
            i = 0;
            cur_job.clear();
        }
        cur_job.push(item.clone());
        i += 1;
    }
    jobs_vec
}
