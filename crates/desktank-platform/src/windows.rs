//! Windows platform integration (power and session event sources land in O1-Win).

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::power::PowerSignals;

/// Closes a Win32 handle when dropped.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `OpenProcessToken` and is closed
        // exactly once, here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Frees a `LocalAlloc` allocation (such as a string from
/// `ConvertSidToStringSidW`) when dropped.
struct LocalAllocation(*mut u16);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `ConvertSidToStringSidW`, which
        // allocates with `LocalAlloc`, and is freed exactly once, here.
        unsafe { LocalFree(self.0.cast::<c_void>()) };
    }
}

/// The current user's security identifier (SID) as a string such as
/// `S-1-5-21-…`, read from the current process token.
///
/// The IPC pipe uses it to build a DACL that only the current user can open.
///
/// # Errors
///
/// Returns the OS error if the process token cannot be opened or queried, or
/// the SID cannot be converted to a string.
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = core::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no
    // closing, and `token` is a valid out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);

    // First call: learn the buffer size `TokenUser` needs.
    let mut needed = 0_u32;
    // SAFETY: a null buffer with length zero is the documented way to ask for
    // the required size; `needed` is a valid out pointer.
    let probe = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            core::ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    let error = io::Error::last_os_error();
    let too_small = error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        == Some(ERROR_INSUFFICIENT_BUFFER);
    if probe != 0 || !too_small {
        return Err(error);
    }

    // `u64` elements keep the buffer 8-byte aligned, which `TOKEN_USER` (a
    // struct holding a pointer) needs.
    let words = (needed as usize).div_ceil(size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    // SAFETY: `buffer` holds at least `needed` bytes and is valid for writes;
    // `needed` is a valid out pointer.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &raw mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success the buffer starts with a `TOKEN_USER`, and it is
    // sufficiently aligned (see above). The SID it points to lives inside
    // `buffer`, which outlives every use below.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };

    let mut wide: *mut u16 = core::ptr::null_mut();
    // SAFETY: `user.User.Sid` is a valid SID inside `buffer`; `wide` is a
    // valid out pointer.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut wide) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let wide = LocalAllocation(wide);

    let mut len = 0;
    // SAFETY: on success `wide.0` points at a NUL-terminated UTF-16 string, so
    // every offset up to and including the terminator is readable.
    while unsafe { *wide.0.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: the `len` code units before the terminator are initialised and
    // stay allocated until `wide` is dropped.
    let units = unsafe { core::slice::from_raw_parts(wide.0, len) };
    String::from_utf16(units).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Stub power monitor for Windows; real `RegisterPowerSettingNotification` and
/// `WTSRegisterSessionNotification` wiring lands in `O1-Win` (SPEC §9 O3).
pub(crate) struct PowerMonitor;

impl PowerMonitor {
    pub(crate) fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        drop(on_change);
        Self
    }

    #[allow(clippy::unused_self)] // macOS reads self; keep uniform backend signature
    pub(crate) fn signals(&self) -> PowerSignals {
        PowerSignals::default()
    }
}

#[cfg(test)]
mod tests {
    use super::current_user_sid;

    #[test]
    fn current_user_sid_is_a_sid_string() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
    }
}
