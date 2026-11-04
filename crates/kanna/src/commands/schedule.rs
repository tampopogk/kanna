use super::{absolute, caller, repo_here, usage, Command, Flow};
use crate::args::Argv;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "schedule",
    group: "Information",
    summary: "Prompt an agent at set times: a schedule leaves a task or the planning agent a message, which it gets when its turn is over.",
    usage: "kanna schedule add RULE TARGET PROMPT... [--name NAME] [--repo DIR]\n       kanna schedule list [--repo DIR]\n       kanna schedule remove NAME|ID [--repo DIR]",
    options: &[
        ("--name NAME", "add: a name to remove it by, one word, unique in its repo. Default: none; it's known by its ID (s3)."),
        ("--repo DIR", "The repo the schedule belongs to, and whose planning agent planner means. Default: the repo you're in."),
    ],
    notes: "RULE is two words in local time, quoted or not: daily 09:00, weekdays 18:00, weekends 10:00, mon,thu 10:30, every 30m or every 2h. TARGET is a task (any form kanna task list takes) or planner, the repo's planning agent. At each time, the daemon leaves the target a message, as kanna task send does, from \"schedule NAME\": the agent gets it as [message from schedule NAME] PROMPT when its turn is over, never mid-turn, and a stopped Shelf task runs for it. One message waits at a time: a time that comes while the last is still waiting leaves no second. A time missed while the Mac slept or Kanna wasn't running fires once when it's back. Every repo has a schedule named quiz (daily 09:00, planner) that prompts the planning agent to offer a quiz if anything landed on main; remove it like any other. A NAME is looked up in this repo first.",
    examples: &[
        ("kanna schedule add 'weekdays 17:30' planner \"Summarise what landed today.\" --name evening", "Prompt this repo's planning agent every weekday at 17:30."),
        ("kanna schedule add every 2h 71af \"Pull main and run the tests.\"", "Prompt task 71af every two hours."),
        ("kanna schedule list", "Every schedule, when it next fires, and whether its message is waiting."),
        ("kanna schedule remove evening", "Remove the schedule named evening."),
    ],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let name = args.option("--name")?;
    let repo = args.option("--repo")?;
    let action = args.positional().ok_or_else(|| usage("schedule", "say what: add, list or remove"))?;
    let mut out = json!({ "action": action });
    if let Some(repo) = repo.map(|r| absolute(&r)).or_else(|| repo_here().ok()) {
        out["repo"] = json!(repo);
    }
    if action != "add" && name.is_some() {
        return Err(usage("schedule", "--name is for schedule add"));
    }
    match action.as_str() {
        "add" => {
            let missing = || usage("schedule", "add needs a RULE, a TARGET and a PROMPT: kanna schedule add 'daily 09:00' planner \"Offer a quiz.\"");
            let first = args.positional().ok_or_else(missing)?;
            out["rule"] = json!(if first.contains(char::is_whitespace) { first } else { format!("{first} {}", args.positional().ok_or_else(missing)?) });
            out["target"] = json!(args.positional().ok_or_else(missing)?);
            out["prompt"] = json!(args.rest().ok_or_else(missing)?);
            if let Some(name) = name {
                out["name"] = json!(name);
            }
            if let Some(by) = caller() {
                out["by"] = json!(by);
            }
        }
        "list" => {}
        "remove" => out["schedule"] = json!(args.positional().ok_or_else(|| usage("schedule", "remove needs a schedule's NAME or ID (s3)"))?),
        other => return Err(usage("schedule", &format!("unknown action \"{other}\" (add, list or remove)"))),
    }
    Ok(out)
}

