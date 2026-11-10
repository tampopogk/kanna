mod check;
mod graph;
mod parse;
mod revise;
mod walk;

pub use check::Context;
pub use graph::{layout, who, Edge, Graph, Node, Route};
pub use parse::parse;
pub use revise::{edited, emitted, Revision};

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

pub const DONE: &str = "done";
pub const AGENTS: &[&str] = &["claude", "codex", "opencode", "agy"];
pub const ASSUMED_PARTS: u32 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Pipeline {
    pub stages: Vec<Stage>,
    pub arrows: Vec<Arrow>,
    pub budget: Option<Budget>,
    pub start: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub name: String,
    pub line: usize,
    pub kind: Kind,
    pub on: Option<String>,
    pub within: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Agent(Box<Agent>),
    Run { command: String },
    Ask { question: String },
    Merge(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agent {
    pub agent: String,
    pub effort: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub verdict: bool,
    pub each: Option<Output>,
    pub attempts: Option<Attempts>,
    pub guard: Option<String>,
    pub emits: bool,
    pub resume: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Output {
    pub stage: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attempts {
    pub count: u32,
    pub by: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verdict {
    Pass,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arrow {
    pub from: String,
    pub to: String,
    pub when: Option<Verdict>,
    pub max: Option<u32>,
    pub reset: bool,
    pub progress: bool,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub sessions: Option<u32>,
    pub time: Option<Duration>,
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Syntax,
    Redefined,
    Undefined,
    AfterDone,
    Verdict,
    Progress,
    Reset,
    Unreachable,
    NoDone,
    Unbounded,
    Each,
    Merge,
    Scorer,
    Resume,
    Prompt,
    Placement,
    Budget,
    Emits,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub line: usize,
    pub message: String,
    pub rule: Rule,
}

pub type Sizes = BTreeMap<String, u32>;

impl Error {
    pub fn new(line: usize, rule: Rule, message: impl Into<String>) -> Error {
        Error { line, message: message.into(), rule }
    }

    pub fn at(&self, file: &str) -> String {
        format!("{file}:{}: {}", self.line, self.message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.line, self.message)
    }
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
        }
    }
}

impl Stage {
    pub fn agent(&self) -> Option<&Agent> {
        match &self.kind {
            Kind::Agent(agent) => Some(agent),
            _ => None,
        }
    }

    pub fn is_run(&self) -> bool {
        matches!(self.kind, Kind::Run { .. })
    }

    pub fn verdicts(&self) -> bool {
        match &self.kind {
            Kind::Agent(agent) => agent.verdict,
            _ => true,
        }
    }

    pub fn prompt(&self) -> Option<&str> {
        self.agent().map(|agent| agent.prompt.as_deref().unwrap_or(&self.name))
    }

    pub fn what(&self) -> &'static str {
        match &self.kind {
            Kind::Agent(_) => "an agent stage",
            Kind::Run { .. } => "a run stage",
            Kind::Ask { .. } => "an ask stage",
            Kind::Merge(_) => "a merge stage",
        }
    }

    pub fn sessions(&self, sizes: &Sizes) -> u32 {
        let Some(agent) = self.agent() else { return 0 };
        let attempts = agent.attempts.as_ref().map_or(1, |a| a.count);
        let parts = agent.each.as_ref().map_or(1, |each| sizes.get(&each.to_string()).copied().unwrap_or(ASSUMED_PARTS));
        attempts * parts
    }
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}", self.stage, self.key)
    }
}

impl Arrow {
    pub fn options(&self) -> Vec<String> {
        let mut options = Vec::new();
        options.extend(self.when.map(|v| v.name().to_string()));
        options.extend(self.max.map(|n| format!("max {n}")));
        if self.reset {
            options.push("reset".into());
        }
        if self.progress {
            options.push("progress".into());
        }
        options
    }

    pub fn render(&self) -> String {
        let options = self.options();
        if options.is_empty() {
            format!("{} -> {}", self.from, self.to)
        } else {
            format!("{} -[{}]-> {}", self.from, options.join(", "), self.to)
        }
    }

    fn same(&self, other: &Arrow) -> bool {
        (&self.from, &self.to, self.when, self.max, self.reset, self.progress) == (&other.from, &other.to, other.when, other.max, other.reset, other.progress)
    }
}

impl Pipeline {
    pub fn stage(&self, name: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.name == name)
    }

    pub fn pick(&self, stage: &str, verdict: Option<Verdict>, taken: &[u32], stalled: bool) -> Option<(usize, &Arrow)> {
        self.arrows.iter().enumerate().find(|(index, arrow)| {
            arrow.from == stage
                && arrow.when.is_none_or(|when| Some(when) == verdict)
                && !(stalled && arrow.progress)
                && arrow.max.is_none_or(|max| taken.get(*index).copied().unwrap_or(0) < max)
        })
    }

    pub fn worst(&self, cost: impl Fn(&Stage) -> u32) -> u32 {
        walk::worst(self, &cost)
    }

    pub fn max_sessions(&self, sizes: &Sizes) -> u32 {
        self.worst(|stage| stage.sessions(sizes))
    }

    pub fn fanouts(&self) -> Vec<&Stage> {
        self.stages.iter().filter(|s| s.agent().is_some_and(|a| a.each.is_some())).collect()
    }

    pub fn check(&self, context: &Context) -> Result<(), Vec<Error>> {
        check::check(self, context)
    }
}

pub fn load(text: &str, context: &Context) -> Result<Pipeline, Vec<Error>> {
    let pipeline = parse(text)?;
    pipeline.check(context)?;
    Ok(pipeline)
}

pub fn kojo_block(answer: &str) -> Option<String> {
    let mut lines = answer.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let fence: String = trimmed.chars().take_while(|&c| c == '`' || c == '~').collect();
        if fence.len() < 3 || trimmed[fence.len()..].split_whitespace().next() != Some("kojo") {
            continue;
        }
        let mut block = Vec::new();
        for line in lines.by_ref() {
            if line.trim_start().starts_with(&fence) && line.trim().trim_start_matches(fence.chars().next().unwrap()).is_empty() {
                return Some(block.join("\n"));
            }
            block.push(line);
        }
        return Some(block.join("\n"));
    }
    None
}
