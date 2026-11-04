pub mod arrange;
pub mod capabilities;
pub mod close;
pub mod comment;
pub mod crashes;
pub mod diagrams;
pub mod focus;
pub mod markup;
pub mod r#move;
pub mod move_tab;
pub mod navigate;
pub mod open;
pub mod project;
pub mod projects;
pub mod quiz;
pub mod read;
pub mod rename_pane;
pub mod rename_tab;
pub mod resize;
pub mod restart;
pub mod run;
pub mod schedule;
pub mod screenshot;
pub mod send;
pub mod send_key;
pub mod shared;
pub mod stats;
pub mod status;
pub mod task;
pub mod toast;
pub mod tour;
pub mod window;
pub mod workspace;
pub mod zoom;

use crate::args::Argv;
use crate::output;
use crate::target::{Env, Session, Target};
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};
pub use shared::*;

pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub options: &'static [(&'static str, &'static str)],
    pub notes: &'static str,
    pub examples: &'static [(&'static str, &'static str)],
    pub parse: fn(&mut Argv) -> Result<Value, Failure>,
    pub show: fn(&Value, &Value) -> String,
    pub flow: Flow,
}

pub struct Flow {
    pub wire: Option<fn(&Value) -> &'static str>,
    pub unplaced: Option<fn(&Value) -> Option<&'static str>>,
    pub offline: Option<fn(&Value, &Invocation) -> Option<Result<(), Failure>>>,
    pub online: Option<fn(&Value, &mut Session, bool) -> Option<Result<(), Failure>>>,
    pub before: Option<fn(&mut Value, &mut Session) -> Result<Call, Failure>>,
    pub after: Option<fn(&Value, &mut Value, &mut Session, &mut Call) -> Result<(), Failure>>,
}

impl Flow {
    pub const PLAIN: Flow = Flow { wire: None, unplaced: None, offline: None, online: None, before: None, after: None };
}

impl Command {
    pub fn wire(&self, request: &Value) -> &'static str {
        self.flow.wire.map_or(self.name, |wire| wire(request))
    }
}

pub struct Invocation<'a> {
    pub target: &'a Target,
    pub env: Env<'a>,
    pub host: Option<String>,
    pub json_output: bool,
}

#[derive(Default)]
pub struct Call {
    pub task: Option<String>,
    pub shorts: Vec<(String, String)>,
    pub listed: bool,
    pub waiting: Option<Value>,
    pub warnings: Vec<String>,
}

pub const GROUPS: &[&str] = &["Panes", "Tabs and windows", "Information"];

pub const LOCAL: &[&str] = &["diagrams", "pipeline"];