pub fn show(request: &Value, reply: &Value) -> String {
    let id = |s: &Value| s["schedule"].as_str().unwrap_or("?").to_string();
    let named = |s: &Value| s["name"].as_str().map_or_else(|| id(s), |n| format!("{} {n}", id(s)));
    match request["action"].as_str() {
        Some("add") => format!("{}: {} → {}, next {}", named(reply), reply["rule"].as_str().unwrap_or("?"), reply["target"].as_str().unwrap_or("?"), reply["nextText"].as_str().unwrap_or("?")),
        Some("remove") => format!("removed {}", named(reply)),
        _ => {
            let all = reply["schedules"].as_array().cloned().unwrap_or_default();
            if all.is_empty() {
                return "no schedules".into();
            }
            let rows: Vec<[String; 7]> = all
                .iter()
                .map(|s| {
                    let repo = s["repo"].as_str().map_or("-".to_string(), |r| r.rsplit('/').next().unwrap_or(r).to_string());
                    let waiting = if s["waiting"] == true { "waiting" } else { "" };
                    let prompt: String = s["prompt"].as_str().unwrap_or("").chars().take(60).collect();
                    [id(s), s["name"].as_str().unwrap_or("-").to_string(), repo, s["rule"].as_str().unwrap_or("?").to_string(), s["target"].as_str().unwrap_or("?").to_string(), format!("next {}", s["nextText"].as_str().unwrap_or("?")), format!("{waiting:7} {prompt}")]
                })
                .collect();
            let widths: Vec<usize> = (0..6).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
            rows.iter()
                .map(|r| {
                    let cells: Vec<String> = (0..6).map(|i| format!("{:w$}", r[i], w = widths[i])).collect();
                    format!("{}  {}", cells.join("  "), r[6]).trim_end().to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn a_schedule_takes_a_two_word_rule_quoted_or_not_a_target_and_the_rest_as_its_prompt() {
        let git = |args: &[&str]| String::from_utf8(std::process::Command::new("git").args(args).output().unwrap().stdout).unwrap().trim().to_string();
        let repo = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).trim_end_matches("/.git").to_string();
        let want = json!({ "action": "add", "repo": repo, "rule": "weekdays 17:30", "target": "planner", "prompt": "Summarise what landed today.", "name": "evening" });
        assert_eq!(parse("kanna schedule add 'weekdays 17:30' planner \"Summarise what landed today.\" --name evening").unwrap().1, want);
        assert_eq!(parse("kanna schedule add weekdays 17:30 planner Summarise what landed today. --name evening").unwrap().1, want);
        assert_eq!(parse("kanna schedule remove s3 --repo /tmp").unwrap().1, json!({ "action": "remove", "repo": "/tmp", "schedule": "s3" }));
        assert_eq!(parse("kanna schedule list").unwrap().1, json!({ "action": "list", "repo": repo }));
        let refused = |line: &str| parse(line).err().unwrap_or_else(|| panic!("{line} parsed")).message;
        assert!(refused("kanna schedule add daily 09:00 planner").contains("needs a RULE, a TARGET and a PROMPT"));
        assert!(refused("kanna schedule remove").contains("NAME or ID"));
        assert!(refused("kanna schedule list --name x").contains("--name is for schedule add"));
        assert!(refused("kanna schedule pause x").contains("unknown action"));
    }

    #[test]
    fn schedules_list_as_columns_with_what_s_waiting() {
        let reply = json!({ "schedules": [
            { "schedule": "s1", "name": "quiz", "repo": "/Users/x/work/kanna-v4", "rule": "daily 09:00", "target": "planner", "nextText": "Sat 17 Oct 09:00", "waiting": true, "prompt": "Morning quiz check." },
            { "schedule": "s12", "name": null, "repo": null, "rule": "every 2h", "target": "71af", "nextText": "Fri 16 Oct 14:00", "waiting": false, "prompt": "Run the tests." },
        ]});
        assert_eq!(show(&json!({ "action": "list" }), &reply), "s1   quiz  kanna-v4  daily 09:00  planner  next Sat 17 Oct 09:00  waiting Morning quiz check.\ns12  -     -         every 2h     71af     next Fri 16 Oct 14:00          Run the tests.");
        assert_eq!(show(&json!({ "action": "list" }), &json!({ "schedules": [] })), "no schedules");
        assert_eq!(show(&json!({ "action": "add" }), &reply["schedules"][0]), "s1 quiz: daily 09:00 → planner, next Sat 17 Oct 09:00");
        assert_eq!(show(&json!({ "action": "remove" }), &json!({ "schedule": "s12", "name": null })), "removed s12");
    }
}
