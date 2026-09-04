//! Audited sqlite-vec auto-extension shim.
//!
//! Registers the bundled `sqlite-vec` entry point once per process via
//! `sqlite3_auto_extension` so lazily opened connections gain `vec0`.

use std::sync::OnceLock;

static REGISTERED: OnceLock<()> = OnceLock::new();

/// Ensure the vec extension is registered process-wide (idempotent).
pub(crate) fn ensure_registered() {
    REGISTERED.get_or_init(register_once);
}

// SAFETY: `sqlite3_vec_init` is the bundled sqlite-vec static entry point and
// `sqlite3_auto_extension` only stores the pointer to invoke it on each new
// connection; registration happens once behind a process-global `OnceLock`
// and never unregisters, so no use-after-free or data race is introduced.
#[allow(unsafe_code)]
fn register_once() {
    type EntryPoint = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut core::ffi::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> core::ffi::c_int;
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), EntryPoint>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }
}