pub static COMMANDS: &[Command] = &[
    open::COMMAND,
    close::COMMAND,
    focus::COMMAND,
    zoom::COMMAND,
    restart::COMMAND,
    send::COMMAND,
    send_key::COMMAND,
    read::COMMAND,
    screenshot::COMMAND,
    resize::COMMAND,
    r#move::COMMAND,
    arrange::COMMAND,
    rename_tab::COMMAND,
    move_tab::COMMAND,
    workspace::COMMAND,
    window::COMMAND,
    rename_pane::COMMAND,
    task::COMMAND,
    navigate::COMMAND,
    capabilities::COMMAND,
    diagrams::COMMAND,
    Command {
        name: "pipeline",
        group: "Information",
        summary: "Check a pipeline of the software factory, or list the repo's, with each one's worst case.",
        usage: "kanna pipeline list\n       kanna pipeline check NAME|FILE",
        options: &[],
        notes: "A pipeline is .kanna/pipelines/NAME.kojo in your repo: stages (an agent session, run COMMAND, ask \"QUESTION\", merge main or merge STAGE) joined by arrows, with a budget (docs/specs/kojo.md). check reads one, by its name in the repo you're in or by its file, with the prompts in .kanna/prompts beside it, and prints each error as FILE:LINE: message, or `valid:` with its stages and its worst case: the most agent sessions any sequence of verdicts could take, against the budget. A fan-out (each STAGE.KEY) counts as 2 parts until a run knows the list. list checks every pipeline in the repo and prints one line each.\nAn `on MACHINE` that isn't this machine or one of your machines (kanna machine list) is a warning here and an error when a run starts. Needs no Kanna.app; it asks the daemon for your machines when one is running.",
        examples: &[
            ("kanna pipeline check feature", "Check .kanna/pipelines/feature.kojo of this repo."),
            ("kanna pipeline check ~/work/app/.kanna/pipelines/bugfix.kojo --json", "Check a file; its errors, warnings and worst case as JSON."),
            ("kanna pipeline list", "The repo's pipelines, each valid with its worst case, or how many errors it has."),
        ],
        parse: |args| match args.positional().as_deref() {
            Some("list") => Ok(json!({ "action": "list" })),
            Some("check") => {
                let pipeline = args.positional().ok_or_else(|| usage("pipeline", "check needs a NAME or a FILE"))?;
                Ok(json!({ "action": "check", "pipeline": pipeline }))
            }
            Some(other) => Err(usage("pipeline", &format!("unknown action \"{other}\" (list or check)"))),
            None => Err(usage("pipeline", "missing list or check")),
        },
        show: |_, _| String::new(),
        flow: Flow { offline: Some(pipeline_offline), ..Flow::PLAIN },
    },
    run::COMMAND,
    status::COMMAND,
    toast::COMMAND,
    Command {
        name: "serial",
        group: "Panes",
        summary: "List serial ports, and read, write, reset or borrow the port a serial pane has open.",
        usage: "kanna serial list\n       kanna serial read PANE [--since T|--last N] [--follow] [--until REGEX [--timeout S]] [--raw]\n       kanna serial send PANE TEXT [--hex] [--no-ending]\n       kanna serial reset PANE [--hold MS]\n       kanna serial set PANE [--baud N] [--line-ending lf|crlf|cr]\n       kanna serial borrow PANE|PORT -- CMD...\n       kanna serial log PANE",
        options: &[
            ("--since T", "read: rows from T on, a time today (14:37:40.118, as send and reset print) or a duration back (10s, 2m)."),
            ("--last N", "read: the last N rows."),
            ("--follow", "read: keep printing rows as they come, through unplugging, until interrupted."),
            ("--until REGEX", "read: follow until a line the device sent matches, print it and exit 0; exit 1 after --timeout."),
            ("--timeout S", "read --until: seconds to wait (default 30)."),
            ("--raw", "read: only what the device sent, as its bytes."),
            ("--hex", "send: TEXT is hex bytes (1b 5b 41), sent with no line ending."),
            ("--no-ending", "send: leave off the pane's line ending."),
            ("--hold MS", "reset: how long to hold reset (default 100)."),
        ],
        notes: "Kanna's daemon owns each port, so a serial pane (kanna open serial PORT), these commands and an agent share one connection. Rows print as TIME TEXT; → marks a line sent to the device and # one from Kanna (connected, unplugged, back, reset, lent). read without --since or --last prints the rows kept in memory (the last 10,000); with --follow or --until and neither, it starts from now, so an old boot banner doesn't match. Bytes that aren't text show as \\xNN. Every row is also appended to the port's log file in the state folder (kanna serial log PANE), to grep a whole session.\nborrow lends the port to CMD, a flash for example: the pane lets go of the port, CMD runs here, and the pane takes the port back when CMD exits (or this command is killed); it exits with CMD's code. With PORT (a path or USB vid:pid), a script needn't know the pane; with no pane open on the port it just runs CMD.\nsend, reset and set fail with unplugged or lent while the port isn't there to use.",
        examples: &[
            ("kanna serial list", "The ports plugged in, by USB name, and the panes that have each open."),
            ("kanna serial read serial-1 --until 'app: ready|Guru Meditation' --timeout 20", "Wait for the firmware to boot, or to crash."),
            ("kanna serial send serial-1 'wifi status'", "Type a line to the device's console."),
            ("kanna serial borrow serial-1 -- idf.py flash", "Flash while the pane lends the port, and watch it boot again after."),
        ],
        parse: crate::serial::parse,
        show: |_, _| String::new(),
        flow: Flow { online: Some(serial_online), ..Flow::PLAIN },
    },
    crashes::COMMAND,
    quiz::COMMAND,
    stats::COMMAND,
    projects::COMMAND,
    schedule::COMMAND,
    project::COMMAND,
    Command {
        name: "machine",
        group: "Information",
        summary: "Your machines: pair one, list them, check one's link, give an address or a relay, or revoke one. kanna --host MACHINE COMMAND runs any command on one.",
        usage: "kanna machine [list]\n       kanna machine pair\n       kanna machine pair CODE [--peer KEY[@HOST[:PORT]]]\n       kanna machine name NAME\n       kanna machine ping MACHINE\n       kanna machine addr MACHINE HOST[:PORT]\n       kanna machine relay URL [--token-file FILE]|none\n       kanna machine revoke MACHINE\n       kanna --host MACHINE COMMAND ...",
        options: &[
            ("--peer KEY[@HOST[:PORT]]", "pair CODE: the machine to pair with, as `kanna machine pair` prints it there, when it isn't on this LAN."),
            ("--host MACHINE", "Before any command: run it on that machine (its name or key) instead of this one. KANNA_HOST does the same."),
        ],
        notes: "Each machine's daemon has a key of its own (device.key in its state folder) and listens for the others over iroh, on UDP port 7470 when it's free. Machines on a LAN find each other by themselves.\nTo add a machine, run `kanna machine pair` on it: it shows a six-digit code and waits. On one of your machines (or the first one, which starts them), run `kanna machine pair CODE`; Kanna.app there asks you to confirm, and the new machine joins. A code works once, for five minutes, and three wrong codes close it.\nA machine is known by its name, the first part of its host name unless `kanna machine name` changed it before pairing. A name that comes back with another key is refused; a revoked machine is never let back in, under any name, and has to be paired with a new key.\n`kanna machine addr` gives an address to try for a machine that isn't on this LAN, such as one on your VPN. ping opens a connection and one stream of each kind, and says whether the path is direct.\n`kanna machine relay URL` names the iroh-relay this machine meets the others through when they aren't on its LAN; run it on each machine, and the app dials them through it too. A relay that admits only holders of a secret (iroh-relay's access.shared_token) takes it with --token-file FILE; the token stays in this machine's kanna.db and is never put in the peer log. iroh moves to a direct path by itself whenever one opens. `none` goes back to the LAN and given addresses only.",
        examples: &[
            ("kanna machine list", "This machine, your other machines and how each is reached."),
            ("kanna machine pair", "On a new machine: show a code to pair it with."),
            ("kanna machine pair 042917", "On your Mac: add the machine showing 042917."),
            ("kanna machine ping r720-1", "Whether r720-1 answers, by which path, and that each kind of stream opens."),
            ("kanna machine addr r720-1 10.0.0.5", "Also try r720-1 at 10.0.0.5, port 7470."),
            ("kanna machine relay https://relay.example.com:8443", "Meet your other machines through your own relay when away."),
            ("kanna machine revoke old-laptop", "Shut old-laptop out of every machine for good."),
        ],
        parse: parse_machine,
        show: output::machine,
        flow: Flow { online: Some(machine_online), ..Flow::PLAIN },
    },
    Command {
        name: "accounts",
        group: "Information",
        summary: "Your Claude and Codex accounts for the agents on all your machines: add one once, on your Mac, and every machine's agents use it.",
        usage: "kanna accounts [list]\n       kanna accounts add claude|codex [--api-key] [--label TEXT]\n       kanna accounts remove ACCOUNT|claude|codex\n       kanna accounts sign-in claude|codex",
        options: &[
            ("--api-key", "add: an API key (Anthropic Console or OpenAI) instead of your subscription."),
            ("--label TEXT", "add: a name to know the account by."),
        ],
        notes: "add runs the agent's own sign-in. For Claude, claude setup-token gives a token that lasts a year; for Codex, codex login signs in to ChatGPT in a folder of Kanna's, never touching your everyday login. What it yields is kept in this Mac's Keychain (in the state folder's agent-secrets on Linux), never in a login file. A token or API key goes to each of your machines when you add it, or when a machine joins, and stays there; a ChatGPT login stays on the machine you added it on, which hands each Codex short-lived access tokens and refreshes them, one refresh at a time. Each agent uses one account: adding another replaces it. remove takes it off every machine, as revoking a machine (kanna machine revoke) takes back what it was given. sign-in is the fallback: this machine signs in to the agent by itself, with the sign-in page opened in your browser on the Mac you're at. Whether a subscription may run agents on several of your machines is Anthropic's and OpenAI's call: see docs/specs/agent-sign-in.md.",
        examples: &[
            ("kanna accounts", "Your accounts, which machine holds each, and the machines that get them."),
            ("kanna accounts add claude", "Sign in to your Claude subscription once, for every machine's agents."),
            ("kanna accounts add codex", "Sign in to ChatGPT once; this Mac holds the login and hands out access tokens."),
            ("kanna accounts add codex --api-key", "Use an OpenAI API key instead."),
            ("kanna accounts remove claude", "Take the Claude account off every machine."),
            ("kanna accounts sign-in codex", "On a machine without the shared account: sign it in to Codex by itself."),
        ],
        parse: parse_accounts,
        show: output::accounts,
        flow: Flow { offline: Some(accounts_offline), ..Flow::PLAIN },
    },
    Command {
        name: "agents",
        group: "Information",
        summary: "Set up your agents for Kanna: Kanna's skills for Claude and Codex, a hook that tells them they're in Kanna, and Claude's sandbox letting them reach Kanna but not read its device keys.",
        usage: "kanna agents [status]\n       kanna agents setup [--ask]\n       kanna agents update [--ask] [--force]\n       kanna agents remove",
        options: &[
            ("--ask", "Say what it would do and ask first; says nothing if there's nothing to ask (the planning terminal runs it)."),
            ("--force", "update: put Kanna's version over files you've changed."),
        ],
        notes: "setup copies Kanna's skills (kanna-tasks, kanna-planning, kanna-new-project, kanna-markup, kanna-explain, kanna-quiz) to ~/.agents/skills, where Codex reads them, links each from ~/.claude/skills for Claude, adds Kanna's socket to sandbox.network.allowUnixSockets in ~/.claude/settings.json, and denies reading every instance's device.key there (sandbox.filesystem.denyRead and a Read rule in permissions.deny). update brings the skills up to this Kanna's: a file you've changed is merged with Kanna's changes, and where they conflict it offers your merge editor (git's merge.tool, else opendiff). remove takes back what Kanna put there. Setup and update also take out the SessionStart hooks earlier Kannas added; Kanna tells the agents it starts that they're in Kanna in their first message. Kanna's planning terminal asks before setting up and before each update; nothing is written until you agree.",
        examples: &[
            ("kanna agents", "Whether your agents are set up, and whether this Kanna has newer skills."),
            ("kanna agents setup", "Install Kanna's skills, the hook and the sandbox entry."),
            ("kanna agents update", "Bring the skills up to this Kanna's, merging your changes."),
            ("kanna agents remove", "Take them back out."),
        ],
        parse: parse_agents,
        show: output::agents,
        flow: Flow { online: Some(agents_online), ..Flow::PLAIN },
    },
    markup::COMMAND,
    comment::COMMAND,
    tour::COMMAND,
];

