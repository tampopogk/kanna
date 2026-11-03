use crate::output;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CLAUDE_TOKEN: &str = "sk-ant-oat01-";

pub fn run(request: &Value, call: &mut dyn FnMut(&str, Value) -> Result<Value, Failure>, json_output: bool) -> Result<(), Failure> {
    let agent = request["agent"].as_str().unwrap_or_default();
    match request["action"].as_str() {
        Some("add") => {
            let kind = match (agent, request["api_key"] == true) {
                ("claude", false) => "claude-subscription",
                ("claude", true) => "claude-api-key",
                ("codex", false) => "codex-chatgpt",
                _ => "codex-api-key",
            };
            let (secret, program) = match kind {
                "claude-subscription" => (claude_setup_token()?, None),
                "codex-chatgpt" => codex_login()?,
                _ => (hidden(&format!("Paste your {} API key (it isn't shown): ", if agent == "claude" { "Anthropic" } else { "OpenAI" }))?, None),
            };
            let reply = call("accounts", json!({ "action": "add", "kind": kind, "secret": secret, "label": request["label"], "program": program }))?;
            print(json_output, request, &reply);
            Ok(())
        }
        Some("sign-in") => sign_in(agent, call),
        _ => Ok(()),
    }
}

fn print(json_output: bool, request: &Value, reply: &Value) {
    match json_output {
        true => println!("{reply}"),
        false => println!("{}", output::accounts(request, reply)),
    }
}

fn claude_setup_token() -> Result<String, Failure> {
    println!("Kanna runs Claude's own sign-in for a token that lasts a year. Finish it in your browser.\n");
    let mut seen = Vec::new();
    let status = passthrough(Command::new("claude").arg("setup-token"), &mut |bytes| seen.extend_from_slice(bytes))?;
    let found = token_in(&plain(&seen));
    print!("\x1b[H\x1b[2J\x1b[3J");
    let _ = std::io::stdout().flush();
    if !status {
        return Err(fail(exit::FAILED, "claude setup-token didn't finish: nothing was added"));
    }
    if let Some(token) = found {
        let last: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        if ask(&format!("Kanna read the token Claude printed ({CLAUDE_TOKEN}…{last}). Keep it for your agents? [Y/n] "))? {
            return Ok(token);
        }
    }
    hidden("Paste the token Claude printed (it isn't shown): ")
}

pub fn token_in(text: &str) -> Option<String> {
    let start = text.rfind(CLAUDE_TOKEN)?;
    let mut token = String::new();
    let mut rest = text[start..].chars().peekable();
    while let Some(c) = rest.next() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            token.push(c);
            continue;
        }
        if c == '\r' || c == '\n' {
            let mut breaks = usize::from(c == '\n');
            while rest.peek().is_some_and(|c| *c == '\r' || *c == '\n' || *c == ' ') {
                breaks += usize::from(rest.next() == Some('\n'));
            }
            let next: String = rest.clone().take_while(|c| !c.is_whitespace()).collect();
            if breaks <= 1 && !next.is_empty() && next.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                continue;
            }
        }
        break;
    }
    (token.len() >= CLAUDE_TOKEN.len() + 40).then_some(token)
}

pub fn plain(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if c.is_control() && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn codex_login() -> Result<(String, Option<String>), Failure> {
    let program = on_path("codex").ok_or_else(|| fail(exit::NOT_FOUND, "codex isn't installed here"))?;
    let home = private_home()?;
    println!("Kanna runs Codex's own sign-in in a folder of its own, so your everyday Codex login stays as it is. Finish it in your browser.\n");
    let status = Command::new(&program).args(["-c", "cli_auth_credentials_store=\"file\"", "login"]).env("CODEX_HOME", &home).current_dir(&home).status();
    let login = std::fs::read_to_string(home.join("auth.json"));
    let _ = std::fs::remove_dir_all(&home);
    match (status, login) {
        (Ok(status), Ok(login)) if status.success() => Ok((login, Some(program.to_string_lossy().into_owned()))),
        (Err(error), _) => Err(fail(exit::FAILED, format!("can't run codex login: {error}"))),
        _ => Err(fail(exit::FAILED, "codex login didn't finish: nothing was added")),
    }
}

fn private_home() -> Result<PathBuf, Failure> {
    use std::os::unix::fs::DirBuilderExt;
    let home = std::env::temp_dir().join(format!("kanna-codex-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::DirBuilder::new().mode(0o700).create(&home).map_err(|e| fail(exit::FAILED, format!("can't make a private folder for Codex's sign-in: {e}")))?;
    Ok(home)
}

pub fn on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|dir| dir.join(program)).find(|p| p.is_file()))
}

