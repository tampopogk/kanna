use crate::args::Argv;
use crate::commands::usage;
use crate::target::Session;
use kanna_client::{exit, fail, Failure};
use regex::Regex;
use serde_json::{json, Value};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POLL_MS: u64 = 10_000;

pub fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("serial", "say list, read, send, reset, set, borrow or log"))?;
    let pane = |args: &mut Argv, what: &str| args.positional().ok_or_else(|| usage("serial", &format!("{what} needs a serial pane, like serial-1")));
    let mut out = json!({ "action": action });
    match action.as_str() {
        "list" => {}
        "read" => {
            out["pane"] = json!(pane(args, "read")?);
            if let Some(since) = args.option("--since")? {
                out["since"] = json!(since_ms(&since).ok_or_else(|| usage("serial", "--since takes a time (14:37:40 or 14:37:40.118) or a duration (10s, 2m, 1h)"))?);
            }
            if let Some(last) = args.option("--last")? {
                out["last"] = json!(last.parse::<u64>().map_err(|_| usage("serial", "--last takes a number of rows"))?);
            }
            out["follow"] = json!(args.flag("--follow"));
            out["raw"] = json!(args.flag("--raw"));
            if let Some(until) = args.option("--until")? {
                Regex::new(&until).map_err(|e| usage("serial", &format!("--until isn't a regular expression: {e}")))?;
                out["until"] = json!(until);
            }
            if let Some(timeout) = args.option("--timeout")? {
                out["timeout"] = json!(timeout.trim_end_matches('s').parse::<f64>().ok().filter(|t| *t > 0.0).ok_or_else(|| usage("serial", "--timeout takes seconds"))?);
            }
        }
        "send" => {
            out["pane"] = json!(pane(args, "send")?);
            let hex = args.flag("--hex");
            if args.flag("--no-ending") {
                out["ending"] = json!(false);
            }
            let text = args.rest().ok_or_else(|| usage("serial", "send needs TEXT"))?;
            out[if hex { "hex" } else { "text" }] = json!(text);
        }
        "reset" => {
            out["pane"] = json!(pane(args, "reset")?);
            if let Some(hold) = args.option("--hold")? {
                out["hold"] = json!(hold.trim_end_matches("ms").parse::<u64>().map_err(|_| usage("serial", "--hold takes milliseconds"))?);
            }
        }
        "set" => {
            out["pane"] = json!(pane(args, "set")?);
            if let Some(baud) = args.option("--baud")? {
                out["baud"] = json!(baud.parse::<u32>().map_err(|_| usage("serial", "--baud takes a number"))?);
            }
            if let Some(ending) = args.option("--line-ending")? {
                out["lineEnding"] = json!(ending);
            }
        }
        "log" | "state" => out["pane"] = json!(pane(args, &action)?),
        "borrow" => {
            let target = args.positional().ok_or_else(|| usage("serial", "borrow needs a serial pane or port, then -- and the command"))?;
            let words = args.remaining();
            let cmd: Vec<String> = match words.split_first() {
                Some((dash, cmd)) if dash == "--" && !cmd.is_empty() => cmd.to_vec(),
                _ => return Err(usage("serial", "borrow runs a command after --: kanna serial borrow serial-1 -- idf.py flash")),
            };
            out[if is_port(&target) { "port" } else { "pane" }] = json!(target);
            out["argv"] = json!(cmd);
        }
        other => return Err(usage("serial", &format!("unknown action \"{other}\" (list, read, send, reset, set, borrow or log)"))),
    }
    Ok(out)
}