fn parse_agents(args: &mut Argv) -> Result<Value, Failure> {
    let force = args.flag("--force");
    let ask = args.flag("--ask");
    let action = args.positional();
    let request = match action.as_deref() {
        None | Some("status") => json!({ "action": "status" }),
        Some("setup") => json!({ "action": "setup", "ask": ask }),
        Some("update") => json!({ "action": "update", "ask": ask, "force": force }),
        Some("remove") => json!({ "action": "remove" }),
        Some(other) => return Err(usage("agents", &format!("unknown action \"{other}\" (status, setup, update or remove)"))),
    };
    if force && request["action"] != "update" || ask && !matches!(action.as_deref(), Some("setup" | "update")) {
        return Err(usage("agents", "--ask goes with setup or update, --force with update"));
    }
    Ok(request)
}

fn parse_accounts(args: &mut Argv) -> Result<Value, Failure> {
    let api_key = args.flag("--api-key");
    let label = args.option("--label")?;
    let agent = |what: &str, args: &mut Argv| match args.positional().as_deref() {
        Some(agent @ ("claude" | "codex")) => Ok(agent.to_string()),
        _ => Err(usage("accounts", &format!("accounts {what} needs an agent: claude or codex"))),
    };
    let request = match args.positional().as_deref() {
        None | Some("list") => json!({ "action": "list" }),
        Some("add") => json!({ "action": "add", "agent": agent("add", args)?, "api_key": api_key, "label": label, "local": "interactive" }),
        Some("sign-in") => json!({ "action": "sign-in", "agent": agent("sign-in", args)?, "local": "interactive" }),
        Some("remove") => json!({ "action": "remove", "account": args.positional().ok_or_else(|| usage("accounts", "accounts remove needs an ACCOUNT: its ID, or claude or codex"))? }),
        Some(other) => return Err(usage("accounts", &format!("unknown action \"{other}\" (list, add, remove or sign-in)"))),
    };
    if (api_key || label.is_some()) && request["action"] != "add" {
        return Err(usage("accounts", "--api-key and --label go with add"));
    }
    Ok(request)
}

