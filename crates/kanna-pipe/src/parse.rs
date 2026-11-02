use crate::{Agent, Arrow, Attempts, Budget, Error, Kind, Output, Pipeline, Rule, Stage, Verdict, AGENTS, DONE};
use std::time::Duration;

pub(crate) enum Line {
    Stage(Box<Stage>),
    Flow(Vec<String>, Vec<Arrow>),
    Budget(Budget),
}

pub fn parse(text: &str) -> Result<Pipeline, Vec<Error>> {
    let (lines, mut errors) = read(text);
    let mut stages: Vec<Stage> = Vec::new();
    let mut arrows = Vec::new();
    let mut budget: Option<Budget> = None;
    let mut start = None;
    for line in lines {
        match line {
            Line::Stage(stage) => {
                let stage = *stage;
                if let Some(first) = stages.iter().find(|s| s.name == stage.name) {
                    errors.push(Error::new(stage.line, Rule::Redefined, format!("stage '{}' is defined twice (first on line {})", stage.name, first.line)));
                } else {
                    stages.push(stage);
                }
            }
            Line::Flow(nodes, found) => {
                start = start.or_else(|| nodes.first().cloned());
                arrows.extend(found);
            }
            Line::Budget(found) => match &budget {
                Some(first) => errors.push(Error::new(found.line, Rule::Syntax, format!("the budget is given twice (first on line {})", first.line))),
                None => budget = Some(found),
            },
        }
    }
    if arrows.is_empty() && stages.len() == 1 {
        let only = &stages[0];
        start = Some(only.name.clone());
        arrows.push(Arrow { from: only.name.clone(), to: DONE.into(), when: None, max: None, reset: false, progress: false, line: only.line });
    }
    let start = match start {
        Some(start) => start,
        None if stages.is_empty() => {
            if errors.is_empty() {
                errors.push(Error::new(1, Rule::Syntax, "define at least one stage, and a flow starting at it"));
            }
            String::new()
        }
        None if !errors.is_empty() => String::new(),
        None => {
            errors.push(Error::new(stages[0].line, Rule::Syntax, "a pipeline of several stages needs a flow line (`a -> b`); its first stage starts the run"));
            String::new()
        }
    };
    if !errors.is_empty() {
        errors.sort_by_key(|e| e.line);
        return Err(errors);
    }
    Ok(Pipeline { stages, arrows, budget, start, text: text.to_string() })
}

pub(crate) fn read(text: &str) -> (Vec<Line>, Vec<Error>) {
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = uncomment(raw).trim();
        if line.is_empty() {
            continue;
        }
        match read_line(line, number) {
            Ok(found) => lines.push(found),
            Err(message) => errors.push(Error::new(number, Rule::Syntax, message)),
        }
    }
    (lines, errors)
}

pub(crate) fn uncomment(raw: &str) -> &str {
    let mut quoted = false;
    for (at, c) in raw.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &raw[..at],
            _ => {}
        }
    }
    raw
}

fn read_line(line: &str, number: usize) -> Result<Line, String> {
    if let Some((name, body)) = definition(line) {
        if name == "budget" {
            return budget(body, number).map(Line::Budget);
        }
        if name == DONE {
            return Err("'done' is reserved: it ends the run".into());
        }
        return stage(name, body, number).map(|s| Line::Stage(Box::new(s)));
    }
    if line.contains("->") {
        return flow(line, number).map(|(nodes, arrows)| Line::Flow(nodes, arrows));
    }
    if let Some((name, _)) = line.split_once('=') {
        return Err(format!("expected a stage name ([a-z][a-z0-9_]*), got '{}'", name.trim()));
    }
    Err("expected a stage (`name = …`), a flow (`a -> b`) or the budget (`budget = N sessions, 8h`)".into())
}

fn definition(line: &str) -> Option<(&str, &str)> {
    let end = line.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')).unwrap_or(line.len());
    let (name, rest) = line.split_at(end);
    let body = rest.trim_start().strip_prefix('=')?;
    is_name(name).then_some((name, body.trim()))
}

pub(crate) fn is_name(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn name(word: &str, what: &str) -> Result<String, String> {
    if is_name(word) {
        Ok(word.to_string())
    } else {
        Err(format!("{what} needs a stage name, got '{word}'"))
    }
}

pub(crate) fn duration(word: &str) -> Option<Duration> {
    let mut total = 0u64;
    let mut digits = String::new();
    let mut units = 0;
    for c in word.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let scale = match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        total += digits.parse::<u64>().ok()? * scale;
        digits.clear();
        units += 1;
    }
    (digits.is_empty() && units > 0 && total > 0).then(|| Duration::from_secs(total))
}