fn is_port(target: &str) -> bool {
    target.starts_with("/dev/") || target.contains(':')
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn local(ms: i64) -> libc::tm {
    let secs = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    tm
}

pub fn clock(ms: i64) -> String {
    let tm = local(ms);
    format!("{:02}:{:02}:{:02}.{:03}", tm.tm_hour, tm.tm_min, tm.tm_sec, ms.rem_euclid(1000))
}

pub fn since_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let unit = |suffix: &str, scale: f64| text.strip_suffix(suffix).and_then(|n| n.parse::<f64>().ok()).map(|n| now_ms() - (n * scale) as i64);
    if let Some(ms) = unit("ms", 1.0).or_else(|| unit("s", 1000.0)).or_else(|| unit("m", 60_000.0)).or_else(|| unit("h", 3_600_000.0)) {
        return Some(ms);
    }
    let (clock, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let parts: Vec<i32> = clock.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (h, m, s) = match parts[..] {
        [h, m, s] => (h, m, s),
        [h, m] => (h, m, 0),
        _ => return None,
    };
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    let mut tm = local(now_ms());
    tm.tm_hour = h;
    tm.tm_min = m;
    tm.tm_sec = s;
    tm.tm_isdst = -1;
    let secs = unsafe { libc::mktime(&mut tm) };
    (secs >= 0).then_some(secs as i64 * 1000 + millis)
}

pub fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if bytes.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            if let Some(byte) = (bytes.get(i + 1) == Some(&b'x')).then(|| text.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten() {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn strip_ansi(text: &str) -> String {
    Regex::new(r"\x1b(\[[0-?]*[ -/]*[@-~]|[@-Z\\-_])").map(|re| re.replace_all(text, "").into_owned()).unwrap_or_else(|_| text.to_string())
}

pub fn row_line(row: &Value, colour: bool) -> String {
    let text = row["text"].as_str().unwrap_or("");
    let text = if colour { text.to_string() } else { strip_ansi(text) };
    let t = clock(row["t"].as_i64().unwrap_or(0));
    match row["d"].as_str() {
        Some(">") => format!("{t} → {text}"),
        Some("#") => format!("{t} # {text}"),
        _ if colour && text.contains('\x1b') => format!("{t} {text}\x1b[0m"),
        _ => format!("{t} {text}"),
    }
}

fn print_rows(rows: &[Value], raw: bool, json_output: bool, out: &mut impl Write) {
    let colour = std::io::stdout().is_terminal();
    for row in rows {
        let _ = if json_output {
            writeln!(out, "{row}")
        } else if raw {
            if row["d"] == "<" {
                let mut bytes = unescape(row["text"].as_str().unwrap_or(""));
                bytes.push(b'\n');
                out.write_all(&bytes)
            } else {
                Ok(())
            }
        } else {
            writeln!(out, "{}", row_line(row, colour))
        };
    }
    let _ = out.flush();
}

pub fn run(request: &Value, mux: &mut Session, json_output: bool) -> Result<(), Failure> {
    match request["action"].as_str().unwrap_or("") {
        "list" => {
            let reply = mux.call("serial", json!({ "action": "list" }))?;
            if json_output {
                println!("{reply}");
            } else {
                println!("{}", list_text(&reply));
            }
            Ok(())
        }
        "read" => read(request, mux, json_output),
        "borrow" => borrow(request, mux),
        "log" => {
            let reply = mux.call("serial", json!({ "action": "state", "pane": request["pane"] }))?;
            println!("{}", reply["port"]["log"].as_str().unwrap_or(""));
            Ok(())
        }
        action => {
            let reply = mux.call("serial", request.clone())?;
            if json_output {
                println!("{reply}");
                return Ok(());
            }
            let pane = request["pane"].as_str().unwrap_or("?");
            match action {
                "send" => println!("sent to {pane} at {}", clock(reply["at"].as_i64().unwrap_or(0))),
                "reset" => println!("reset {pane} (DTR/RTS) at {}", clock(reply["at"].as_i64().unwrap_or(0))),
                _ => println!("{}", state_text(pane, &reply["port"])),
            }
            Ok(())
        }
    }
}

pub fn state_text(pane: &str, port: &Value) -> String {
    let state = match port["state"].as_str() {
        Some("lent") => format!("lent to {}", port["cmd"].as_str().unwrap_or("a command")),
        Some(state) => state.to_string(),
        None => "?".into(),
    };
    format!("{pane}: {} {state} at {} ({})", port["path"].as_str().unwrap_or("?"), port["baud"], port["lineEnding"].as_str().unwrap_or("lf"))
}

pub fn list_text(reply: &Value) -> String {
    let ports = reply["ports"].as_array().cloned().unwrap_or_default();
    if ports.is_empty() {
        return "no serial ports plugged in".into();
    }
    let width = ports.iter().filter_map(|p| p["path"].as_str()).map(str::len).max().unwrap_or(4).max(4);
    let mut lines = vec![format!("{:width$}  {:9}  {:40}  OPEN", "PORT", "USB", "NAME")];
    for port in &ports {
        let name = match (port["product"].as_str(), port["vendor"].as_str()) {
            (Some(product), Some(vendor)) if !product.contains(vendor) => format!("{product} ({vendor})"),
            (Some(product), _) => product.to_string(),
            (None, Some(vendor)) => vendor.to_string(),
            (None, None) => String::new(),
        };
        let open = port["panes"].as_array().map(|p| p.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        lines.push(format!("{:width$}  {:9}  {:40}  {open}", port["path"].as_str().unwrap_or("?"), port["usb"].as_str().unwrap_or("-"), name).trim_end().to_string());
    }
    lines.join("\n")
}

fn read(request: &Value, mux: &mut Session, json_output: bool) -> Result<(), Failure> {
    let pane = request["pane"].clone();
    let raw = request["raw"] == true;
    let until = request["until"].as_str().map(|u| Regex::new(u).unwrap());
    let follow = request["follow"] == true || until.is_some();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut first = json!({ "action": "read", "pane": pane });
    for key in ["since", "last"] {
        if !request[key].is_null() {
            first[key] = request[key].clone();
        }
    }
    let fresh = follow && request["since"].is_null() && request["last"].is_null();
    if fresh {
        first["last"] = json!(0);
    }
    let reply = mux.call("serial", first)?;
    let started = Instant::now();
    let deadline = until.as_ref().map(|_| started + Duration::from_secs_f64(request["timeout"].as_f64().unwrap_or(30.0)));
    let check = |rows: &[Value], out: &mut std::io::StdoutLock| -> Option<usize> {
        let until = until.as_ref()?;
        let at = rows.iter().position(|row| row["d"] == "<" && until.is_match(&strip_ansi(row["text"].as_str().unwrap_or(""))))?;
        print_rows(&rows[..=at], raw, json_output, out);
        Some(at)
    };
    let rows = reply["rows"].as_array().cloned().unwrap_or_default();
    if check(&rows, &mut out).is_some() {
        return matched(request, started);
    }
    print_rows(&rows, raw, json_output, &mut out);
    if !follow {
        return Ok(());
    }
    let mut after = reply["next"].as_u64().unwrap_or(0);
    loop {
        let wait = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    let shown = request["until"].as_str().unwrap_or("");
                    return Err(fail(exit::FAILED, format!("no line matched '{shown}' in {} s", request["timeout"].as_f64().unwrap_or(30.0))));
                }
                (left.as_millis() as u64).clamp(1, POLL_MS)
            }
            None => POLL_MS,
        };
        let reply = mux.call("serial", json!({ "action": "read", "pane": pane, "after": after, "wait": wait }))?;
        let rows = reply["rows"].as_array().cloned().unwrap_or_default();
        if check(&rows, &mut out).is_some() {
            return matched(request, started);
        }
        print_rows(&rows, raw, json_output, &mut out);
        after = reply["next"].as_u64().unwrap_or(after);
    }
}

fn matched(request: &Value, started: Instant) -> Result<(), Failure> {
    eprintln!("matched '{}' after {:.1} s", request["until"].as_str().unwrap_or(""), started.elapsed().as_secs_f64());
    Ok(())
}

fn borrow(request: &Value, mux: &mut Session) -> Result<(), Failure> {
    let argv: Vec<String> = request["argv"].as_array().into_iter().flatten().filter_map(|w| w.as_str().map(String::from)).collect();
    let shown = argv.join(" ");
    let mut lend = json!({ "action": "lend", "cmd": shown, "pid": std::process::id() });
    for key in ["pane", "port"] {
        if !request[key].is_null() {
            lend[key] = request[key].clone();
        }
    }
    let reply = match mux.call("serial", lend.clone()) {
        Err(failure) if failure.code == exit::NOT_FOUND && !request["pane"].is_null() => {
            lend["port"] = lend["pane"].take();
            lend.as_object_mut().map(|l| l.remove("pane"));
            mux.call("serial", lend)?
        }
        other => other?,
    };
    let lease = reply["lease"].as_u64().filter(|_| reply["lent"] == true);
    let target = request["pane"].as_str().or(request["port"].as_str()).unwrap_or("?").to_string();
    if lease.is_some() {
        eprintln!("lent {target} to {shown}");
    }
    let status = std::process::Command::new(&argv[0]).args(&argv[1..]).status();
    if let Some(lease) = lease {
        let _ = mux.call("serial", json!({ "action": "return", "lease": lease }));
        eprintln!("{target} back");
    }
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => Err(fail(exit::FAILED, format!("can't run {}: {error}", argv[0]))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Argv {
        Argv::new(words.iter().map(|w| w.to_string()).collect())
    }

    #[test]
    fn borrow_takes_the_command_after_the_double_dash() {
        let request = parse(&mut argv(&["borrow", "serial-1", "--", "idf.py", "-p", "/dev/x", "flash"])).unwrap();
        assert_eq!(request["pane"], "serial-1");
        assert_eq!(request["argv"], json!(["idf.py", "-p", "/dev/x", "flash"]));
        let request = parse(&mut argv(&["borrow", "/dev/cu.usbmodem1101", "--", "make"])).unwrap();
        assert_eq!(request["port"], "/dev/cu.usbmodem1101");
        assert!(parse(&mut argv(&["borrow", "serial-1", "make"])).is_err());
    }

    #[test]
    fn send_joins_its_words_and_takes_hex() {
        let request = parse(&mut argv(&["send", "serial-1", "wifi", "status"])).unwrap();
        assert_eq!(request["text"], "wifi status");
        let request = parse(&mut argv(&["send", "serial-1", "--hex", "1b 5b 41", "--no-ending"])).unwrap();
        assert_eq!(request["hex"], "1b 5b 41");
        assert_eq!(request["ending"], false);
    }

    #[test]
    fn since_takes_a_clock_time_or_a_duration() {
        let ten = since_ms("10s").unwrap();
        assert!((now_ms() - ten - 10_000).abs() < 1000);
        let at = since_ms("14:37:40.118").unwrap();
        assert_eq!(clock(at), "14:37:40.118");
        assert_eq!(clock(since_ms("09:05").unwrap()), "09:05:00.000");
        assert!(since_ms("soon").is_none());
    }

    #[test]
    fn escapes_come_back_as_bytes() {
        assert_eq!(unescape("a\\x00\\\\b\\xff"), vec![b'a', 0, b'\\', b'b', 0xff]);
        assert_eq!(unescape("\\xzz"), b"\\xzz".to_vec());
    }
}
