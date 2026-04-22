use core::marker::PhantomData;

///# Safety
/// no alias и тд трубования для гарантий
pub unsafe fn mut_ptr_cast_slice<T>(
    ptr: *mut T,
    len: usize,
    _phantom: PhantomData<&mut T>,
) -> &mut [T] {
    unsafe { std::slice::from_raw_parts_mut(ptr, len) }
}

///# Safety
/// no alias и тд трубования для гарантий
pub unsafe fn ptr_cast_slice<T>(ptr: *mut T, len: usize, _phantom: PhantomData<&mut T>) -> &[T] {
    unsafe { std::slice::from_raw_parts(ptr, len) }
}
