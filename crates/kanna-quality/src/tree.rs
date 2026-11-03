use crate::clones::{self, Clones};
use crate::code::{analyze, Facts, Function, Lang};
use crate::git::{self, Blobs};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

pub const HIGH_COMPLEXITY: usize = 10;

const SKIPPED: &[&str] = &["tests", "Tests", "test", "benches", "examples", "vendor", "third_party", "references", "fixtures", "target", "build", ".build"];

pub fn counted(path: &str) -> bool {
    Lang::of(path).is_some() && !path.split('/').rev().skip(1).any(|part| SKIPPED.contains(&part) || part.starts_with('.'))
}

pub struct File {
    pub path: String,
    pub facts: Arc<Facts>,
}

pub struct Tree {
    pub files: Vec<File>,
    clones: std::sync::OnceLock<Clones>,
}

#[derive(Default)]
pub struct Cache {
    facts: HashMap<String, Arc<Facts>>,
}

impl Cache {
    pub fn tree(&mut self, repo: &Path, blobs: &mut Blobs, commit: &str) -> Result<Tree, String> {
        let entries = git::tree(repo, commit)?;
        let mut kept = HashSet::new();
        let mut files = Vec::new();
        for entry in entries.into_iter().filter(|e| counted(&e.path)) {
            let facts = match self.facts.get(&entry.blob) {
                Some(facts) => facts.clone(),
                None => {
                    let text = blobs.read(&entry.blob)?;
                    let facts = Arc::new(analyze(Lang::of(&entry.path).unwrap_or(Lang::Rust), &text));
                    self.facts.insert(entry.blob.clone(), facts.clone());
                    facts
                }
            };
            kept.insert(entry.blob);
            files.push(File { path: entry.path, facts });
        }
        self.facts.retain(|blob, _| kept.contains(blob));
        Ok(Tree::new(files))
    }
}

pub struct Ranked<'a> {
    pub file: &'a str,
    pub function: &'a Function,
}

impl Tree {
    pub fn new(files: Vec<File>) -> Tree {
        Tree { files, clones: std::sync::OnceLock::new() }
    }

    pub fn lines(&self) -> usize {
        self.files.iter().map(|f| f.facts.lines).sum()
    }

