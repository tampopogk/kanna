use crate::Row;
use std::collections::{HashMap, HashSet};

pub const FOLD_OVER: usize = 3;

pub fn shape(rows: &mut [Row], main: Option<&str>) {
    let at: HashMap<String, usize> = rows.iter().enumerate().map(|(i, r)| (r.hash.clone(), i)).collect();
    let head = main.filter(|m| at.contains_key(*m)).map(String::from).or_else(|| rows.first().map(|r| r.hash.clone()));
    let mut spine = vec![];
    let mut on_spine = HashSet::new();
    let mut next = head;
    while let Some(hash) = next.and_then(|h| at.get(&h).copied()) {
        if !on_spine.insert(hash) {
            break;
        }
        spine.push(hash);
        next = rows[hash].parents.first().cloned();
    }
    let mut taken: HashMap<usize, usize> = HashMap::new();
    let mut sides: Vec<(usize, Vec<usize>)> = vec![];
    for &merge in spine.iter().rev() {
        let mut side = vec![];
        let mut stack: Vec<usize> = rows[merge].parents.iter().skip(1).filter_map(|p| at.get(p).copied()).collect();
        let mut seen = HashSet::new();
        while let Some(i) = stack.pop() {
            if on_spine.contains(&i) || taken.contains_key(&i) || !seen.insert(i) {
                continue;
            }
            side.push(i);
            stack.extend(rows[i].parents.iter().filter_map(|p| at.get(p).copied()));
        }
        for &i in &side {
            taken.insert(i, merge);
        }
        if !side.is_empty() {
            sides.push((merge, side));
        }
    }
    for &i in &spine {
        rows[i].spine = true;
    }
    let mut folding: Vec<(usize, Vec<usize>)> = sides.into_iter().filter(|(_, side)| side.len() > FOLD_OVER).collect();
    loop {
        let hidden: HashMap<usize, usize> = folding.iter().flat_map(|(merge, side)| side.iter().map(move |i| (*i, *merge))).collect();
        let before = folding.len();
        folding.retain(|(merge, side)| {
            let inside: HashSet<usize> = side.iter().copied().collect();
            !rows.iter().enumerate().any(|(i, r)| i != *merge && !hidden.contains_key(&i) && r.parents.iter().any(|p| at.get(p).is_some_and(|j| inside.contains(j))))
        });
        if folding.len() == before {
            break;
        }
    }
    for (merge, side) in folding {
        rows[merge].fold = side.len();
        let hash = rows[merge].hash.clone();
        for i in side {
            rows[i].side = Some(hash.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hash: &str, parents: &[&str]) -> Row {
        Row { hash: hash.into(), parents: parents.iter().map(|p| p.to_string()).collect(), ..Default::default() }
    }

    fn shaped(rows: &[(&str, &[&str])], main: &str) -> Vec<Row> {
        let mut rows: Vec<Row> = rows.iter().map(|(h, p)| row(h, p)).collect();
        shape(&mut rows, Some(main));
        rows
    }

    fn by<'a>(rows: &'a [Row], hash: &str) -> &'a Row {
        rows.iter().find(|r| r.hash == hash).unwrap()
    }

    #[test]
    fn mains_first_parent_line_is_the_spine() {
        let rows = shaped(&[("m2", &["m1", "b1"]), ("b1", &["m0"]), ("m1", &["m0"]), ("m0", &[])], "m2");
        assert_eq!(rows.iter().filter(|r| r.spine).map(|r| r.hash.as_str()).collect::<Vec<_>>(), ["m2", "m1", "m0"]);
        assert!(!by(&rows, "b1").spine);
    }

    #[test]
    fn a_long_merged_branch_folds_into_its_merge_and_a_short_one_does_not() {
        let rows = shaped(
            &[("m3", &["m2", "a4"]), ("a4", &["a3"]), ("a3", &["a2"]), ("a2", &["a1"]), ("a1", &["m1"]), ("m2", &["m1", "b1"]), ("b1", &["m1"]), ("m1", &["m0"]), ("m0", &[])],
            "m3",
        );
        assert_eq!(by(&rows, "m3").fold, 4);
        assert!(["a1", "a2", "a3", "a4"].iter().all(|h| by(&rows, h).side.as_deref() == Some("m3")));
        assert_eq!(by(&rows, "m2").fold, 0);
        assert_eq!(by(&rows, "b1").side, None);
    }

    #[test]
    fn merges_of_main_into_the_branch_stay_in_its_side_and_main_stays_out() {
        let rows = shaped(
            &[("m3", &["m2", "a4"]), ("a4", &["a3", "m2"]), ("a3", &["a2"]), ("m2", &["m1"]), ("a2", &["a1"]), ("a1", &["m1"]), ("m1", &["m0"]), ("m0", &[])],
            "m3",
        );
        assert_eq!(by(&rows, "m3").fold, 4);
        assert_eq!(by(&rows, "m2").side, None);
    }

    #[test]
    fn a_branch_merged_twice_folds_its_older_commits_into_the_older_merge() {
        let rows = shaped(
            &[
                ("m3", &["m2", "a8"]),
                ("a8", &["a7"]),
                ("a7", &["a6"]),
                ("a6", &["a5"]),
                ("a5", &["a4"]),
                ("m2", &["m1", "a4"]),
                ("a4", &["a3"]),
                ("a3", &["a2"]),
                ("a2", &["a1"]),
                ("a1", &["m1"]),
                ("m1", &["m0"]),
                ("m0", &[]),
            ],
            "m3",
        );
        assert_eq!((by(&rows, "m2").fold, by(&rows, "m3").fold), (4, 4));
        assert_eq!(by(&rows, "a5").side.as_deref(), Some("m3"));
        assert_eq!(by(&rows, "a4").side.as_deref(), Some("m2"));
    }

    #[test]
    fn an_octopus_merge_folds_every_side_it_brought_in_together() {
        let rows = shaped(&[("m2", &["m1", "a2", "b2", "c1"]), ("a2", &["a1"]), ("a1", &["m1"]), ("b2", &["b1"]), ("b1", &["m1"]), ("c1", &["m1"]), ("m1", &[])], "m2");
        assert_eq!(by(&rows, "m2").fold, 5);
    }

    #[test]
    fn a_side_another_branch_builds_on_does_not_fold() {
        let rows = shaped(
            &[("t2", &["t1"]), ("t1", &["a2"]), ("m2", &["m1", "a4"]), ("a4", &["a3"]), ("a3", &["a2"]), ("a2", &["a1"]), ("a1", &["m1"]), ("m1", &[])],
            "m2",
        );
        assert_eq!(by(&rows, "m2").fold, 0);
        assert!(rows.iter().all(|r| r.side.is_none()));
    }

    #[test]
    fn without_main_in_the_range_the_first_row_heads_the_spine() {
        let mut rows = vec![row("b2", &["b1"]), row("b1", &[])];
        shape(&mut rows, Some("elsewhere"));
        assert!(rows.iter().all(|r| r.spine));
    }
}