fn budget(body: &str, number: usize) -> Result<Budget, String> {
    let mut found = Budget { sessions: None, time: None, line: number };
    for part in body.split(',').map(str::trim) {
        let words: Vec<&str> = part.split_whitespace().collect();
        match words.as_slice() {
            [count, "sessions" | "session"] if found.sessions.is_none() => {
                found.sessions = Some(count.parse().ok().filter(|&n: &u32| n > 0).ok_or_else(|| format!("expected a number of sessions, got '{count}'"))?);
            }
            [time] if found.time.is_none() && duration(time).is_some() => found.time = duration(time),
            _ => return Err(format!("expected `budget = N sessions, DURATION` (DURATION like 8h or 90m), got '{part}'")),
        }
    }
    Ok(found)
}

fn flow(line: &str, number: usize) -> Result<(Vec<String>, Vec<Arrow>), String> {
    let mut nodes = Vec::new();
    let mut options = Vec::new();
    let mut rest = line;
    loop {
        rest = rest.trim_start();
        let end = rest.find(|c: char| c.is_whitespace() || c == '-').unwrap_or(rest.len());
        let (node, after) = rest.split_at(end);
        if node.is_empty() {
            return Err("expected a stage name after an arrow".into());
        }
        nodes.push(name(node, "a flow")?);
        rest = after.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after) = rest.strip_prefix("->") {
            options.push(None);
            rest = after;
        } else if let Some(after) = rest.strip_prefix("-[") {
            let close = after.find("]->").ok_or("an arrow's options end with `]->`, as in `a -[fail, max 3]-> b`")?;
            options.push(Some(&after[..close]));
            rest = &after[close + 3..];
        } else {
            return Err(format!("expected `->` or `-[…]->` after '{node}'"));
        }
    }
    let mut arrows = Vec::new();
    for ((from, to), option) in nodes.iter().zip(nodes.iter().skip(1)).zip(options) {
        arrows.push(arrow(from, to, option.unwrap_or(""), number)?);
    }
    Ok((nodes, arrows))
}

fn arrow(from: &str, to: &str, options: &str, number: usize) -> Result<Arrow, String> {
    let mut found = Arrow { from: from.into(), to: to.into(), when: None, max: None, reset: false, progress: false, line: number };
    for item in options.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        let max = item.strip_prefix("max").filter(|n| n.starts_with([' ', '='])).map(|n| n[1..].trim());
        match item {
            "pass" | "fail" if found.when.is_none() => found.when = Some(if item == "pass" { Verdict::Pass } else { Verdict::Fail }),
            "reset" if !found.reset => found.reset = true,
            "progress" if !found.progress => found.progress = true,
            _ if max.is_some() && found.max.is_none() => {
                found.max = Some(max.and_then(|n| n.parse().ok()).filter(|&n: &u32| n > 0).ok_or_else(|| format!("`{item}`: max takes a whole number above 0"))?);
            }
            _ => return Err(format!("unknown or repeated arrow option '{item}'; use pass or fail, max N, reset and progress, each once")),
        }
    }
    Ok(found)
}