fn parse_machine(args: &mut Argv) -> Result<Value, Failure> {
    let peer = args.option("--peer")?;
    let token_file = args.option("--token-file")?;
    let action = args.positional();
    let machine = |what: &str, args: &mut Argv| args.positional().ok_or_else(|| usage("machine", &format!("machine {what} needs a MACHINE: its name or key")));
    let request = match action.as_deref() {
        None | Some("list") => json!({ "action": "status" }),
        Some("pair") => match args.positional() {
            None => json!({ "action": "offer" }),
            Some(code) => json!({ "action": "pair", "code": code, "peer": peer }),
        },
        Some("name") => json!({ "action": "name", "name": args.positional().ok_or_else(|| usage("machine", "machine name needs a NAME"))? }),
        Some(what @ ("ping" | "revoke" | "sync")) => json!({ "action": what, "host": machine(what, args)? }),
        Some("addr") => {
            let host = machine("addr", args)?;
            json!({ "action": "addr", "host": host, "addr": args.positional().ok_or_else(|| usage("machine", "machine addr needs a HOST or HOST:PORT"))? })
        }
        Some("relay") => match args.positional().ok_or_else(|| usage("machine", "machine relay needs a relay's URL, or none"))?.as_str() {
            "none" => json!({ "action": "relay", "url": null }),
            url => {
                let token = match &token_file {
                    Some(path) => Some(std::fs::read_to_string(path).map_err(|e| usage("machine", &format!("can't read the relay's token from {path}: {e}")))?.trim().to_string()),
                    None => None,
                };
                json!({ "action": "relay", "url": url, "token": token })
            }
        },
        Some(other) => return Err(usage("machine", &format!("unknown action \"{other}\" (list, pair, name, ping, addr, relay or revoke)"))),
    };
    if token_file.is_some() && (request["action"] != "relay" || request["url"].is_null()) {
        return Err(usage("machine", "--token-file goes with relay URL"));
    }
    if peer.is_some() && request["action"] != "pair" {
        return Err(usage("machine", "--peer goes with pair CODE"));
    }
    Ok(request)
}

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn similar(name: &str) -> Vec<&'static str> {
    let mut names: Vec<(usize, &str)> =
        COMMANDS.iter().map(|c| (distance(name, c.name), c.name)).filter(|(d, n)| *d <= 2 || n.starts_with(name) || name.starts_with(n)).collect();
    names.sort();
    names.into_iter().map(|(_, n)| n).take(3).collect()
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + usize::from(ca != b[j]));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

