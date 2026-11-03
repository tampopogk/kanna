use kanna_quality::git::{git, Blobs};
use kanna_quality::tree::Cache;
use std::path::Path;
fn main() {
    let repo = Path::new(".");
    let t = std::time::Instant::now();
    let log = git(repo, &["log", "--first-parent", "--format=%H %ct", "main"]).unwrap();
    let mut blobs = Blobs::open(repo).unwrap();
    let mut cache = Cache::default();
    let commits: Vec<&str> = log.lines().collect();
    for (i, line) in commits.iter().enumerate() {
        let (id, at) = line.split_once(' ').unwrap();
        let tree = cache.tree(repo, &mut blobs, id).unwrap();
        if i % 20 == 0 { println!("{at} {} lines {} erosion {:.3} high {} verbosity {:.3} dead {} wrappers {}", &id[..8], tree.lines(), tree.erosion().unwrap_or(0.0), tree.high(), tree.verbosity().unwrap_or(0.0), tree.dead().len(), tree.wrappers().len()); }
    }
    eprintln!("{} commits in {:?}", commits.len(), t.elapsed());
}
