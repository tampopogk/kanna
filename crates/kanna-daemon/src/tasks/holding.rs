use super::*;

pub const ASK_AFTER: Duration = Duration::from_secs(5 * 60);

const SHELLS: &[&str] = &["zsh", "bash", "sh", "dash", "fish"];

const LABEL_MAX: usize = 60;

pub fn ask_after() -> Duration {
    std::env::var("KANNA_ASK_AFTER").ok().and_then(|s| s.parse().ok()).map_or(ASK_AFTER, Duration::from_secs)
}

#[derive(Clone, Debug)]
pub struct Holding {
    pub processes: Vec<Process>,
    pub wait: Duration,
    pub next: Instant,
    pub asked: u32,
    pub reason: Option<String>,
    pub question: Option<i64>,
}

impl Holding {
    fn new(now: Instant) -> Holding {
        Holding { processes: vec![], wait: ask_after(), next: now + ask_after(), asked: 0, reason: None, question: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contended {
    pub free_one: bool,
    pub waiting: Vec<(String, usize, usize)>,
}

impl Contended {
    pub fn for_repo(&self, repo: &str) -> bool {
        self.free_one && self.waiting.iter().any(|(r, in_repo, slots)| in_repo.saturating_sub(usize::from(r == repo)) < *slots)
    }
}

fn program(process: &Process) -> String {
    let base = |s: &Path| s.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty());
    process.argv.first().and_then(|a| base(Path::new(a))).or_else(|| process.executable.as_deref().and_then(base)).unwrap_or_else(|| format!("pid {}", process.pid))
}

fn label(process: &Process) -> String {
    let name = program(process);
    let args = process.argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
    let full = if args.is_empty() { name.clone() } else { format!("{name} {args}") };
    if full.chars().count() <= LABEL_MAX { full } else { name }
}

pub fn shown(processes: &[Process]) -> Vec<(&Process, String)> {
    let shell = |p: &Process| SHELLS.contains(&program(p).trim_start_matches('-'));
    let named: Vec<&Process> = processes.iter().filter(|p| !shell(p)).collect();
    let pool = if named.is_empty() { processes.iter().collect() } else { named };
    let mut kept: Vec<(&Process, String)> = vec![];
    for process in pool {
        let name = program(process);
        if !kept.iter().any(|(_, k)| name.starts_with(k.as_str())) {
            kept.push((process, name));
        }
    }
    kept
}

pub fn question(processes: &[Process]) -> String {
    let named: Vec<String> = shown(processes).into_iter().map(|(p, _)| format!("{} (pid {})", label(p), p.pid)).collect();
    format!(
        "You ended your turn with these still running: {}. Another task is waiting for your runner. If you're waiting on them, run `kanna task wait \"WHAT FOR\"`; if not, run `kanna task yield`.",
        named.join(", ")
    )
}

impl Tasks {
    pub(super) fn held_by(&self, pane: &str, found: &[Process], started: bool, now: Instant) {
        let names = |processes: &[Process]| shown(processes).into_iter().map(|(_, n)| n).collect::<Vec<_>>();
        let mut holding = self.holding.borrow_mut();
        let (changed, gone) = if found.is_empty() {
            let gone = holding.remove(pane);
            (gone.as_ref().is_some_and(|h| !h.processes.is_empty()), gone)
        } else {
            let held = holding.entry(pane.to_string()).or_insert_with(|| Holding::new(now));
            let before = names(&held.processes);
            held.processes = found.to_vec();
            (started || before != names(found), None)
        };
        drop(holding);
        if let Some(gone) = gone {
            self.withdraw(&gone);
        }
        if changed {
            self.changed();
        }
    }

    pub(super) fn holding_of(&self, entry: &Entry) -> Option<Value> {
        let pane = entry.held.as_deref()?;
        if !self.turns.borrow().get(pane).is_some_and(Turn::waiting) {
            return None;
        }
        let holding = self.holding.borrow();
        let held = holding.get(pane).filter(|h| !h.processes.is_empty())?;
        let on: Vec<String> = shown(&held.processes).into_iter().map(|(_, n)| n).collect();
        Some(json!({ "on": on, "for": opt(held.reason.as_deref()), "asked": held.asked }))
    }

    pub(super) fn ask_holders(&self, running: &[(String, Entry)], now: Instant) {
        let gone: Vec<Holding> = {
            let mut holding = self.holding.borrow_mut();
            let left: Vec<String> = holding.keys().filter(|pane| !running.iter().any(|(p, _)| p == *pane)).cloned().collect();
            left.iter().filter_map(|pane| holding.remove(pane)).collect()
        };
        gone.iter().for_each(|h| self.withdraw(h));
        let due: Vec<(String, Entry)> = running
            .iter()
            .filter(|(pane, _)| self.holding.borrow().get(pane).is_some_and(|h| h.next <= now && !h.processes.is_empty()))
            .filter(|(pane, _)| self.turns.borrow().get(pane).is_some_and(Turn::waiting))
            .filter(|(_, entry)| !self.picking.borrow().contains(&entry.id) && !self.waiting.borrow().iter().any(|m| m.recipient == entry.id))
            .filter(|(_, entry)| self.contended.borrow().for_repo(&entry.repo))
            .cloned()
            .collect();
        for (pane, entry) in due {
            let Some(processes) = self.holding.borrow().get(&pane).map(|h| h.processes.clone()) else { continue };
            let Some(message) = self.tell(&entry.id, question(&processes)) else { continue };
            let wait = {
                let mut holding = self.holding.borrow_mut();
                let Some(held) = holding.get_mut(&pane) else { continue };
                held.question = Some(message);
                held.asked += 1;
                held.wait *= 2;
                held.next = now + held.wait;
                held.wait
            };
            let named: Vec<String> = shown(&processes).into_iter().map(|(_, n)| n).collect();
            log!("kanna-daemon: {} holds its runner with {} running while another task waits, so it's asked whether it needs them (again in {}m)", entry.id, named.join(", "), wait.as_secs() / 60);
            self.note(&pane, Change::new("asked").by(Some("daemon")).with("why", json!("runner")).with("processes", json!(named)));
        }
    }

    fn withdraw(&self, holding: &Holding) {
        let Some(id) = holding.question else { return };
        let before = self.waiting.borrow().len();
        self.waiting.borrow_mut().retain(|m| m.id != id);
        if self.waiting.borrow().len() < before {
            log!("kanna-daemon: the question about its runner is withdrawn, undelivered: its task no longer holds a runner with background work");
            if let Err(error) = self.db.withdraw_message(id) {
                log!("kanna-daemon: couldn't withdraw message {id} in {}: {error}", db::FILE);
            }
        }
    }

    fn holder(&self, args: &Value, what: &str) -> Result<(Entry, String), KmuxError> {
        let id = string(args, "task").ok_or_else(|| bad(format!("kanna task {what} is for a Shelf task's own terminal")))?;
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else {
            return Err(KmuxError::new("not_on_shelf", format!("{} isn't running on the Shelf, so it holds no runner", self.short(id))));
        };
        let core = self.core.upgrade().ok_or_else(gone)?;
        let pane = entry.held.clone().filter(|pane| alive(&core, pane)).ok_or_else(|| KmuxError::new("stopped", format!("{} is stopped, so it holds no runner", self.short(id))))?;
        Ok((entry, pane))
    }

    pub(super) fn wait_on(&self, args: &Value) -> Result<Map, KmuxError> {
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("say what you're waiting for: kanna task wait \"WHAT FOR\""))?.to_string();
        let (entry, pane) = self.holder(args, "wait")?;
        let now = Instant::now();
        let again = {
            let mut holding = self.holding.borrow_mut();
            let held = holding.entry(pane.clone()).or_insert_with(|| Holding::new(now));
            if held.asked == 0 {
                held.wait *= 2;
            }
            held.next = now + held.wait;
            held.reason = Some(text.clone());
            held.wait
        };
        log!("kanna-daemon: {} keeps its runner, waiting for {text} (asked again in {}m if another task waits)", entry.id, again.as_secs() / 60);
        self.note(&pane, Change::new("waits").by(Some(&pane)).with("for", json!(text)).with("again", json!(again.as_secs())));
        self.changed();
        Ok(Map::from_iter([("task".to_string(), json!(entry.id)), ("for".to_string(), json!(text)), ("again".to_string(), json!(again.as_secs()))]))
    }

    pub(super) fn yield_runner(self: &Rc<Self>, args: &Value, responder: Responder) {
        let (entry, pane) = match self.holder(args, "yield") {
            Ok(found) => found,
            Err(error) => return responder.reply(Err(error)),
        };
        log!("kanna-daemon: {} yields its runner, so it stops", entry.id);
        let held = self.holding.borrow_mut().remove(&pane);
        if let Some(held) = held {
            self.withdraw(&held);
        }
        let mut args = args.clone();
        args["task"] = json!(entry.id);
        args["why"] = json!(YIELDED);
        if string(&args, "by").is_none() {
            args["by"] = json!(pane);
        }
        self.stop(&args, responder);
    }
}

pub const YIELDED: &str = "yielded";

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: i32, argv: &[&str]) -> Process {
        Process { pid, executable: None, argv: argv.iter().map(|a| a.to_string()).collect() }
    }

    #[test]
    fn what_holds_a_runner_is_named_without_its_shells_and_helpers() {
        let processes = [
            process(10, &["/bin/zsh", "-c", "python3 -m http.server 52688"]),
            process(11, &["python3", "-m", "http.server", "52688"]),
            process(12, &["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "--remote-debugging-pipe", "--user-data-dir=/var/folders/xy/playwright_chromiumdev_profile-abc"]),
            process(13, &["/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)", "--type=renderer"]),
        ];
        let names: Vec<String> = shown(&processes).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["python3", "Google Chrome"]);
        assert_eq!(
            question(&processes),
            "You ended your turn with these still running: python3 -m http.server 52688 (pid 11), Google Chrome (pid 12). Another task is waiting for your runner. If you're waiting on them, run `kanna task wait \"WHAT FOR\"`; if not, run `kanna task yield`."
        );
        let shells = [process(20, &["-zsh"]), process(21, &["zsh", "-c", "eza --icons"])];
        assert_eq!(shown(&shells).into_iter().map(|(_, n)| n).collect::<Vec<_>>(), ["-zsh", "zsh"], "only shells: the shells");
    }

    #[test]
    fn a_runner_freed_helps_a_waiting_task_only_when_the_limits_let_it_start() {
        let full = Contended { free_one: true, waiting: vec![("/a".into(), 1, 1)] };
        assert!(full.for_repo("/a"), "a repo at its cap frees its own place");
        assert!(!full.for_repo("/b"), "another repo's runner doesn't lift /a's cap");
        let total = Contended { free_one: true, waiting: vec![("/a".into(), 0, 3)] };
        assert!(total.for_repo("/b"));
        assert!(!Contended { free_one: false, ..total.clone() }.for_repo("/b"), "more run than the total: one freed starts nothing");
        assert!(!Contended::default().for_repo("/a"), "no one waits");
    }
}