pub fn sent_as(name: &str) -> &str {
    match name {
        "workspace" => "list",
        name => name,
    }
}

fn pipeline_offline(request: &Value, invocation: &Invocation) -> Option<Result<(), Failure>> {
    Some(crate::pipeline::run(request, invocation.json_output, invocation.target))
}

fn serial_online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    Some(crate::serial::run(request, mux, json_output))
}

fn machine_online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    (request["action"] == "offer").then(|| crate::machine::offer(mux, json_output))
}

fn accounts_offline(request: &Value, invocation: &Invocation) -> Option<Result<(), Failure>> {
    (request["local"] == "interactive").then(|| {
        let mut mux = Session::connect(invocation.target, invocation.env)?;
        mux.host = invocation.host.clone();
        crate::accounts::run(request, &mut |cmd, args| mux.call(cmd, args), invocation.json_output)
    })
}

fn agents_online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    if json_output || !matches!(request["action"].as_str(), Some("setup" | "update")) {
        return None;
    }
    let tool = crate::agents::merge_tool();
    let mut call = |args: Value| mux.call("agents", args);
    let mut merge = |path: &std::path::Path, base: &std::path::Path, new: &std::path::Path| crate::agents::merge_in_editor(path, base, new, &tool);
    let stdin = std::io::stdin();
    let mut io = crate::agents::Io { call: &mut call, input: &mut stdin.lock(), out: &mut std::io::stdout(), merge: &mut merge, tool: tool.clone() };
    Some(crate::agents::run(request, &mut io))
}

