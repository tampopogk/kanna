use crate::Event;
use std::sync::mpsc::Sender;

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
        eprintln!("kanna-daemon: can't watch its owner (pid {pid}): {error}");
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
