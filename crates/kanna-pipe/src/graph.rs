use crate::{Kind, Pipeline, Stage, Verdict, DONE};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Next,
    Down,
    Up,
    Over,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Next => "next",
            Route::Down => "down",
            Route::Up => "up",
            Route::Over => "over",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: &'static str,
    pub who: String,
    pub col: usize,
    pub row: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub arrow: usize,
    pub from: String,
    pub to: String,
    pub route: Route,
    pub label: String,
    pub fail: bool,
    pub lane: usize,
    pub lanes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub columns: usize,
    pub rows: usize,
}

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

const SHELLS: &[&str] = &["sh", "bash", "zsh", "python", "python3", "node", "ruby", "perl", "exec", "env"];

fn base(word: &str) -> String {
    word.rsplit('/').next().unwrap_or(word).to_string()
}

pub fn who(stage: &Stage) -> String {
    let what = match &stage.kind {
        Kind::Agent(agent) => match agent.attempts.as_ref() {
            Some(attempts) => format!("{} ×{}", agent.agent, attempts.count),
            None => agent.agent.clone(),
        },
        Kind::Run { command } => {
            let words: Vec<&str> = command.split_whitespace().filter(|w| !w.contains('=')).collect();
            let named = words.iter().position(|w| !SHELLS.contains(&base(w).as_str())).map_or(words.first(), |i| words.get(i));
            named.map(|w| base(w)).unwrap_or_default()
        }
        Kind::Ask { .. } => "you".into(),
        Kind::Merge(_) => "git".into(),
    };
    match stage.on.as_deref().filter(|on| *on != "here") {
        Some(on) => format!("{what} · {on}"),
        None => what,
    }
}

fn kind(stage: &Stage) -> &'static str {
    match &stage.kind {
        Kind::Agent(_) => "agent",
        Kind::Run { .. } => "run",
        Kind::Ask { .. } => "ask",
        Kind::Merge(_) => "merge",
    }
}

fn main_line(pipeline: &Pipeline) -> Vec<String> {
    let mut line = vec![];
    let mut at = Some(pipeline.start.clone());
    while let Some(stage) = at.take() {
        line.push(stage.clone());
        if stage == DONE {
            break;
        }
        let next = pipeline.arrows.iter().find(|a| a.from == stage && a.when != Some(Verdict::Fail) && !line.contains(&a.to));
        at = next.map(|a| a.to.clone());
    }
    if !line.iter().any(|s| s == DONE) {
        line.push(DONE.into());
    }
    line
}

pub fn layout(pipeline: &Pipeline) -> Graph {
    let line = main_line(pipeline);
    let mut place: BTreeMap<String, (usize, usize)> = line.iter().enumerate().map(|(col, id)| (id.clone(), (col, 0))).collect();
    let mut rest: Vec<&Stage> = pipeline.stages.iter().filter(|s| !place.contains_key(&s.name)).collect();
    while !rest.is_empty() {
        let at = rest.iter().position(|s| pipeline.arrows.iter().any(|a| (a.from == s.name && place.contains_key(&a.to)) || (a.to == s.name && place.contains_key(&a.from)))).unwrap_or(0);
        let stage = rest.remove(at);
        let mut cols: Vec<usize> = pipeline
            .arrows
            .iter()
            .filter_map(|a| if a.from == stage.name { place.get(&a.to) } else if a.to == stage.name { place.get(&a.from) } else { None })
            .filter(|(_, row)| *row == 0)
            .map(|(col, _)| *col)
            .collect();
        cols.sort_unstable();
        cols.dedup();
        let col = cols.get(cols.len() / 2).copied().unwrap_or(0);
        let mut row = 1;
        while place.values().any(|&(c, r)| c == col && r == row) {
            row += 1;
        }
        place.insert(stage.name.clone(), (col, row));
    }
    let ids = line.iter().chain(pipeline.stages.iter().map(|s| &s.name).filter(|n| !line.contains(n)));
    let nodes: Vec<Node> = ids
        .map(|id| {
            let stage = pipeline.stage(id);
            let (col, row) = place[id];
            Node { id: id.clone(), kind: stage.map_or("done", kind), who: stage.map_or_else(String::new, who), col, row }
        })
        .collect();
    let mut edges: Vec<Edge> = pipeline
        .arrows
        .iter()
        .enumerate()
        .filter(|(_, a)| place.contains_key(&a.from) && place.contains_key(&a.to))
        .map(|(index, arrow)| {
            let ((fc, fr), (tc, tr)) = (place[&arrow.from], place[&arrow.to]);
            let route = if fr < tr {
                Route::Down
            } else if fr > tr {
                Route::Up
            } else if tc == fc + 1 {
                Route::Next
            } else {
                Route::Over
            };
            Edge { arrow: index, from: arrow.from.clone(), to: arrow.to.clone(), route, label: arrow.options().join(" · "), fail: arrow.when == Some(Verdict::Fail), lane: 0, lanes: 1 }
        })
        .collect();
    let mut groups: BTreeMap<(String, &'static str), Vec<usize>> = BTreeMap::new();
    for (i, edge) in edges.iter().enumerate() {
        let key = match edge.route {
            Route::Down => (edge.to.clone(), "down"),
            Route::Over => (String::new(), "over"),
            _ => continue,
        };
        groups.entry(key).or_default().push(i);
    }
    for ((_, route), mut members) in groups {
        if route == "down" {
            members.sort_by_key(|&i| place[&edges[i].from].0);
        } else {
            members.sort_by_key(|&i| {
                let (a, b) = (place[&edges[i].from].0, place[&edges[i].to].0);
                (a.abs_diff(b), a.min(b))
            });
        }
        let lanes = members.len();
        for (lane, i) in members.into_iter().enumerate() {
            edges[i].lane = lane;
            edges[i].lanes = lanes;
        }
    }
    let columns = place.values().map(|(c, _)| c + 1).max().unwrap_or(0);
    let rows = place.values().map(|(_, r)| r + 1).max().unwrap_or(0);
    Graph { nodes, edges, columns, rows }
}
