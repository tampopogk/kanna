use kanna_pipe::{layout, parse, Route};
use std::path::PathBuf;

fn feature() -> kanna_pipe::Pipeline {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.kanna/pipelines/feature.kojo");
    parse(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn spots(graph: &kanna_pipe::Graph) -> Vec<(&str, &str, &str, usize, usize)> {
    graph.nodes.iter().map(|n| (n.id.as_str(), n.kind, n.who.as_str(), n.col, n.row)).collect()
}

fn routes(graph: &kanna_pipe::Graph) -> Vec<(String, Route, &str, bool, usize, usize)> {
    graph.edges.iter().map(|e| (format!("{}>{}", e.from, e.to), e.route, e.label.as_str(), e.fail, e.lane, e.lanes)).collect()
}

#[test]
fn feature_lays_out_as_the_mockup_draws_it() {
    let graph = layout(&feature());
    assert_eq!(
        spots(&graph),
        [
            ("plan", "agent", "claude", 0, 0),
            ("build", "agent", "claude", 1, 0),
            ("join", "merge", "git", 2, 0),
            ("check", "run", "check.sh", 3, 0),
            ("review", "agent", "codex", 4, 0),
            ("e2e", "run", "e2e-kanna.sh · mini", 5, 0),
            ("done", "done", "", 6, 0),
            ("fix", "agent", "claude", 4, 1),
        ]
    );
    assert_eq!((graph.columns, graph.rows), (7, 2));
    assert_eq!(
        routes(&graph),
        [
            ("plan>build".into(), Route::Next, "", false, 0, 1),
            ("build>join".into(), Route::Next, "", false, 0, 1),
            ("join>check".into(), Route::Next, "", false, 0, 1),
            ("check>fix".into(), Route::Down, "fail · max 3 · progress", true, 0, 3),
            ("fix>check".into(), Route::Up, "", false, 0, 1),
            ("check>review".into(), Route::Next, "", false, 0, 1),
            ("review>fix".into(), Route::Down, "fail · max 3", true, 1, 3),
            ("review>e2e".into(), Route::Next, "", false, 0, 1),
            ("e2e>fix".into(), Route::Down, "fail · max 2", true, 2, 3),
            ("e2e>done".into(), Route::Next, "", false, 0, 1),
        ]
    );
}

#[test]
fn an_arrow_that_skips_or_goes_back_along_the_line_goes_over_it() {
    let graph = layout(&parse("build = claude\ncheck = run sh scripts/check.sh\nship = run make ship\nbuild -> check\ncheck -[fail, max 2]-> build\ncheck -[pass]-> ship\nbuild -[pass]-> ship\nship -> done\n").unwrap());
    assert_eq!(spots(&graph), [("build", "agent", "claude", 0, 0), ("check", "run", "check.sh", 1, 0), ("ship", "run", "make", 2, 0), ("done", "done", "", 3, 0)]);
    let over: Vec<_> = routes(&graph).into_iter().filter(|r| r.1 == Route::Over).collect();
    assert_eq!(over, [("check>build".into(), Route::Over, "fail · max 2", true, 0, 2), ("build>ship".into(), Route::Over, "pass", false, 1, 2)]);
}

#[test]
fn stages_off_the_line_stack_under_their_neighbours() {
    let graph = layout(&parse("a = claude\nb = claude\nfix = claude\nlint = run cargo clippy\na -> b -> done\nb -[fail, max 1]-> fix -> b\nb -[fail, max 1]-> lint -> b\n").unwrap());
    assert_eq!(spots(&graph), [("a", "agent", "claude", 0, 0), ("b", "agent", "claude", 1, 0), ("done", "done", "", 2, 0), ("fix", "agent", "claude", 1, 1), ("lint", "run", "cargo", 1, 2)]);
}
