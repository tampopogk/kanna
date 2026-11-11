use kanna_pipe::{diff, edited, emitted, parse, kojo_block, Context, Rule, Sizes, Verdict};
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn context() -> Context {
    let mut context = Context { machines: vec!["mini".into()], ..Context::default() };
    for name in ["plan", "build", "fix", "review"] {
        context.prompts.insert(name.into(), std::fs::read_to_string(repo().join(format!(".kanna/prompts/{name}.md"))).unwrap());
    }
    context
}

fn feature() -> kanna_pipe::Pipeline {
    parse(&std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/examples/feature.kojo")).unwrap()).unwrap()
}

fn to(pipeline: &kanna_pipe::Pipeline, stage: &str, verdict: Option<Verdict>, taken: &[u32], stalled: bool) -> Option<String> {
    pipeline.pick(stage, verdict, taken, stalled).map(|(_, arrow)| arrow.to.clone())
}

#[test]
fn pick_takes_the_first_arrow_that_holds() {
    let pipeline = feature();
    let fresh = vec![0; pipeline.arrows.len()];
    assert_eq!(to(&pipeline, "plan", None, &fresh, false).as_deref(), Some("build"));
    assert_eq!(to(&pipeline, "check", Some(Verdict::Fail), &fresh, false).as_deref(), Some("fix"));
    assert_eq!(to(&pipeline, "check", Some(Verdict::Pass), &fresh, false).as_deref(), Some("review"));
    assert_eq!(to(&pipeline, "e2e", Some(Verdict::Pass), &fresh, false).as_deref(), Some("done"));
}

#[test]
fn pick_skips_a_spent_max_and_a_stalled_progress() {
    let pipeline = feature();
    let (index, _) = pipeline.pick("check", Some(Verdict::Fail), &vec![0; pipeline.arrows.len()], false).unwrap();
    let mut taken = vec![0; pipeline.arrows.len()];
    taken[index] = 3;
    assert_eq!(to(&pipeline, "check", Some(Verdict::Fail), &taken, false).as_deref(), Some("review"));
    assert_eq!(to(&pipeline, "check", Some(Verdict::Fail), &vec![0; pipeline.arrows.len()], true).as_deref(), Some("review"));
}

#[test]
fn nothing_eligible_is_stuck() {
    let pipeline = parse("b = claude\nc = run make\nb -> c\nc -[fail, max 1]-> b\nc -[pass]-> done").unwrap();
    assert_eq!(pipeline.pick("c", Some(Verdict::Fail), &[0, 1, 0], false), None);
}

#[test]
fn worst_takes_any_cost() {
    let pipeline = feature();
    assert_eq!(pipeline.worst(|stage| u32::from(stage.is_run())), 12);
    assert_eq!(pipeline.worst(|stage| u32::from(stage.name == "fix")), 8);
}

#[test]
fn a_progress_loop_and_its_fall_through_are_both_counted() {
    let pipeline = parse("b = claude\nc = run make\nr = claude verdict\nb -> c\nc -[fail, max 2, progress]-> b\nc -> r\nr -[fail, max 1]-> b\nr -[pass]-> done").unwrap();
    assert_eq!(pipeline.max_sessions(&Sizes::new()), 6);
}

#[test]
fn an_emitted_block_goes_ahead_of_the_emitters_arrows() {
    let pipeline = feature();
    let block = "lint = run scripts/lint.sh   # the new stage\nplan -> lint -> build\n";
    let revision = emitted(&pipeline, "plan", block, &context()).unwrap();
    let text = revision.pipeline.text.clone();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[12], "lint = run scripts/lint.sh   # the new stage");
    assert_eq!(lines[14], "plan -> lint");
    assert_eq!(lines[15], "plan -> build -> join -> check");
    assert_eq!(lines.last(), Some(&"lint -> build"));
    let fresh = vec![0; revision.pipeline.arrows.len()];
    assert_eq!(to(&revision.pipeline, "plan", None, &fresh, false).as_deref(), Some("lint"));
    assert_eq!(revision.pipeline.max_sessions(&Sizes::new()), 17);
    let mut before = vec![0; pipeline.arrows.len()];
    before[3] = 2;
    let after = revision.taken(&before);
    assert_eq!(after.len(), pipeline.arrows.len() + 2);
    assert_eq!(after[0], 0);
    assert_eq!(after[4], 2);
    assert_eq!(revision.carried[0], None);
    assert_eq!(revision.carried[1], Some(0));
    assert_eq!(revision.carried.last(), Some(&None));
}

