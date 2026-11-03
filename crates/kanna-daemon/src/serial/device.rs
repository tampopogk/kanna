use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const GATHER: Duration = Duration::from_millis(8);
const CHUNK: usize = 64 * 1024;
pub const IDLE: Duration = Duration::from_millis(1000);

pub enum Read {
    Bytes(Vec<u8>),
    Idle,
    Gone(String),
}

pub struct Device {
    fd: Arc<OwnedFd>,
    wake: OwnedFd,
    reader: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lines {
    BothClear,
    DtrOnly,
}

impl Device {
    pub fn open(path: &str, baud: u32, lines: Lines, on_read: impl Fn(Read) -> bool + Send + 'static) -> io::Result<Device> {
        let fd = open_port(path, baud)?;
        let _ = set_lines(fd.as_raw_fd(), lines);
        let fd = Arc::new(fd);
        let (wake_read, wake) = pipe()?;
        let reading = fd.clone();
        let reader = std::thread::Builder::new().name("kanna.serial".into()).spawn(move || read_loop(&reading, &wake_read, on_read))?;
        Ok(Device { fd, wake, reader: Some(reader) })
    }

    pub fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn share(&self) -> Arc<OwnedFd> {
        self.fd.clone()
    }

    pub fn set_baud(&self, baud: u32) -> io::Result<()> {
        set_speed(self.raw(), baud)
    }

    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        write_all(self.raw(), data, Duration::from_secs(2))
    }

    pub fn close(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn open_port(path: &str, baud: u32) -> io::Result<OwnedFd> {
    let c = CString::new(path).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path has a NUL in it"))?;
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::ioctl(raw, libc::TIOCEXCL as _) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOTTY) {
            return Err(error);
        }
    }
    let mut tio: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(raw, &mut tio) } != 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { libc::cfmakeraw(&mut tio) };
    tio.c_cflag |= libc::CLOCAL | libc::CREAD | libc::CS8;
    tio.c_cflag &= !(libc::CSTOPB | libc::PARENB | libc::CRTSCTS);
    tio.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
    tio.c_cc[libc::VMIN] = 1;
    tio.c_cc[libc::VTIME] = 0;
    if let Some(speed) = standard(baud) {
        unsafe { libc::cfsetspeed(&mut tio, speed) };
    }
    if unsafe { libc::tcsetattr(raw, libc::TCSANOW, &tio) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if standard(baud).is_none() {
        set_speed(raw, baud)?;
    }
    Ok(fd)
}

fn standard(baud: u32) -> Option<libc::speed_t> {
    Some(match baud {
        50 => libc::B50,
        75 => libc::B75,
        110 => libc::B110,
        134 => libc::B134,
        150 => libc::B150,
        200 => libc::B200,
        300 => libc::B300,
        600 => libc::B600,
        1200 => libc::B1200,
        1800 => libc::B1800,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn set_speed(fd: RawFd, baud: u32) -> io::Result<()> {
    const IOSSIOSPEED: libc::c_ulong = 0x8008_5402;
    let speed: libc::speed_t = baud as libc::speed_t;
    if unsafe { libc::ioctl(fd, IOSSIOSPEED, &speed) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match standard(baud) {
        Some(speed) => set_standard(fd, speed),
        None if error.raw_os_error() == Some(libc::ENOTTY) => Ok(()),
        None => Err(error),
    }
}

#[cfg(not(target_os = "macos"))]
fn set_speed(fd: RawFd, baud: u32) -> io::Result<()> {
    match standard(baud) {
        Some(speed) => set_standard(fd, speed),
        None => Err(io::Error::new(io::ErrorKind::Unsupported, format!("{baud} isn't a standard rate"))),
    }
}

fn set_standard(fd: RawFd, speed: libc::speed_t) -> io::Result<()> {
    let mut tio: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut tio) } != 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { libc::cfsetspeed(&mut tio, speed) };
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &tio) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn modem(fd: RawFd, request: libc::c_ulong, bits: libc::c_int) -> io::Result<()> {
    if unsafe { libc::ioctl(fd, request as _, &bits) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn set_lines(fd: RawFd, lines: Lines) -> io::Result<()> {
    match lines {
        Lines::BothClear => modem(fd, libc::TIOCMBIC as _, libc::TIOCM_DTR | libc::TIOCM_RTS),
        Lines::DtrOnly => {
            modem(fd, libc::TIOCMBIC as _, libc::TIOCM_RTS)?;
            modem(fd, libc::TIOCMBIS as _, libc::TIOCM_DTR)
        }
    }
}

pub fn pulse_reset(fd: RawFd, hold: Duration, lines: Lines) -> io::Result<()> {
    modem(fd, libc::TIOCMBIC as _, libc::TIOCM_DTR).map_err(|e| if e.raw_os_error() == Some(libc::ENOTTY) { io::Error::new(io::ErrorKind::Unsupported, "the port has no modem lines to reset with") } else { e })?;
    modem(fd, libc::TIOCMBIS as _, libc::TIOCM_RTS)?;
    std::thread::sleep(hold);
    modem(fd, libc::TIOCMBIC as _, libc::TIOCM_RTS)?;
    set_lines(fd, lines)
}

fn write_all(fd: RawFd, mut data: &[u8], within: Duration) -> io::Result<()> {
    let deadline = Instant::now() + within;
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n > 0 {
            data = &data[n as usize..];
            continue;
        }
        let error = io::Error::last_os_error();
        if n < 0 && !matches!(error.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EINTR)) {
            return Err(error);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the device isn't taking the bytes"));
        }
        let mut poll = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
        unsafe { libc::poll(&mut poll, 1, left.as_millis().min(1000) as libc::c_int) };
    }
    Ok(())
}

fn read_loop(fd: &OwnedFd, wake: &OwnedFd, on_read: impl Fn(Read) -> bool) {
    let mut buffer = vec![0u8; CHUNK];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let timeout = if pending.is_empty() { IDLE } else { GATHER };
        let mut polls = [libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        let n = unsafe { libc::poll(polls.as_mut_ptr(), 2, timeout.as_millis() as libc::c_int) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            on_read(Read::Gone(error.to_string()));
            return;
        }
        if polls[1].revents != 0 {
            return;
        }
        if n == 0 {
            let more = if pending.is_empty() { on_read(Read::Idle) } else { on_read(Read::Bytes(std::mem::take(&mut pending))) };
            if !more {
                return;
            }
            continue;
        }
        if polls[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0 {
            continue;
        }
        let got = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        match got {
            n if n > 0 => {
                pending.extend_from_slice(&buffer[..n as usize]);
                if pending.len() >= CHUNK && !on_read(Read::Bytes(std::mem::take(&mut pending))) {
                    return;
                }
            }
            0 => {
                if !pending.is_empty() {
                    on_read(Read::Bytes(std::mem::take(&mut pending)));
                }
                on_read(Read::Gone("the device closed the port".into()));
                return;
            }
            _ => {
                let error = io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EINTR)) {
                    continue;
                }
                if !pending.is_empty() {
                    on_read(Read::Bytes(std::mem::take(&mut pending)));
                }
                on_read(Read::Gone(error.to_string()));
                return;
            }
        }
    }
}
