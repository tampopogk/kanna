use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOST_SETUP: &str = include_str!("../../../scripts/cloud/host-setup.sh");
const RELEASE_VM: &str = include_str!("../../../scripts/cloud/release-vm.sh");
const VM_SETUP: &str = include_str!("../../../scripts/cloud/vm-setup.sh");
const SERVER_SETUP: &str = include_str!("../../../scripts/server-setup.sh");
const IMAGE: &str = "images:ubuntu/26.04/cloud";
const VM_DIR: &str = "/var/lib/kanna-cloud";

const USAGE: &str = "usage: kanna-cloud list [--json]
       kanna-cloud setup-host HOST
       kanna-cloud provision HOST [--for LABEL] [--tarball FILE] [--relay URL [--relay-token-file FILE]]
                             [--agents LIST] [--cpus N] [--memory SIZE] [--disk SIZE] [--json]
       kanna-cloud release INSTANCE [--yes] [--json]

Kanna Cloud's internal operator tool (docs/specs/cloud-vms.md): it sets up hosts and makes and
destroys the VMs Cloud tasks run on. HOST is a host's ssh destination with passwordless sudo.
Customers never run it.";

struct Failure(String);

type Result<T> = std::result::Result<T, Failure>;

fn failed(message: impl Into<String>) -> Failure {
    Failure(message.into())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(()) => {}
        Err(Failure(message)) => {
            eprintln!("kanna-cloud: {message}");
            std::process::exit(1);
        }
    }
}

#[derive(Default, Debug, PartialEq)]
struct Args {
    action: String,
    target: Option<String>,
    options: std::collections::BTreeMap<String, String>,
    json: bool,
    yes: bool,
}

const TAKES_VALUE: &[&str] = &["--for", "--tarball", "--relay", "--relay-token-file", "--agents", "--cpus", "--memory", "--disk"];

fn parse(words: Vec<String>) -> Result<Args> {
    let mut args = Args::default();
    let mut words = words.into_iter();
    let mut positionals = vec![];
    while let Some(word) = words.next() {
        match word.as_str() {
            "--json" => args.json = true,
            "--yes" => args.yes = true,
            "-h" | "--help" => return Err(failed(USAGE)),
            option if TAKES_VALUE.contains(&option) => {
                let value = words.next().ok_or_else(|| failed(format!("{option} needs a value\n{USAGE}")))?;
                args.options.insert(option.to_string(), value);
            }
            option if option.starts_with("--") => return Err(failed(format!("unknown option {option}\n{USAGE}"))),
            _ => positionals.push(word),
        }
    }
    let mut positionals = positionals.into_iter();
    args.action = positionals.next().unwrap_or_else(|| "list".into());
    args.target = positionals.next();
    if let Some(extra) = positionals.next() {
        return Err(failed(format!("unexpected {extra}\n{USAGE}")));
    }
    let provisioning = args.action == "provision";
    if !provisioning && !args.options.is_empty() {
        return Err(failed(format!("{} go with provision", args.options.keys().cloned().collect::<Vec<_>>().join(", "))));
    }
    if args.yes && args.action != "release" {
        return Err(failed("--yes goes with release"));
    }
    if args.options.contains_key("--relay-token-file") && !args.options.contains_key("--relay") {
        return Err(failed("--relay-token-file goes with --relay"));
    }
    for key in ["--cpus", "--memory", "--disk"] {
        if args.options.get(key).is_some_and(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return Err(failed(format!("{key} is a number, or a size like 8GiB")));
        }
    }
    if args.options.get("--agents").is_some_and(|a| !a.split(',').all(|a| matches!(a, "claude" | "codex" | "none"))) {
        return Err(failed("--agents is claude, codex, both (claude,codex) or none"));
    }
    if args.options.get("--relay").is_some_and(|r| !(r.starts_with("https://") || r.starts_with("http://"))) {
        return Err(failed("--relay is a relay's URL, like https://relay.example.com:8443"));
    }
    match args.action.as_str() {
        "list" => {}
        "setup-host" | "provision" if args.target.is_none() => return Err(failed(format!("{} needs a HOST: its ssh destination", args.action))),
        "release" if !args.target.as_deref().is_some_and(valid_instance) => return Err(failed("release needs an INSTANCE: kc- and eight hex digits (kanna-cloud list)")),
        "setup-host" | "provision" | "release" => {}
        other => return Err(failed(format!("unknown action {other}\n{USAGE}"))),
    }
    Ok(args)
}

fn run(words: Vec<String>) -> Result<()> {
    let args = parse(words)?;
    let target = args.target.clone().unwrap_or_default();
    match args.action.as_str() {
        "setup-host" => setup_host(&target),
        "provision" => provision(&target, &args),
        "release" => release(&target, &args),
        _ => list(args.json),
    }
}