#[test]
fn an_emitted_block_cant_redefine_a_stage_or_the_budget() {
    let pipeline = feature();
    let refused = emitted(&pipeline, "plan", "x = run true\ncheck = run true\nbudget = 99 sessions", &context()).unwrap_err();
    assert_eq!(refused.lines(), ["the kojo block:2: stage 'check' exists already; a kojo block can only add stages", "the kojo block:3: a kojo block can't change the budget"]);
    assert_eq!(refused.merged, None);
}

#[test]
fn only_an_emits_stage_can_emit() {
    let refused = emitted(&feature(), "review", "x = run true\nreview -> x", &context()).unwrap_err();
    assert_eq!(refused.lines(), ["the kojo block:1: stage 'review' has no emits, so its answer can't change the pipeline"]);
    assert_eq!(refused.errors[0].rule, Rule::Emits);
}

#[test]
fn an_emitted_revision_is_checked_whole() {
    let refused = emitted(&feature(), "plan", "lint = run true\nplan -> lint\nlint -[fail]-> plan", &context()).unwrap_err();
    assert_eq!(refused.errors.iter().map(|e| (e.rule, e.line)).collect::<Vec<_>>(), [(Rule::Unbounded, 23)]);
    assert_eq!(refused.merged.as_deref().and_then(|text| text.lines().nth(22)), Some("lint -[fail]-> plan"));
    assert!(refused.lines()[0].starts_with("the merged pipeline:23: "));
    let refused = emitted(&feature(), "plan", "more = claude x10 by check each plan.parts\nagain = claude x10 by check each plan.parts\nplan -> more -> again -> build", &context()).unwrap_err();
    assert_eq!(refused.errors.iter().map(|e| e.rule).collect::<Vec<_>>(), [Rule::Budget, Rule::Prompt, Rule::Prompt]);
}

#[test]
fn an_emitted_block_adds_stages_after_the_last_stage_and_its_other_arrows_at_the_end() {
    let pipeline = parse("budget = 4 sessions\nplan = claude emits\nlast = run true\nplan -> last\nlast -> done").unwrap();
    let mut context = Context::default();
    context.prompts.insert("plan".into(), String::new());
    let revision = emitted(&pipeline, "plan", "lint = run true\nlint -> last\nplan -> lint", &context).unwrap();
    assert_eq!(revision.pipeline.text, "budget = 4 sessions\nplan = claude emits\nlast = run true\nlint = run true\nplan -> lint\nplan -> last\nlast -> done\nlint -> last\n");
    assert_eq!(revision.carried, [None, Some(0), Some(1), None]);
    assert_eq!(revision.taken(&[1, 0]), [0, 1, 0, 0]);
    let unreachable = emitted(&pipeline, "plan", "lint = run true\nlint -> last", &context).unwrap_err();
    assert_eq!(unreachable.errors.iter().map(|e| e.rule).collect::<Vec<_>>(), [Rule::Unreachable]);
    let over = emitted(&pipeline, "plan", "a = claude prompt plan\nb = claude prompt plan\nc = claude prompt plan\nd = claude prompt plan\nplan -> a -> b -> c -> d -> last", &context).unwrap_err();
    assert_eq!(over.errors.iter().map(|e| e.rule).collect::<Vec<_>>(), [Rule::Budget]);
    assert_eq!(emitted(&pipeline, "plan", "plan -[oops]-> last", &context).unwrap_err().lines().len(), 1);
}

#[test]
fn a_diff_marks_the_lines_a_revision_added_and_removed() {
    let changes = diff("a = run true\na -> done\n", "a = run true\nb = run true\na -> b\nb -> done\n");
    let shown: Vec<String> = changes.iter().map(|c| format!("{}{}", c.sign, c.line)).collect();
    assert_eq!(shown, [" a = run true", "-a -> done", "+b = run true", "+a -> b", "+b -> done"]);
    assert!(diff("same\n", "same\n").iter().all(|c| c.sign == ' '));
}

#[test]
fn an_edit_keeps_the_counts_of_arrows_it_didnt_change() {
    let pipeline = feature();
    let text = pipeline.text.replace("review -[fail, max 3]-> fix", "review -[fail, max 4]-> fix");
    let revision = edited(&pipeline, &text, &context()).unwrap();
    let before: Vec<u32> = (1..=pipeline.arrows.len() as u32).collect();
    let after = revision.taken(&before);
    assert_eq!(after[6], 0);
    assert_eq!(after[5], 6);
    assert_eq!(after[7], 8);
    assert!(edited(&pipeline, "plan = claude\nplan -> nowhere", &context()).is_err());
}

#[test]
fn a_kojo_block_is_found_in_an_answer() {
    let answer = "The plan.\n\n```json name=parts\n[]\n```\n\n```kojo\nlint = run true\nplan -> lint\n```\nDone.";
    assert_eq!(kojo_block(answer).as_deref(), Some("lint = run true\nplan -> lint"));
    assert_eq!(kojo_block("no block here"), None);
}
