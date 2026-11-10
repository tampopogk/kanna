use crate::quiz::KINDS;
use kanna_mux::json::{json, Value};

pub const QUESTIONS: usize = 5;

const CHOICES: usize = 4;

const DRAWINGS: usize = 3;

const MOST_LINES: u64 = 60;

const AROUND: u64 = 2;

pub struct Source<'a> {
    pub head: &'a str,
    pub show: &'a dyn Fn(&str) -> Option<String>,
    pub again: &'a dyn Fn(&str) -> bool,
}

fn text<'a>(question: &'a Value, key: &str) -> Option<&'a str> {
    question.get(key).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty())
}

fn body(question: &Value, kind: &str, say: &mut dyn FnMut(String)) -> usize {
    let list = |key: &str| question.get(key).and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    match kind {
        "drawing" => {
            if list("drawings").len() != DRAWINGS || !list("drawings").iter().all(Value::is_string) {
                say(format!("a drawing question has {DRAWINGS} mermaid sources in \"drawings\""));
            }
            DRAWINGS
        }
        "bug" => {
            let lines = list("lines");
            if lines.len() != CHOICES || !lines.iter().all(|l| l["n"].is_u64() && l["text"].is_string()) {
                say(format!("a spot-the-bug question has {CHOICES} entries {{\"n\", \"text\"}} in \"lines\""));
            }
            CHOICES
        }
        _ => {
            let choices = list("choices");
            if choices.len() != CHOICES || !choices.iter().all(|c| text(c, "label").is_some()) {
                say(format!("a question has {CHOICES} choices, each with a \"label\""));
            }
            CHOICES
        }
    }
}

fn code(question: &Value, source: &Source, say: &mut dyn FnMut(String)) -> Option<Value> {
    let Some(file) = text(question, "file") else {
        say("say which \"file\" it's about".into());
        return None;
    };
    let Some(line) = question.get("line").and_then(Value::as_u64).filter(|l| *l > 0) else {
        say("say which \"line\" of the file it's about".into());
        return None;
    };
    let last = question.get("last").and_then(Value::as_u64).unwrap_or(line);
    if last < line || last - line >= MOST_LINES {
        say(format!("\"last\" is from {line} to {} ", line + MOST_LINES - 1).trim_end().to_string());
        return None;
    }
    let Some(found) = (source.show)(file) else {
        say(format!("{file} isn't in {}", source.head));
        return None;
    };
    let lines: Vec<&str> = found.lines().collect();
    if last as usize > lines.len() {
        say(format!("{file} has {} lines at {}, so there's no line {last}", lines.len(), source.head));
        return None;
    }
    let first = line.saturating_sub(AROUND).max(1);
    let end = (last + AROUND).min(lines.len() as u64);
    let shown: Vec<&str> = lines[first as usize - 1..end as usize].to_vec();
    Some(json!({ "file": file, "line": first, "commit": source.head, "hot": [line, last], "lines": shown }))
}

