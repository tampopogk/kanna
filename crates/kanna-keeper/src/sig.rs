use anyhow::Result;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

static mut WRITE_FD: libc::c_int = -1;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub unsafe fn errno() -> *mut libc::c_int {
    libc::__errno_location()
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub unsafe fn errno() -> *mut libc::c_int {
    libc::__error()
}

pub const STOP_GRACE: libc::c_uint = 5;

extern "C" fn handler(signo: libc::c_int) {
    unsafe {
        if signo == libc::SIGTERM || signo == libc::SIGINT {
            libc::alarm(STOP_GRACE);
        }
        let saved = *errno();
        let fd = WRITE_FD;
        if fd >= 0 {
            let byte = [signo as u8];
            libc::write(fd, byte.as_ptr().cast(), 1);
        }
        *errno() = saved;
    }
}

pub fn set_nonblocking(fd: libc::c_int) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

pub fn set_cloexec(fd: libc::c_int) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
    }
}

pub fn clear_cloexec(fd: libc::c_int) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
    }
}

pub fn open_fds() -> Vec<libc::c_int> {
    let Ok(dir) = std::fs::read_dir("/dev/fd") else { return Vec::new() };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect()
}

pub fn cloexec_inherited() {
    for fd in open_fds().into_iter().filter(|&fd| fd > 2) {
        set_cloexec(fd);
    }
}

pub fn close_inherited(keep: &[libc::c_int]) {
    for fd in open_fds().into_iter().filter(|fd| *fd > 2 && !keep.contains(fd)) {
        unsafe { libc::close(fd) };
    }
}

pub fn pipe(signals: &[libc::c_int]) -> Result<OwnedFd> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    for fd in fds {
        set_nonblocking(fd);
        set_cloexec(fd);
    }
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    unsafe {
        WRITE_FD = write.as_raw_fd();
        std::mem::forget(write);
        for &signo in signals {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signo, &action, std::ptr::null_mut()) < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    Ok(read)
}

pub fn drain(fd: &OwnedFd) -> Vec<libc::c_int> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        out.extend(buf[..n as usize].iter().map(|&b| b as libc::c_int));
    }
    out
}
