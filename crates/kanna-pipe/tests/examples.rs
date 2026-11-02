use kanna_pipe::{load, parse, Context, Kind, Rule, Sizes};
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn context() -> Context {
    let mut context = Context { machines: vec!["mini".into()], ..Context::default() };
    for entry in std::fs::read_dir(repo().join(".kanna/prompts")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        context.prompts.insert(name, std::fs::read_to_string(&path).unwrap());
    }
    context
}

fn example(name: &str) -> String {
    std::fs::read_to_string(repo().join(format!(".kanna/pipelines/{name}.kojo"))).unwrap()
}

#[test]
fn feature_is_valid_with_the_mockups_worst_case() {
    let pipeline = load(&example("feature"), &context()).unwrap();
    assert_eq!(pipeline.start, "plan");
    assert_eq!(pipeline.stages.len(), 7);
    assert_eq!(pipeline.arrows.len(), 10);
    assert_eq!(pipeline.budget.as_ref().unwrap().sessions, Some(40));
    assert_eq!(pipeline.budget.as_ref().unwrap().time, Some(std::time::Duration::from_secs(8 * 3600)));
    assert_eq!(pipeline.max_sessions(&Sizes::new()), 17);
    assert_eq!(pipeline.stage("e2e").unwrap().on.as_deref(), Some("mini"));
    assert_eq!(pipeline.stage("e2e").unwrap().kind, Kind::Run { command: "scripts/e2e-kanna.sh".into() });
}

#[test]
fn feature_counts_the_fan_out_by_its_real_length_once_known() {
    let pipeline = parse(&example("feature")).unwrap();
    let sizes = Sizes::from([("plan.parts".to_string(), 3)]);
    assert_eq!(pipeline.max_sessions(&sizes), 18);
    let too_many = Context { sizes: Sizes::from([("plan.parts".to_string(), 30)]), ..context() };
    let errors = pipeline.check(&too_many).unwrap_err();
    assert_eq!(errors[0].at("feature.kojo"), "feature.kojo:4: the worst case is 45 sessions, over the budget of 40");
}

#[test]
fn bugfix_is_valid() {
    let pipeline = load(&example("bugfix"), &context()).unwrap();
    assert_eq!(pipeline.start, "repro");
    assert_eq!(pipeline.max_sessions(&Sizes::new()), 14);
}

#[test]
fn an_unpaired_machine_is_a_placement_error() {
    let context = Context { machines: vec![], ..context() };
    let errors = load(&example("feature"), &context).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].rule, Rule::Placement);
    assert_eq!(errors[0].at("feature.kojo"), "feature.kojo:12: no such machine 'mini': use here, any, or one of your machines or roles");
}
