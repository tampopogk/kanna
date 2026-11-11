use kanna_pipe::{load, parse, Context, Rule};

fn context() -> Context {
    let mut context = Context { machine: Some("mbp".into()), machines: vec!["mini".into()], roles: vec!["e2e".into()], ..Context::default() };
    for name in ["plan", "build", "fix", "review", "a", "b", "c", "x", "nap"] {
        context.prompts.insert(name.into(), format!("Do {name}."));
    }
    context
}

fn errors(text: &str) -> Vec<(String, Rule)> {
    match load(text, &context()) {
        Ok(_) => vec![],
        Err(errors) => errors.iter().map(|e| (e.at("t.kojo"), e.rule)).collect(),
    }
}

fn only(text: &str, rule: Rule, expected: &str) {
    let found = errors(text);
    assert_eq!(found, vec![(expected.to_string(), rule)], "for\n{text}");
}

#[test]
fn a_valid_file_has_no_errors() {
    assert_eq!(errors("budget = 4 sessions, 1h\nb = claude\nc = run make test\nb -> c\nc -[fail, max 2]-> b\nc -> done\n"), vec![]);
}

#[test]
fn a_lone_stage_needs_no_flow_line() {
    let pipeline = load("b = claude # just one\n", &context()).unwrap();
    assert_eq!(pipeline.start, "b");
    assert_eq!(pipeline.arrows[0].to, "done");
}

#[test]
fn comments_end_at_hash_but_not_inside_a_question() {
    let pipeline = load("s = ask \"Is #3 right?\" # a gate\ns -> done", &context()).unwrap();
    assert_eq!(pipeline.stages[0].kind, kanna_pipe::Kind::Ask { question: "Is #3 right?".into() });
}

#[test]
fn syntax_errors_name_their_line() {
    only("b = claude\n\nb -> done\nwhat is this\n", Rule::Syntax, "t.kojo:4: expected a stage (`name = …`), a flow (`a -> b`) or the budget (`budget = N sessions, 8h`)");
    only("b = gemini\nb -> done", Rule::Syntax, "t.kojo:1: unknown agent 'gemini'; use claude, codex, opencode, agy, or run, ask or merge");
    only("b = claude:max\nb -> done", Rule::Syntax, "t.kojo:1: claude takes the efforts low, medium, high, not 'max'");
    only("b = claude\nb -[fail, sometimes]-> done", Rule::Syntax, "t.kojo:2: unknown or repeated arrow option 'sometimes'; use pass or fail, max N, reset and progress, each once");
    only("b = claude\nb -[max 0]-> done", Rule::Syntax, "t.kojo:2: `max 0`: max takes a whole number above 0");
    only("b = claude x3\nb -> done", Rule::Syntax, "t.kojo:1: x3 needs `by STAGE`: the run stage that scores each attempt");
    only("b = claude x11 by c\nb -> done", Rule::Syntax, "t.kojo:1: xN takes 2 to 10 attempts");
    only("b = claude verdict verdict\nb -> done", Rule::Syntax, "t.kojo:1: verdict is given twice");
    only("budget = 3 tokens\nb = claude\nb -> done", Rule::Syntax, "t.kojo:1: expected `budget = N sessions, DURATION` (DURATION like 8h or 90m), got '3 tokens'");
    only("b = ask Ship?\nb -> done", Rule::Syntax, "t.kojo:1: `ask` needs a question in double quotes: ask \"Is the plan right?\"");
    only("b = claude each a.x\nj = merge b verdict\nb -> j -> done", Rule::Syntax, "t.kojo:2: a merge stage takes only `on PLACEMENT` and `within DURATION` after it, got 'verdict'");
    only("b = merge main\nb -> done", Rule::Syntax, "t.kojo:1: `merge main` isn't a stage: a run hands its branch back to the task's worktree, and you merge from there");
    only("b = claude within soon\nb -> done", Rule::Syntax, "t.kojo:1: within takes a duration like 30m or 2h, got 'soon'");
    only("done = claude\n", Rule::Syntax, "t.kojo:1: 'done' is reserved: it ends the run");
    only("b = claude\nb -> \n", Rule::Syntax, "t.kojo:2: expected a stage name after an arrow");
    only("b = claude\nc = claude\n", Rule::Syntax, "t.kojo:1: a pipeline of several stages needs a flow line (`a -> b`); its first stage starts the run");
}

#[test]
fn a_stage_defined_twice() {
    only("b = claude\nb = codex\nb -> done", Rule::Redefined, "t.kojo:2: stage 'b' is defined twice (first on line 1)");
}

#[test]
fn every_stage_in_a_flow_is_defined() {
    only("b = claude\nb -> c -> done", Rule::Undefined, "t.kojo:2: no stage named 'c'");
}

#[test]
fn nothing_follows_done() {
    only("b = claude\nb -> done -> b", Rule::AfterDone, "t.kojo:2: nothing can follow 'done': it ends the run");
}

#[test]
fn pass_and_fail_need_a_verdict() {
    only("b = claude\nc = claude\nb -> c\nc -[fail, max 2]-> b\nc -> done", Rule::Verdict, "t.kojo:4: [fail] needs a stage with a verdict, and 'c' gives none: add verdict to it, or make it a run, ask or merge stage");
}

