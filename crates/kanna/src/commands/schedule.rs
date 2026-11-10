use super::{absolute, caller, repo_here, usage, Command, Flow};
use crate::args::Argv;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "schedule",
    group: "Information",
    summary: "Prompt an agent at set times: a schedule leaves a task or the planning agent a message, which it gets when its turn is over.",
    usage: "kanna schedule add RULE TARGET PROMPT... [--name NAME] [--repo DIR]\n       kanna schedule list [--repo DIR]\n       kanna schedule remove NAME|ID [--repo DIR]\n       kanna schedule pause NAME|ID [--repo DIR]\n       kanna schedule resume NAME|ID [--repo DIR]\n       kanna schedule run NAME|ID [--repo DIR]\n       kanna schedule reply NAME|ID TEXT... [--repo DIR]",
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
        ("kanna schedule pause evening", "Stop it firing; a time missed while paused fires once on resume."),
        ("kanna schedule resume evening", "Let it fire again."),
        ("kanna schedule run evening", "Leave its message now, unless its last still waits."),
        ("kanna schedule reply evening \"Three branches landed.\"", "From its target's agent: show this beside the firing it was last delivered, on the Schedules tab."),
    ],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let name = args.option("--name")?;
    let repo = args.option("--repo")?;
    let action = args.positional().ok_or_else(|| usage("schedule", "say what: add, list, remove, pause, resume, run or reply"))?;
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
        "remove" | "pause" | "resume" | "run" => out["schedule"] = json!(args.positional().ok_or_else(|| usage("schedule", &format!("{action} needs a schedule's NAME or ID (s3)")))?),
        "reply" => {
            let missing = || usage("schedule", "reply needs a schedule's NAME or ID and the TEXT: kanna schedule reply evening \"All done.\"");
            out["schedule"] = json!(args.positional().ok_or_else(missing)?);
            out["text"] = json!(args.rest().ok_or_else(missing)?);
            if let Some(by) = caller() {
                out["by"] = json!(by);
            }
        }
        other => return Err(usage("schedule", &format!("unknown action \"{other}\" (add, list, remove, pause, resume, run or reply)"))),
    }
    Ok(out)
}

