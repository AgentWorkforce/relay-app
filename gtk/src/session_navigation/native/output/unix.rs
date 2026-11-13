//! The sole read descriptor is nonblocking before any read can run.
use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd},
    process::ChildStdout,
};
pub(super) fn nonblocking(stdout: &ChildStdout) -> io::Result<()> {
    let descriptor = stdout.as_fd();
    let flags = flags(descriptor)?;
    set_flags(descriptor, flags | libc::O_NONBLOCK)
}

fn flags(descriptor: BorrowedFd<'_>) -> io::Result<i32> {
    // SAFETY: BorrowedFd keeps this descriptor live during fcntl; F_GETFL
    // reads descriptor flags and does not access Rust memory.
    let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(flags)
    }
}

fn set_flags(descriptor: BorrowedFd<'_>, flags: i32) -> io::Result<()> {
    // SAFETY: BorrowedFd keeps this descriptor live during fcntl; F_SETFL
    // takes only integer flags and does not access Rust memory.
    let result = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_SETFL, flags) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
