use crate::{Pipeline, Stage, Verdict, DONE};
use std::collections::HashMap;

pub fn worst(pipeline: &Pipeline, cost: &dyn Fn(&Stage) -> u32) -> u32 {
    let mut seen = HashMap::new();
    let taken = vec![0; pipeline.arrows.len()];
    walk(pipeline, cost, &pipeline.start, taken, &mut seen)
}

fn walk(pipeline: &Pipeline, cost: &dyn Fn(&Stage) -> u32, stage: &str, taken: Vec<u32>, seen: &mut HashMap<(String, Vec<u32>), u32>) -> u32 {
    let key = (stage.to_string(), taken);
    if let Some(&known) = seen.get(&key) {
        return known;
    }
    let Some(here) = pipeline.stage(stage) else { return 0 };
    let progress = pipeline.arrows.iter().any(|a| a.from == stage && a.progress);
    let mut cases = vec![];
    if here.verdicts() {
        cases.extend([(Some(Verdict::Pass), false), (Some(Verdict::Fail), false)]);
        if progress {
            cases.push((Some(Verdict::Fail), true));
        }
    } else {
        cases.push((None, false));
    }
    let mut best = 0;
    for (verdict, stalled) in cases {
        if let Some((index, arrow)) = pipeline.pick(stage, verdict, &key.1, stalled) {
            if arrow.to != DONE {
                let mut next = key.1.clone();
                next[index] += 1;
                best = best.max(walk(pipeline, cost, &arrow.to, next, seen));
            }
        }
    }
    let total = cost(here) + best;
    seen.insert(key, total);
    total
}
