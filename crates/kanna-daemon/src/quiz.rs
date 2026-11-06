use crate::db::{self, Answer, Db, Ready, Round};
use crate::quiz_map::{self, Inputs, WINDOW_DAYS};
use crate::quiz_round::{self, Source};
use crate::quiz_writer::{Writers, Written};
use crate::tasks::git;
use kanna_mux::json::{json, num, opt, string, Map, Value};
use crate::tasks::{config_files, prompts, started_as, task_agent, QUIZ_ROLE};
use kanna_mux::{project, CoreHandle, KmuxError, PaneState, Responder, WeakCore};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const KINDS: &[&str] = &["architecture", "drawing", "behaviour", "where", "why", "bug"];

pub struct Quiz {
    db: Rc<Db>,
    core: Option<WeakCore>,
    writers: Option<Rc<Writers>>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(what: &str, error: rusqlite::Error) -> KmuxError {
    log!("kanna-daemon: couldn't {what} in {}: {error}", db::FILE);
    KmuxError::new("failed", format!("couldn't {what}: {error}"))
}

fn id_of(prefix: char, value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.strip_prefix(prefix).unwrap_or(s).parse().ok(),
        _ => None,
    }
    .filter(|id| *id > 0)
}

fn doc(args: &Value) -> Result<Value, KmuxError> {
    match args.get("doc") {
        None | Some(Value::Null) => Ok(json!({})),
        Some(doc @ Value::Object(_)) => Ok(doc.clone()),
        Some(_) => Err(bad("the doc is a JSON object")),
    }
}

fn repo(args: &Value) -> Result<&str, KmuxError> {
    string(args, "repo").filter(|r| Path::new(r).is_absolute()).ok_or_else(|| bad("say which repo: an absolute path to its folder"))
}

fn opened(reply: &Value, pane: &str, new: bool) -> Result<Map, KmuxError> {
    if reply.get("ok") != Some(&Value::Bool(true)) {
        let error = reply.get("error").cloned().unwrap_or(Value::Null);
        return Err(KmuxError::new(string(&error, "code").unwrap_or("failed"), string(&error, "message").unwrap_or("it failed")));
    }
    Ok(Map::from_iter([("pane".to_string(), json!(pane)), ("window".to_string(), reply.get("window").cloned().unwrap_or(Value::Null)), ("opened".to_string(), json!(new))]))
}

fn round_value(round: &Round, answers: Vec<Value>) -> Value {
    json!({ "round": format!("r{}", round.id), "at": num(round.at), "head": round.head, "doc": round.doc, "answers": answers })
}

fn answer_value(answer: &Answer) -> Value {
    json!({
        "answer": format!("a{}", answer.id),
        "at": num(answer.at),
        "kind": answer.kind,
        "file": answer.file,
        "right": answer.right,
        "again": opt(answer.again.map(|a| format!("a{a}")).as_deref()),
        "doc": answer.doc,
    })
}

impl Quiz {
    pub fn new(db: Rc<Db>, core: Option<WeakCore>, writers: Option<Rc<Writers>>) -> Quiz {
        Quiz { db, core, writers }
    }

