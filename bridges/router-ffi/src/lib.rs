//! Contains the only C-pointer ownership boundary; engines and search remain safe Rust.
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

#[unsafe(no_mangle)]
pub extern "C" fn mmr_protocol_version() -> i32 {
    mmrouter_service::PROTOCOL_VERSION
}

/// Executes a version-1 request; the returned UTF-8 JSON string must be freed exactly once.
/// # Safety
/// `input` must point to a valid NUL-terminated UTF-8 string for this call's duration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mmr_request(handle: i64, input: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if input.is_null() {
            return serde_json::json!({"ok":false,"error":"null request"}).to_string();
        }
        // SAFETY: Caller promises a live, terminated buffer; it is copied/parsed during this call.
        match unsafe { CStr::from_ptr(input) }.to_str() {
            Ok(value) => mmrouter_service::request(handle, value),
            Err(_) => serde_json::json!({"ok":false,"error":"request must be UTF-8"}).to_string(),
        }
    }))
    .unwrap_or_else(|_| {
        serde_json::json!({"ok":false,"error":"native router panicked"}).to_string()
    });
    // JSON encodes embedded NULs as escapes, so the owned response can always be a C string.
    CString::new(result)
        .expect("serialized JSON has no NUL")
        .into_raw()
}

/// Releases a response allocated by mmr_request; null is allowed.
/// # Safety
/// `response` must be null or an unfreed pointer returned by this library's mmr_request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mmr_free(response: *mut c_char) {
    if !response.is_null() {
        // SAFETY: Ownership is returned once by the caller, to the same allocator that created it.
        drop(unsafe { CString::from_raw(response) });
    }
}
