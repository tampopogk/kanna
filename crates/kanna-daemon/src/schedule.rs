use crate::backlog::Backlog;
use crate::db::{self, Db, Schedule};
use crate::messages::{PLANNER, SCHEDULE};
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const QUIZ: &str = "quiz";

pub const QUIZ_RULE: &str = "daily 09:00";

pub const QUIZ_PROMPT: &str = "Morning quiz check (the kanna-quiz skill's Offering): if anything landed on main since the last quiz round (kanna quiz history), offer me a round; if nothing has, say nothing.";

const EXAMPLES: &str = "\"daily 09:00\", \"weekdays 18:00\", \"mon,thu 10:30\" or \"every 2h\"";

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const DAY: i64 = 86_400;

const LONGEST_WAIT: Duration = Duration::from_secs(60);

pub trait Zone {
    fn offset(&self, at: i64) -> i64;
}

pub struct Local;

extern "C" {
    fn tzset();
}

impl Zone for Local {
    fn offset(&self, at: i64) -> i64 {
        let time = at as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { tzset() };
        if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

fn weekday(day: i64) -> usize {
    (day + 3).rem_euclid(7) as usize
}

fn resolve(day: i64, minute: i64, zone: &dyn Zone) -> i64 {
    let wall = day * DAY + minute * 60;
    let before = zone.offset(wall - DAY);
    let after = zone.offset(wall + DAY);
    [before, after].into_iter().map(|o| wall - o).filter(|t| t + zone.offset(*t) == wall).min().unwrap_or(wall - before)
}

pub fn local_text(at: f64, zone: &dyn Zone) -> String {
    let at = at.floor() as i64;
    let wall = at + zone.offset(at);
    let day = wall.div_euclid(DAY);
    let minute = wall.rem_euclid(DAY) / 60;
    let (_, m, d) = civil_from_days(day);
    let name = DAYS[weekday(day)];
    format!("{}{} {d} {} {:02}:{:02}", name[..1].to_uppercase(), &name[1..], MONTHS[(m - 1) as usize], minute / 60, minute % 60)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    At { days: [bool; 7], minute: i64 },
    Every(i64),
}

fn clock(text: &str) -> Option<i64> {
    let (h, m) = text.split_once(':')?;
    let digits = |s: &str, most: usize| (!s.is_empty() && s.len() <= most && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<i64>().ok()).flatten();
    let (h, m) = (digits(h, 2)?, digits(m, 2).filter(|_| m.len() == 2)?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn interval(text: &str) -> Option<i64> {
    let (count, unit) = text.split_at(text.len().checked_sub(1)?);
    let count: i64 = count.parse().ok().filter(|n| (1..=10_000).contains(n))?;
    match unit {
        "m" => Some(count * 60),
        "h" => Some(count * 3600),
        _ => None,
    }
}

fn day_list(text: &str) -> Option<[bool; 7]> {
    let mut days = [false; 7];
    for name in text.split(',') {
        days[DAYS.iter().position(|d| *d == name)?] = true;
    }
    Some(days)
}

impl Rule {
    pub fn parse(text: &str) -> Result<Rule, String> {
        let refuse = |why: &str| format!("\"{}\" isn't a rule ({why}): try {EXAMPLES}", text.trim());
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let [first, second] = words.as_slice() else { return Err(refuse("a rule is two words")) };
        if first == "every" {
            return interval(second).map(Rule::Every).ok_or_else(|| refuse("every takes minutes or hours, like 30m or 2h"));
        }
        let days = match first.as_str() {
            "daily" => [true; 7],
            "weekdays" => [true, true, true, true, true, false, false],
            "weekends" => [false, false, false, false, false, true, true],
            list => day_list(list).ok_or_else(|| refuse("days are daily, weekdays, weekends, or some of mon,tue,wed,thu,fri,sat,sun"))?,
        };
        let minute = clock(second).ok_or_else(|| refuse("a time is HH:MM, from 00:00 to 23:59"))?;
        Ok(Rule::At { days, minute })
    }

    pub fn text(&self) -> String {
        match self {
            Rule::Every(secs) if secs % 3600 == 0 => format!("every {}h", secs / 3600),
            Rule::Every(secs) => format!("every {}m", secs / 60),
            Rule::At { days, minute } => {
                let named = match days {
                    [true, true, true, true, true, true, true] => "daily".to_string(),
                    [true, true, true, true, true, false, false] => "weekdays".to_string(),
                    [false, false, false, false, false, true, true] => "weekends".to_string(),
                    _ => DAYS.iter().zip(days).filter(|(_, on)| **on).map(|(d, _)| *d).collect::<Vec<_>>().join(","),
                };
                format!("{named} {:02}:{:02}", minute / 60, minute % 60)
            }
        }
    }

    pub fn next(&self, due: f64, now: f64, zone: &dyn Zone) -> f64 {
        match self {
            Rule::Every(secs) => {
                let secs = *secs as f64;
                let next = due + secs;
                if next > now { next } else { next + ((now - next) / secs).floor() * secs + secs }
            }
            Rule::At { days, minute } => {
                let at = now.floor() as i64;
                let today = (at + zone.offset(at)).div_euclid(DAY);
                (today - 1..=today + 8)
                    .filter(|day| days[weekday(*day)])
                    .map(|day| resolve(day, *minute, zone) as f64)
                    .find(|t| *t > now)
                    .unwrap_or(now + DAY as f64)
            }
        }
    }
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(what: &str, error: rusqlite::Error) -> KmuxError {
    log!("kanna-daemon: couldn't {what} in {}: {error}", db::FILE);
    KmuxError::new("failed", format!("couldn't {what}: {error}"))
}

fn id_of(text: &str) -> Option<i64> {
    text.strip_prefix('s').filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).and_then(|n| n.parse().ok())
}

fn label(schedule: &Schedule) -> String {
    schedule.name.clone().unwrap_or_else(|| format!("s{}", schedule.id))
}

pub trait Mailbox {
    fn recipient(&self, target: &str) -> Result<String, KmuxError>;
    fn post(&self, recipient: &str, repo: Option<&str>, from: &str, text: &str) -> Result<i64, KmuxError>;
    fn waiting(&self, message: i64) -> bool;
    fn label(&self, recipient: &str) -> String;
}

impl Mailbox for Backlog {
    fn recipient(&self, target: &str) -> Result<String, KmuxError> {
        Backlog::recipient(self, target)
    }

    fn post(&self, recipient: &str, repo: Option<&str>, from: &str, text: &str) -> Result<i64, KmuxError> {
        Backlog::post(self, recipient, repo, from, text)
    }

    fn waiting(&self, message: i64) -> bool {
        Backlog::waiting(self, message)
    }

    fn label(&self, recipient: &str) -> String {
        self.label_of(recipient)
    }
}

pub fn repo_of(folder: &str) -> Option<String> {
    let found = kanna_mux::project(Some(folder))?;
    found.join(".git").exists().then(|| found.to_string_lossy().into_owned())
}

fn system_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

pub struct Schedules {
    db: Rc<Db>,
    zone: Box<dyn Zone>,
    mailbox: Rc<dyn Mailbox>,
    skew: Cell<f64>,
    known: RefCell<Option<Vec<Schedule>>>,
}

impl Schedules {
    pub fn new(db: Rc<Db>, zone: Box<dyn Zone>, mailbox: Rc<dyn Mailbox>) -> Schedules {
        Schedules { db, zone, mailbox, skew: Cell::new(0.0), known: RefCell::default() }
    }

    pub fn now(&self) -> f64 {
        system_now() + self.skew.get()
    }

    pub fn advance(&self, secs: f64) {
        self.skew.set(self.skew.get() + secs);
    }

    fn all(&self) -> Result<Vec<Schedule>, KmuxError> {
        if let Some(known) = self.known.borrow().as_ref() {
            return Ok(known.clone());
        }
        let all = self.db.schedules().map_err(|e| failed("read the schedules", e))?;
        *self.known.borrow_mut() = Some(all.clone());
        Ok(all)
    }

    fn changed(&self) {
        self.known.borrow_mut().take();
    }

    pub fn handle(&self, args: &Value) -> Result<Map, KmuxError> {
        match args.get("action").and_then(Value::as_str) {
            Some("add") => self.add(args),
            Some("list") => self.list(args),
            Some("remove") => self.remove(args),
            _ => Err(bad("schedule needs an action: add, list or remove")),
        }
    }

    fn repo(&self, args: &Value) -> Result<Option<String>, KmuxError> {
        let Some(folder) = string(args, "repo").filter(|r| !r.is_empty()) else { return Ok(None) };
        if !Path::new(folder).is_absolute() {
            return Err(bad("say which repo: an absolute path to its folder"));
        }
        let repo = repo_of(folder).ok_or_else(|| bad(format!("{folder} isn't in a git repo")))?;
        self.seed(&repo);
        Ok(Some(repo))
    }

    fn add(&self, args: &Value) -> Result<Map, KmuxError> {
        let rule = Rule::parse(string(args, "rule").unwrap_or_default()).map_err(bad)?;
        let target = string(args, "target").filter(|t| !t.is_empty() && !t.contains(char::is_whitespace)).ok_or_else(|| bad("say who to prompt: a task, or planner"))?;
        let prompt = string(args, "prompt").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
        if prompt.is_empty() {
            return Err(bad("say what to prompt the agent with"));
        }
        let name = match string(args, "name") {
            None => None,
            Some(name) if name.is_empty() || name.contains(char::is_whitespace) => return Err(bad("a schedule's name is one word")),
            Some(name) if id_of(name).is_some() => return Err(bad(format!("{name} looks like a schedule's ID; pick another name"))),
            Some(name) => Some(name.to_string()),
        };
        let repo = self.repo(args)?;
        let recipient = self.mailbox.recipient(target)?;
        if recipient == PLANNER && repo.is_none() {
            return Err(bad("planner is a repo's planning agent: run this in the repo, or say which with --repo"));
        }
        if let Some(name) = &name {
            if self.all()?.iter().any(|s| s.repo == repo && s.name.as_deref() == Some(name)) {
                return Err(bad(format!("there's a schedule named {name} here already")));
            }
        }
        let now = self.now();
        let mut doc = json!({});
        if let Some(by) = string(args, "by").filter(|b| !b.is_empty()) {
            doc["by"] = json!(by);
        }
        let mut schedule = Schedule { id: 0, repo, name, target: recipient, rule: rule.text(), prompt, next: rule.next(now, now, &*self.zone), message: None, made_at: now, doc };
        schedule.id = self.db.add_schedule(&schedule).map_err(|e| failed("save the schedule", e))?;
        self.changed();
        Ok(match self.value(&schedule) {
            Value::Object(map) => map,
            _ => Map::new(),
        })
    }

    fn waiting(&self, schedule: &Schedule) -> bool {
        schedule.message.is_some_and(|m| self.mailbox.waiting(m))
    }

    fn value(&self, schedule: &Schedule) -> Value {
        json!({
            "schedule": format!("s{}", schedule.id),
            "name": opt(schedule.name.as_deref()),
            "repo": opt(schedule.repo.as_deref()),
            "rule": schedule.rule,
            "target": self.mailbox.label(&schedule.target),
            "task": schedule.target,
            "prompt": schedule.prompt,
            "next": num(schedule.next),
            "nextText": local_text(schedule.next, &*self.zone),
            "message": opt(schedule.message.map(|m| m.to_string()).as_deref()),
            "waiting": self.waiting(schedule),
        })
    }

    fn list(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = self.repo(args)?;
        let all = self.all()?;
        let schedules = all.iter().map(|s| self.value(s)).collect();
        Ok(Map::from_iter([("repo".to_string(), opt(repo.as_deref())), ("schedules".to_string(), Value::Array(schedules))]))
    }

    fn remove(&self, args: &Value) -> Result<Map, KmuxError> {
        let which = string(args, "schedule").filter(|s| !s.is_empty()).ok_or_else(|| bad("say which schedule: its name or ID (s3)"))?;
        let repo = self.repo(args)?;
        let all = self.all()?;
        let found: Vec<&Schedule> = match id_of(which) {
            Some(id) => all.iter().filter(|s| s.id == id).collect(),
            None => {
                let named: Vec<&Schedule> = all.iter().filter(|s| s.name.as_deref() == Some(which)).collect();
                let here: Vec<&Schedule> = named.iter().copied().filter(|s| s.repo == repo).collect();
                if here.is_empty() { named } else { here }
            }
        };
        let schedule = match found.as_slice() {
            [one] => *one,
            [] => return Err(KmuxError::new("not_found", format!("no schedule {which}"))),
            many => return Err(bad(format!("{which} names {} schedules ({}): say which by ID", many.len(), many.iter().map(|s| format!("s{}", s.id)).collect::<Vec<_>>().join(", ")))),
        };
        self.db.remove_schedule(schedule.id).map_err(|e| failed("remove the schedule", e))?;
        self.changed();
        Ok(Map::from_iter([("schedule".to_string(), json!(format!("s{}", schedule.id))), ("name".to_string(), opt(schedule.name.as_deref()))]))
    }

    pub fn seed(&self, repo: &str) {
        let rule = Rule::parse(QUIZ_RULE).expect("the quiz's rule parses");
        let now = self.now();
        let schedule = Schedule {
            id: 0,
            repo: Some(repo.to_string()),
            name: Some(QUIZ.to_string()),
            target: PLANNER.to_string(),
            rule: rule.text(),
            prompt: QUIZ_PROMPT.to_string(),
            next: rule.next(now, now, &*self.zone),
            message: None,
            made_at: now,
            doc: json!({ "by": "kanna" }),
        };
        match self.db.seed_once(&format!("quiz_schedule:{repo}"), &schedule) {
            Ok(true) => {
                self.changed();
                log!("kanna-daemon: added the daily quiz schedule for {repo}")
            }
            Ok(false) => {}
            Err(error) => log!("kanna-daemon: couldn't add the quiz schedule for {repo}: {error}"),
        }
    }

    pub fn fire(&self) -> usize {
        let now = self.now();
        let Ok(all) = self.all() else { return 0 };
        let due: Vec<&Schedule> = all.iter().filter(|s| s.next <= now).collect();
        if due.is_empty() {
            return 0;
        }
        self.changed();
        let mut fired = 0;
        for schedule in due {
            let rule = match Rule::parse(&schedule.rule) {
                Ok(rule) => rule,
                Err(error) => {
                    log!("kanna-daemon: schedule s{} can't run: {error}", schedule.id);
                    continue;
                }
            };
            let message = match self.waiting(schedule) {
                true => None,
                false => match self.mailbox.post(&schedule.target, schedule.repo.as_deref(), &format!("{SCHEDULE}{}", label(schedule)), &schedule.prompt) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        log!("kanna-daemon: schedule {} couldn't leave its message for {}: {}", label(schedule), schedule.target, error.message);
                        None
                    }
                },
            };
            match self.db.schedule_fired(schedule.id, rule.next(schedule.next, now, &*self.zone), message) {
                Ok(()) => fired += 1,
                Err(error) => log!("kanna-daemon: couldn't fire schedule s{}: {error}", schedule.id),
            }
        }
        fired
    }

    pub fn next_due(&self) -> Option<f64> {
        self.all().ok()?.iter().map(|s| s.next).reduce(f64::min)
    }
}

pub struct Scheduler {
    pub schedules: Schedules,
}

impl Scheduler {
    pub fn new(db: Rc<Db>, backlog: Rc<Backlog>) -> Scheduler {
        let schedules = Schedules::new(db.clone(), Box::new(Local), backlog);
        for repo in db.repos().unwrap_or_default() {
            if let Some(repo) = repo_of(&repo) {
                schedules.seed(&repo);
            }
        }
        Scheduler { schedules }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle, debug: bool) {
        let this = Rc::downgrade(self);
        core.add_command(
            "schedule",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                responder.reply(this.schedules.handle(&args))
            }),
        );
        if !debug {
            return;
        }
        let this = Rc::downgrade(self);
        core.add_command(
            "debug.clock",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                this.schedules.advance(args.get("advance").and_then(Value::as_f64).unwrap_or(0.0));
                responder.reply(Ok(Map::from_iter([("now".to_string(), num(this.schedules.now()))])))
            }),
        );
    }

    pub fn tick(&self) -> Option<Instant> {
        self.schedules.fire();
        let now = Instant::now();
        self.schedules.next_due().map(|at| now + Duration::from_secs_f64((at - self.schedules.now()).max(0.0)).min(LONGEST_WAIT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    struct Eastern;

    const SPRING: i64 = 1_772_953_200;
    const FALL: i64 = 1_793_512_800;

    impl Zone for Eastern {
        fn offset(&self, at: i64) -> i64 {
            if (SPRING..FALL).contains(&at) { -4 * 3600 } else { -5 * 3600 }
        }
    }

    fn wall(y: i64, m: i64, d: i64, h: i64, min: i64) -> f64 {
        let local = days_from_civil(y, m, d) * DAY + h * 3600 + min * 60;
        resolve(local.div_euclid(DAY), local.rem_euclid(DAY) / 60, &Eastern) as f64
    }

    fn utc(y: i64, m: i64, d: i64, h: i64, min: i64) -> f64 {
        (days_from_civil(y, m, d) * DAY + h * 3600 + min * 60) as f64
    }

    #[test]
    fn rules_parse_into_their_plain_words_and_bad_ones_are_refused_with_an_example() {
        for (text, canonical) in [
            ("daily 09:00", "daily 09:00"),
            ("Daily 9:00", "daily 09:00"),
            ("weekdays 18:00", "weekdays 18:00"),
            ("weekends 10:15", "weekends 10:15"),
            ("mon,thu 10:30", "mon,thu 10:30"),
            ("thu,mon 00:00", "mon,thu 00:00"),
            ("mon,tue,wed,thu,fri 07:05", "weekdays 07:05"),
            ("every 2h", "every 2h"),
            ("every 90m", "every 90m"),
            ("every 120m", "every 2h"),
            ("  every   1m ", "every 1m"),
        ] {
            assert_eq!(Rule::parse(text).map(|r| r.text()), Ok(canonical.to_string()), "{text}");
        }
        for (text, why) in [
            ("", "two words"),
            ("daily", "two words"),
            ("daily at 09:00", "two words"),
            ("dialy 09:00", "days are"),
            ("mon,funday 09:00", "days are"),
            ("daily 24:00", "HH:MM"),
            ("daily 9:5", "HH:MM"),
            ("daily 0900", "HH:MM"),
            ("every 2d", "minutes or hours"),
            ("every 0m", "minutes or hours"),
            ("every h", "minutes or hours"),
            ("every -1h", "minutes or hours"),
        ] {
            let error = Rule::parse(text).unwrap_err();
            assert!(error.contains(why) && error.contains("try \"daily 09:00\""), "{text}: {error}");
        }
    }

    #[test]
    fn the_next_time_is_the_rule_s_next_wall_clock_time_across_dst() {
        let daily = Rule::parse("daily 09:00").unwrap();
        let next = |rule: &Rule, now: f64| rule.next(now, now, &Eastern);
        assert_eq!(next(&daily, wall(2026, 3, 7, 10, 0)), utc(2026, 3, 8, 13, 0), "09:00 EDT, the day clocks go forward");
        assert_eq!(next(&daily, wall(2026, 3, 8, 9, 0)), utc(2026, 3, 9, 13, 0), "exactly at the time: the next day");
        assert_eq!(next(&daily, wall(2026, 10, 31, 9, 30)), utc(2026, 11, 1, 14, 0), "09:00 EST, the day clocks go back");
        assert_eq!(next(&daily, wall(2026, 3, 8, 8, 59)), utc(2026, 3, 8, 13, 0));
        let gap = Rule::parse("daily 02:30").unwrap();
        assert_eq!(next(&gap, wall(2026, 3, 7, 12, 0)), utc(2026, 3, 8, 7, 30), "02:30 doesn't happen: 03:30 EDT");
        assert_eq!(local_text(utc(2026, 3, 8, 7, 30), &Eastern), "Sun 8 Mar 03:30");
        let twice = Rule::parse("daily 01:30").unwrap();
        let first = next(&twice, wall(2026, 10, 31, 12, 0));
        assert_eq!(first, utc(2026, 11, 1, 5, 30), "01:30 happens twice: the first, EDT");
        assert_eq!(next(&twice, first), utc(2026, 11, 2, 6, 30), "and not again an hour later");
        let days = Rule::parse("mon,thu 10:30").unwrap();
        assert_eq!(next(&days, wall(2026, 10, 16, 8, 0)), wall(2026, 10, 19, 10, 30), "Friday to Monday");
        assert_eq!(next(&days, wall(2026, 10, 19, 11, 0)), wall(2026, 10, 22, 10, 30), "Monday to Thursday");
        let weekdays = Rule::parse("weekdays 18:00").unwrap();
        assert_eq!(next(&weekdays, wall(2026, 10, 16, 18, 30)), wall(2026, 10, 19, 18, 0), "Friday evening to Monday");
        let every = Rule::parse("every 2h").unwrap();
        let made = wall(2026, 3, 8, 1, 0);
        assert_eq!(every.next(made, made, &Eastern), made + 7200.0, "real time: 01:00 EST + 2h is 04:00 EDT");
        assert_eq!(every.next(made, made + 7200.0 * 3.5, &Eastern), made + 7200.0 * 4.0, "from the last time, past now");
    }

    #[test]
    fn the_mac_s_own_zone_gives_its_offset() {
        let now = system_now() as i64;
        let offset = Local.offset(now);
        assert!(offset.abs() <= 14 * 3600, "{offset}");
        let text = local_text(now as f64, &Local);
        assert_eq!(text.split(' ').count(), 4, "{text}");
    }

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-schedule-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn repo(dir: &Path) -> String {
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        repo_of(&repo.to_string_lossy()).unwrap()
    }

    #[derive(Default)]
    struct Fake {
        db: RefCell<Option<Rc<Db>>>,
        posted: RefCell<Vec<(i64, String, Option<String>, String, String)>>,
        delivered: RefCell<Vec<i64>>,
    }

    impl Mailbox for Fake {
        fn recipient(&self, target: &str) -> Result<String, KmuxError> {
            match target {
                PLANNER => Ok(PLANNER.into()),
                "done" => Err(KmuxError::new("done", "task-dead is done, so it takes no messages")),
                short => Ok(format!("task-{short}")),
            }
        }
        fn post(&self, recipient: &str, repo: Option<&str>, from: &str, text: &str) -> Result<i64, KmuxError> {
            let db = self.db.borrow().clone().expect("the fake's db");
            let id = db.put_message(recipient, repo.unwrap_or("/r"), from, text, None).unwrap().id;
            self.posted.borrow_mut().push((id, recipient.into(), repo.map(String::from), from.into(), text.into()));
            Ok(id)
        }
        fn waiting(&self, message: i64) -> bool {
            self.posted.borrow().iter().any(|p| p.0 == message) && !self.delivered.borrow().contains(&message)
        }
        fn label(&self, recipient: &str) -> String {
            recipient.strip_prefix("task-").unwrap_or(recipient).to_string()
        }
    }

    impl Fake {
        fn deliver_all(&self) {
            let ids: Vec<i64> = self.posted.borrow().iter().map(|p| p.0).collect();
            self.delivered.borrow_mut().extend(ids);
        }
        fn taken(&self) -> Vec<(String, Option<String>, String, String)> {
            self.posted.borrow().iter().map(|(_, to, repo, from, text)| (to.clone(), repo.clone(), from.clone(), text.clone())).collect()
        }
    }

    fn schedules(dir: &Path, mailbox: &Rc<Fake>) -> Schedules {
        let db = Rc::new(Db::open(dir));
        *mailbox.db.borrow_mut() = Some(db.clone());
        Schedules::new(db, Box::new(Eastern), mailbox.clone())
    }

    fn ok(schedules: &Schedules, args: Value) -> Value {
        Value::Object(schedules.handle(&args).unwrap_or_else(|e| panic!("{args}: {}", e.message)))
    }

    #[test]
    fn schedules_are_added_listed_removed_and_survive_a_restart() {
        let dir = folder("restart");
        let repo = repo(&dir);
        let mail = Rc::new(Fake::default());
        let s = schedules(&dir, &mail);
        let added = ok(&s, json!({ "action": "add", "rule": "Weekdays 18:00", "target": "planner", "prompt": "Summarise\n today.", "name": "evening", "repo": repo, "by": "term-3" }));
        assert_eq!((added["schedule"].as_str(), added["rule"].as_str(), added["prompt"].as_str()), (Some("s2"), Some("weekdays 18:00"), Some("Summarise today.")), "s1 is the repo's quiz");
        let task = ok(&s, json!({ "action": "add", "rule": "every 2h", "target": "71af", "prompt": "Run the tests." }));
        assert_eq!((task["task"].as_str(), task["target"].as_str()), (Some("task-71af"), Some("71af")), "a task is kept by its ID");
        let refuse = |args: Value| s.handle(&args).err().unwrap_or_else(|| panic!("{args} worked")).message;
        assert!(refuse(json!({ "action": "add", "rule": "dialy 09:00", "target": "71af", "prompt": "x" })).contains("try \"daily 09:00\""));
        assert!(refuse(json!({ "action": "add", "rule": "daily 09:00", "target": "planner", "prompt": "x" })).contains("--repo"));
        assert!(refuse(json!({ "action": "add", "rule": "daily 09:00", "target": "done", "prompt": "x" })).contains("takes no messages"));
        assert!(refuse(json!({ "action": "add", "rule": "daily 09:00", "target": "71af", "prompt": " " })).contains("what to prompt"));
        assert!(refuse(json!({ "action": "add", "rule": "daily 09:00", "target": "71af", "prompt": "x", "name": "evening", "repo": repo })).contains("already"));
        assert!(refuse(json!({ "action": "add", "rule": "daily 09:00", "target": "71af", "prompt": "x", "name": "s9" })).contains("ID"));
        drop(s);
        let s = schedules(&dir, &mail);
        let listed = ok(&s, json!({ "action": "list" }));
        let names: Vec<(String, String, String)> = listed["schedules"].as_array().unwrap().iter().map(|v| (v["schedule"].as_str().unwrap().into(), v["name"].as_str().unwrap_or("-").into(), v["task"].as_str().unwrap().into())).collect();
        assert_eq!(names, [("s1".into(), "quiz".into(), "planner".into()), ("s2".into(), "evening".into(), "planner".into()), ("s3".into(), "-".into(), "task-71af".into())]);
        assert_eq!(listed["schedules"][0]["rule"], "daily 09:00");
        assert_eq!(ok(&s, json!({ "action": "remove", "schedule": "evening", "repo": repo }))["schedule"], "s2");
        assert_eq!(ok(&s, json!({ "action": "remove", "schedule": "s1" }))["name"], "quiz");
        s.seed(&repo);
        assert_eq!(ok(&s, json!({ "action": "list", "repo": repo }))["schedules"].as_array().unwrap().len(), 1, "a removed quiz schedule doesn't come back");
        assert_eq!(s.handle(&json!({ "action": "remove", "schedule": "evening" })).unwrap_err().code, "not_found");
        assert!(mail.taken().is_empty(), "nothing fired");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_missed_while_kanna_was_away_leaves_one_message_when_it_starts() {
        let dir = folder("missed");
        let mail = Rc::new(Fake::default());
        let s = schedules(&dir, &mail);
        ok(&s, json!({ "action": "add", "rule": "every 1h", "target": "71af", "prompt": "Check in." }));
        ok(&s, json!({ "action": "add", "rule": "daily 09:00", "target": "5e2c", "prompt": "Morning.", "name": "morning" }));
        drop(s);
        let s = schedules(&dir, &mail);
        s.advance(3.0 * DAY as f64);
        assert_eq!(s.fire(), 2, "each fires once, not once per missed time");
        assert_eq!(s.fire(), 0);
        assert_eq!(mail.taken(), [
            ("task-71af".to_string(), None, "schedule s1".to_string(), "Check in.".to_string()),
            ("task-5e2c".to_string(), None, "schedule morning".to_string(), "Morning.".to_string()),
        ]);
        let listed = ok(&s, json!({ "action": "list" }));
        let every = &listed["schedules"][0];
        assert!(every["next"].as_f64().unwrap() > s.now() && every["next"].as_f64().unwrap() <= s.now() + 3600.0, "{every}");
        assert_eq!(every["waiting"], true);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_firing_while_its_last_message_still_waits_leaves_no_second_one() {
        let dir = folder("waits");
        let mail = Rc::new(Fake::default());
        let s = schedules(&dir, &mail);
        ok(&s, json!({ "action": "add", "rule": "every 5m", "target": "71af", "prompt": "Pull main.", "name": "pull" }));
        s.advance(301.0);
        assert_eq!(s.fire(), 1);
        s.advance(300.0);
        assert_eq!(s.fire(), 1, "it fires, but its agent hasn't taken the last one yet");
        assert_eq!(mail.taken().len(), 1, "one message, not a pile");
        assert_eq!(ok(&s, json!({ "action": "list" }))["schedules"][0]["waiting"], true);
        mail.deliver_all();
        assert_eq!(ok(&s, json!({ "action": "list" }))["schedules"][0]["waiting"], false);
        s.advance(300.0);
        s.fire();
        assert_eq!(mail.taken().len(), 2, "taken, so the next firing leaves another");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_planner_schedule_leaves_its_message_for_the_repo_s_planner() {
        let dir = folder("planner");
        let repo = repo(&dir);
        let mail = Rc::new(Fake::default());
        let s = schedules(&dir, &mail);
        ok(&s, json!({ "action": "add", "rule": "every 1h", "target": "planner", "prompt": "Plan.", "name": "plan", "repo": repo }));
        s.advance(DAY as f64);
        s.fire();
        let taken = mail.taken();
        assert!(taken.contains(&("planner".to_string(), Some(repo.clone()), "schedule plan".to_string(), "Plan.".to_string())), "{taken:?}");
        assert!(taken.contains(&("planner".to_string(), Some(repo.clone()), "schedule quiz".to_string(), QUIZ_PROMPT.to_string())), "the quiz schedule too: {taken:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_repo_gets_the_quiz_schedule_once() {
        let dir = folder("quiz");
        let repo = repo(&dir);
        let mail = Rc::new(Fake::default());
        let s = schedules(&dir, &mail);
        s.seed(&repo);
        s.seed(&repo);
        let listed = ok(&s, json!({ "action": "list" }));
        let quiz = &listed["schedules"].as_array().unwrap()[..];
        assert_eq!(quiz.len(), 1);
        assert_eq!((quiz[0]["name"].as_str(), quiz[0]["rule"].as_str(), quiz[0]["target"].as_str(), quiz[0]["repo"].as_str()), (Some("quiz"), Some("daily 09:00"), Some("planner"), Some(repo.as_str())));
        assert!(quiz[0]["prompt"].as_str().unwrap().contains("kanna quiz history"));
        assert!(local_text(quiz[0]["next"].as_f64().unwrap(), &Eastern).ends_with("09:00"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
