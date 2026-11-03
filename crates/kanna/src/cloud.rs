use crate::target::Target;
use kanna_client::{exit, fail, Failure, Kmux};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOST_SETUP: &str = include_str!("../../../scripts/cloud/host-setup.sh");
const RELEASE_VM: &str = include_str!("../../../scripts/cloud/release-vm.sh");
const VM_SETUP: &str = include_str!("../../../scripts/cloud/vm-setup.sh");
const SERVER_SETUP: &str = include_str!("../../../scripts/server-setup.sh");
const IMAGE: &str = "images:ubuntu/26.04/cloud";
const GITHUB_HOST_KEY: &str = "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
const VM_DIR: &str = "/var/lib/kanna-cloud";

pub fn run(request: &Value, json_output: bool, target: &Target) -> Result<(), Failure> {
    match request["action"].as_str() {
        Some("setup-host") => setup_host(request["host"].as_str().unwrap_or_default()),
        Some("provision") => provision(request, json_output, target),
        Some("release") => release(request, json_output, target),
        _ => list(json_output),
    }
}

fn step(text: &str) {
    eprintln!("==> {text}");
}

fn failed(message: impl Into<String>) -> Failure {
    fail(exit::FAILED, message)
}

pub fn quote(word: &str) -> String {
    if !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:=@,+".contains(&b)) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn ssh(host: &str, command: &str, input: Option<&[u8]>) -> Result<String, Failure> {
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15", host, "--", command])
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failed(format!("can't run ssh: {e}")))?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input).map_err(|e| failed(format!("ssh {host}: {e}")))?;
    }
    let out = child.wait_with_output().map_err(|e| failed(format!("ssh {host}: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(failed(format!("on {host}: {}: {}", command.chars().take(120).collect::<String>(), err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ssh_streamed(host: &str, command: &str, input: &[u8]) -> Result<(), Failure> {
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15", host, "--", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| failed(format!("can't run ssh: {e}")))?;
    child.stdin.take().unwrap().write_all(input).map_err(|e| failed(format!("ssh {host}: {e}")))?;
    let status = child.wait().map_err(|e| failed(format!("ssh {host}: {e}")))?;
    if !status.success() {
        return Err(failed(format!("on {host}: {} failed", command.chars().take(120).collect::<String>())));
    }
    Ok(())
}

fn as_kanna(instance: &str, command: &str) -> String {
    format!("sudo incus exec {instance} --user 1000 --group 1000 --cwd /home/kanna --env HOME=/home/kanna --env USER=kanna --env XDG_RUNTIME_DIR=/run/user/1000 --env PATH=/home/kanna/.local/bin:/usr/local/bin:/usr/bin:/bin -- bash -c {}", quote(command))
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buffer)).expect("no /dev/urandom");
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

fn token() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut buffer = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buffer)).expect("no /dev/urandom");
    buffer.iter().map(|b| ALPHABET[*b as usize % 62] as char).collect()
}

pub fn records() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share"))
    };
    std::env::var("KANNA_CLOUD_RECORDS").map(PathBuf::from).unwrap_or_else(|_| base.join("kanna-cloud/vms"))
}

fn load(name: &str) -> Result<Value, Failure> {
    let path = records().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| fail(exit::NOT_FOUND, format!("no Kanna Cloud VM \"{name}\" made from this Mac (kanna cloud list)")))?;
    serde_json::from_str(&text).map_err(|e| failed(format!("{} isn't a record: {e}", path.display())))
}

fn save(record: &Value) -> Result<(), Failure> {
    let dir = records();
    std::fs::create_dir_all(&dir).map_err(|e| failed(format!("can't make {}: {e}", dir.display())))?;
    let path = dir.join(format!("{}.json", record["name"].as_str().unwrap_or_default()));
    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(record).unwrap_or_default())).map_err(|e| failed(format!("can't write {}: {e}", path.display())))
}

