use crate::db::Db;
use crate::store::{pane_number, Store};
use kanna_mux::json::Value;
use kanna_mux::{content_prefix, PaneType};
use std::collections::HashMap;
use std::path::Path;

pub fn migrate(state_dir: &Path, db: &Db) {
    let mut renamed = Store::content_ids(state_dir);
    if let Some(mut state) = crate::layout::saved(db, state_dir) {
        let mut numbers: HashMap<&'static str, usize> = HashMap::new();
        let records: Vec<(String, PaneType)> = state
            .get("panes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p.get("id")?.as_str()?.to_string(), PaneType::parse(p.get("type")?.as_str()?)?)))
            .collect();
        for (id, _) in &records {
            if let Some((prefix, n)) = id.split_once('-').and_then(|(prefix, n)| Some((prefix, n.parse::<usize>().ok()?))) {
                if let Some(prefix) = ["file", "page", "ios", "diff"].into_iter().find(|p| *p == prefix) {
                    let at = numbers.entry(prefix).or_insert(0);
                    *at = (*at).max(n);
                }
            }
        }
        for (id, kind) in records.iter().filter(|(id, kind)| pane_number(id).is_some() && *kind != PaneType::Term) {
            let prefix = content_prefix(*kind);
            let n = numbers.entry(prefix).or_insert(0);
            *n += 1;
            renamed.insert(id.clone(), format!("{prefix}-{n}"));
        }
        if !renamed.is_empty() && rename(&mut state, &renamed) {
            if let Err(error) = db.put_layout(&state) {
                eprintln!("kanna-daemon: couldn't save the layout with content IDs: {error}");
            }
        }
    }
    if renamed.is_empty() {
        return;
    }
    if let Err(error) = db.rename_terminals(&renamed) {
        eprintln!("kanna-daemon: couldn't rename the tasks' terminals: {error}");
    }
    eprintln!("kanna-daemon: named {} panes by their content (p1 → term-1, file-1, …)", renamed.len());
}

fn rename(value: &mut Value, renamed: &HashMap<String, String>) -> bool {
    match value {
        Value::String(text) => match renamed.get(text.as_str()) {
            Some(to) => {
                *text = to.clone();
                true
            }
            None => false,
        },
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| rename(item, renamed) || changed),
        Value::Object(fields) => fields.iter_mut().filter(|(key, _)| key.as_str() != "name").fold(false, |changed, (_, item)| rename(item, renamed) || changed),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanna_mux::json::json;

    #[test]
    fn every_pane_id_in_the_saved_layout_is_renamed_but_names_are_left_alone() {
        let mut state = json!({ "windows": [{ "id": "w1", "tabs": [{ "id": "t1", "root": { "split": "row", "children": [{ "pane": "p1" }, { "pane": "p2" }] }, "lastFocus": "p2" }] }],
                                "panes": [{ "id": "p1", "name": "p2", "type": "term" }, { "id": "p2", "type": "md" }] });
        let renamed = HashMap::from([("p1".to_string(), "term-4".to_string()), ("p2".to_string(), "file-1".to_string())]);
        assert!(rename(&mut state, &renamed));
        assert_eq!(state["windows"][0]["tabs"][0]["root"]["children"], json!([{ "pane": "term-4" }, { "pane": "file-1" }]));
        assert_eq!(state["windows"][0]["tabs"][0]["lastFocus"], "file-1");
        assert_eq!(state["panes"][0], json!({ "id": "term-4", "name": "p2", "type": "term" }));
    }
}
