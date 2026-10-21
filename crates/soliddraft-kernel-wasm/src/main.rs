//! The SMLib kernel as a WebAssembly module of its own, built with Emscripten for the
//! web build of the app (docs/web.md).
//!
//! The app is a `wasm32-unknown-unknown` module and the kernel is C++, which needs
//! Emscripten's C++ runtime, so the two are separate modules in one page. The page copies
//! a kernel request, in the `soliddraft-kernel-remote` protocol, into a buffer from
//! [`dw_alloc`], calls [`dw_kernel_call`], and reads the reply: the same JSON the
//! server's `/api/kernel` answers with.
//!
//! Build it with `cargo build -p soliddraft-kernel-wasm --target wasm32-unknown-emscripten`
//! (Emscripten's `em++` on the path; `.cargo/config.toml` has the flags).

use std::cell::RefCell;
use std::ffi::{CString, c_char};

use soliddraft_kernel_remote::Session;
use soliddraft_kernel_smlib::SmlibKernel;

thread_local! {
    static KERNEL: (SmlibKernel, RefCell<Session>) = (SmlibKernel::new(), RefCell::new(Session::new()));
}

/// A buffer of `len` bytes for a request.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn dw_alloc(len: usize) -> *mut u8 {
    let mut buffer = vec![0u8; len].into_boxed_slice();
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Answers the request in `request[..len]`, UTF-8 JSON, and frees its buffer. The reply
/// is NUL-terminated UTF-8 JSON, freed with [`dw_free_reply`].
///
/// # Safety
///
/// `request` must come from [`dw_alloc`] with the same `len`, and not be used again.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dw_kernel_call(request: *mut u8, len: usize) -> *mut c_char {
    // SAFETY: the buffer came from dw_alloc(len) as a boxed slice of exactly len bytes.
    let bytes = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(request, len)) };
    let text = String::from_utf8_lossy(&bytes);
    let reply = KERNEL.with(|(kernel, session)| session.borrow_mut().handle(kernel, &text));
    CString::new(reply).expect("JSON has no NUL bytes").into_raw()
}

/// Frees a reply from [`dw_kernel_call`].
///
/// # Safety
///
/// `reply` must come from [`dw_kernel_call`] and not be freed before.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dw_free_reply(reply: *mut c_char) {
    if !reply.is_null() {
        // SAFETY: the pointer came from CString::into_raw in dw_kernel_call.
        drop(unsafe { CString::from_raw(reply) });
    }
}

fn main() {}
