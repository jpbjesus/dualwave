use std::{ffi::c_void, io, ptr::NonNull};

extern "C" {
    fn dw_pw_new() -> *mut c_void;
    fn dw_pw_free(bridge: *mut c_void);
    fn dw_pw_state(bridge: *mut c_void, capture: i32) -> i32;
    fn dw_pw_available(bridge: *mut c_void) -> u32;
    fn dw_pw_read(bridge: *mut c_void, out: *mut f32, frames: u32) -> u32;
    fn dw_pw_write(bridge: *mut c_void, input: *const f32, frames: u32) -> u32;
    fn dw_pw_clear(bridge: *mut c_void, capture: i32);
}

pub struct Bridge(NonNull<c_void>);
impl Bridge {
    pub fn new() -> io::Result<Self> {
        NonNull::new(unsafe { dw_pw_new() }).map(Self)
            .ok_or_else(|| io::Error::other("Cannot connect to PipeWire"))
    }
    pub fn active(&self, capture: bool) -> io::Result<bool> {
        match unsafe { dw_pw_state(self.0.as_ptr(), i32::from(capture)) } {
            -1 | 0 => Err(io::Error::other("PipeWire endpoint disconnected or failed")),
            3 => Ok(true),
            _ => Ok(false),
        }
    }
    pub fn available(&self) -> usize { unsafe { dw_pw_available(self.0.as_ptr()) as usize } }
    pub fn read(&mut self, out: &mut [[f32; 2]]) -> usize {
        unsafe { dw_pw_read(self.0.as_ptr(), out.as_mut_ptr().cast(), out.len() as u32) as usize }
    }
    pub fn write(&mut self, input: &[[f32; 2]]) -> usize {
        unsafe { dw_pw_write(self.0.as_ptr(), input.as_ptr().cast(), input.len() as u32) as usize }
    }
    pub fn clear(&mut self, capture: bool) { unsafe { dw_pw_clear(self.0.as_ptr(), i32::from(capture)); } }
}
impl Drop for Bridge {
    fn drop(&mut self) { unsafe { dw_pw_free(self.0.as_ptr()); } }
}
