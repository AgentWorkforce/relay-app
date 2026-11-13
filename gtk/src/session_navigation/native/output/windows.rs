//! ChildStdout has a single reader; a positive peek bounds a subsequent read
//! without waiting for a descendant to close its inherited write handle.
use std::{io, os::windows::io::AsRawHandle, process::ChildStdout};
use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};
pub(super) fn available(stdout: &ChildStdout) -> io::Result<Option<usize>> {
    let mut count = 0;
    // SAFETY: stdout keeps the pipe open, count is a live u32 output, all
    // optional buffer outputs are null. No read or pointer escapes this call.
    let success = unsafe {
        PeekNamedPipe(
            stdout.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut count,
            std::ptr::null_mut(),
        )
    };
    if success != 0 {
        return Ok(Some(count as usize));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
        Ok(None)
    } else {
        Err(error)
    }
}
