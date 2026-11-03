use kanna_quality::code::{analyze, Lang};
use kanna_quality::tree::{File, Tree};
use std::path::Path;
use std::sync::Arc;

fn collect(dir: &Path, root: &Path, files: &mut Vec<File>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, root, files);
        } else if let Some(lang) = path.to_str().and_then(Lang::of) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                files.push(File { path: relative, facts: Arc::new(analyze(lang, &text)) });
            }
        }
    }
}

fn main() {
    for dir in std::env::args().skip(1) {
        let root = Path::new(&dir);
        let mut files = Vec::new();
        collect(root, root, &mut files);
        let tree = Tree::new(files);
        let functions = tree.functions();
        let mass = |f: &kanna_quality::code::Function, cog: bool| (if cog { f.cognitive } else { f.cyclomatic }) as f64 * (f.lines as f64).sqrt();
        let share = |cog: bool| {
            let all: f64 = functions.iter().map(|r| mass(r.function, cog)).sum();
            let high: f64 = functions.iter().filter(|r| (if cog { r.function.cognitive } else { r.function.cyclomatic }) > 10).map(|r| mass(r.function, cog)).sum();
            high / all
        };
        let wrapped: usize = tree.files.iter().map(|f| f.facts.wrapped.len()).sum();
        let flagged: usize = tree.files.iter().map(|f| f.facts.flagged.len()).sum();
        println!(
            "{dir}: files {} loc {} functions {} high_cc {} high_cog {} erosion {:.3} cog_erosion {:.3} clone_loc {} wrappers {} wrapper_loc {} flagged_loc {} verbosity {:.3}",
            tree.files.len(),
            tree.lines(),
            functions.len(),
            functions.iter().filter(|r| r.function.cyclomatic > 10).count(),
            functions.iter().filter(|r| r.function.cognitive > 10).count(),
            share(false),
            share(true),
            tree.clones().duplicated_lines(),
            tree.wrappers().len(),
            wrapped,
            flagged,
            tree.verbosity().unwrap_or(0.0)
        );
    }
}