pub fn show(request: &Value, reply: &Value) -> String {
    let id = |s: &Value| s["schedule"].as_str().unwrap_or("?").to_string();
    let named = |s: &Value| s["name"].as_str().map_or_else(|| id(s), |n| format!("{} {n}", id(s)));
    match request["action"].as_str() {
        Some("add") => format!("{}: {} → {}, next {}", named(reply), reply["rule"].as_str().unwrap_or("?"), reply["target"].as_str().unwrap_or("?"), reply["nextText"].as_str().unwrap_or("?")),
        Some("remove") => format!("removed {}", named(reply)),
        Some("pause") => format!("paused {}", named(reply)),
        Some("resume") if reply["missed"] == true => format!("resumed {}: it missed {}, so it fires once now", named(reply), reply["nextText"].as_str().unwrap_or("?")),
        Some("resume") => format!("resumed {}, next {}", named(reply), reply["nextText"].as_str().unwrap_or("?")),
        Some("run") if reply["left"] == true => format!("ran {}: its message is left", named(reply)),
        Some("run") => format!("{}: its last message still waits, so this left none", named(reply)),
        Some("reply") => format!("replied to {}", named(reply)),
        _ => {
            let all = reply["schedules"].as_array().cloned().unwrap_or_default();
            if all.is_empty() {
                return "no schedules".into();
            }
            let rows: Vec<[String; 8]> = all
                .iter()
                .map(|s| {
                    let repo = s["repo"].as_str().map_or("-".to_string(), |r| r.rsplit('/').next().unwrap_or(r).to_string());
                    let status = [(s["paused"] == true, "paused"), (s["waiting"] == true, "waiting")].iter().filter(|(on, _)| *on).map(|(_, word)| *word).collect::<Vec<_>>().join(" ");
                    let last = s["firings"][0]["atText"].as_str().map_or(String::new(), |at| format!("last {at} {}", s["firings"][0]["state"].as_str().unwrap_or("?")));
                    let prompt: String = s["prompt"].as_str().unwrap_or("").chars().take(60).collect();
                    [id(s), s["name"].as_str().unwrap_or("-").to_string(), repo, s["rule"].as_str().unwrap_or("?").to_string(), s["target"].as_str().unwrap_or("?").to_string(), format!("next {}", s["nextText"].as_str().unwrap_or("?")), status, format!("{last}  {prompt}")]
                })
                .collect();
            let widths: Vec<usize> = (0..7).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
            rows.iter()
                .map(|r| {
                    let cells: Vec<String> = (0..7).map(|i| format!("{:w$}", r[i], w = widths[i])).collect();
                    format!("{}  {}", cells.join("  "), r[7].trim_start()).trim_end().to_string()
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
        assert_eq!(parse("kanna schedule pause evening --repo /tmp").unwrap().1, json!({ "action": "pause", "repo": "/tmp", "schedule": "evening" }));
        assert_eq!(parse("kanna schedule resume s3 --repo /tmp").unwrap().1, json!({ "action": "resume", "repo": "/tmp", "schedule": "s3" }));
        assert_eq!(parse("kanna schedule run s3 --repo /tmp").unwrap().1, json!({ "action": "run", "repo": "/tmp", "schedule": "s3" }));
        assert_eq!(parse("kanna schedule reply s3 All green. --repo /tmp").unwrap().1["text"], "All green.");
        assert!(refused("kanna schedule pause").contains("pause needs a schedule's NAME or ID"));
        assert!(refused("kanna schedule reply s3").contains("reply needs"));
        assert!(refused("kanna schedule nap x").contains("unknown action"));
    }

    #[test]
    fn schedules_list_as_columns_with_what_s_waiting() {
        let reply = json!({ "schedules": [
            { "schedule": "s1", "name": "quiz", "repo": "/Users/x/work/kanna-v4", "rule": "daily 09:00", "target": "planner", "nextText": "Sat 17 Oct 09:00", "waiting": true, "prompt": "Morning quiz check." },
            { "schedule": "s12", "name": null, "repo": null, "rule": "every 2h", "target": "71af", "nextText": "Fri 16 Oct 14:00", "waiting": false, "prompt": "Run the tests." },
        ]});
        assert_eq!(show(&json!({ "action": "list" }), &reply), "s1   quiz  kanna-v4  daily 09:00  planner  next Sat 17 Oct 09:00  waiting  Morning quiz check.\ns12  -     -         every 2h     71af     next Fri 16 Oct 14:00           Run the tests.");
        let fired = json!({ "schedules": [{ "schedule": "s3", "name": "pull", "repo": null, "rule": "every 2h", "target": "71af", "nextText": "Fri 16 Oct 14:00", "paused": true, "waiting": false, "prompt": "Pull.", "firings": [{ "atText": "Thu 15 Oct 12:00", "state": "delivered" }] }]});
        assert_eq!(show(&json!({ "action": "list" }), &fired), "s3  pull  -  every 2h  71af  next Fri 16 Oct 14:00  paused  last Thu 15 Oct 12:00 delivered  Pull.");
        assert_eq!(show(&json!({ "action": "pause" }), &json!({ "schedule": "s3", "name": "pull" })), "paused s3 pull");
        assert_eq!(show(&json!({ "action": "resume" }), &json!({ "schedule": "s3", "name": "pull", "nextText": "Fri 16 Oct 14:00", "missed": true })), "resumed s3 pull: it missed Fri 16 Oct 14:00, so it fires once now");
        assert_eq!(show(&json!({ "action": "run" }), &json!({ "schedule": "s3", "name": "pull", "left": false })), "s3 pull: its last message still waits, so this left none");
        assert_eq!(show(&json!({ "action": "list" }), &json!({ "schedules": [] })), "no schedules");
        assert_eq!(show(&json!({ "action": "add" }), &reply["schedules"][0]), "s1 quiz: daily 09:00 → planner, next Sat 17 Oct 09:00");
        assert_eq!(show(&json!({ "action": "remove" }), &json!({ "schedule": "s12", "name": null })), "removed s12");
    }
}