    pub fn writers(&self) -> Option<&Rc<Writers>> {
        self.writers.as_ref()
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        core.add_command(
            "quiz",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                match args.get("action").and_then(Value::as_str) {
                    Some("open") => this.open(&args, responder),
                    Some("ready") => this.ready(&args, responder),
                    Some("map") if args.get("doc").is_none_or(Value::is_null) => match repo(&args).map(String::from) {
                        Ok(repo) => this.map(&repo, responder),
                        Err(error) => responder.reply(Err(error)),
                    },
                    _ => responder.reply(this.handle(&args)),
                }
            }),
        );
    }

    pub fn handle(&self, args: &Value) -> Result<Map, KmuxError> {
        match args.get("action").and_then(Value::as_str) {
            Some("start") => self.start(args),
            Some("answer") => self.answer(args),
            Some("history") => self.history(args),
            Some("map") => self.set_map(args),
            Some("notes") => self.notes(args),
            Some("ahead") => self.ahead(args),
            _ => Err(bad("quiz needs an action: open, start, answer, history, ready, map, notes or ahead")),
        }
    }

    fn open(&self, args: &Value, responder: Responder) {
        let Some(core) = self.core.as_ref().and_then(WeakCore::upgrade) else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let repo = match repo(args).map(|r| project(Some(r))) {
            Ok(Some(main)) => main.to_string_lossy().to_string(),
            Ok(None) => return responder.reply(Err(bad("say which repo"))),
            Err(error) => return responder.reply(Err(error)),
        };
        let open = core.with_panes(|panes| {
            panes.iter().find(|p| p.state != PaneState::Exited && p.state != PaneState::Failed && p.command.as_deref().is_some_and(|c| c.contains(QUIZ_ROLE)) && project(p.opened_in.as_deref()).is_some_and(|r| r.to_string_lossy() == repo)).map(|p| p.id.clone())
        });
        if let Some(pane) = open {
            return core.handle(json!({ "cmd": "focus", "args": { "pane": pane } }), move |reply| responder.reply(opened(&reply, &pane, false)));
        }
        let agent = string(args, "agent").filter(|a| !a.trim().is_empty()).map_or_else(|| task_agent(&config_files(Path::new(&repo))), String::from);
        let base = Path::new(&repo).file_name().map_or_else(|| repo.clone(), |n| n.to_string_lossy().to_string());
        let name = ["Quiz".to_string(), format!("Quiz {base}")].into_iter().chain((2..).map(|n| format!("Quiz {base} {n}"))).find(|n| core.pane(n).is_none()).unwrap_or_default();
        let open = json!({ "type": "term", "lead": true, "window": "new", "name": name, "cwd": repo, "cmd": started_as(&agent, QUIZ_ROLE, None, prompts(&config_files(Path::new(&repo)))), "wait": false });
        core.handle(json!({ "cmd": "open", "args": open }), move |reply| {
            let pane = reply.get("pane").and_then(|p| p.get("id")).and_then(Value::as_str).unwrap_or_default().to_string();
            responder.reply(opened(&reply, &pane, true))
        });
    }

    fn start(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = repo(args)?;
        let head = string(args, "head").filter(|h| !h.is_empty()).ok_or_else(|| bad("say which commit of main the round asks about"))?;
        let mut doc = doc(args)?;
        if let Some(by) = string(args, "by").filter(|b| !b.is_empty()) {
            doc["by"] = json!(by);
        }
        let id = self.db.start_round(repo, head, &doc).map_err(|e| failed("start a round", e))?;
        Ok(Map::from_iter([("round".to_string(), json!(format!("r{id}")))]))
    }

    fn answer(&self, args: &Value) -> Result<Map, KmuxError> {
        let round = id_of('r', args.get("round")).ok_or_else(|| bad("say which round: the ID quiz start gave, like r12"))?;
        let kind = string(args, "kind").ok_or_else(|| bad(format!("say the question's kind: {}", KINDS.join(", "))))?;
        if !KINDS.contains(&kind) {
            return Err(bad(format!("\"{kind}\" isn't a kind of question ({})", KINDS.join(", "))));
        }
        let file = string(args, "file").filter(|f| !f.is_empty()).ok_or_else(|| bad("say which file the question is about"))?;
        let right = args.get("right").and_then(Value::as_bool).ok_or_else(|| bad("say whether the answer was right"))?;
        let found = self.db.round(round).map_err(|e| failed("read the round", e))?.ok_or_else(|| KmuxError::new("not_found", format!("no round r{round}")))?;
        let again = match args.get("again").filter(|a| !a.is_null()) {
            None => None,
            Some(value) => {
                let id = id_of('a', Some(value)).ok_or_else(|| bad("--again takes an answer's ID, like a7"))?;
                match self.db.find_answer(id).map_err(|e| failed("read the answer", e))? {
                    Some((_, repo)) if repo == found.repo => Some(id),
                    Some(_) => return Err(bad(format!("a{id} is from another repo's quiz"))),
                    None => return Err(KmuxError::new("not_found", format!("no answer a{id}"))),
                }
            }
        };
        let mut doc = doc(args)?;
        for key in ["line", "last"] {
            if let Some(line) = args.get(key).and_then(Value::as_u64) {
                doc[key] = json!(line);
            }
        }
        let id = self.db.answer(round, kind, file, right, again, &doc).map_err(|e| failed("save the answer", e))?;
        Ok(Map::from_iter([("round".to_string(), json!(format!("r{round}"))), ("answer".to_string(), json!(format!("a{id}")))]))
    }

    fn ready(self: &Rc<Self>, args: &Value, responder: Responder) {
        let repo = match repo(args) {
            Ok(repo) => repo.to_string(),
            Err(error) => return responder.reply(Err(error)),
        };
        let doc = match args.get("doc") {
            Some(doc @ Value::Object(_)) => doc.clone(),
            _ => return responder.reply(Err(bad("a round is a JSON object: {\"head\", \"questions\": [...]}"))),
        };
        let Some(head) = string(&doc, "head").filter(|h| !h.is_empty()).map(String::from) else { return responder.reply(Err(bad("say which commit of main the round asks about: \"head\""))) };
        let about = self.writers.as_ref().and_then(|w| w.subject(&repo)).or_else(|| string(&doc, "about").filter(|a| !a.trim().is_empty()).map(String::from));
        if about.is_none() && self.db.round_waits(&repo).unwrap_or(false) {
            return responder.reply(Err(KmuxError::new("already_ready", "a round is ready and not played yet, so this one isn't needed")));
        }
        let misses: Vec<String> = self.db.answers(&repo, true).unwrap_or_default().iter().map(|a| format!("a{}", a.id)).collect();
        let (this, by) = (Rc::downgrade(self), string(args, "by").map(String::from));
        crate::runtime::spawn("a quiz round's check", async move {
            let folder = PathBuf::from(&repo);
            let checked = crate::runtime::off_loop({
                let (folder, head) = (folder.clone(), head.clone());
                move || {
                    if git(&folder, &["cat-file", "-e", &format!("{head}^{{commit}}")]).is_err() {
                        return Err(vec![format!("{head} isn't a commit here")]);
                    }
                    let show = |file: &str| git(&folder, &["show", &format!("{head}:{file}")]).ok();
                    let again = |id: &str| misses.iter().any(|m| m == id);
                    quiz_round::check(doc, &Source { head: &head, show: &show, again: &again })
                }
            })
            .await;
            let Some(this) = this.upgrade() else { return };
            responder.reply(match checked {
                None => Err(KmuxError::new("failed", "couldn't check the round")),
                Some(Err(problems)) => Err(bad(format!("round refused: fix these and run kanna quiz ready again:\n{}", problems.join("\n")))),
                Some(Ok(doc)) => this.keep_round(&repo, &head, about, by, doc),
            });
        });
    }

    fn keep_round(&self, repo: &str, head: &str, about: Option<String>, by: Option<String>, doc: Value) -> Result<Map, KmuxError> {
        let title = string(&doc, "title").filter(|t| !t.trim().is_empty()).map(String::from).or_else(|| about.clone()).unwrap_or_else(|| "Next round".into());
        let mut kept = json!({ "title": title, "about": about, "says": doc.get("says"), "questions": doc.get("questions") });
        if kept["says"].is_null() {
            kept["says"] = json!(about.as_ref().map_or("about what's new on main".to_string(), |a| format!("about {a}")));
        }
        let mut round_doc = json!({ "commits": doc.get("commits"), "agent": doc.get("agent") });
        if let Some(by) = by {
            round_doc["by"] = json!(by);
        }
        let round = self.db.put_ready(repo, head, &round_doc, &kept).map_err(|e| failed("keep the round", e))?;
        if let Some(writers) = &self.writers {
            writers.written(repo, Written { round, title, about });
        }
        Ok(Map::from_iter([("round".to_string(), json!(format!("r{round}")))]))
    }

    fn set_map(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = repo(args)?;
        let doc = doc(args)?;
        let map = quiz_map::read_map(&doc).map_err(bad)?;
        self.db.set_quiz_map(repo, &doc).map_err(|e| failed("keep the map", e))?;
        Ok(Map::from_iter([("subsystems".to_string(), json!(map.len()))]))
    }

    fn notes(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = repo(args)?;
        if let Some(notes) = args.get("doc").filter(|d| !d.is_null()) {
            let notes = notes.get("notes").and_then(Value::as_object).ok_or_else(|| bad("notes are {\"notes\": {\"SUBSYSTEM\": \"a few plain lines\"}}"))?;
            for (subsystem, text) in notes {
                let text = text.as_str().ok_or_else(|| bad(format!("the notes on {subsystem} are text")))?;
                self.db.set_quiz_note(repo, subsystem, text).map_err(|e| failed("keep the notes", e))?;
            }
        }
        let notes = self.db.quiz_notes(repo).map_err(|e| failed("read the notes", e))?;
        Ok(Map::from_iter([("notes".to_string(), Value::Object(notes.into_iter().map(|(subsystem, text)| (subsystem, json!(text))).collect()))]))
    }

    pub fn ahead(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = repo(args)?;
        let about = string(args, "about").map(str::trim).filter(|a| !a.is_empty()).map(String::from);
        let writers = self.writers.as_ref().ok_or_else(|| KmuxError::new("unavailable", "this daemon can't start a writer"))?;
        writers.ahead(repo, about, args.get("again").and_then(Value::as_bool).unwrap_or(false));
        Ok(self.writing(repo))
    }

    fn writing(&self, repo: &str) -> Map {
        match self.writers.as_ref().map(|w| w.writing(repo)) {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }

    pub fn map(self: &Rc<Self>, repo: &str, responder: Responder) {
        let Some(kept) = self.db.quiz_map(repo).ok().flatten() else { return responder.reply(Ok(Map::from_iter([("subsystems".to_string(), json!([]))]))) };
        let map = match quiz_map::read_map(&kept) {
            Ok(map) => map,
            Err(error) => return responder.reply(Err(bad(error))),
        };
        let read = || -> rusqlite::Result<_> { Ok((self.db.answers(repo, false)?, self.db.answers(repo, true)?, self.db.rounds(repo)?, self.db.quiz_notes(repo)?)) };
        let (answers, misses, rounds, notes) = match read() {
            Ok(read) => read,
            Err(error) => return responder.reply(Err(failed("read the quiz", error))),
        };
        let folder = PathBuf::from(repo);
        crate::runtime::spawn("a quiz map", async move {
            let worked = crate::runtime::off_loop(move || {
                let time = now();
                let since = time - WINDOW_DAYS * 86400.0;
                let head = quiz_map::head_of(&folder)?;
                let facts = quiz_map::read_facts(&folder, &head, since, |path| quiz_map::wanted(&map, path))?;
                let known: HashMap<String, _> = quiz_map::last_rights(&map, &answers, &rounds).into_iter().map(|last| (last.clone(), quiz_map::before(&folder, &last, since))).collect();
                let tiles = quiz_map::numbers(&Inputs { map: &map, facts: &facts, answers: &answers, misses: &misses, rounds: &rounds, notes: &notes, known: &known, now: time });
                Ok::<_, String>(json!({ "head": head, "subsystems": tiles }))
            })
            .await;
            responder.reply(match worked {
                Some(Ok(Value::Object(map))) => Ok(map),
                Some(Err(error)) => Err(KmuxError::new("failed", error)),
                _ => Err(KmuxError::new("failed", "couldn't work out the map")),
            });
        });
    }

    fn ready_value(&self, ready: &Ready, map: &[quiz_map::Subsystem]) -> Value {
        let mut doc = ready.doc.clone();
        if let Some(questions) = doc.get_mut("questions").and_then(Value::as_array_mut) {
            for question in questions {
                let subsystem = quiz_map::subsystem_of(map, question["file"].as_str().unwrap_or_default());
                question["subsystem"] = json!(subsystem);
            }
        }
        let answers: Vec<Value> = self.db.picks(ready.round).unwrap_or_default().into_iter().map(|(n, pick)| json!({ "n": n, "pick": pick })).collect();
        doc["round"] = json!(format!("r{}", ready.round));
        doc["written"] = num(ready.written);
        doc["taken"] = ready.taken.map_or(Value::Null, num);
        doc["head"] = json!(ready.head);
        doc["answers"] = Value::Array(answers);
        doc
    }

    pub fn rounds(&self, repo: &str) -> Result<Map, KmuxError> {
        let map = self.db.quiz_map(repo).ok().flatten().and_then(|doc| quiz_map::read_map(&doc).ok()).unwrap_or_default();
        let rounds = self.db.open_rounds(repo).map_err(|e| failed("read the rounds", e))?;
        let mut out = self.writing(repo);
        out.insert("rounds".to_string(), Value::Array(rounds.iter().map(|ready| self.ready_value(ready, &map)).collect()));
        Ok(out)
    }

    pub fn phone_answer(&self, args: &Value) -> Result<Map, KmuxError> {
        let round = id_of('r', args.get("round")).ok_or_else(|| bad("say which round"))?;
        let n = args.get("n").and_then(Value::as_i64).filter(|n| *n >= 0).ok_or_else(|| bad("say which question: n"))?;
        let pick = args.get("pick").and_then(Value::as_i64).filter(|p| *p >= 0).ok_or_else(|| bad("say which was picked"))?;
        let ready = self.db.ready(round).map_err(|e| failed("read the round", e))?.ok_or_else(|| KmuxError::new("not_found", format!("no round r{round}")))?;
        let question = ready.doc.get("questions").and_then(|q| q.get(n as usize)).ok_or_else(|| bad(format!("r{round} has no question {n}")))?;
        let right_one = question.get("answer").and_then(Value::as_i64).unwrap_or(-1);
        let right = pick == right_one;
        let kind = string(question, "kind").unwrap_or("why");
        let file = string(question, "file").unwrap_or_default();
        let given = args.get("again").filter(|a| !a.is_null()).or(question.get("again")).filter(|a| !a.is_null());
        let again = id_of('a', given).filter(|id| self.db.find_answer(*id).ok().flatten().is_some_and(|(_, repo)| repo == ready.repo));
        let mut doc = json!({ "question": question.get("text"), "pick": pick, "answer": right_one, "explanation": question.get("why"), "topic": question.get("topic"), "line": question.get("line"), "from": "phone" });
        for key in ["choices", "lines", "last"] {
            if let Some(value) = question.get(key).filter(|v| !v.is_null()) {
                doc[key] = value.clone();
            }
        }
        let (id, added) = self.db.answer_question(round, n, kind, file, right, again, &doc).map_err(|e| failed("save the answer", e))?;
        if self.db.take_ready(round).map_err(|e| failed("start the round", e))? {
            if let Some(writers) = &self.writers {
                writers.played(&ready.repo);
            }
        }
        Ok(Map::from_iter([("answer".to_string(), json!(format!("a{id}"))), ("right".to_string(), json!(right)), ("new".to_string(), json!(added))]))
    }

    fn history(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = repo(args)?;
        let misses = args.get("misses").and_then(Value::as_bool).unwrap_or(false);
        let rounds = self.db.rounds(repo).map_err(|e| failed("read the rounds", e))?;
        let answers = self.db.answers(repo, misses).map_err(|e| failed("read the answers", e))?;
        let rounds = rounds
            .iter()
            .filter_map(|round| {
                let mine: Vec<Value> = answers.iter().filter(|a| a.round == round.id).map(answer_value).collect();
                (!misses || !mine.is_empty()).then(|| round_value(round, mine))
            })
            .collect();
        Ok(Map::from_iter([("repo".to_string(), json!(repo)), ("misses".to_string(), json!(misses)), ("rounds".to_string(), Value::Array(rounds))]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn quiz(name: &str) -> (Quiz, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("kanna-quiz-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (Quiz::new(Rc::new(Db::open(&dir).unwrap()), None, None), dir)
    }

    fn ok(quiz: &Quiz, args: Value) -> Value {
        Value::Object(quiz.handle(&args).unwrap_or_else(|e| panic!("{args}: {}", e.message)))
    }

    fn fails(quiz: &Quiz, args: Value) -> String {
        quiz.handle(&args).err().unwrap_or_else(|| panic!("{args} worked")).message
    }

    #[test]
    fn a_round_and_its_answers_are_read_back_newest_round_first() {
        let (quiz, dir) = quiz("history");
        assert_eq!(ok(&quiz, json!({ "action": "start", "repo": "/r", "head": "abc", "by": "p3", "doc": { "commits": ["abc"] } }))["round"], "r1");
        let answer = |round: &str, kind: &str, right: bool, again: Value| ok(&quiz, json!({ "action": "answer", "round": round, "kind": kind, "file": "src/a.rs", "line": 4, "right": right, "again": again, "doc": { "question": "Where?" } }))["answer"].clone();
        assert_eq!(answer("r1", "where", false, Value::Null), "a1");
        assert_eq!(answer("r1", "why", true, Value::Null), "a2");
        ok(&quiz, json!({ "action": "start", "repo": "/r", "head": "def" }));
        ok(&quiz, json!({ "action": "start", "repo": "/other", "head": "x" }));
        let history = ok(&quiz, json!({ "action": "history", "repo": "/r" }));
        let rounds = history["rounds"].as_array().unwrap();
        assert_eq!(rounds.iter().map(|r| r["round"].as_str().unwrap()).collect::<Vec<_>>(), ["r2", "r1"]);
        assert_eq!((rounds[1]["head"].as_str(), &rounds[1]["doc"]), (Some("abc"), &json!({ "commits": ["abc"], "by": "p3" })));
        let first = &rounds[1]["answers"][0];
        assert_eq!((first["kind"].as_str(), first["file"].as_str(), first["right"].as_bool()), (Some("where"), Some("src/a.rs"), Some(false)));
        assert_eq!(first["doc"], json!({ "question": "Where?", "line": 4 }));
        let misses = ok(&quiz, json!({ "action": "history", "repo": "/r", "misses": true }));
        assert_eq!(misses["rounds"].as_array().unwrap().iter().map(|r| r["round"].as_str().unwrap()).collect::<Vec<_>>(), ["r1"]);
        assert_eq!(misses["rounds"][0]["answers"].as_array().unwrap().len(), 1);
        assert_eq!(answer("r2", "where", true, json!("a1")), "a3");
        assert_eq!(ok(&quiz, json!({ "action": "history", "repo": "/r", "misses": true }))["rounds"], json!([]));
        fs::remove_dir_all(&dir).unwrap();
    }

    fn written(quiz: &Quiz) -> i64 {
        let question = json!({ "kind": "why", "file": "src/a.rs", "line": 4, "last": 5, "text": "Why?", "why": "Because", "topic": "T", "choices": [{ "label": "a" }, { "label": "b" }, { "label": "c" }, { "label": "d" }], "answer": 2 });
        quiz.keep_round("/r", "abc", None, None, json!({ "questions": vec![question; 5], "commits": ["abc"] })).unwrap()["round"].as_str().unwrap()[1..].parse().unwrap()
    }

    #[test]
    fn a_phone_answer_counts_once_and_lands_in_the_mac_s_table() {
        let (quiz, dir) = quiz("phone");
        let round = written(&quiz);
        assert_eq!(quiz.rounds("/r").unwrap()["rounds"][0]["taken"], Value::Null);
        let answer = |n: i64, pick: i64| Value::Object(quiz.phone_answer(&json!({ "round": format!("r{round}"), "n": n, "pick": pick })).unwrap());
        let first = answer(0, 1);
        assert_eq!((first["right"].as_bool(), first["new"].as_bool()), (Some(false), Some(true)));
        let again = answer(0, 2);
        assert_eq!((again["answer"].clone(), again["new"].as_bool()), (first["answer"].clone(), Some(false)), "a sync after a train ride can't count twice");
        let saved = ok(&quiz, json!({ "action": "history", "repo": "/r" }));
        let answers = saved["rounds"][0]["answers"].as_array().unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!((answers[0]["kind"].as_str(), answers[0]["file"].as_str(), answers[0]["right"].as_bool()), (Some("why"), Some("src/a.rs"), Some(false)));
        assert_eq!((answers[0]["doc"]["line"].as_u64(), answers[0]["doc"]["question"].as_str()), (Some(4), Some("Why?")));
        let open = quiz.rounds("/r").unwrap();
        assert!(open["rounds"][0]["taken"].is_number());
        assert_eq!(open["rounds"][0]["answers"], json!([{ "n": 0, "pick": 1 }]), "a round closed halfway keeps its place");
        for n in 1..5 {
            answer(n, 2);
        }
        assert_eq!(quiz.rounds("/r").unwrap()["rounds"], json!([]), "a finished round isn't open");
        assert!(quiz.phone_answer(&json!({ "round": format!("r{round}"), "n": 5, "pick": 1 })).unwrap_err().message.contains("no question 5"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_round_waits_at_a_time_and_a_subject_s_round_is_apart() {
        let (quiz, dir) = quiz("waits");
        assert!(!quiz.db.round_waits("/r").unwrap());
        written(&quiz);
        assert!(quiz.db.round_waits("/r").unwrap());
        assert!(!quiz.db.round_waits("/other").unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notes_and_a_map_are_kept_by_repo() {
        let (quiz, dir) = quiz("map");
        let map = json!({ "subsystems": [{ "name": "A", "paths": ["a/**"] }, { "name": "B", "paths": ["b"] }, { "name": "C", "paths": ["c"] }, { "name": "D", "paths": ["d"] }, { "name": "E", "paths": ["e"] }, { "name": "F", "paths": ["f"] }] });
        assert_eq!(ok(&quiz, json!({ "action": "map", "repo": "/r", "doc": map }))["subsystems"], 6);
        assert!(fails(&quiz, json!({ "action": "map", "repo": "/r", "doc": { "subsystems": [] } })).contains("6 to 12"));
        assert_eq!(quiz.db.quiz_map("/other").unwrap(), None);
        ok(&quiz, json!({ "action": "notes", "repo": "/r", "doc": { "notes": { "A": "Strong." } } }));
        ok(&quiz, json!({ "action": "notes", "repo": "/r", "doc": { "notes": { "A": "Weak.", "B": "New." } } }));
        assert_eq!(ok(&quiz, json!({ "action": "notes", "repo": "/r" }))["notes"], json!({ "A": "Weak.", "B": "New." }));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_answer_is_refused_and_nothing_is_saved() {
        let (quiz, dir) = quiz("refused");
        ok(&quiz, json!({ "action": "start", "repo": "/r", "head": "abc" }));
        ok(&quiz, json!({ "action": "start", "repo": "/other", "head": "abc" }));
        ok(&quiz, json!({ "action": "answer", "round": "r2", "kind": "why", "file": "x", "right": false }));
        let answer = |extra: Value| {
            let mut args = json!({ "action": "answer", "round": "r1", "kind": "why", "file": "src/a.rs", "right": true });
            for (key, value) in extra.as_object().unwrap() {
                args[key] = value.clone();
            }
            fails(&quiz, args)
        };
        assert!(answer(json!({ "round": "r9" })).contains("no round r9"));
        assert!(answer(json!({ "round": "twelve" })).contains("say which round"));
        assert!(answer(json!({ "kind": "trivia" })).contains("isn't a kind"));
        assert!(answer(json!({ "right": Value::Null })).contains("right"));
        assert!(answer(json!({ "doc": "text" })).contains("JSON object"));
        assert!(answer(json!({ "again": "a1" })).contains("another repo"));
        assert!(answer(json!({ "again": "a9" })).contains("no answer a9"));
        assert!(fails(&quiz, json!({ "action": "start", "repo": "here", "head": "abc" })).contains("absolute"));
        assert!(fails(&quiz, json!({ "action": "start", "repo": "/r" })).contains("commit"));
        assert!(fails(&quiz, json!({ "action": "score" })).contains("open, start, answer, history"));
        assert_eq!(ok(&quiz, json!({ "action": "history", "repo": "/r" }))["rounds"][0]["answers"], json!([]));
        fs::remove_dir_all(&dir).unwrap();
    }
}
