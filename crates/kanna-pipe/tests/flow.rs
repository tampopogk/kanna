use kanna_pipe::{edited, emitted, parse, pipe_block, Context, Rule, Sizes, Verdict};
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
    parse(&std::fs::read_to_string(repo().join(".kanna/pipelines/feature.kojo")).unwrap()).unwrap()
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
    let errors = emitted(&pipeline, "plan", "x = run true\ncheck = run true\nbudget = 99 sessions", &context()).unwrap_err();
    assert_eq!(errors.iter().map(|e| e.at("plan")).collect::<Vec<_>>(), ["plan:2: stage 'check' exists already; a pipe block can only add stages", "plan:3: a pipe block can't change the budget"]);
}

#[test]
fn only_an_emits_stage_can_emit() {
    let errors = emitted(&feature(), "review", "x = run true\nreview -> x", &context()).unwrap_err();
    assert_eq!((errors[0].rule, errors[0].line), (Rule::Emits, 10));
}

#[test]
fn an_emitted_revision_is_checked_whole() {
    let errors = emitted(&feature(), "plan", "lint = run true\nplan -> lint\nlint -[fail]-> plan", &context()).unwrap_err();
    assert_eq!(errors.iter().map(|e| (e.rule, e.line)).collect::<Vec<_>>(), [(Rule::Unbounded, 23)]);
    let errors = emitted(&feature(), "plan", "more = claude x10 by check each plan.parts\nagain = claude x10 by check each plan.parts\nplan -> more -> again -> build", &context()).unwrap_err();
    assert_eq!(errors.iter().map(|e| e.rule).collect::<Vec<_>>(), [Rule::Budget, Rule::Prompt, Rule::Prompt]);
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
fn a_pipe_block_is_found_in_an_answer() {
    let answer = "The plan.\n\n```json name=parts\n[]\n```\n\n```pipe\nlint = run true\nplan -> lint\n```\nDone.";
    assert_eq!(pipe_block(answer).as_deref(), Some("lint = run true\nplan -> lint"));
    assert_eq!(pipe_block("no block here"), None);
}
