use kanna_quality::git::Blobs;
use kanna_quality::tree::Cache;
use std::path::Path;

fn main() {
    let repo = std::env::args().nth(1).unwrap_or(".".into());
    let rev = std::env::args().nth(2).unwrap_or("HEAD".into());
    let repo = Path::new(&repo);
    let started = std::time::Instant::now();
    let mut blobs = Blobs::open(repo).unwrap();
    let tree = Cache::default().tree(repo, &mut blobs, &rev).unwrap();
    eprintln!("read and parsed {} files in {:?}", tree.files.len(), started.elapsed());
    let functions = tree.functions();
    println!("code lines {} functions {} erosion {:.3} cc>10 {} gini {:.3} verbosity {:.3}", tree.lines(), functions.len(), tree.erosion().unwrap_or(0.0), tree.high(), tree.gini().unwrap_or(0.0), tree.verbosity().unwrap_or(0.0));
    let clones = tree.clones();
    println!("clone lines {} groups {} wrappers {} dead {}", clones.duplicated_lines(), clones.groups.len(), tree.wrappers().len(), tree.dead().len());
    for r in tree.worst().iter().take(10) {
        println!("  mass {:6.0} cc {:3} cog {:3} L{:4} {}:{} {}", r.function.mass(), r.function.cyclomatic, r.function.cognitive, r.function.lines, r.file, r.function.line, r.function.name);
    }
    for g in clones.groups.iter().take(8) {
        println!("  {} lines x{}: {}", g.lines, g.places.len(), g.places.iter().map(|p| format!("{}:{}-{}", p.file, p.line, p.end)).collect::<Vec<_>>().join(", "));
    }
    eprintln!("total {:?}", started.elapsed());
}
