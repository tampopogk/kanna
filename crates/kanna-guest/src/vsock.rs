use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub const JOB_PORT: u32 = 7100;

pub const EGRESS_PORT: u32 = 7101;

pub const HOST: u32 = 2;

pub struct Vsock(OwnedFd);

impl Read for Vsock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
    }
}

impl Write for Vsock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.0.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Vsock {
    pub fn try_clone(&self) -> io::Result<Vsock> {
        Ok(Vsock(self.0.try_clone()?))
    }

    pub fn shutdown_write(&self) {
        unsafe { libc::shutdown(self.0.as_raw_fd(), libc::SHUT_WR) };
    }
}

#[cfg(target_os = "macos")]
fn address(cid: u32, port: u32) -> libc::sockaddr_vm {
    let mut address: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    address.svm_len = std::mem::size_of::<libc::sockaddr_vm>() as u8;
    address.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    address.svm_port = port;
    address.svm_cid = cid;
    address
}

#[cfg(target_os = "macos")]
fn socket() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub struct Listener(OwnedFd);

impl Listener {
    #[cfg(target_os = "macos")]
    pub fn bind(port: u32) -> io::Result<Listener> {
        let fd = socket()?;
        let address = address(libc::VMADDR_CID_ANY, port);
        if unsafe { libc::bind(fd.as_raw_fd(), (&address as *const libc::sockaddr_vm).cast(), std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::listen(fd.as_raw_fd(), 16) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Listener(fd))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn bind(_port: u32) -> io::Result<Listener> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "vsock is for a macOS guest"))
    }

    pub fn accept(&self) -> io::Result<Vsock> {
        let fd = unsafe { libc::accept(self.0.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Vsock(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
}

#[cfg(target_os = "macos")]
pub fn connect(cid: u32, port: u32) -> io::Result<Vsock> {
    let fd = socket()?;
    let address = address(cid, port);
    if unsafe { libc::connect(fd.as_raw_fd(), (&address as *const libc::sockaddr_vm).cast(), std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Vsock(fd))
}

#[cfg(not(target_os = "macos"))]
pub fn connect(_cid: u32, _port: u32) -> io::Result<Vsock> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "vsock is for a macOS guest"))
}

pub fn relay_egress(local: u16, port: u32) -> io::Result<()> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", local))?;
    std::thread::Builder::new().name("kanna-guest egress".into()).spawn(move || {
        for tcp in listener.incoming().flatten() {
            let _ = std::thread::Builder::new().name("kanna-guest egress conn".into()).spawn(move || {
                let Ok(host) = connect(HOST, port) else { return };
                let (Ok(mut tcp_read), Ok(mut host_read)) = (tcp.try_clone(), host.try_clone()) else { return };
                let mut host_write = host;
                let mut tcp_write = tcp;
                let up = std::thread::spawn(move || {
                    let _ = io::copy(&mut tcp_read, &mut host_write);
                    host_write.shutdown_write();
                });
                let _ = io::copy(&mut host_read, &mut tcp_write);
                let _ = tcp_write.shutdown(std::net::Shutdown::Write);
                let _ = up.join();
            });
        }
    })?;
    Ok(())
}