fn sign_in(agent: &str, call: &mut dyn FnMut(&str, Value) -> Result<Value, Failure>) -> Result<(), Failure> {
    let mut command = match agent {
        "claude" => {
            let mut command = Command::new("claude");
            command.args(["auth", "login"]);
            command
        }
        _ => {
            let mut command = Command::new("codex");
            command.args(["login", "--device-auth"]);
            command
        }
    };
    println!("This machine signs in to {agent} by itself. Kanna opens the sign-in page in your browser, on the Mac you're at.\n");
    let mut seen = Vec::new();
    let mut opened = Vec::<String>::new();
    let ok = passthrough(&mut command, &mut |bytes| {
        seen.extend_from_slice(bytes);
        for url in urls(&plain(&seen)) {
            if sign_in_page(&url) && !opened.contains(&url) {
                opened.push(url.clone());
                if call("app.open_sign_in", json!({ "url": url })).is_err() {
                    print!("\r\n(Kanna.app isn't connected to this machine: open the address above yourself.)\r\n");
                }
            }
        }
    })?;
    match ok {
        true => Ok(println!("\n{agent} is signed in on this machine; its agents use this sign-in when no account is set up in Kanna.")),
        false => Err(fail(exit::FAILED, format!("{agent}'s sign-in didn't finish"))),
    }
}

const SIGN_IN_HOSTS: &[&str] = &["claude.ai", "claude.com", "platform.claude.com", "console.anthropic.com", "auth.openai.com", "chatgpt.com"];

pub fn sign_in_page(url: &str) -> bool {
    let host = url.strip_prefix("https://").and_then(|rest| rest.split(['/', '?', '#']).next()).unwrap_or_default().to_lowercase();
    SIGN_IN_HOSTS.contains(&host.as_str())
}

pub fn urls(text: &str) -> Vec<String> {
    let mut found = vec![];
    for (at, _) in text.match_indices("https://") {
        let url: String = text[at..].chars().take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | ')' | ']')).collect();
        let complete = text[at + url.len()..].starts_with(|c: char| c.is_whitespace() && c != '\r' && c != '\n') || text[at + url.len()..].starts_with('\n') || text[at + url.len()..].starts_with('\r');
        if complete && url.len() > "https://".len() && !found.contains(&url) {
            found.push(url);
        }
    }
    found
}

fn hidden(prompt: &str) -> Result<String, Failure> {
    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    let tty = unsafe { libc::tcgetattr(fd, &mut saved) } == 0;
    if tty {
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
    }
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if tty {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
        println!();
    }
    read.map_err(|e| fail(exit::FAILED, format!("can't read it: {e}")))?;
    let secret = line.trim().to_string();
    match secret.is_empty() {
        true => Err(fail(exit::USAGE, "nothing was pasted: nothing was added")),
        false => Ok(secret),
    }
}

fn ask(question: &str) -> Result<bool, Failure> {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| fail(exit::FAILED, format!("can't read the answer: {e}")))?;
    Ok(!line.trim().to_lowercase().starts_with('n'))
}

struct Raw(Option<libc::termios>);

impl Raw {
    fn on(fd: i32) -> Raw {
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return Raw(None);
        }
        let mut raw = saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
        Raw(Some(saved))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        if let Some(saved) = self.0 {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
        }
    }
}

