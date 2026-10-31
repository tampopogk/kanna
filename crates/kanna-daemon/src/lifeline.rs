use crate::Event;
use std::sync::mpsc::Sender;
use std::time::Duration;

pub const OWNER: &str = "KANNA_E2E_OWNER";

pub fn owner() -> Option<i32> {
    std::env::var(OWNER).ok()?.trim().parse().ok().filter(|pid| *pid > 0)
}

pub fn gone(pid: i32) -> bool {
    (unsafe { libc::kill(pid, 0) }) != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

pub fn watch(events: Sender<Event>) -> Option<i32> {
    let pid = owner()?;
    let spawned = std::thread::Builder::new().name("kanna.lifeline".into()).spawn(move || {
        wait_for_exit(pid);
        let _ = events.send(Event::Shutdown(format!("its owner (pid {pid}) exited")));
    });
    if let Err(error) = spawned {
        log!("kanna-daemon: can't watch its owner (pid {pid}): {error}");
        return None;
    }
    Some(pid)
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn wait_for_exit(pid: i32) {
    unsafe {
        let queue = libc::kqueue();
        if queue < 0 {
            return poll_for_exit(pid);
        }
        let mut change: libc::kevent = std::mem::zeroed();
        change.ident = pid as libc::uintptr_t;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD | libc::EV_ONESHOT;
        change.fflags = libc::NOTE_EXIT;
        let mut event: libc::kevent = std::mem::zeroed();
        loop {
            let n = libc::kevent(queue, &change, 1, &mut event, 1, std::ptr::null());
            if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }
        libc::close(queue);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
fn wait_for_exit(pid: i32) {
    poll_for_exit(pid)
}

fn poll_for_exit(pid: i32) {
    while !gone(pid) {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(target_os = "macos")]
pub fn age() -> Option<Duration> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read = unsafe { libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    if read != size {
        return None;
    }
    let started = std::time::UNIX_EPOCH + Duration::new(info.pbi_start_tvsec, info.pbi_start_tvusec as u32 * 1000);
    started.elapsed().ok()
}

#[cfg(target_os = "linux")]
pub fn age() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let ticks: u64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let hz = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok().filter(|hz| *hz > 0)?;
    let mut now: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } != 0 {
        return None;
    }
    let since_boot = Duration::new(now.tv_sec as u64, now.tv_nsec as u32);
    since_boot.checked_sub(Duration::from_millis(ticks * 1000 / hz))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn age() -> Option<Duration> {
    None
}