fn daemon(target: &Target) -> Result<Kmux, Failure> {
    target.try_connect().ok_or_else(|| fail(exit::UNREACHABLE, "Kanna isn't running on this Mac: the VM enrolls into its machines, with a yes in Kanna.app"))
}

pub fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) && p != "." && p != "..");
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

fn script(args: &str) -> String {
    format!("f=$(mktemp) && cat > \"$f\" && sudo bash \"$f\" {args}; status=$?; rm -f \"$f\"; exit $status")
}

fn setup_host(host: &str) -> Result<(), Failure> {
    step(&format!("Setting up {host} for Kanna Cloud: Incus, the tenant bridge, its ACL and profile, the pool"));
    ssh_streamed(host, &script(""), HOST_SETUP.as_bytes())
}

fn host_ready(host: &str) -> Result<(), Failure> {
    let ready = ssh(host, "sudo incus profile show kcloud-tenant >/dev/null 2>&1 && sudo incus network show kcloud0 >/dev/null 2>&1 && echo ready", None)?;
    if ready.trim() != "ready" {
        return Err(failed(format!("{host} isn't set up for Kanna Cloud yet: kanna cloud setup-host {host}")));
    }
    Ok(())
}

fn user_data(name: &str, token: &str) -> String {
    format!(
        "#cloud-config\nhostname: {name}\nusers:\n  - name: kanna\n    shell: /bin/bash\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    lock_passwd: true\nwrite_files:\n  - path: {VM_DIR}/token\n    owner: kanna:kanna\n    permissions: '0600'\n    defer: true\n    content: {token}\nruncmd:\n  - [chown, kanna:kanna, {VM_DIR}]\n  - [chmod, '0700', {VM_DIR}]\n  - [loginctl, enable-linger, kanna]\n"
    )
}

fn instance_config(name: &str, token: &str, cpus: &str, memory: &str, disk: &str) -> String {
    let data = user_data(name, token);
    let indented: String = data.lines().map(|l| format!("    {l}\n")).collect();
    format!("config:\n  limits.cpu: \"{cpus}\"\n  limits.memory: {memory}\n  user.kanna-cloud.machine: {name}\n  cloud-init.user-data: |\n{indented}devices:\n  root:\n    type: disk\n    pool: kcloud\n    path: /\n    size: {disk}\n")
}

fn wait_agent(host: &str, instance: &str) -> Result<(), Failure> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if ssh(host, &format!("sudo incus exec {instance} -- true"), None).is_ok() {
            break;
        }
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

fn push(host: &str, instance: &str, path: &str, mode: &str, content: &[u8]) -> Result<(), Failure> {
    ssh(host, &format!("sudo incus exec {instance} -- install -o kanna -g kanna -m {mode} /dev/stdin {}", quote(path)), Some(content)).map(|_| ())
}

fn push_file(host: &str, instance: &str, path: &str, local: &str) -> Result<(), Failure> {
    let file = std::fs::File::open(local).map_err(|e| failed(format!("can't read {local}: {e}")))?;
    let status = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, "--", &format!("sudo incus exec {instance} -- install -o kanna -g kanna -m 0600 /dev/stdin {}", quote(path))])
        .stdin(file)
        .status()
        .map_err(|e| failed(format!("can't run ssh: {e}")))?;
    if !status.success() {
        return Err(failed(format!("couldn't copy {local} into {instance}")));
    }
    Ok(())
}

fn tarball_protocol(local: &str) -> Option<u64> {
    let out = Command::new("tar").args(["-xzOf", local, "kanna/release.json"]).output().ok()?;
    serde_json::from_slice::<Value>(&out.stdout).ok()?["protocol"].as_u64()
}