    pub fn functions(&self) -> Vec<Ranked<'_>> {
        self.files.iter().flat_map(|f| f.facts.functions.iter().map(move |function| Ranked { file: &f.path, function })).collect()
    }

    pub fn worst(&self) -> Vec<Ranked<'_>> {
        let mut all = self.functions();
        all.sort_by(|a, b| b.function.mass().total_cmp(&a.function.mass()).then_with(|| b.function.cognitive.cmp(&a.function.cognitive)).then_with(|| a.file.cmp(b.file)).then_with(|| a.function.line.cmp(&b.function.line)));
        all
    }

    pub fn complexity(&self) -> usize {
        self.files.iter().flat_map(|f| &f.facts.functions).map(|f| f.cognitive).sum()
    }

    pub fn erosion(&self) -> Option<f64> {
        let (mut high, mut all) = (0.0, 0.0);
        for f in self.files.iter().flat_map(|f| &f.facts.functions) {
            let mass = f.mass();
            all += mass;
            if f.cyclomatic > HIGH_COMPLEXITY {
                high += mass;
            }
        }
        (all > 0.0).then(|| high / all)
    }

    pub fn high(&self) -> usize {
        self.files.iter().flat_map(|f| &f.facts.functions).filter(|f| f.cyclomatic > HIGH_COMPLEXITY).count()
    }

    pub fn verbose_lines(&self) -> HashMap<&str, BTreeSet<usize>> {
        let clones = self.clones();
        let mut lines: HashMap<&str, BTreeSet<usize>> = HashMap::new();
        for file in &self.files {
            let mut flagged: BTreeSet<usize> = clones.lines.get(&file.path).cloned().unwrap_or_default();
            for wrapper in file.facts.functions.iter().filter(|f| f.wrapper) {
                flagged.extend(wrapper.line..=wrapper.end);
            }
            if !flagged.is_empty() {
                lines.insert(file.path.as_str(), flagged);
            }
        }
        lines
    }

    pub fn verbosity(&self) -> Option<f64> {
        let total = self.lines();
        (total > 0).then(|| (self.verbose_lines().values().map(|l| l.len()).sum::<usize>() as f64 / total as f64).min(1.0))
    }

    pub fn gini(&self) -> Option<f64> {
        let mut values: Vec<f64> = self.functions().iter().map(|r| r.function.cognitive as f64).collect();
        let total: f64 = values.iter().sum();
        if values.is_empty() || total == 0.0 {
            return None;
        }
        values.sort_by(f64::total_cmp);
        let n = values.len() as f64;
        Some(values.iter().enumerate().map(|(i, x)| (2.0 * (i as f64 + 1.0) - n - 1.0) * x).sum::<f64>() / (n * total))
    }

    pub fn clones(&self) -> &Clones {
        self.clones.get_or_init(|| {
            let input: Vec<(&str, &[crate::code::Block])> = self.files.iter().map(|f| (f.path.as_str(), f.facts.blocks.as_slice())).collect();
            clones::find(&input)
        })
    }

    pub fn dead(&self) -> Vec<Ranked<'_>> {
        let mut names: HashMap<&str, usize> = HashMap::new();
        for file in &self.files {
            for (name, count) in &file.facts.names {
                *names.entry(name.as_str()).or_default() += count;
            }
        }
        let mut dead: Vec<Ranked> = self
            .functions()
            .into_iter()
            .filter(|r| !r.function.exempt)
            .filter(|r| {
                let name = r.function.name.rsplit("::").next().unwrap_or("");
                names.get(name).copied().unwrap_or(0) <= 1
            })
            .collect();
        dead.sort_by(|a, b| b.function.lines.cmp(&a.function.lines).then_with(|| a.file.cmp(b.file)));
        dead
    }

    pub fn wrappers(&self) -> Vec<Ranked<'_>> {
        self.functions().into_iter().filter(|r| r.function.wrapper).collect()
    }

    pub fn files_by_complexity(&self) -> Vec<(&str, usize, usize)> {
        let mut files: Vec<(&str, usize, usize)> = self.files.iter().map(|f| (f.path.as_str(), f.facts.functions.iter().map(|g| g.cognitive).sum(), f.facts.lines)).collect();
        files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)).then_with(|| a.0.cmp(b.0)));
        files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> File {
        File { path: path.into(), facts: Arc::new(analyze(Lang::of(path).unwrap(), text)) }
    }

    #[test]
    fn counted_files_are_rust_and_swift_outside_tests_and_models() {
        assert!(counted("crates/a/src/lib.rs"));
        assert!(counted("apps/kanna/Sources/Kanna/App.swift"));
        assert!(!counted("crates/a/tests/flow.rs"));
        assert!(!counted("apps/kanna/Tests/KannaTests/A.swift"));
        assert!(!counted("references/prototypes/x/main.rs"));
        assert!(!counted("docs/specs/stats.md"));
        assert!(!counted(".build/checkouts/x.swift"));
    }

    #[test]
    fn concentration_dead_code_and_wrappers_over_a_tree() {
        let busy = "fn busy(a: u8) -> u8 {\n    if a > 1 { if a > 2 { if a > 3 { 1 } else { 2 } } else { 3 } } else { 4 }\n}\n";
        let tree = Tree::new(vec![
            file("a/src/lib.rs", &format!("{busy}fn calm() -> u8 {{ busy(1) }}\nfn pass(a: u8) -> u8 {{ busy(a) }}\n")),
            file("b/src/lib.rs", "fn unused() -> u8 { 0 }\nfn user() -> u8 { calm() + pass(2) }\n"),
        ]);
        assert_eq!(tree.worst()[0].function.name, "busy");
        assert_eq!(tree.complexity(), 1 + 2 + 3 + 1 + 1 + 1);
        assert_eq!(tree.erosion(), Some(0.0), "no function above 10");
        assert!(tree.gini().unwrap() > 0.7);
        assert_eq!(tree.dead().iter().map(|r| r.function.name.as_str()).collect::<Vec<_>>(), ["unused", "user"]);
        assert_eq!(tree.wrappers().iter().map(|r| r.function.name.as_str()).collect::<Vec<_>>(), ["pass"]);
        assert_eq!(tree.files_by_complexity()[0].0, "a/src/lib.rs");
    }
}
