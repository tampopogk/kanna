use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};
use std::path::Path;

pub struct Report {
    pub count: usize,
    pub failures: Vec<Value>,
}

pub fn check(path: &str) -> Result<Report, Failure> {
    let text = std::fs::read_to_string(path).map_err(|e| fail(exit::NOT_FOUND, format!("diagrams: can't read {}: {e}", shown(path))))?;
    let checked = kanna_merman::check(&text);
    let failures = checked
        .iter()
        .filter_map(|c| {
            let message = c.failure.as_ref()?;
            Some(json!({ "file": shown(path), "line": c.line, "type": c.diagram_type, "message": message }))
        })
        .collect();
    Ok(Report { count: checked.len(), failures })
}

pub fn line(failure: &Value) -> String {
    format!("{}:{}: {}", failure["file"].as_str().unwrap_or_default(), failure["line"], failure["message"].as_str().unwrap_or_default())
}

pub fn run(request: &Value, json_output: bool) -> Result<(), Failure> {
    let mut count = 0;
    let mut failures = Vec::new();
    for file in request["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let report = check(file)?;
        count += report.count;
        failures.extend(report.failures);
    }
    if json_output {
        println!("{}", json!({ "diagrams": count, "failures": failures }));
    } else {
        for failure in &failures {
            println!("{}", line(failure));
        }
    }
    match failures.len() {
        0 if json_output => Ok(()),
        0 => {
            println!("{}", all_draw(count));
            Ok(())
        }
        failed => Err(fail(exit::FAILED, format!("{failed} of {count} diagrams don't draw"))),
    }
}

fn all_draw(count: usize) -> String {
    match count {
        0 => "no diagrams".into(),
        1 => "1 diagram, it draws".into(),
        n => format!("{n} diagrams, all draw"),
    }
}

fn shown(path: &str) -> String {
    let cwd = std::env::current_dir().ok();
    cwd.and_then(|dir| Path::new(path).strip_prefix(dir).ok().map(|p| p.to_string_lossy().into_owned())).unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_file_line_message() {
        let failure = json!({ "file": "docs/a.md", "line": 12, "type": "er", "message": "unsupported diagram type: er" });
        assert_eq!(line(&failure), "docs/a.md:12: unsupported diagram type: er");
    }

    #[test]
    fn the_count_reads_as_english() {
        assert_eq!([all_draw(0), all_draw(1), all_draw(3)], ["no diagrams", "1 diagram, it draws", "3 diagrams, all draw"]);
    }
}
