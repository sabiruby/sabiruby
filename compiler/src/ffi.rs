//! The only `unsafe` of the crate: the functions of `csrc/shim.c`.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};

pub const DEBUG_INFO: c_uint = 1;
pub const REMOVE_LV: c_uint = 2;
pub const NO_EXT_OPS: c_uint = 4;
pub const NO_OPTIMIZE: c_uint = 8;

pub const OK: c_int = 0;
pub const COMPILE_ERROR: c_int = 1;
pub const DUMP_ERROR: c_int = 2;
pub const NO_MEMORY: c_int = 3;

unsafe extern "C" {
    fn sabiruby_mrc_compile(src: *const u8, len: usize, filename: *const c_char, flags: c_uint,
                            out: *mut *mut u8, out_len: *mut usize, diag: *mut *mut c_char) -> c_int;
    fn sabiruby_mrc_compile_eval(src: *const u8, len: usize, filename: *const c_char, line: c_uint,
                                 flags: c_uint, scopes: *const u8, scopes_len: usize, nscopes: c_uint,
                                 out: *mut *mut u8, out_len: *mut usize, diag: *mut *mut c_char) -> c_int;
    fn sabiruby_mrc_highlight(src: *const u8, len: usize, out: *mut u8);
    fn sabiruby_mrc_free(p: *mut c_void);
    fn sabiruby_mrc_version() -> *const c_char;
    #[cfg(feature = "ast")]
    fn sabiruby_mrc_ast(src: *const u8, len: usize, filename: *const c_char) -> *mut c_char;
}

/// Prism's pretty-printed tree, or None when out of memory.
#[cfg(feature = "ast")]
pub fn ast(src: &[u8], filename: &CString) -> Option<String> {
    // SAFETY: `src` and `filename` outlive the call; the result is null or a malloc'ed
    // NUL-terminated string, copied and released with the shim's `free`.
    unsafe {
        let p = sabiruby_mrc_ast(src.as_ptr(), src.len(), filename.as_ptr());
        if p.is_null() { return None; }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        sabiruby_mrc_free(p as *mut c_void);
        Some(s)
    }
}

/// One compilation of an `eval` string, with the enclosing scopes as the blob `csrc/shim.c`
/// documents. Same result triple as [`compile`].
pub fn compile_eval(src: &[u8], filename: &CString, line: u32, flags: c_uint, scopes: &[u8], nscopes: u32) -> (c_int, Vec<u8>, String) {
    let mut out: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;
    let mut diag: *mut c_char = std::ptr::null_mut();
    // SAFETY: as `compile` below; the buffers outlive the call and both results are
    // malloc'ed by the shim and released with its `free`.
    unsafe {
        let code = sabiruby_mrc_compile_eval(src.as_ptr(), src.len(), filename.as_ptr(), line, flags,
                                             scopes.as_ptr(), scopes.len(), nscopes,
                                             &mut out, &mut out_len, &mut diag);
        let bin = if out.is_null() { Vec::new() } else { std::slice::from_raw_parts(out, out_len).to_vec() };
        if !out.is_null() { sabiruby_mrc_free(out as *mut c_void); }
        let text = if diag.is_null() { String::new() } else { CStr::from_ptr(diag).to_string_lossy().into_owned() };
        if !diag.is_null() { sabiruby_mrc_free(diag as *mut c_void); }
        (code, bin, text)
    }
}

/// One compilation: the shim's result code, the RITE binary (empty unless `OK`) and the
/// raw diagnostics text (records `\x1e`, fields `\x1f`).
pub fn compile(src: &[u8], filename: &CString, flags: c_uint) -> (c_int, Vec<u8>, String) {
    let mut out: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;
    let mut diag: *mut c_char = std::ptr::null_mut();
    // SAFETY: `src` and `filename` outlive the call; the shim copies what it keeps. On
    // return `out` is null or a malloc'ed block of `out_len` bytes and `diag` is null or
    // a malloc'ed NUL-terminated string; both are copied, then released with
    // `sabiruby_mrc_free` (the shim's `free`).
    unsafe {
        let code = sabiruby_mrc_compile(src.as_ptr(), src.len(), filename.as_ptr(), flags, &mut out, &mut out_len, &mut diag);
        let bin = if out.is_null() { Vec::new() } else { std::slice::from_raw_parts(out, out_len).to_vec() };
        let text = if diag.is_null() { String::new() } else { CStr::from_ptr(diag).to_string_lossy().into_owned() };
        sabiruby_mrc_free(out as *mut c_void);
        sabiruby_mrc_free(diag as *mut c_void);
        (code, bin, text)
    }
}

/// One category byte (0..=8) per byte of `src`; see `csrc/shim.c`.
pub fn highlight(src: &[u8]) -> Vec<u8> {
    let mut map = vec![0u8; src.len()];
    // SAFETY: as `compile` above. `src` outlives the call and the shim only reads it; `map`
    // is `src.len()` bytes long, which is the `len` the shim is told to fill, and it is
    // written before it is read back here. Nothing is allocated for the caller to release.
    unsafe { sabiruby_mrc_highlight(src.as_ptr(), src.len(), map.as_mut_ptr()) };
    map
}

pub fn version() -> &'static str {
    // SAFETY: the shim returns a pointer to a static NUL-terminated ASCII literal.
    unsafe { CStr::from_ptr(sabiruby_mrc_version()).to_str().unwrap_or("mruby 4.1.0-rc2") }
}
