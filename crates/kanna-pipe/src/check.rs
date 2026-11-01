use crate::{Error, Kind, Merge, Pipeline, Rule, Sizes, Stage, DONE};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub prompts: BTreeMap<String, String>,
    pub machine: Option<String>,
    pub machines: Vec<String>,
    pub roles: Vec<String>,
    pub sizes: Sizes,
}

const STRUCTURE: &[Rule] = &[Rule::Undefined, Rule::AfterDone, Rule::Verdict, Rule::Progress, Rule::Reset, Rule::Unbounded];

pub fn check(pipeline: &Pipeline, context: &Context) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    let stages: HashMap<&str, &Stage> = pipeline.stages.iter().map(|s| (s.name.as_str(), s)).collect();
    arrows(pipeline, &stages, &mut errors);
    reach(pipeline, &stages, &mut errors);
    loops(pipeline, &mut errors);
    for stage in &pipeline.stages {
        references(pipeline, &stages, stage, context, &mut errors);
        placement(stage, context, &mut errors);
    }
    if !errors.iter().any(|e| STRUCTURE.contains(&e.rule)) {
        if let Some(budget) = &pipeline.budget {
            let worst = pipeline.max_sessions(&context.sizes);
            if let Some(sessions) = budget.sessions.filter(|&n| worst > n) {
                errors.push(Error::new(budget.line, Rule::Budget, format!("the worst case is {worst} sessions, over the budget of {sessions}")));
            }
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    errors.sort_by_key(|e| e.line);
    errors.dedup();
    Err(errors)
}

fn arrows(pipeline: &Pipeline, stages: &HashMap<&str, &Stage>, errors: &mut Vec<Error>) {
    for arrow in &pipeline.arrows {
        if arrow.from == DONE {
            errors.push(Error::new(arrow.line, Rule::AfterDone, "nothing can follow 'done': it ends the run"));
            continue;
        }
        for end in [&arrow.from, &arrow.to] {
            if end != DONE && !stages.contains_key(end.as_str()) {
                errors.push(Error::new(arrow.line, Rule::Undefined, format!("no stage named '{end}'")));
            }
        }
        let (Some(from), to) = (stages.get(arrow.from.as_str()), stages.get(arrow.to.as_str())) else { continue };
        if let Some(when) = arrow.when.filter(|_| !from.verdicts()) {
            errors.push(Error::new(arrow.line, Rule::Verdict, format!("[{}] needs a stage with a verdict, and '{}' gives none: add verdict to it, or make it a run, ask or merge stage", when.name(), from.name)));
        }
        if arrow.progress && !from.is_run() {
            errors.push(Error::new(arrow.line, Rule::Progress, format!("[progress] needs a run stage, and '{}' is {}", from.name, from.what())));
        }
        if arrow.reset && to.is_none_or(|to| to.agent().is_none()) {
            let what = to.map_or("done", |to| to.what());
            errors.push(Error::new(arrow.line, Rule::Reset, format!("[reset] must lead to an agent stage, and '{}' is {what}", arrow.to)));
        }
    }
}

fn reachable(pipeline: &Pipeline, avoid: Option<&str>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![pipeline.start.clone()];
    while let Some(stage) = todo.pop() {
        if Some(stage.as_str()) == avoid || !seen.insert(stage.clone()) || stage == DONE {
            continue;
        }
        todo.extend(pipeline.arrows.iter().filter(|a| a.from == stage).map(|a| a.to.clone()));
    }
    seen
}

fn runs_before(pipeline: &Pipeline, first: &str, then: &str) -> bool {
    first != then && reachable(pipeline, None).contains(first) && !reachable(pipeline, Some(first)).contains(then)
}

fn reach(pipeline: &Pipeline, stages: &HashMap<&str, &Stage>, errors: &mut Vec<Error>) {
    if !stages.contains_key(pipeline.start.as_str()) {
        return;
    }
    let seen = reachable(pipeline, None);
    for stage in pipeline.stages.iter().filter(|s| !seen.contains(&s.name)) {
        errors.push(Error::new(stage.line, Rule::Unreachable, format!("stage '{}' is never reached from '{}', where the run starts", stage.name, pipeline.start)));
    }
    if !seen.contains(DONE) {
        let line = pipeline.arrows.first().map_or(1, |a| a.line);
        errors.push(Error::new(line, Rule::NoDone, format!("no flow from '{}' reaches done, so the run could only end stuck", pipeline.start)));
    }
}

fn loops(pipeline: &Pipeline, errors: &mut Vec<Error>) {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Open,
        Closed,
    }
    fn visit(pipeline: &Pipeline, stage: &str, state: &mut HashMap<String, State>) -> Option<usize> {
        match state.get(stage) {
            Some(State::Closed) => return None,
            Some(State::Open) => return Some(0),
            None => {}
        }
        state.insert(stage.into(), State::Open);
        for arrow in pipeline.arrows.iter().filter(|a| a.from == stage && a.max.is_none() && a.to != DONE) {
            if let Some(line) = visit(pipeline, &arrow.to, state) {
                return Some(if line == 0 { arrow.line } else { line });
            }
        }
        state.insert(stage.into(), State::Closed);
        None
    }
    let mut state = HashMap::new();
    for stage in &pipeline.stages {
        if let Some(line) = visit(pipeline, &stage.name, &mut state) {
            errors.push(Error::new(line, Rule::Unbounded, "this loop has no [max N] on any of its arrows, so the run might never end"));
            return;
        }
    }
}