fn passthrough(command: &mut Command, seen: &mut dyn FnMut(&[u8])) -> Result<bool, Failure> {
    use std::os::unix::process::CommandExt;
    let (mut primary, mut secondary) = (0, 0);
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let sized = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } == 0;
    let size_ptr = if sized { &mut size as *mut libc::winsize } else { std::ptr::null_mut() };
    if unsafe { libc::openpty(&mut primary, &mut secondary, std::ptr::null_mut(), std::ptr::null_mut(), size_ptr) } != 0 {
        return Err(fail(exit::FAILED, format!("can't open a terminal for the sign-in: {}", std::io::Error::last_os_error())));
    }
    let primary = unsafe { OwnedFd::from_raw_fd(primary) };
    let secondary = unsafe { OwnedFd::from_raw_fd(secondary) };
    let held = secondary.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            libc::ioctl(held, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let program = command.get_program().to_string_lossy().into_owned();
    let stdio = |fd: &OwnedFd| fd.try_clone().map(Stdio::from).map_err(|e| fail(exit::FAILED, e.to_string()));
    let mut child = command.stdin(stdio(&secondary)?).stdout(stdio(&secondary)?).stderr(stdio(&secondary)?).spawn().map_err(|e| fail(exit::NOT_FOUND, format!("can't run {program}: {e}")))?;
    drop(secondary);
    let raw = Raw::on(0);
    let mut buffer = [0u8; 4096];
    let mut stdout = std::io::stdout();
    loop {
        let mut fds = [libc::pollfd { fd: primary.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 }];
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, 200) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let n = unsafe { libc::read(primary.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if n <= 0 {
                break;
            }
            let bytes = &buffer[..n as usize];
            let _ = stdout.write_all(bytes);
            let _ = stdout.flush();
            seen(bytes);
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(0, buffer.as_mut_ptr().cast(), buffer.len()) };
            if n > 0 {
                unsafe { libc::write(primary.as_raw_fd(), buffer.as_ptr().cast(), n as usize) };
            }
        }
        if let Ok(Some(_)) = child.try_wait() {
            loop {
                let n = unsafe { libc::read(primary.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
                if n <= 0 {
                    break;
                }
                let _ = stdout.write_all(&buffer[..n as usize]);
                seen(&buffer[..n as usize]);
            }
            break;
        }
    }
    drop(raw);
    let status = child.wait().map_err(|e| fail(exit::FAILED, e.to_string()))?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_claude_prints_is_read_even_where_the_terminal_wrapped_it() {
        let token = format!("{CLAUDE_TOKEN}{}", "Ab9_-".repeat(19));
        let printed = format!("\x1b[32m✓\x1b[0m Long-lived authentication token created successfully!\r\n\r\nYour OAuth token (valid for 1 year):\r\n\r\n\x1b[1m{}\r\n{}\x1b[0m\r\n\r\nStore this token securely.", &token[..50], &token[50..]);
        assert_eq!(token_in(&plain(printed.as_bytes())).as_deref(), Some(token.as_str()));
        assert_eq!(token_in(&plain(format!("export CLAUDE_CODE_OAUTH_TOKEN={token}\n").as_bytes())).as_deref(), Some(token.as_str()));
        assert_eq!(token_in("export CLAUDE_CODE_OAUTH_TOKEN=<token>"), None);
        assert_eq!(token_in(&format!("{CLAUDE_TOKEN}short\nStore")), None);
    }

    #[test]
    fn sign_in_pages_are_found_in_what_the_agent_prints_once_they_are_whole() {
        let text = plain(b"Follow these steps:\r\n1. Open \x1b[94mhttps://auth.openai.com/codex/device\x1b[0m\r\n2. Enter this one-time code \x1b[94mABCD-1234\x1b[0m\r\n");
        assert_eq!(urls(&text), ["https://auth.openai.com/codex/device"]);
        assert!(urls("Open https://auth.openai.com/co").is_empty(), "not yet all printed");
        assert!(sign_in_page("https://auth.openai.com/codex/device"));
        assert!(sign_in_page("https://claude.ai/oauth/authorize?code=true"));
        assert!(!sign_in_page("https://claude.ai.example.com/oauth"), "only the vendors' own sign-in pages");
        assert!(!sign_in_page("http://claude.ai/oauth"));
    }
}
