//! The system log, as Search writes to it with `Logger` (docs/PLAN.md §5.3): subsystem
//! `com.timwuhaotian.tap`, a category per concern, and each value marked public or
//! private, so `log show` names what happened but never, unless asked, the site.
//!
//! `os_log` is a C macro; underneath it is `_os_log_impl`, given the image's header, the
//! log, a constant format string inside the image, and the arguments packed in `os_log`'s
//! own buffer format. That is done here directly.
//!
//! ```text
//! /usr/bin/log show --last 1h --predicate 'subsystem == "com.timwuhaotian.tap"'
//! ```

use std::{
    ffi::{CString, c_char, c_void},
    sync::OnceLock,
};

use tap_core::identity;

unsafe extern "C" {
    /// This image's Mach-O header: `os_log` finds its format strings relative to it.
    static __dso_handle: u8;
    fn os_log_create(subsystem: *const c_char, category: *const c_char) -> *mut c_void;
    fn _os_log_impl(
        dso: *const c_void,
        log: *mut c_void,
        kind: u8,
        format: *const c_char,
        buffer: *const u8,
        size: u32,
    );
}

/// `OS_LOG_TYPE_ERROR`, as Search's `pageLog.error`.
const ERROR: u8 = 0x10;
/// A string argument, public or private: `os_log`'s argument descriptor.
const STRING_PUBLIC: u8 = 0x22;
const STRING_PRIVATE: u8 = 0x21;
/// The buffer's header: it holds private values and values that aren't plain numbers.
const HAS_PRIVATE: u8 = 0x01;
const HAS_NON_SCALAR: u8 = 0x02;

/// The format string, a constant inside the image as `os_log` requires.
static PAGE_ENDED: &[u8] = b"A page's process ended (%{public}s) on %{private}s: %{public}s\0";

struct Log(*mut c_void);
// SAFETY: an os_log handle is a process-wide, thread-safe object.
unsafe impl Send for Log {}
// SAFETY: as above.
unsafe impl Sync for Log {}

fn pages() -> *mut c_void {
    static PAGES: OnceLock<Log> = OnceLock::new();
    PAGES
        .get_or_init(|| {
            let subsystem = CString::new(identity::LOG_SUBSYSTEM).unwrap_or_default();
            let category = CString::new("Pages").unwrap_or_default();
            // SAFETY: two valid C strings; the handle lives for the process.
            Log(unsafe { os_log_create(subsystem.as_ptr(), category.as_ptr()) })
        })
        .0
}

/// "A page's process ended (crash) on <private>: loaded again" (Search #849): the reason
/// and what was done are public; the site is not.
pub fn page_ended(reason: &str, site: &str, outcome: &str) {
    let values = [
        (STRING_PUBLIC, reason),
        (STRING_PRIVATE, site),
        (STRING_PUBLIC, outcome),
    ];
    let strings: Vec<CString> = values
        .iter()
        .map(|(_, text)| CString::new(*text).unwrap_or_default())
        .collect();
    let mut buffer = vec![
        HAS_PRIVATE | HAS_NON_SCALAR,
        u8::try_from(values.len()).unwrap_or(0),
    ];
    for ((descriptor, _), string) in values.iter().zip(&strings) {
        buffer.push(*descriptor);
        buffer.push(8);
        buffer.extend_from_slice(&(string.as_ptr() as u64).to_ne_bytes());
    }
    // SAFETY: the image header, a live log handle, a NUL-terminated constant format in the
    // image, and a buffer whose pointers outlive the call (`strings` is still alive).
    unsafe {
        _os_log_impl(
            (&raw const __dso_handle).cast(),
            pages(),
            ERROR,
            PAGE_ENDED.as_ptr().cast(),
            buffer.as_ptr(),
            u32::try_from(buffer.len()).unwrap_or(0),
        );
    }
    drop(strings);
}
