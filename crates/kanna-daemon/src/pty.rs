use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

pub struct Spec {
    pub id: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, Option<String>)>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Spawned {
    pub pid: i32,
    pub master: OwnedFd,
}

pub struct Adopted {
    pub id: String,
    pub pid: i32,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub master: OwnedFd,
    pub exit: Option<i32>,
}

pub enum Heard {
    Exited { keeper: String, pid: i32, code: i32 },
    Spawned(u64, io::Result<Spawned>),
}

pub trait Spawner {
    fn spawn(&self, spec: &Spec) -> io::Result<u64>;
    fn close(&self, id: &str);
    fn acknowledge_exit(&self, id: &str);
    fn adopt(&self) -> io::Result<Vec<Adopted>> {
        Ok(Vec::new())
    }
}

pub fn set_size(master: &OwnedFd, cols: u16, rows: u16, width: u16, height: u16) {
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: width, ws_ypixel: height };
    unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
}

pub fn size(master: &OwnedFd) -> Option<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCGWINSZ as _, &mut size) } == 0;
    (ok && size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
}

pub fn redraw(master: &OwnedFd) {
    let group = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    if group > 0 {
        unsafe { libc::killpg(group, libc::SIGWINCH) };
    }
}
