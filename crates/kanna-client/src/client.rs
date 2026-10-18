use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

#[cfg(target_vendor = "apple")]
fn no_sigpipe(stream: &UnixStream) {
    let on: libc::c_int = 1;
    unsafe {
        libc::setsockopt(std::os::fd::AsRawFd::as_raw_fd(stream), libc::SOL_SOCKET, libc::SO_NOSIGPIPE, &on as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as libc::socklen_t);
    }
}

#[cfg(not(target_vendor = "apple"))]
fn no_sigpipe(_: &UnixStream) {}

pub struct Kmux {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

#[derive(Debug)]
pub enum Error {
    Unreachable(String),
    Mux { code: String, message: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brand {
    pub id: &'static str,
    pub name: &'static str,
    pub bundle_id: &'static str,
}

impl Brand {
    pub const KMUX: Brand = Brand { id: "kmux", name: "kmux", bundle_id: "dev.kanna.kmux" };

    pub fn variable(&self, name: &str) -> String {
        format!("{}_{name}", self.id.to_ascii_uppercase())
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(self.variable(name)).ok().filter(|v| !v.is_empty())
    }

    pub fn socket_dir(&self) -> PathBuf {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support").join(self.id)
    }

    pub fn instance_socket(&self, name: &str) -> PathBuf {
        let id = self.id;
        self.socket_dir().join(if name == DEFAULT_INSTANCE { format!("{id}.sock") } else { format!("{id}-{name}.sock") })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub brand: Brand,
    pub instance: String,
    pub socket: PathBuf,
    pub background: bool,
}

pub const DEFAULT_INSTANCE: &str = "default";

pub fn socket_dir() -> PathBuf {
    Brand::KMUX.socket_dir()
}

pub fn instance_socket(name: &str) -> PathBuf {
    Brand::KMUX.instance_socket(name)
}

pub fn valid_instance(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Target {
    pub fn from_env() -> Target {
        Target::from_env_for(Brand::KMUX)
    }

    pub fn from_env_for(brand: Brand) -> Target {
        let instance = brand.env("INSTANCE").unwrap_or_else(|| DEFAULT_INSTANCE.into());
        let socket = brand.env("SOCKET").map(PathBuf::from);
        let background = brand.env("BG").is_some_and(|v| v == "1");
        Target { socket: socket.unwrap_or_else(|| brand.instance_socket(&instance)), instance, background, brand }
    }

    pub fn named(name: &str) -> Target {
        Target::named_for(Brand::KMUX, name)
    }

    pub fn named_for(brand: Brand, name: &str) -> Target {
        let background = brand.env("BG").is_some_and(|v| v == "1");
        Target { brand, instance: name.into(), socket: brand.instance_socket(name), background }
    }

    pub fn running() -> Vec<Target> {
        Target::running_for(Brand::KMUX)
    }

    pub fn running_for(brand: Brand) -> Vec<Target> {
        let (sock, prefix) = (format!("{}.sock", brand.id), format!("{}-", brand.id));
        let mut names: Vec<String> = std::fs::read_dir(brand.socket_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name().into_string().ok()?;
                if file == sock {
                    return Some(DEFAULT_INSTANCE.to_string());
                }
                Some(file.strip_prefix(&prefix)?.strip_suffix(".sock")?.to_string())
            })
            .collect();
        names.sort_by_key(|n| (n != DEFAULT_INSTANCE, n.clone()));
        names.into_iter().map(|n| Target::named_for(brand, &n)).filter(|t| UnixStream::connect(&t.socket).is_ok()).collect()
    }
}

pub fn socket_path() -> PathBuf {
    Target::from_env().socket
}

impl Kmux {
    pub fn connect() -> Result<Kmux, Error> {
        Kmux::connect_to(&Target::from_env())
    }

    pub fn connect_for(brand: Brand) -> Result<Kmux, Error> {
        Kmux::connect_to(&Target::from_env_for(brand))
    }

    pub fn connect_to(target: &Target) -> Result<Kmux, Error> {
        if let Some(mux) = Kmux::try_connect(target) {
            return Ok(mux);
        }
        launch(target)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match UnixStream::connect(&target.socket) {
                Ok(stream) => return Kmux::from_stream(stream),
                Err(err) if Instant::now() >= deadline => {
                    return Err(Error::Unreachable(format!("{} did not start ({}: {err})", target.brand.name, target.socket.display())))
                }
                Err(_) => sleep(Duration::from_millis(100)),
            }
        }
    }

    pub fn try_connect(target: &Target) -> Option<Kmux> {
        UnixStream::connect(&target.socket).ok().and_then(|stream| Kmux::from_stream(stream).ok())
    }

    fn from_stream(stream: UnixStream) -> Result<Kmux, Error> {
        no_sigpipe(&stream);
        let writer = stream.try_clone().map_err(|e| Error::Unreachable(e.to_string()))?;
        Ok(Kmux { reader: BufReader::new(stream), writer, next_id: 1 })
    }

    pub fn call(&mut self, cmd: &str, args: Value) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({ "id": id, "cmd": cmd, "args": args }).to_string();
        line.push('\n');
        let lost = |e: std::io::Error| Error::Unreachable(format!("lost connection to kmux: {e}"));
        self.writer.write_all(line.as_bytes()).map_err(lost)?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply).map_err(lost)? == 0 {
            return Err(Error::Unreachable("kmux closed the connection".into()));
        }
        let mut reply: Value =
            serde_json::from_str(&reply).map_err(|e| Error::Unreachable(format!("bad reply from kmux: {e}")))?;
        if reply["ok"] != json!(true) {
            let error = &reply["error"];
            return Err(Error::Mux {
                code: error["code"].as_str().unwrap_or("bad_request").into(),
                message: error["message"].as_str().unwrap_or("unknown error").into(),
            });
        }
        if let Some(fields) = reply.as_object_mut() {
            fields.remove("id");
            fields.remove("ok");
        }
        Ok(reply)
    }
}

fn launch(target: &Target) -> Result<(), Error> {
    let brand = target.brand;
    let mut open = Command::new("/usr/bin/open");
    open.arg("-n");
    if target.background {
        open.arg("-g");
    }
    if target.socket != brand.instance_socket(&target.instance) {
        open.arg("--env").arg(format!("{}={}", brand.variable("SOCKET"), target.socket.display()));
    }
    match brand.env("APP") {
        Some(app) => open.arg("-a").arg(app),
        None => open.arg("-b").arg(brand.bundle_id),
    };
    open.args(["--args", "--instance", &target.instance]);
    if target.background {
        open.arg("--bg");
    }
    if brand.env("FRESH").as_deref() == Some("1") {
        open.arg("--fresh");
    }
    let (name, app) = (brand.name, brand.variable("APP"));
    let status = open.status().map_err(|e| Error::Unreachable(format!("could not start {name}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Unreachable(format!("{name} isn't running and couldn't be started (is {name}.app installed? set {app})")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brands_have_their_own_sockets_and_variables() {
        let kanna = Brand { id: "kanna", name: "Kanna", bundle_id: "dev.kanna.kanna" };
        assert!(Brand::KMUX.instance_socket(DEFAULT_INSTANCE).ends_with("Application Support/kmux/kmux.sock"));
        assert!(kanna.instance_socket(DEFAULT_INSTANCE).ends_with("Application Support/kanna/kanna.sock"));
        assert!(kanna.instance_socket("work").ends_with("Application Support/kanna/kanna-work.sock"));
        assert_eq!(kanna.variable("SOCKET"), "KANNA_SOCKET");
        assert_eq!(Brand::KMUX.variable("PANE"), "KMUX_PANE");
    }
}