pub fn check(mut doc: Value, source: &Source) -> Result<Value, Vec<String>> {
    let mut problems = vec![];
    let count = doc.get("questions").and_then(Value::as_array).map_or(0, Vec::len);
    if count != QUESTIONS {
        problems.push(format!("a round has {QUESTIONS} questions, and this has {count}"));
    }
    for index in 0..count {
        let mut said = vec![];
        let mut say = |problem: String| said.push(problem);
        let question = doc["questions"][index].clone();
        let kind = text(&question, "kind").unwrap_or_default();
        let known = KINDS.contains(&kind);
        if !known {
            say(format!("\"{kind}\" isn't a kind of question ({})", KINDS.join(", ")));
        }
        for key in ["text", "why", "topic"] {
            if text(&question, key).is_none() {
                say(format!("it needs \"{key}\""));
            }
        }
        if known {
            let choices = body(&question, kind, &mut say);
            if question.get("answer").and_then(Value::as_u64).is_none_or(|a| a as usize >= choices) {
                say(format!("\"answer\" is the right one's index, from 0 to {}", choices - 1));
            }
        }
        if let Some(again) = question.get("again").filter(|a| !a.is_null()) {
            if !again.as_str().is_some_and(source.again) {
                say(format!("\"again\" is a miss of this repo's, and {again} isn't"));
            }
        }
        let shown = code(&question, source, &mut say);
        problems.extend(said.into_iter().map(|problem| format!("question {}: {problem}", index + 1)));
        if let Some(shown) = shown {
            doc["questions"][index]["code"] = shown;
        }
    }
    match problems.is_empty() {
        true => Ok(doc),
        false => Err(problems),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(kind: &str) -> Value {
        let choices = (0..4).map(|n| json!({ "label": format!("c{n}") })).collect::<Vec<_>>();
        json!({ "kind": kind, "text": "Which?", "why": "Because", "topic": "t", "file": "a.rs", "line": 3, "choices": choices, "answer": 1 })
    }

    fn round(questions: Vec<Value>) -> Value {
        json!({ "questions": questions })
    }

    fn checked(doc: Value) -> Result<Value, Vec<String>> {
        let show = |file: &str| (file == "a.rs").then(|| "1\n2\n3\n4\n5\n6\n".to_string());
        let again = |id: &str| id == "a7";
        check(doc, &Source { head: "abc", show: &show, again: &again })
    }

    #[test]
    fn a_good_round_comes_back_with_each_question_s_code() {
        let doc = checked(round(vec![question("why"); 5])).unwrap();
        let code = &doc["questions"][0]["code"];
        assert_eq!((code["file"].as_str(), code["line"].as_u64(), code["commit"].as_str()), (Some("a.rs"), Some(1), Some("abc")));
        assert_eq!(code["lines"], json!(["1", "2", "3", "4", "5"]));
        assert_eq!(code["hot"], json!([3, 3]));
    }

    #[test]
    fn every_problem_is_told_with_its_question() {
        let mut bad = vec![question("why"); 5];
        bad[0]["answer"] = json!(4);
        bad[1]["line"] = json!(99);
        bad[2]["kind"] = json!("trivia");
        bad[3]["file"] = json!("gone.rs");
        bad[4]["again"] = json!("a9");
        let problems = checked(round(bad)).unwrap_err();
        assert_eq!(problems.len(), 5, "{problems:?}");
        assert!(problems[0].starts_with("question 1:") && problems[0].contains("answer"));
        assert!(problems[1].contains("no line 99"));
        assert!(problems[2].contains("isn't a kind"));
        assert!(problems[3].contains("gone.rs isn't in abc"));
        assert!(problems[4].contains("a9"));
        assert!(checked(round(vec![question("why")])).unwrap_err()[0].contains("has 1"));
    }

    #[test]
    fn a_drawing_has_three_and_a_bug_has_four_lines() {
        let mut drawing = question("drawing");
        assert!(checked(round(vec![drawing.clone(), question("why"), question("why"), question("why"), question("why")])).unwrap_err()[0].contains("drawings"));
        drawing["drawings"] = json!(["flowchart LR\nA-->B", "flowchart LR\nB-->A", "flowchart LR\nA-->C"]);
        let mut bug = question("bug");
        bug["lines"] = json!([{ "n": 1, "text": "a" }, { "n": 2, "text": "b" }, { "n": 3, "text": "c" }, { "n": 4, "text": "d" }]);
        checked(round(vec![drawing.clone(), bug, question("why"), question("why"), question("why")])).unwrap();
        drawing["answer"] = json!(3);
        assert!(checked(round(vec![drawing, question("why"), question("why"), question("why"), question("why")])).unwrap_err()[0].contains("from 0 to 2"));
    }

    #[test]
    fn a_miss_brought_back_names_one_of_this_repo_s() {
        let mut back = question("why");
        back["again"] = json!("a7");
        checked(round(vec![back, question("why"), question("why"), question("why"), question("why")])).unwrap();
    }
}
