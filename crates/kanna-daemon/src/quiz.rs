use crate::db::{self, Answer, Db, Round};
use kanna_mux::json::{json, num, opt, string, Map, Value};
use crate::tasks::{config_files, started_as, task_agent, QUIZ_ROLE};
use kanna_mux::{project, CoreHandle, KmuxError, PaneState, Responder, WeakCore};
use std::path::Path;
use std::rc::Rc;

pub const KINDS: &[&str] = &["architecture", "drawing", "behaviour", "where", "why", "bug"];

pub struct Quiz {
    db: Rc<Db>,
    core: Option<WeakCore>,
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
    pub fn new(db: Rc<Db>, core: Option<WeakCore>) -> Quiz {
        Quiz { db, core }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        core.add_command(
            "quiz",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                match args.get("action").and_then(Value::as_str) {
                    Some("open") => this.open(&args, responder),
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
            _ => Err(bad("quiz needs an action: open, start, answer or history")),
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
        let open = json!({ "type": "term", "lead": true, "window": "new", "name": name, "cwd": repo, "cmd": started_as(&agent, QUIZ_ROLE, None), "wait": false });
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
        (Quiz::new(Rc::new(Db::open(&dir)), None), dir)
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
        assert!(fails(&quiz, json!({ "action": "score" })).contains("open, start, answer or history"));
        assert_eq!(ok(&quiz, json!({ "action": "history", "repo": "/r" }))["rounds"][0]["answers"], json!([]));
        fs::remove_dir_all(&dir).unwrap();
    }
}
