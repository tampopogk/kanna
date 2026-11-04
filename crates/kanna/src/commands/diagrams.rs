use super::{usage, Command, Flow, Invocation};
use kanna_client::Failure;
use kanna_client::{exit, fail};
use serde_json::{json, Value};
use std::path::Path;

pub const COMMAND: Command = Command {
    name: "diagrams",
    group: "Information",
    summary: "Check that a markdown file's mermaid diagrams and call trees draw in Kanna, without opening anything.",
    usage: "kanna diagrams check FILE.md...",
    options: &[],
    notes: "Prints FILE:LINE: message for each ```mermaid block that won't draw, LINE being its fence line: merman can't parse it, or Kanna.app doesn't draw its type. For a ```calltree block, LINE is the line at fault: a call that isn't NAME  PATH:LINE, an indent that jumps a level, a call before any process, a hop with no process after it, a repo path that doesn't exist or a line past its file's end. Then `N diagrams, all draw`, or exits 1 saying how many don't. `kanna open md` checks the same and prints the failures after it opens the pane.\nNeeds no Kanna.app or daemon. It can't see labels that overlap or clip: take `kanna screenshot PANE` of the markdown pane and look.",
    examples: &[
        ("kanna diagrams check docs/specs/plan.md", "Check one doc's diagrams."),
        ("kanna diagrams check docs/specs/*.md --json", "Check many; the count and the failures as JSON."),
    ],
    parse: |args| {
        match args.positional().as_deref() {
            Some("check") => {}
            Some(other) => return Err(usage("diagrams", &format!("unknown action \"{other}\" (check)"))),
            None => return Err(usage("diagrams", "missing check")),
        }
        let files = args.positionals();
        if files.is_empty() {
            return Err(usage("diagrams", "missing FILE.md"));
        }
        Ok(json!({ "action": "check", "files": files }))
    },
    show: |_, _| String::new(),
    flow: Flow { offline: Some(offline), ..Flow::PLAIN },
};

fn offline(request: &Value, invocation: &Invocation) -> Option<Result<(), Failure>> {
    Some(run(request, invocation.json_output))
}

pub struct Report {
    pub count: usize,
    pub failed: usize,
    pub failures: Vec<Value>,
}

pub fn check(path: &str) -> Result<Report, Failure> {
    let text = std::fs::read_to_string(path).map_err(|e| fail(exit::NOT_FOUND, format!("diagrams: can't read {}: {e}", shown(path))))?;
    let checked = kanna_merman::check(&text);
    let mut failures: Vec<Value> = checked
        .iter()
        .filter_map(|c| {
            let message = c.failure.as_ref()?;
            Some(json!({ "file": shown(path), "line": c.line, "type": c.diagram_type, "message": message }))
        })
        .collect();
    let mut failed = failures.len();
    let trees = kanna_calltree::check(&text, &kanna_calltree::repo_root(Path::new(path)));
    for (_, found) in &trees {
        failed += usize::from(!found.is_empty());
        failures.extend(found.iter().map(|(line, message)| json!({ "file": shown(path), "line": line, "type": kanna_calltree::FENCE, "message": message })));
    }
    failures.sort_by_key(|f| f["line"].as_u64());
    Ok(Report { count: checked.len() + trees.len(), failed, failures })
}

pub fn line(failure: &Value) -> String {
    format!("{}:{}: {}", failure["file"].as_str().unwrap_or_default(), failure["line"], failure["message"].as_str().unwrap_or_default())
}

pub fn run(request: &Value, json_output: bool) -> Result<(), Failure> {
    let (mut count, mut failed) = (0, 0);
    let mut failures = Vec::new();
    for file in request["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let report = check(file)?;
        count += report.count;
        failed += report.failed;
        failures.extend(report.failures);
    }
    if json_output {
        println!("{}", json!({ "diagrams": count, "failures": failures }));
    } else {
        for failure in &failures {
            println!("{}", line(failure));
        }
    }
    match failed {
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

pub fn shown(path: &str) -> String {
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