fn words(body: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in body.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                found.push((from, &body[from..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        found.push((from, &body[from..]));
    }
    found
}

fn stage(name: &str, body: &str, number: usize) -> Result<Stage, String> {
    let mut stage = Stage { name: name.into(), line: number, kind: Kind::Run { command: String::new() }, on: None, within: None };
    let all = words(body);
    let Some(&(_, first)) = all.first() else {
        return Err(format!("stage '{name}' needs a body: an agent (claude, codex, …), `run COMMAND`, `ask \"QUESTION\"` or `merge STAGE`"));
    };
    match first {
        "run" => {
            let mut rest = &all[1..];
            while let [head @ .., (_, key @ ("on" | "within")), (_, value)] = rest {
                if !placed(&mut stage, key, value)? {
                    break;
                }
                rest = head;
            }
            let command = match rest {
                [] => "",
                [(from, _), .., (last, word)] => body[*from..last + word.len()].trim(),
                [(from, word)] => &body[*from..from + word.len()],
            };
            if command.is_empty() {
                return Err("`run` needs a command".into());
            }
            stage.kind = Kind::Run { command: command.into() };
        }
        "ask" => {
            let after = body[3..].trim_start();
            let question = after.strip_prefix('"').and_then(|q| q.split_once('"'));
            let Some((question, rest)) = question.filter(|(q, _)| !q.trim().is_empty()) else {
                return Err("`ask` needs a question in double quotes: ask \"Is the plan right?\"".into());
            };
            only_placement(&mut stage, rest, "ask")?;
            stage.kind = Kind::Ask { question: question.into() };
        }
        "merge" => {
            let Some(&(at, into)) = all.get(1) else { return Err("`merge` needs a fanned-out stage: merge build".into()) };
            if into == "main" {
                return Err("`merge main` isn't a stage: a run hands its branch back to the task's worktree, and you merge from there".into());
            }
            let into = self::name(into, "merge")?;
            only_placement(&mut stage, &body[at + all[1].1.len()..], "merge")?;
            stage.kind = Kind::Merge(into);
        }
        _ => stage.kind = Kind::Agent(Box::new(agent(&mut stage, &all)?)),
    }
    Ok(stage)
}

fn placed(stage: &mut Stage, key: &str, value: &str) -> Result<bool, String> {
    match key {
        "on" if stage.on.is_none() => {
            stage.on = Some(value.into());
            Ok(true)
        }
        "within" if stage.within.is_none() => {
            stage.within = Some(duration(value).ok_or_else(|| format!("within takes a duration like 30m or 2h, got '{value}'"))?);
            Ok(true)
        }
        "on" | "within" => Err(format!("{key} is given twice")),
        _ => Ok(false),
    }
}

fn only_placement(stage: &mut Stage, rest: &str, what: &str) -> Result<(), String> {
    let rest: Vec<&str> = rest.split_whitespace().collect();
    for pair in rest.chunks(2) {
        match pair {
            [key, value] if placed(stage, key, value)? => {}
            _ => return Err(format!("a {what} stage takes only `on PLACEMENT` and `within DURATION` after it, got '{}'", pair.join(" "))),
        }
    }
    Ok(())
}

fn agent(stage: &mut Stage, all: &[(usize, &str)]) -> Result<Agent, String> {
    let (tool, effort) = match all[0].1.split_once(':') {
        Some((tool, effort)) => (tool, Some(effort)),
        None => (all[0].1, None),
    };
    if !AGENTS.contains(&tool) {
        return Err(format!("unknown agent '{tool}'; use {}, or run, ask or merge", AGENTS.join(", ")));
    }
    let efforts: &[&str] = if tool == "codex" { &["low", "medium", "high", "xhigh"] } else { &["low", "medium", "high"] };
    if let Some(effort) = effort.filter(|e| !efforts.contains(e)) {
        return Err(format!("{tool} takes the efforts {}, not '{effort}'", efforts.join(", ")));
    }
    let mut agent = Agent { agent: tool.into(), effort: effort.map(String::from), ..Agent::default() };
    let mut words = all[1..].iter().map(|(_, w)| *w);
    let mut seen: Vec<&str> = Vec::new();
    while let Some(word) = words.next() {
        let key = if word.starts_with('x') && word[1..].parse::<u32>().is_ok() { "xN" } else { word };
        if seen.contains(&key) {
            return Err(format!("{key} is given twice"));
        }
        seen.push(key);
        let mut value = |what: &str| words.next().ok_or_else(|| format!("{word} needs {what}"));
        match key {
            "verdict" => agent.verdict = true,
            "emits" => agent.emits = true,
            "prompt" => {
                let prompt = value("a prompt's name")?;
                if !prompt.chars().next().is_some_and(|c| c.is_ascii_lowercase()) || !prompt.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
                    return Err(format!("prompt takes a name like review or fix-tests (.kanna/prompts/NAME.md), got '{prompt}'"));
                }
                agent.prompt = Some(prompt.into());
            }
            "model" => agent.model = Some(value("a model's name")?.into()),
            "each" => {
                let output = value("STAGE.KEY, a named list a stage before it produced")?;
                let Some((from, key)) = output.split_once('.').filter(|(s, k)| is_name(s) && !k.is_empty()) else {
                    return Err(format!("each takes STAGE.KEY, as in each plan.parts, got '{output}'"));
                };
                agent.each = Some(Output { stage: from.into(), key: key.into() });
            }
            "xN" => {
                let count: u32 = word[1..].parse().unwrap_or(0);
                if !(2..=10).contains(&count) {
                    return Err("xN takes 2 to 10 attempts".into());
                }
                if words.next() != Some("by") {
                    return Err(format!("{word} needs `by STAGE`: the run stage that scores each attempt"));
                }
                let by = words.next().ok_or("by needs a run stage that scores each attempt")?;
                agent.attempts = Some(Attempts { count, by: name(by, "by")? });
            }
            "by" => return Err("`by STAGE` goes after xN, as in x3 by check".into()),
            "guard" => agent.guard = Some(name(value("a run stage that scores the code")?, "guard")?),
            "resume" => agent.resume = Some(name(value("the stage whose session it continues")?, "resume")?),
            "on" | "within" => {
                let given = value(if word == "on" { "a placement: here, any, a machine or a role" } else { "a duration like 30m" })?;
                placed(stage, word, given)?;
            }
            _ => return Err(format!("unexpected '{word}'; an agent stage takes prompt NAME, verdict, each STAGE.KEY, xN by STAGE, guard STAGE, emits, on PLACEMENT, within DURATION, resume STAGE and model NAME")),
        }
    }
    Ok(agent)
}