fn valid_instance(name: &str) -> bool {
    name.len() == 11 && name.starts_with("kc-") && name[3..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn step(text: &str) {
    eprintln!("==> {text}");
}

pub fn quote(word: &str) -> String {
    if !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:=@,+".contains(&b)) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn ssh_command(host: &str, command: &str) -> Command {
    let mut ssh = Command::new("ssh");
    ssh.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15", host, "--", command]);
    ssh
}

fn ssh(host: &str, command: &str, input: Option<&[u8]>) -> Result<String> {
    let mut child = ssh_command(host, command)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failed(format!("can't run ssh: {e}")))?;
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).map_err(|e| failed(format!("ssh {host}: {e}")))?;
    }
    let out = child.wait_with_output().map_err(|e| failed(format!("ssh {host}: {e}")))?;
    if !out.status.success() {
        return Err(failed(format!("on {host}: {}: {}", command.chars().take(120).collect::<String>(), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ssh_shown(host: &str, command: &str, input: Option<std::fs::File>, script: Option<&[u8]>) -> Result<()> {
    let mut ssh = ssh_command(host, command);
    ssh.stdout(Stdio::from(std::io::stderr()));
    match (input, script) {
        (Some(file), _) => ssh.stdin(file),
        (None, Some(_)) => ssh.stdin(Stdio::piped()),
        (None, None) => ssh.stdin(Stdio::null()),
    };
    let mut child = ssh.spawn().map_err(|e| failed(format!("can't run ssh: {e}")))?;
    if let Some(script) = script {
        child.stdin.take().unwrap().write_all(script).map_err(|e| failed(format!("ssh {host}: {e}")))?;
    }
    let status = child.wait().map_err(|e| failed(format!("ssh {host}: {e}")))?;
    if !status.success() {
        return Err(failed(format!("on {host}: {} failed", command.chars().take(120).collect::<String>())));
    }
    Ok(())
}

fn script(args: &str) -> String {
    format!("f=$(mktemp) && cat > \"$f\" && sudo bash \"$f\" {args}; status=$?; rm -f \"$f\"; exit $status")
}

fn as_kanna(instance: &str, command: &str) -> String {
    format!("sudo incus exec {instance} --user 1000 --group 1000 --cwd /home/kanna --env HOME=/home/kanna --env USER=kanna --env XDG_RUNTIME_DIR=/run/user/1000 --env PATH=/home/kanna/.local/bin:/usr/local/bin:/usr/bin:/bin -- bash -c {}", quote(command))
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buffer)).expect("no /dev/urandom");
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

fn records() -> PathBuf {
    if let Ok(dir) = std::env::var("KANNA_CLOUD_RECORDS") {
        return PathBuf::from(dir);
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let base = if cfg!(target_os = "macos") { home.join("Library/Application Support") } else { std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share")) };
    base.join("kanna-cloud/vms")
}

fn save(record: &Value) -> Result<()> {
    let dir = records();
    std::fs::create_dir_all(&dir).map_err(|e| failed(format!("can't make {}: {e}", dir.display())))?;
    let path = dir.join(format!("{}.json", record["instance"].as_str().unwrap_or_default()));
    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(record).unwrap_or_default())).map_err(|e| failed(format!("can't write {}: {e}", path.display())))
}

fn load(instance: &str) -> Result<Value> {
    let path = records().join(format!("{instance}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| failed(format!("no record of {instance} (kanna-cloud list)")))?;
    serde_json::from_str(&text).map_err(|e| failed(format!("{} isn't a record: {e}", path.display())))
}

fn setup_host(host: &str) -> Result<()> {
    step(&format!("Setting up {host} for Kanna Cloud: Incus, the tenant bridge, its ACL and profile, the pool"));
    ssh_shown(host, &script(""), None, Some(HOST_SETUP.as_bytes()))
}

fn host_ready(host: &str) -> Result<()> {
    let ready = ssh(host, "sudo incus profile show kcloud-tenant >/dev/null 2>&1 && sudo incus network show kcloud0 >/dev/null 2>&1 && echo ready", None)?;
    if ready.trim() != "ready" {
        return Err(failed(format!("{host} isn't set up for Kanna Cloud yet: kanna-cloud setup-host {host}")));
    }
    Ok(())
}

fn user_data(instance: &str) -> String {
    format!("#cloud-config\nhostname: {instance}\nusers:\n  - name: kanna\n    shell: /bin/bash\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    lock_passwd: true\nruncmd:\n  - [install, -d, -o, kanna, -g, kanna, -m, '0700', {VM_DIR}]\n  - [loginctl, enable-linger, kanna]\n")
}

fn instance_config(instance: &str, label: &str, cpus: &str, memory: &str, disk: &str) -> String {
    let indented: String = user_data(instance).lines().map(|l| format!("    {l}\n")).collect();
    format!("config:\n  limits.cpu: \"{cpus}\"\n  limits.memory: {memory}\n  user.kanna-cloud.for: {}\n  cloud-init.user-data: |\n{indented}devices:\n  root:\n    type: disk\n    pool: kcloud\n    path: /\n    size: {disk}\n", serde_json::to_string(label).unwrap_or_default())
}

fn wait_agent(host: &str, instance: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(300);
    while ssh(host, &format!("sudo incus exec {instance} -- true"), None).is_err() {
        if Instant::now() > deadline {
            return Err(failed(format!("{instance} on {host} never started its agent")));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    let status = ssh(host, &format!("sudo incus exec {instance} -- cloud-init status --wait; true"), None)?;
    if !status.contains("done") {
        return Err(failed(format!("cloud-init in {instance} didn't finish: {}", status.trim())));
    }
    Ok(())
}

fn push(host: &str, instance: &str, path: &str, mode: &str, content: &[u8]) -> Result<()> {
    ssh(host, &format!("sudo incus exec {instance} -- install -o kanna -g kanna -m {mode} /dev/stdin {}", quote(path)), Some(content)).map(|_| ())
}

fn tarball_protocol(local: &str) -> Option<u64> {
    let out = Command::new("tar").args(["-xzOf", local, "kanna/release.json"]).output().ok()?;
    serde_json::from_slice::<Value>(&out.stdout).ok()?["protocol"].as_u64()
}

fn provision(host: &str, args: &Args) -> Result<()> {
    let option = |key: &str| args.options.get(key).map(String::as_str);
    if let Some(local) = option("--tarball") {
        if tarball_protocol(local).is_none() {
            return Err(failed(format!("{local} isn't a Kanna Linux tarball (no kanna/release.json)")));
        }
    }
    let relay_token = match option("--relay-token-file") {
        Some(path) => Some(std::fs::read_to_string(path).map_err(|e| failed(format!("can't read {path}: {e}")))?.trim().to_string()),
        None => None,
    };
    host_ready(host)?;
    let instance = format!("kc-{}", random_hex(4));
    let label = option("--for").unwrap_or("");
    let (cpus, memory, disk) = (option("--cpus").unwrap_or("4"), option("--memory").unwrap_or("8GiB"), option("--disk").unwrap_or("40GiB"));
    let mut record = json!({ "instance": instance, "host": host, "for": label, "created": unix_time() });
    save(&record)?;
    step(&format!("Making {instance} on {host}: {IMAGE}, {cpus} CPUs, {memory}, {disk}"));
    let made = ssh(host, &format!("sudo incus init {IMAGE} {instance} --vm --no-profiles --profile kcloud-tenant && sudo incus start {instance}"), Some(instance_config(&instance, label, cpus, memory, disk).as_bytes()));
    let result = made.and_then(|_| set_up(host, &instance, args, relay_token.as_deref()));
    let key = match result {
        Ok(key) => key,
        Err(Failure(message)) => return Err(failed(format!("{message}\n{instance} is left as it got: kanna-cloud release {instance} removes it"))),
    };
    record["key"] = json!(key);
    record["ready"] = json!(unix_time());
    save(&record)?;
    if args.json {
        println!("{record}");
    } else {
        println!("{instance} on {host} runs Kanna, key {key}");
    }
    Ok(())
}

fn set_up(host: &str, instance: &str, args: &Args, relay_token: Option<&str>) -> Result<String> {
    let option = |key: &str| args.options.get(key).map(String::as_str);
    step(&format!("Waiting for {instance} to boot"));
    wait_agent(host, instance)?;
    ssh(host, &format!("sudo incus config unset {instance} cloud-init.user-data"), None)?;
    step("Copying in server setup");
    push(host, instance, &format!("{VM_DIR}/server-setup.sh"), "0700", SERVER_SETUP.as_bytes())?;
    push(host, instance, &format!("{VM_DIR}/vm-setup.sh"), "0700", VM_SETUP.as_bytes())?;
    push(host, instance, &format!("{VM_DIR}/relay-token"), "0600", relay_token.unwrap_or("").as_bytes())?;
    let tarball = match option("--tarball") {
        Some(local) => {
            let path = format!("{VM_DIR}/kanna-linux.tar.gz");
            let file = std::fs::File::open(local).map_err(|e| failed(format!("can't read {local}: {e}")))?;
            ssh_shown(host, &format!("sudo incus exec {instance} -- install -o kanna -g kanna -m 0600 /dev/stdin {path}"), Some(file), None)?;
            path
        }
        None => "latest".to_string(),
    };
    step(&format!("Server setup in {instance} (a few minutes)"));
    let setup = format!("{VM_DIR}/vm-setup.sh {} {} {} {}", quote(instance), quote(&tarball), quote(option("--agents").unwrap_or("claude,codex")), quote(option("--relay").unwrap_or("none")));
    ssh_shown(host, &as_kanna(instance, &setup), None, None)?;
    let status = ssh(host, &as_kanna(instance, "KANNA_SOCKET=$HOME/.local/share/kanna-prod/kanna.sock kanna machine list --json"), None)?;
    let status: Value = serde_json::from_str(&status).map_err(|_| failed(format!("{instance}'s daemon didn't answer: {}", status.trim())))?;
    status["key"].as_str().map(String::from).ok_or_else(|| failed(format!("{instance}'s daemon has no key")))
}

fn unix_time() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn all() -> Vec<Value> {
    let mut found: Vec<Value> = std::fs::read_dir(records())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    found.sort_by_key(|r: &Value| r["created"].as_u64().unwrap_or(0));
    found
}

fn list(json_output: bool) -> Result<()> {
    let vms = all();
    if json_output {
        println!("{}", json!({ "vms": vms }));
        return Ok(());
    }
    if vms.is_empty() {
        println!("no VMs made from here: kanna-cloud provision HOST");
    }
    for vm in vms {
        let state = if vm["ready"].is_u64() { "ready" } else { "not ready" };
        let label = vm["for"].as_str().filter(|l| !l.is_empty()).map(|l| format!("  for {l}")).unwrap_or_default();
        println!("{}  on {}  {state}{label}", vm["instance"].as_str().unwrap_or("?"), vm["host"].as_str().unwrap_or("?"));
    }
    Ok(())
}

fn release(instance: &str, args: &Args) -> Result<()> {
    let record = load(instance)?;
    let host = record["host"].as_str().unwrap_or_default().to_string();
    if !args.yes {
        eprint!("Release {instance}? It destroys it on {host} with its disks, snapshots and logs. Type its name to go on: ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).map_err(|e| failed(e.to_string()))?;
        if answer.trim() != instance {
            return Err(failed("not released"));
        }
    }
    step(&format!("Destroying {instance} on {host}"));
    let released = ssh(&host, &script(instance), Some(RELEASE_VM.as_bytes()))?;
    std::fs::remove_file(records().join(format!("{instance}.json"))).map_err(|e| failed(format!("released, but can't remove its record: {e}")))?;
    if args.json {
        println!("{}", json!({ "instance": instance, "host": host, "released": released.trim() }));
    } else {
        println!("{}", released.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn arguments_are_checked_before_anything_runs() {
        assert_eq!(parse(vec![]).ok().map(|a| a.action), Some("list".into()));
        let provision = parse(words("provision h --for acme --cpus 8 --relay https://r.example:8443 --json")).ok().unwrap();
        assert_eq!((provision.target.as_deref(), provision.options["--cpus"].as_str(), provision.json), (Some("h"), "8", true));
        for bad in ["provision", "release", "release cloud-1", "setup-host", "list --cpus 4", "provision h --cpus 4;rm", "provision h --relay ftp://x", "provision h --relay-token-file f", "provision h --agents vim", "list --yes", "nope"] {
            assert!(parse(words(bad)).is_err(), "{bad}");
        }
        assert!(parse(words("release kc-0a1b2c3d --yes")).is_ok());
    }

    #[test]
    fn instances_are_fresh_and_named_only_by_kanna() {
        assert!(valid_instance("kc-0a1b2c3d"));
        for bad in ["kc-0A1B2C3D", "kc-0a1b2c3", "cloud-1", "kc-0a1b2c3d;x"] {
            assert!(!valid_instance(bad), "{bad}");
        }
        assert!(valid_instance(&format!("kc-{}", random_hex(4))));
        assert_ne!(random_hex(4), random_hex(4));
    }

    #[test]
    fn a_vm_gets_no_secret_in_its_cloud_init() {
        let config = instance_config("kc-0a1b2c3d", "acme 'quoted'", "4", "8GiB", "40GiB");
        assert!(config.contains("  limits.cpu: \"4\"\n") && config.contains("size: 40GiB") && config.contains("hostname: kc-0a1b2c3d"));
        assert!(config.contains("user.kanna-cloud.for: \"acme 'quoted'\""));
        assert!(!config.contains("token"));
    }

    #[test]
    fn quoting_survives_the_remote_shell() {
        assert_eq!(quote("plain-word_1.2"), "plain-word_1.2");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
    }
}
