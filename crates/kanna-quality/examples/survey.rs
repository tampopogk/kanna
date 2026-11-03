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
    let mut rules: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    for file in &tree.files {
        for hit in &file.facts.hits {
            let entry = rules.entry(hit.rule).or_default();
            entry.0 += 1;
            entry.1 += hit.end - hit.line + 1;
        }
    }
    if let Ok(wanted) = std::env::var("SURVEY_RULE") {
        let mut blobs = Blobs::open(repo).unwrap();
        let entries = kanna_quality::git::tree(repo, &rev).unwrap();
        for file in &tree.files {
            let hits: Vec<_> = file.facts.hits.iter().filter(|h| h.rule == wanted).take(2).collect();
            if hits.is_empty() {
                continue;
            }
            let blob = &entries.iter().find(|e| e.path == file.path).unwrap().blob;
            let text = blobs.read(blob).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for hit in hits {
                println!("--- {}:{}", file.path, hit.line);
                for line in &lines[hit.line - 1..hit.end.min(hit.line + 3)] {
                    println!("    {line}");
                }
            }
        }
        return;
    }
    let flagged: usize = tree.files.iter().map(|f| f.facts.flagged.len()).sum();
    let wrapped: usize = tree.files.iter().map(|f| f.facts.wrapped.len()).sum();
    println!("flagged lines {flagged} wrapper lines {wrapped}");
    let mut ranked: Vec<_> = rules.into_iter().collect();
    ranked.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for (rule, (hits, lines)) in ranked {
        println!("  {rule:24} {hits:5} hits {lines:6} lines");
    }
    for r in tree.worst().iter().take(10) {
        println!("  mass {:6.0} cc {:3} cog {:3} L{:4} {}:{} {}", r.function.mass(), r.function.cyclomatic, r.function.cognitive, r.function.lines, r.file, r.function.line, r.function.name);
    }
    for g in clones.groups.iter().take(8) {
        println!("  {} lines x{}: {}", g.lines, g.places.len(), g.places.iter().map(|p| format!("{}:{}-{}", p.file, p.line, p.end)).collect::<Vec<_>>().join(", "));
    }
    eprintln!("total {:?}", started.elapsed());
}