#[test]
fn an_agent_with_verdict_may_fail() {
    assert_eq!(errors("b = claude\nc = claude verdict\nb -> c\nc -[fail, max 2]-> b\nc -> done"), vec![]);
}

#[test]
fn progress_needs_a_run_stage() {
    only("b = claude\nc = ask \"OK?\"\nb -> c\nc -[fail, max 2, progress]-> b\nc -> done", Rule::Progress, "t.kojo:4: [progress] needs a run stage, and 'c' is an ask stage");
}

#[test]
fn reset_reaches_an_agent_stage() {
    only("b = claude\nc = run make\nd = run make lint\nb -> c\nc -[fail, max 2, reset]-> d -> done\nc -> done", Rule::Reset, "t.kojo:5: [reset] must lead to an agent stage, and 'd' is a run stage");
}

#[test]
fn every_stage_is_reachable() {
    only("b = claude\nc = claude\nb -> done", Rule::Unreachable, "t.kojo:2: stage 'c' is never reached from 'b', where the run starts");
}

#[test]
fn done_is_reachable() {
    only("b = claude\nc = run make\nb -> c\nc -[fail, max 2]-> b", Rule::NoDone, "t.kojo:3: no flow from 'b' reaches done, so the run could only end stuck");
}

#[test]
fn every_loop_has_a_max() {
    only("b = claude\nc = run make\nb -> c\nc -[fail]-> b\nc -> done", Rule::Unbounded, "t.kojo:4: this loop has no [max N] on any of its arrows, so the run might never end");
}

#[test]
fn each_names_an_agent_stage_that_runs_before_it() {
    only("plan = claude\nbuild = claude each plan.parts\nbuild -> plan -> done", Rule::Each, "t.kojo:2: each plan.parts needs 'plan' to run before 'build' on every path");
    only("plan = run make\nbuild = claude each plan.parts\nplan -> build -> done", Rule::Each, "t.kojo:2: each plan.parts needs an agent stage's answer, and 'plan' is a run stage");
    only("build = claude each plan.parts\nbuild -> done", Rule::Each, "t.kojo:1: each plan.parts: no stage named 'plan'");
}

#[test]
fn merge_names_a_fanned_out_stage() {
    only("build = claude\njoin = merge build\nbuild -> join -> done", Rule::Merge, "t.kojo:2: merge build needs 'build' to be a stage with each");
}

#[test]
fn by_and_guard_name_run_stages() {
    only("b = claude x2 by c\nc = claude\nb -> c -> done", Rule::Scorer, "t.kojo:1: by c needs a run stage to score the code, and 'c' is an agent stage");
    only("b = claude guard nope\nb -> done", Rule::Scorer, "t.kojo:1: guard nope: no stage named 'nope'");
}

#[test]
fn resume_names_an_agent_stage() {
    only("b = claude\nc = run make\nx = claude resume c\nb -> c -> x -> done", Rule::Resume, "t.kojo:3: resume c needs 'c' to be an agent stage");
}

#[test]
fn prompts_exist() {
    only("b = claude prompt missing\nb -> done", Rule::Prompt, "t.kojo:1: no prompt .kanna/prompts/missing.md");
    only("unprompted = claude\nunprompted -> done", Rule::Prompt, "t.kojo:1: no prompt .kanna/prompts/unprompted.md");
}

#[test]
fn a_prompt_names_only_stages_that_ran() {
    let mut context = context();
    context.prompts.insert("x".into(), "Follow {{plan.answer}} and {{b.parts}}.".into());
    let text = "b = claude\nplan = claude\nx = claude\nb -> x -> plan -> done";
    let errors = load(text, &context).unwrap_err();
    assert_eq!(errors.iter().map(|e| e.at("t.kojo")).collect::<Vec<_>>(), ["t.kojo:3: prompt x uses {{plan.…}}, but 'plan' doesn't run before 'x' on every path"]);
    assert_eq!(errors[0].rule, Rule::Prompt);
}

#[test]
fn on_names_a_known_place() {
    assert_eq!(errors("b = claude on here\nc = run make on mini\nd = run e2e on e2e within 30m\nx = run t on mbp\na = claude on any\nb -> c -> d -> x -> a -> done"), vec![]);
    only("b = claude\nc = run make test on moon\nb -> c -> done", Rule::Placement, "t.kojo:2: no such machine 'moon': use here, any, or one of your machines or roles");
}

#[test]
fn the_worst_case_fits_the_budget() {
    let text = "budget = 4 sessions\nb = claude x2 by c\nc = run make\nfix = claude\nb -> c\nc -[fail, max 3]-> fix -> c\nc -> done";
    only(text, Rule::Budget, "t.kojo:1: the worst case is 5 sessions, over the budget of 4");
}

#[test]
fn several_errors_come_in_line_order() {
    let found = errors("b = claude\nb -> c\nb -> d -> done");
    assert_eq!(found.iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>(), ["t.kojo:2: no stage named 'c'", "t.kojo:3: no stage named 'd'"]);
}

#[test]
fn parse_alone_skips_the_checks() {
    assert!(parse("b = claude\nb -> c").is_ok());
}
