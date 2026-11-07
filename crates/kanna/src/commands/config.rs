use crate::args::Argv;
use kanna_client::Failure;

pub const CONFIG_SCHEMA: &str = include_str!("../../../../schemas/config.schema.json");

const HELP: &str = "usage: kanna config schema\n\nKanna's settings are ~/.kanna-v4/config.json, overridden key by key by a repo's .kanna/config.json, e.g.\n  { \"version\": 1, \"planning\": { \"context\": 300000 }, \"tasks\": { \"agent\": \"codex\" }, \"shelf\": { \"runners\": 3 } }\n(agents.prompts, whether agents skip permission prompts, is read only from ~/.kanna-v4/config.json.)\n\n`kanna config schema` prints their JSON Schema: every key, its type, default and what it does.";

pub fn run(args: &mut Argv) -> Result<(), Failure> {
    match args.positional().as_deref() {
        Some("schema") => print!("{CONFIG_SCHEMA}"),
        _ => println!("{HELP}"),
    }
    Ok(())
}