fn provision(request: &Value, json_output: bool, target: &Target) -> Result<(), Failure> {
    let host = request["host"].as_str().unwrap_or_default().to_string();
    let id = random_hex(4);
    let name = request["name"].as_str().map(String::from).unwrap_or_else(|| format!("cloud-{}", &id[..4]));
    if records().join(format!("{name}.json")).exists() {
        return Err(fail(exit::USAGE, format!("there's already a Kanna Cloud VM named {name}: give another --name")));
    }
    let instance = format!("kc-{id}");
    let repos: Vec<String> = request["repos"].as_array().map(|r| r.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut mac = daemon(target)?;
    let status = mac.call("machine", json!({ "action": "status" }))?;
    if status["peers"].as_array().into_iter().flatten().any(|p| p["name"] == name.as_str()) {
        return Err(fail(exit::USAGE, format!("{name} is already a name among your machines: give another --name")));
    }
    let mine = status["release"]["protocol"].as_u64();
    let relay = mac.call("machine", json!({ "action": "relay_config" }))?;
    let Some(relay_url) = relay["relay"].as_str().map(String::from) else {
        return Err(fail(exit::USAGE, "this Mac meets its machines through no relay, and a Kanna Cloud VM is reached only through one: kanna machine relay URL [--token-file FILE]"));
    };
    let tarball = request["tarball"].as_str().map(String::from);
    if let Some(local) = &tarball {
        match tarball_protocol(local) {
            Some(p) if Some(p) == mine => {}
            Some(p) => return Err(fail(exit::USAGE, format!("{local} speaks protocol {p}, and this Mac's Kanna speaks {}: they couldn't work together", mine.unwrap_or(0)))),
            None => return Err(fail(exit::USAGE, format!("{local} isn't a Kanna Linux tarball (no kanna/release.json)"))),
        }
    }
    let deploy_keys = !repos.is_empty();
    if deploy_keys {
        gh(&["auth", "status"]).map_err(|_| fail(exit::USAGE, "--repo makes a deploy key on GitHub with gh, which isn't signed in: gh auth login"))?;
    }
    host_ready(&host)?;

    let token = token();
    let cpus = request["cpus"].as_str().unwrap_or("4");
    let memory = request["memory"].as_str().unwrap_or("8GiB");
    let disk = request["disk"].as_str().unwrap_or("40GiB");
    step(&format!("Making {instance} on {host} for {name}: {IMAGE}, {cpus} CPUs, {memory}, {disk}"));
    let mut record = json!({ "name": name, "host": host, "instance": instance, "created": unix_time(), "repos": [], "relay": relay_url });
    save(&record)?;
    let made = ssh(host.as_str(), &format!("sudo incus init {IMAGE} {instance} --vm --no-profiles --profile kcloud-tenant && sudo incus start {instance}"), Some(instance_config(&name, &token, cpus, memory, disk).as_bytes()));
    if let Err(failure) = made {
        return Err(failed(format!("{}
kanna cloud release {name} removes whatever was made", failure.message)));
    }
    let result = enroll(&host, &instance, &name, &token, &relay_url, relay["token"].as_str(), tarball.as_deref(), request["agents"].as_str().unwrap_or("claude,codex"), &mut mac, &mut record);
    let result = result.and_then(|()| save(&record)).and_then(|()| {
        for repo in &repos {
            credentials(&host, &instance, &name, repo, &mut record)?;
        }
        git_identity(&host, &instance)
    });
    if let Err(failure) = result {
        save(&record)?;
        return Err(failed(format!("{}\n{name} is left as it got: kanna cloud release {name} removes it", failure.message)));
    }
    ssh(&host, &format!("sudo incus config unset {instance} cloud-init.user-data"), None)?;
    record["enrolled"] = json!(unix_time());
    save(&record)?;
    if json_output {
        println!("{record}");
    } else {
        println!("{name} is one of your machines: Kanna Cloud runs it on {host} as {instance}.");
        println!("Your agent accounts reach it from this Mac (kanna accounts); try kanna --host {name} task list");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn enroll(host: &str, instance: &str, name: &str, token: &str, relay: &str, relay_token: Option<&str>, tarball: Option<&str>, agents: &str, mac: &mut Kmux, record: &mut Value) -> Result<(), Failure> {
    step(&format!("Waiting for {instance} to boot"));
    wait_agent(host, instance)?;
    step("Copying in server setup");
    push(host, instance, &format!("{VM_DIR}/server-setup.sh"), "0700", SERVER_SETUP.as_bytes())?;
    push(host, instance, &format!("{VM_DIR}/vm-setup.sh"), "0700", VM_SETUP.as_bytes())?;
    push(host, instance, &format!("{VM_DIR}/relay-token"), "0600", relay_token.unwrap_or("").as_bytes())?;
    let tarball_in_vm = match tarball {
        Some(local) => {
            let path = format!("{VM_DIR}/kanna-linux.tar.gz");
            push_file(host, instance, &path, local)?;
            path
        }
        None => "latest".to_string(),
    };
    step(&format!("Server setup in {instance} (a few minutes)"));
    let setup = format!("{VM_DIR}/vm-setup.sh {} {} {} {}", quote(name), quote(&tarball_in_vm), quote(agents), quote(relay));
    let status = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, "--", &as_kanna(instance, &setup)])
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .status()
        .map_err(|e| failed(format!("can't run ssh: {e}")))?;
    if !status.success() {
        return Err(failed(format!("server setup in {instance} failed")));
    }

    step(&format!("Enrolling {name} by its token: say yes in Kanna.app"));
    let mut offer = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, "--", &as_kanna(instance, &format!("kanna machine pair --token-file {VM_DIR}/token --json; status=$?; rm -f {VM_DIR}/token; exit $status"))])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failed(format!("can't run ssh: {e}")))?;
    let mut lines = BufReader::new(offer.stdout.take().unwrap()).lines();
    let first = lines.next().and_then(Result::ok).unwrap_or_default();
    let offered: Value = serde_json::from_str(&first).map_err(|_| {
        let mut err = String::new();
        let _ = offer.stderr.take().map(|mut e| e.read_to_string(&mut err));
        failed(format!("{name} didn't open its enrollment: {}", if err.trim().is_empty() { first.trim() } else { err.trim() }))
    })?;
    let key = offered["key"].as_str().unwrap_or_default().to_string();
    record["key"] = json!(key);
    save(record)?;
    let paired = match mac.call("machine", json!({ "action": "pair", "code": token, "peer": key, "hosted": true })) {
        Ok(paired) => paired,
        Err(e) => {
            let _ = offer.kill();
            let _ = offer.wait();
            return Err(failed(format!("enrolling {name}: {}", Failure::from(e).message)));
        }
    };
    let joined = lines.next().and_then(Result::ok).unwrap_or_default();
    let _ = offer.wait();
    if paired["key"] != key.as_str() {
        return Err(failed(format!("enrolled {} instead of {name}", paired["key"])));
    }
    if serde_json::from_str::<Value>(&joined).ok().and_then(|j| j["name"].as_str().map(String::from)).is_none() {
        return Err(failed(format!("{name} never said it joined: {}", joined.trim())));
    }
    Ok(())
}

fn gh(args: &[&str]) -> Result<String, Failure> {
    let out = Command::new("gh").args(args).output().map_err(|e| failed(format!("can't run gh: {e}")))?;
    if !out.status.success() {
        return Err(failed(format!("gh {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn slug(repo: &str) -> String {
    repo.replacen('/', "--", 1).to_lowercase()
}

fn credentials(host: &str, instance: &str, name: &str, repo: &str, record: &mut Value) -> Result<(), Failure> {
    step(&format!("A deploy key for {repo}, made in {name}"));
    let slug = slug(repo);
    let key_path = format!("/home/kanna/.ssh/kanna-{slug}");
    let made = format!(
        "set -e; mkdir -p -m 700 ~/.ssh; [ -f {key_path} ] || ssh-keygen -q -t ed25519 -N '' -C {comment} -f {key_path}; cat {key_path}.pub",
        comment = quote(&format!("kanna cloud {name}"))
    );
    let public = ssh(host, &as_kanna(instance, &made), None)?.trim().to_string();
    let title = format!("Kanna Cloud {name} ({instance})");
    let added = gh(&["api", "-X", "POST", &format!("repos/{repo}/keys"), "-f", &format!("title={title}"), "-f", &format!("key={public}"), "-F", "read_only=false"])?;
    let id = serde_json::from_str::<Value>(&added).ok().and_then(|v| v["id"].as_u64()).ok_or_else(|| failed(format!("GitHub didn't say which key it added to {repo}")))?;
    record["repos"].as_array_mut().unwrap().push(json!({ "repo": repo, "deploy_key": id }));
    save(record)?;
    let alias = format!("github.com-kanna-{slug}");
    let configure = format!(
        "set -e; touch ~/.ssh/config ~/.ssh/known_hosts; chmod 600 ~/.ssh/config; grep -qxF {known} ~/.ssh/known_hosts || echo {known} >> ~/.ssh/known_hosts; grep -q '^Host {alias}$' ~/.ssh/config || printf 'Host {alias}\\n  HostName github.com\\n  User git\\n  IdentityFile {key_path}\\n  IdentitiesOnly yes\\n' >> ~/.ssh/config; \
         git config --global --replace-all url.git@{alias}:{repo}.insteadOf git@github.com:{repo} '^git@github.com:{repo}$'; \
         git config --global --add url.git@{alias}:{repo}.insteadOf https://github.com/{repo}; \
         mkdir -p ~/repos/{owner}; [ -d ~/repos/{repo} ] || git clone -q git@github.com:{repo}.git ~/repos/{repo}",
        known = quote(GITHUB_HOST_KEY),
        owner = repo.split('/').next().unwrap_or(repo)
    );
    ssh(host, &as_kanna(instance, &configure), None)?;
    Ok(())
}

fn git_identity(host: &str, instance: &str) -> Result<(), Failure> {
    let get = |key: &str| Command::new("git").args(["config", "--global", key]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|v| !v.is_empty());
    let mut set = vec![];
    for key in ["user.name", "user.email"] {
        if let Some(value) = get(key) {
            set.push(format!("git config --global {key} {}", quote(&value)));
        }
    }
    if set.is_empty() {
        return Ok(());
    }
    ssh(host, &as_kanna(instance, &set.join("; ")), None).map(|_| ())
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

fn list(json_output: bool) -> Result<(), Failure> {
    let vms = all();
    if json_output {
        println!("{}", json!({ "vms": vms }));
        return Ok(());
    }
    if vms.is_empty() {
        println!("no Kanna Cloud VMs made from this Mac: kanna cloud provision HOST");
    }
    for vm in vms {
        let repos: Vec<&str> = vm["repos"].as_array().into_iter().flatten().filter_map(|r| r["repo"].as_str()).collect();
        let repos = if repos.is_empty() { String::new() } else { format!("  {}", repos.join(", ")) };
        let enrolled = if vm["key"].is_string() { "" } else { "  (not enrolled)" };
        println!("{:<14} {} on {}{repos}{enrolled}", vm["name"].as_str().unwrap_or("?"), vm["instance"].as_str().unwrap_or("?"), vm["host"].as_str().unwrap_or("?"));
    }
    Ok(())
}

fn release(request: &Value, json_output: bool, target: &Target) -> Result<(), Failure> {
    let name = request["name"].as_str().unwrap_or_default();
    let mut record = load(name)?;
    let host = record["host"].as_str().unwrap_or_default().to_string();
    let instance = record["instance"].as_str().unwrap_or_default().to_string();
    if request["yes"] != true {
        eprint!("Release {name}? It destroys {instance} on {host} with its disks and snapshots, and revokes it from your machines. Type its name to go on: ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).map_err(|e| failed(e.to_string()))?;
        if answer.trim() != name {
            return Err(failed("not released"));
        }
    }
    let mut report = json!({ "name": name, "instance": instance, "host": host });
    if let Some(key) = record["key"].as_str().map(String::from) {
        let mut mac = daemon(target)?;
        let status = mac.call("machine", json!({ "action": "status" }))?;
        let known = status["peers"].as_array().into_iter().flatten().any(|p| p["key"] == key.as_str() && p["revoked"] != true);
        if known {
            step(&format!("Revoking {name} from your machines"));
            report["revoked"] = mac.call("machine", json!({ "action": "revoke", "host": key }))?;
        }
        let accounts = mac.call("accounts", json!({ "action": "list" })).unwrap_or(Value::Null);
        report["accounts"] = accounts;
    }
    for repo in record["repos"].clone().as_array().into_iter().flatten() {
        let (Some(slug), Some(id)) = (repo["repo"].as_str(), repo["deploy_key"].as_u64()) else { continue };
        step(&format!("Removing {name}'s deploy key from {slug}"));
        match gh(&["api", "-X", "DELETE", &format!("repos/{slug}/keys/{id}")]) {
            Ok(_) => {}
            Err(failure) if failure.message.contains("Not Found") || failure.message.contains("404") => {}
            Err(failure) => return Err(failure),
        }
        if let Some(repos) = record["repos"].as_array_mut() {
            repos.retain(|r| r["deploy_key"].as_u64() != Some(id));
        }
        save(&record)?;
    }
    step(&format!("Destroying {instance} on {host}"));
    let released = ssh(&host, &script(&instance), Some(RELEASE_VM.as_bytes()))?;
    report["host_release"] = json!(released.trim());
    let _ = std::fs::remove_file(records().join(format!("{name}.json")));
    if json_output {
        println!("{report}");
        return Ok(());
    }
    println!("released {name}: {}", released.trim());
    let lasting: Vec<String> = report["accounts"]["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["kind"].as_str().is_some_and(|k| k != "codex-chatgpt"))
        .filter_map(|a| a["label"].as_str().or(a["kind"].as_str()).map(String::from))
        .collect();
    if !lasting.is_empty() {
        println!("{name} held these long-lived secrets, which its operator could read: {}. Rotate them, then add them again: kanna accounts add", lasting.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_the_remote_shell() {
        assert_eq!(quote("plain-word_1.2"), "plain-word_1.2");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn a_vm_is_made_with_its_token_and_a_fresh_name() {
        let config = instance_config("cloud-ab12", "TOKEN123", "4", "8GiB", "40GiB");
        let parsed: Value = serde_yaml_like(&config);
        assert!(parsed["user_data"].as_str().unwrap().contains("content: TOKEN123"));
        assert!(config.contains("  limits.cpu: \"4\"\n") && config.contains("size: 40GiB"));
        assert_eq!(token().len(), 32);
        assert_ne!(token(), token());
        assert!(kanna_like_token(&token()));
    }

    fn serde_yaml_like(config: &str) -> Value {
        let data: String = config.split("cloud-init.user-data: |\n").nth(1).unwrap().lines().take_while(|l| l.starts_with("    ")).map(|l| format!("{}\n", &l[4..])).collect();
        json!({ "user_data": data })
    }

    fn kanna_like_token(token: &str) -> bool {
        token.bytes().all(|b| b.is_ascii_alphanumeric())
    }

    #[test]
    fn repos_are_owner_slash_name() {
        assert!(valid_repo("tampopogk/kanna"));
        assert!(valid_repo("a-b/c.d_e"));
        for bad in ["kanna", "a/b/c", "../x", "a/..", "a/b c", "a/"] {
            assert!(!valid_repo(bad), "{bad}");
        }
        assert_eq!(slug("TampopoGK/kanna.app"), "tampopogk--kanna.app");
        assert_ne!(slug("acme/foo.bar"), slug("acme/foo-bar"));
        assert_ne!(slug("a-b/c"), slug("a/b-c"));
    }
}