pub fn lookup(name: &str) -> Result<&'static Command, Failure> {
    find(name).ok_or_else(|| {
        let similar = similar(name);
        let hint = if similar.is_empty() { String::new() } else { format!(" Did you mean: {}?", similar.join(", ")) };
        fail(exit::USAGE, format!("unknown command \"{name}\".{hint} Run `kanna --help` for all commands."))
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::output;
    use kanna_client::exit;
    use serde_json::json;

    pub fn shell_words(line: &str) -> Vec<String> {
        let mut words = vec![];
        let mut word = String::new();
        let mut quote: Option<char> = None;
        let mut started = false;
        for c in line.chars() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), c) => word.push(c),
                (None, '"' | '\'') => {
                    quote = Some(c);
                    started = true;
                }
                (None, ' ') => {
                    if started || !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                    started = false;
                }
                (None, c) => word.push(c),
            }
        }
        if started || !word.is_empty() {
            words.push(word);
        }
        words
    }

    pub fn parse(line: &str) -> Result<(String, Value), Failure> {
        // Run from a Kanna terminal, a command would name its pane as the caller.
        std::env::remove_var("KANNA_TERMINAL");
        let words = shell_words(line);
        assert_eq!(words[0], "kanna");
        let mut args = Argv::new(words[1..].to_vec());
        args.flag("--json");
        let name = args.positional().unwrap();
        let command = lookup(&name)?;
        let request = (command.parse)(&mut args)?;
        if let Some(extra) = args.first() {
            return Err(usage(command.name, &format!("unexpected argument \"{extra}\"")));
        }
        Ok((command.name.to_string(), request))
    }

    #[test]
    fn every_documented_example_parses() {
        for command in COMMANDS {
            assert!(!command.examples.is_empty(), "{} has no examples", command.name);
            for (example, _) in command.examples {
                let (name, _) = parse(example).unwrap_or_else(|f| panic!("{example}: {}", f.message));
                assert_eq!(name, command.name, "{example}");
            }
        }
    }

    #[test]
    fn requests() {
        let cases = [
            ("kanna open", json!({ "type": "term" })),
            ("kanna open --name logs --split right --size 1/3 --cmd 'tail -f a.log'", json!({ "type": "term", "name": "logs", "split": "right", "size": "1/3", "cmd": "tail -f a.log" })),
            ("kanna open web localhost:3000 --split down", json!({ "type": "web", "url": "localhost:3000", "split": "down" })),
            ("kanna open --tab --no-wait", json!({ "type": "term", "tab": true, "wait": false })),
            ("kanna close logs", json!({ "pane": "logs" })),
            ("kanna close --window win-2", json!({ "window": "win-2" })),
            ("kanna rename-tab tab-1 my servers", json!({ "tab": "tab-1", "title": "my servers" })),
            ("kanna send server npm test", json!({ "pane": "server", "text": "npm test" })),
            ("kanna move logs --to server --side bottom", json!({ "pane": "logs", "to": "server", "side": "bottom" })),
            ("kanna move-tab tab-2 --index 0", json!({ "tab": "tab-2", "index": 0 })),
            ("kanna open web a.test --history", json!({ "type": "web", "url": "a.test", "history": true })),
            ("kanna navigate site --back", json!({ "pane": "site", "back": true })),
            ("kanna open md /tmp/a.md", json!({ "type": "md", "path": "/tmp/a.md" })),
            ("kanna navigate spec /tmp/b.md", json!({ "pane": "spec", "path": "/tmp/b.md" })),
            ("kanna read s1", json!({ "pane": "s1" })),
            ("kanna read s1 --lines 5", json!({ "pane": "s1", "lines": 5 })),
            ("kanna send p1 -- --json", json!({ "pane": "p1", "text": "--json" })),
            ("kanna send-key agent return", json!({ "pane": "agent", "keys": ["return"] })),
            ("kanna send-key p3 up up shift+tab ctrl+c", json!({ "pane": "p3", "keys": ["up", "up", "shift+tab", "ctrl+c"] })),
            ("kanna send p1 echo -- --mux -x", json!({ "pane": "p1", "text": "echo --mux -x" })),
            ("kanna open --cwd /tmp", json!({ "type": "term", "cwd": "/tmp" })),
            ("kanna move agent --dock", json!({ "pane": "agent", "dock": "left" })),
            ("kanna move agent --dock right --window win-2", json!({ "pane": "agent", "dock": "right", "window": "win-2" })),
            ("kanna move build --background", json!({ "pane": "build", "background": true })),
            ("kanna move agent --tab", json!({ "pane": "agent", "tab": "new" })),
            ("kanna move agent --tab tab-3", json!({ "pane": "agent", "tab": "tab-3" })),
            ("kanna move agent --tab --window win-2", json!({ "pane": "agent", "tab": "new", "window": "win-2" })),
            ("kanna open code /tmp/a.rs:42", json!({ "type": "code", "path": "/tmp/a.rs", "line": 42 })),
            ("kanna open code /tmp/a.rs", json!({ "type": "code", "path": "/tmp/a.rs" })),
            ("kanna open code /tmp/a.rs:x", json!({ "type": "code", "path": "/tmp/a.rs:x" })),
            ("kanna navigate src /etc/hosts:3", json!({ "pane": "src", "path": "/etc/hosts", "line": 3 })),
            ("kanna navigate src /etc/hosts", json!({ "pane": "src", "path": "/etc/hosts" })),
            ("kanna navigate site localhost:5173", json!({ "pane": "site", "url": "localhost:5173" })),
        ];
        for (line, expected) in cases {
            let (_, request) = parse(line).unwrap_or_else(|f| panic!("{line}: {}", f.message));
            assert_eq!(request, expected, "{line}");
        }
    }

    #[test]
    fn usage_errors_say_what_to_do() {
        for (line, hint) in [
            ("kanna close", "PANE, --tab TAB or --window WINDOW"),
            ("kanna zoom", "missing PANE"),
            ("kanna open top", "--cmd \"top\""),
            ("kanna open web", "need a URL"),
            ("kanna move logs", "say where"),
            ("kanna zoom a b", "unexpected argument \"b\""),
            ("kanna open --history", "only for web panes"),
            ("kanna open md", "need a file"),
            ("kanna open notes.md", "kanna open md notes.md"),
            ("kanna navigate site --back x", "one of URL"),
            ("kanna workspce list", "Did you mean: workspace"),
            ("kanna rename tab-1 x", "Did you mean: rename-tab"),
            ("kanna read", "missing PANE"),
            ("kanna send-key", "missing PANE"),
            ("kanna send-key p1", "missing KEY"),
            ("kanna read p1 --lines 0", "--lines needs a whole number"),
            ("kanna open ios", "need an app"),
            ("kanna open --app x", "only for iOS panes"),
            ("kanna open --name agent --dock", "make a task with `kanna task new`"),
            ("kanna open --dock right --cmd claude", "a docked pane is a task's agent"),
            ("kanna open web a.test --dock=right --window new", "a docked pane is a task's agent"),
            ("kanna open --frozen", "a docked pane is a task's agent"),
            ("kanna markup a.md --dock", "a docked pane is a task's agent"),
            ("kanna move agent --dock --tab", "--dock goes on its own"),
            ("kanna move agent --dock --to b", "--dock goes on its own"),
            ("kanna move build --background --tab", "--background goes on its own"),
        ] {
            let failure = parse(line).expect_err(line);
            assert_eq!(failure.code, exit::USAGE, "{line}");
            assert!(failure.message.contains(hint), "{line}: {}", failure.message);
        }
    }

    #[test]
    fn relative_paths_are_made_absolute_here() {
        let (_, request) = parse("kanna open md docs/a.md --cwd sub").unwrap();
        assert!(request["path"].as_str().unwrap().starts_with('/'));
        assert!(request["cwd"].as_str().unwrap().starts_with('/'));
    }

    #[test]
    fn layouts_print_as_expressions() {
        let layout = json!({ "split": "row", "children": [
            { "pane": "a", "size": "2/3" },
            { "split": "column", "size": "1/3", "children": [{ "pane": "b", "size": "1/2" }, { "pane": "c", "size": "1/2" }] }
        ] });
        assert_eq!(output::expression(&layout, true), "a:2/3 | (b:1/2 / c:1/2):1/3");
    }

    #[test]
    fn docked_panes_print_on_their_own_line() {
        let all = json!({ "action": "list", "all": true });
        let reply = json!({
            "windows": [],
            "workspaces": [{ "id": "ws-1", "where": { "desk": "mac" }, "dock": { "pane": "agent", "id": "p1", "edge": "left", "width": "2/5", "frozen": true }, "focused": "agent",
                             "tabs": [{ "id": "tab-1", "title": "Tab 1", "active": true, "layout": { "pane": "p2" } }], "windows": [], "panes": ["p2", "p1"] }],
            "panes": [{ "id": "p1", "name": "agent", "type": "term", "state": "running", "cmd": "claude" }, { "id": "p2", "name": null, "type": "term", "state": "running" }]
        });
        let text = crate::commands::workspace::show(&all, &reply);
        assert!(text.contains("\nws-1, on the Desk\n  dock: p1 (agent)  left 2/5  frozen\n  focus: agent\n  *tab-1 \"Tab 1\"  p2"), "{text}");
        let opened = json!({ "pane": { "id": "p1", "name": "agent", "state": "running" }, "window": null, "tab": null, "dock": "left", "workspace": { "id": "ws-1", "where": { "desk": "mac" } } });
        assert_eq!(crate::commands::open::show(&json!({}), &opened), "opened p1 (agent) docked left in ws-1 (on the Desk): running");
        assert_eq!(crate::commands::open::show(&json!({ "frozen": true }), &opened), "opened p1 (agent) docked left, frozen, in ws-1 (on the Desk): running");
        let moved = json!({ "window": null, "workspace": { "id": "ws-1" }, "dock": { "pane": "agent", "edge": "right", "width": "1/3" } });
        assert_eq!(output::moved(&moved), "agent is docked right in ws-1 (width 1/3)");
    }

}
