use crate::parse::{read, Line};
use crate::{parse, Context, Error, Pipeline, Rule};

#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    pub pipeline: Pipeline,
    pub carried: Vec<Option<usize>>,
}

impl Revision {
    pub fn taken(&self, before: &[u32]) -> Vec<u32> {
        self.carried.iter().map(|old| old.and_then(|i| before.get(i).copied()).unwrap_or(0)).collect()
    }
}

#[derive(Clone, Copy)]
enum Origin {
    Line(usize),
    Arrow(usize),
    New,
}

pub fn emitted(current: &Pipeline, emitter: &str, block: &str, context: &Context) -> Result<Revision, Vec<Error>> {
    let Some(stage) = current.stage(emitter) else {
        return Err(vec![Error::new(1, Rule::Emits, format!("no stage named '{emitter}'"))]);
    };
    if !stage.agent().is_some_and(|a| a.emits) {
        return Err(vec![Error::new(stage.line, Rule::Emits, format!("stage '{emitter}' has no emits, so its answer can't change the pipeline"))]);
    }
    let (lines, mut errors) = read(block);
    let raw: Vec<&str> = block.lines().collect();
    let mut added: Vec<(String, Origin)> = Vec::new();
    let mut ahead: Vec<(String, Origin)> = Vec::new();
    let mut after: Vec<(String, Origin)> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for line in lines {
        match line {
            Line::Stage(found) if current.stage(&found.name).is_some() || names.contains(&found.name) => {
                errors.push(Error::new(found.line, Rule::Redefined, format!("stage '{}' exists already; a pipe block can only add stages", found.name)));
            }
            Line::Stage(found) => {
                names.push(found.name.clone());
                added.push((raw[found.line - 1].trim().to_string(), Origin::New));
            }
            Line::Budget(found) => errors.push(Error::new(found.line, Rule::Emits, "a pipe block can't change the budget")),
            Line::Flow(_, arrows) => {
                for arrow in arrows {
                    let group = if arrow.from == emitter { &mut ahead } else { &mut after };
                    group.push((arrow.render(), Origin::New));
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let stage_lines: Vec<usize> = current.stages.iter().map(|s| s.line).collect();
    for (index, arrow) in current.arrows.iter().enumerate().filter(|(_, a)| stage_lines.contains(&a.line)) {
        after.insert(0, (arrow.render(), Origin::Arrow(index)));
    }
    let last_stage = stage_lines.iter().copied().max().unwrap_or(0);
    let first_arrow = current.arrows.iter().find(|a| a.from == emitter && !stage_lines.contains(&a.line)).map(|a| a.line);
    let mut out: Vec<(String, Origin)> = Vec::new();
    for (index, line) in current.text.lines().enumerate() {
        let number = index + 1;
        if Some(number) == first_arrow {
            out.append(&mut ahead);
        }
        out.push((line.to_string(), Origin::Line(number)));
        if number == last_stage {
            out.append(&mut added);
        }
    }
    out.append(&mut added);
    out.append(&mut ahead);
    out.append(&mut after);
    revision(current, &out, context)
}

pub fn edited(current: &Pipeline, text: &str, context: &Context) -> Result<Revision, Vec<Error>> {
    let pipeline = parse(text)?;
    pipeline.check(context)?;
    let mut used = vec![false; current.arrows.len()];
    let carried = pipeline
        .arrows
        .iter()
        .map(|arrow| {
            let old = current.arrows.iter().enumerate().position(|(i, old)| !used[i] && old.same(arrow))?;
            used[old] = true;
            Some(old)
        })
        .collect();
    Ok(Revision { pipeline, carried })
}

fn revision(current: &Pipeline, out: &[(String, Origin)], context: &Context) -> Result<Revision, Vec<Error>> {
    let mut text: String = out.iter().map(|(line, _)| format!("{line}\n")).collect();
    if !current.text.ends_with('\n') && current.text.lines().count() == out.len() {
        text.pop();
    }
    let pipeline = parse(&text)?;
    pipeline.check(context)?;
    let mut carried = Vec::new();
    let mut nth = (0, 0);
    for arrow in &pipeline.arrows {
        nth = if nth.0 == arrow.line { (arrow.line, nth.1 + 1) } else { (arrow.line, 0) };
        let old = match out.get(arrow.line - 1).map(|(_, origin)| *origin) {
            Some(Origin::Line(line)) => current.arrows.iter().enumerate().filter(|(_, a)| a.line == line).nth(nth.1).map(|(i, _)| i),
            Some(Origin::Arrow(index)) => Some(index),
            _ => None,
        };
        carried.push(old);
    }
    Ok(Revision { pipeline, carried })
}