fn references(pipeline: &Pipeline, stages: &HashMap<&str, &Stage>, stage: &Stage, context: &Context, errors: &mut Vec<Error>) {
    let mut fail = |rule, message: String| errors.push(Error::new(stage.line, rule, message));
    if let Kind::Merge(Merge::Stage(from)) = &stage.kind {
        if !stages.get(from.as_str()).and_then(|s| s.agent()).is_some_and(|a| a.each.is_some()) {
            fail(Rule::Merge, format!("merge {from} needs '{from}' to be a stage with each"));
        }
    }
    let Some(agent) = stage.agent() else { return };
    if let Some(each) = &agent.each {
        match stages.get(each.stage.as_str()) {
            Some(from) if from.agent().is_none() => fail(Rule::Each, format!("each {each} needs an agent stage's answer, and '{}' is {}", each.stage, from.what())),
            Some(_) if !runs_before(pipeline, &each.stage, &stage.name) => fail(Rule::Each, format!("each {each} needs '{}' to run before '{}' on every path", each.stage, stage.name)),
            Some(_) => {}
            None => fail(Rule::Each, format!("each {each}: no stage named '{}'", each.stage)),
        }
    }
    let scorers = agent.attempts.iter().map(|a| ("by", &a.by)).chain(agent.guard.iter().map(|g| ("guard", g)));
    for (word, scorer) in scorers {
        match stages.get(scorer.as_str()) {
            Some(found) if found.is_run() => {}
            Some(found) => fail(Rule::Scorer, format!("{word} {scorer} needs a run stage to score the code, and '{scorer}' is {}", found.what())),
            None => fail(Rule::Scorer, format!("{word} {scorer}: no stage named '{scorer}'")),
        }
    }
    if let Some(resumed) = &agent.resume {
        if !stages.get(resumed.as_str()).is_some_and(|s| s.agent().is_some()) {
            fail(Rule::Resume, format!("resume {resumed} needs '{resumed}' to be an agent stage"));
        }
    }
    let prompt = stage.prompt().unwrap_or_default();
    let Some(text) = context.prompts.get(prompt) else {
        fail(Rule::Prompt, format!("no prompt .kanna/prompts/{prompt}.md"));
        return;
    };
    for (from, _) in placeholders(text) {
        if !stages.contains_key(from.as_str()) {
            fail(Rule::Prompt, format!("prompt {prompt} uses {{{{{from}.…}}}}, and there's no stage named '{from}'"));
        } else if !runs_before(pipeline, &from, &stage.name) {
            fail(Rule::Prompt, format!("prompt {prompt} uses {{{{{from}.…}}}}, but '{from}' doesn't run before '{}' on every path", stage.name));
        }
    }
}

pub(crate) fn placeholders(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find("}}") else { break };
        if let Some((stage, key)) = rest[..close].trim().split_once('.').filter(|(s, k)| crate::parse::is_name(s) && !k.is_empty()) {
            found.push((stage.to_string(), key.to_string()));
        }
        rest = &rest[close + 2..];
    }
    found
}

fn placement(stage: &Stage, context: &Context, errors: &mut Vec<Error>) {
    let Some(on) = &stage.on else { return };
    let known = on == "here" || on == "any" || context.machine.as_ref() == Some(on) || context.machines.contains(on) || context.roles.contains(on);
    if !known {
        errors.push(Error::new(stage.line, Rule::Placement, format!("no such machine '{on}': use here, any, or one of your machines or roles")));
    }
}
