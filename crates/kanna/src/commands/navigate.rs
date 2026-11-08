use super::{absolute, file_line, is_markdown, pane, usage, Command, Flow, PANE};
use crate::output;
use serde_json::json;

pub const COMMAND: Command = Command {
    name: "navigate",
    group: "Panes",
    summary: "Point a web pane at a new URL (or go back or forward), a markdown pane at another file, a code pane at another file or line, a diff pane at another diff, or a log pane at another range.",
    usage: "kanna navigate PANE URL\n       kanna navigate PANE --back|--forward\n       kanna navigate PANE FILE.md\n       kanna navigate PANE FILE[:LINE]\n       kanna navigate PANE diff [GIT-DIFF-ARGS]\n       kanna navigate PANE log [REV...]",
    options: &[
        ("--back", "Go back a page: in a markdown pane, or a web pane opened with `open web URL --history`."),
        ("--forward", "Go forward a page (after --back)."),
    ],
    notes: "Markdown panes always keep history of the files they showed. Web panes keep none unless opened with --history.",
    examples: &[
        ("kanna navigate site localhost:5173", "Show localhost:5173 in the site pane."),
        ("kanna navigate site --back", "Go back to the previous page (the pane was opened with --history)."),
        ("kanna navigate spec docs/roadmap.md", "Show another markdown file in the spec pane."),
    ],
    parse: |args| {
        let pane = pane(args, "navigate")?;
        if matches!(args.first(), Some("diff" | "log")) {
            args.positional();
            return Ok(json!({ "pane": pane, "args": args.remaining() }));
        }
        let (back, forward) = (args.flag("--back"), args.flag("--forward"));
        match (back, forward, args.positional()) {
            (true, false, None) => Ok(json!({ "pane": pane, "back": true })),
            (false, true, None) => Ok(json!({ "pane": pane, "forward": true })),
            (false, false, Some(target)) if is_markdown(&target) => Ok(json!({ "pane": pane, "path": absolute(&target) })),
            (false, false, Some(target)) if is_source(&target) => {
                let (file, line) = file_line(&target);
                let mut out = json!({ "pane": pane, "path": absolute(&file) });
                if let Some(line) = line {
                    out["line"] = json!(line);
                }
                Ok(out)
            }
            (false, false, Some(url)) => Ok(json!({ "pane": pane, "url": url })),
            (false, false, None) => Err(usage("navigate", "missing URL (or --back / --forward)")),
            _ => Err(usage("navigate", "give one of URL, --back or --forward")),
        }
    },
    show: |_, reply| output::pane_state(reply),
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};

pub fn is_source(target: &str) -> bool {
    !target.contains("://") && std::path::Path::new(&absolute(&file_line(target).0)).is_file()
}
