use super::{absolute, from_caller, usage, Command, Flow};
use crate::args::Argv;
use kanna_client::Failure;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "screenshot",
    group: "Panes",
    summary: "Save a PNG of a pane or a window as it is drawn now, and print its path.",
    usage: "kanna screenshot [PANE | --window WINDOW] [-o|--out FILE] [--force]",
    options: &[
        ("--window WINDOW", "The whole window, e.g. win-2: its tab bar, every pane showing and the space between them."),
        ("--out FILE", "Where to save it, relative to this folder. It must not exist yet, unless you add --force. Default: a new file in $TMPDIR."),
        ("-o FILE", "Same as --out."),
        ("--force", "Replace FILE if it exists."),
    ],
    notes: "PANE gives just that pane, cut from its window; it must be showing (not in a tab that is behind another). With neither PANE nor --window: the window of the terminal you run it in, else the key (front) window.\nWorks for every kind of pane (terminal, web, markdown, code, iOS), also when the window is behind other windows. The image is at the screen's resolution: a 2x display gives twice the window's size in points.\nWith --json, the reply also has the window, the pane and the image's width and height in pixels. Starts Kanna.app if it isn't running, and shoots the window it opens with.",
    examples: &[
        ("kanna screenshot", "Save this window and print where."),
        ("kanna screenshot diff -o diff.png", "Save the pane named diff to diff.png."),
        ("kanna screenshot --window win-2 --out shots/win-2.png --force", "Save win-2, replacing shots/win-2.png."),
    ],
    parse: parse,
    show: |_, reply| reply["path"].as_str().unwrap_or_default().to_string(),
    flow: Flow { unplaced: Some(unplaced), ..Flow::PLAIN },
};

fn unplaced(request: &Value) -> Option<&'static str> {
    (request["pane"].is_null() && request["window"].is_null() && request["from"].is_null()).then_some("say PANE or --window WINDOW")
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let out = match (args.option("--out")?, args.option("-o")?) {
        (Some(_), Some(_)) => return Err(usage("screenshot", "give --out or -o, not both")),
        (out, short) => out.or(short),
    };
    let force = args.flag("--force");
    let mut request = Map::new();
    match (args.option("--window")?, args.positional()) {
        (Some(_), Some(_)) => return Err(usage("screenshot", "give PANE or --window WINDOW, not both")),
        (Some(window), None) => {
            request.insert("window".into(), json!(window));
        }
        (None, Some(pane)) => {
            request.insert("pane".into(), json!(pane));
        }
        (None, None) => from_caller(&mut request),
    }
    match out {
        Some(path) if path.is_empty() => return Err(usage("screenshot", "--out needs a file")),
        Some(path) => {
            request.insert("path".into(), json!(absolute(&path)));
        }
        None => {
            request.insert("dir".into(), json!(std::env::temp_dir().to_string_lossy()));
        }
    }
    if force {
        request.insert("force".into(), json!(true));
    }
    Ok(Value::Object(request))
}
