use crate::code::Block;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub file: String,
    pub line: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub lines: usize,
    pub places: Vec<Place>,
}

pub struct Clones {
    pub lines: HashMap<String, BTreeSet<usize>>,
    pub groups: Vec<Group>,
}

impl Clones {
    pub fn duplicated_lines(&self) -> usize {
        self.lines.values().map(|l| l.len()).sum()
    }
}

pub fn find(files: &[(&str, &[Block])]) -> Clones {
    let mut by_hash: HashMap<u64, Vec<(&str, &Block)>> = HashMap::new();
    for (file, blocks) in files {
        for block in blocks.iter() {
            by_hash.entry(block.hash).or_default().push((file, block));
        }
    }
    let mut lines: HashMap<String, BTreeSet<usize>> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for instances in by_hash.into_values().filter(|i| i.len() > 1) {
        for (file, block) in &instances {
            lines.entry(file.to_string()).or_default().extend(block.lines.iter().copied());
        }
        let mut places: Vec<Place> = instances.iter().map(|(file, block)| Place { file: file.to_string(), line: block.line, end: block.end }).collect();
        places.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        groups.push(Group { lines: instances.iter().map(|(_, b)| b.lines.len()).max().unwrap_or(0), places });
    }
    let inside = |inner: &Place, outer: &Place| inner.file == outer.file && outer.line <= inner.line && inner.end <= outer.end && (outer.line, outer.end) != (inner.line, inner.end);
    let kept: Vec<Group> = groups.iter().filter(|g| !g.places.iter().all(|p| groups.iter().any(|o| o.places.iter().any(|q| inside(p, q))))).cloned().collect();
    let mut groups = kept;
    groups.sort_by(|a, b| (b.lines * (b.places.len() - 1)).cmp(&(a.lines * (a.places.len() - 1))).then_with(|| a.places[0].file.cmp(&b.places[0].file)).then_with(|| a.places[0].line.cmp(&b.places[0].line)));
    Clones { lines, groups }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{analyze, Lang};

    #[test]
    fn renamed_copies_are_found_whole_and_their_lines_counted() {
        let body = |name: &str, v: &str| format!("fn {name}({v}: &[u32]) -> u32 {{\n    let mut total = 0;\n    for x in {v} {{\n        if *x > 3 {{\n            total += x * 2;\n        }}\n    }}\n    total\n}}\n");
        let a = analyze(Lang::Rust, &format!("{}\nfn other() -> bool {{ true }}\n", body("sum", "xs")));
        let b = analyze(Lang::Rust, &format!("fn lead() {{}}\n{}", body("total_of", "values")));
        let c = analyze(Lang::Rust, "fn unrelated(a: u8) -> u8 {\n    let b = a + 1;\n    b\n}\n");
        let found = find(&[("a.rs", &a.blocks), ("b.rs", &b.blocks), ("c.rs", &c.blocks)]);
        assert_eq!(found.groups.len(), 1, "the for and if inside the copies are part of the copied function");
        assert_eq!(found.groups[0].places, vec![Place { file: "a.rs".into(), line: 1, end: 9 }, Place { file: "b.rs".into(), line: 2, end: 10 }]);
        assert_eq!(found.duplicated_lines(), 12, "six source lines in each copy: closing braces don't count");
        assert!(!found.lines.contains_key("c.rs"));
    }
}
