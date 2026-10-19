use super::{bad_request, reply, CoreHandle};
use crate::error::KmuxError;
use crate::json::{json, num, string, Map, Value};
use crate::pane::{PaneType, Stop, StopMark, Tour};

const PLACEMENT: [&str; 7] = ["name", "tab", "dock", "split", "size", "window", "wait"];

fn is_doc(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

impl CoreHandle {
    pub(super) async fn tour(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "owner").or_else(|| string(args, "from")).ok_or_else(|| bad_request("a tour is an agent's: run it from the agent's terminal"))?;
        let owner = self.need_pane(Some(reference))?;
        if owner.kind != PaneType::Term {
            return Err(KmuxError::new("wrong_type", format!("{} is a {} pane; a tour belongs to an agent's terminal", owner.label(), owner.kind.as_str())));
        }
        let owner = owner.id;
        match string(args, "action").unwrap_or("show") {
            "new" => {
                let stops = self.stops(args).await?;
                self.set_tour(&owner, Some(Tour { stops, at: 0 }));
                self.show_stop(&owner, 0, args).await
            }
            "add" => {
                let mut stops = self.stops(args).await?;
                let next = args.get("next").and_then(Value::as_bool).unwrap_or(false);
                let Some(mut tour) = self.tour_of(&owner) else {
                    self.set_tour(&owner, Some(Tour { stops, at: 0 }));
                    return self.show_stop(&owner, 0, args).await;
                };
                let at = if next { tour.at + 1 } else { tour.stops.len() };
                let added = stops.len();
                tour.stops.splice(at..at, stops.drain(..));
                let current = tour.at;
                self.set_tour(&owner, Some(tour));
                if next {
                    return self.show_stop(&owner, at, args).await;
                }
                let mut out = self.refresh_mark(&owner, current).await;
                out.insert("added".into(), num(added as f64));
                Ok(out)
            }
            "go" => {
                let tour = self.tour_of(&owner).ok_or_else(|| KmuxError::new("not_found", "no tour"))?;
                let to = match args.get("to") {
                    Some(Value::String(to)) if to == "next" => tour.at + 1,
                    Some(Value::String(to)) if to == "back" => tour.at.saturating_sub(1),
                    Some(to) => match to.as_u64().or_else(|| to.as_str().and_then(|t| t.parse().ok())) {
                        Some(n) if n >= 1 && (n as usize) <= tour.stops.len() => n as usize - 1,
                        _ => return Err(bad_request(format!("the tour has stops 1 to {}", tour.stops.len()))),
                    },
                    None => return Err(bad_request("say where: next, back or a stop's number")),
                };
                if to >= tour.stops.len() {
                    let of = tour.stops.len();
                    self.end_tour(&owner).await;
                    return Ok(reply([("done", Value::Bool(true)), ("of", num(of as f64))]));
                }
                self.show_stop(&owner, to, args).await
            }
            "end" => {
                let had = self.tour_of(&owner).is_some();
                self.end_tour(&owner).await;
                Ok(reply([("ended", Value::Bool(had))]))
            }
            "show" => Ok(match self.tour_of(&owner) {
                Some(tour) => reply([("stops", Value::Array(tour.stops.iter().map(Stop::to_json).collect())), ("at", num(tour.at as f64 + 1.0))]),
                None => reply([("stops", json!([]))]),
            }),
            other => Err(bad_request(format!("unknown tour action \"{other}\": new, add, go, show or end"))),
        }
    }

    async fn stops(&self, args: &Value) -> Result<Vec<Stop>, KmuxError> {
        let asked = args.get("stops").and_then(Value::as_array).filter(|s| !s.is_empty()).ok_or_else(|| bad_request("missing stops"))?;
        let mut stops = Vec::new();
        for stop in asked {
            let path = string(stop, "path").filter(|p| !p.is_empty()).ok_or_else(|| bad_request("a stop is missing its path"))?.to_string();
            let text = string(stop, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad_request(format!("{path}: missing text")))?.to_string();
            let line = stop.get("line").and_then(Value::as_u64).filter(|l| *l > 0).ok_or_else(|| bad_request(format!("{path}: missing line")))?;
            let last = stop.get("last").and_then(Value::as_u64);
            if last.is_some_and(|last| last < line) {
                return Err(bad_request(format!("{path}: lines {line}-{} run backwards", last.unwrap_or(line))));
            }
            if is_doc(&path) {
                if last.is_some() {
                    return Err(bad_request(format!("{path}: a comment on a markdown file is on the block at one line")));
                }
                let source = std::fs::read_to_string(&path).map_err(|_| KmuxError::new("not_found", format!("No such file: {path}")))?;
                let (first, _) = kanna_threads::block_at(&source, line as usize).ok_or_else(|| bad_request(format!("{path} has {} lines", source.lines().count())))?;
                stops.push(Stop { path, first: first as u64, last: first as u64, text, doc: true, thread: None });
                continue;
            }
            let mut ask = json!({ "path": path, "line": line });
            if let Some(last) = last {
                ask["last"] = json!(last);
            }
            let snapped = self.ask_app("app.snap", ask).await.map_err(|error| match error.code.as_str() {
                "no_app" => KmuxError::new("no_app", "a tour needs Kanna.app, and none is attached to this daemon"),
                _ => error,
            })?;
            let first = snapped.get("first").and_then(Value::as_u64).unwrap_or(line);
            let last = snapped.get("last").and_then(Value::as_u64).unwrap_or(first).max(first);
            stops.push(Stop { path, first, last, text, doc: false, thread: None });
        }
        Ok(stops)
    }

    fn tour_of(&self, owner: &str) -> Option<Tour> {
        self.state().panes.get(owner).and_then(|p| p.tour.clone())
    }

    fn set_tour(&self, owner: &str, tour: Option<Tour>) {
        if let Some(pane) = self.state().panes.get_mut(owner) {
            pane.tour = tour;
        }
        self.changed();
    }

    async fn show_stop(&self, owner: &str, at: usize, args: &Value) -> Result<Map, KmuxError> {
        let Some(mut tour) = self.tour_of(owner) else { return Err(KmuxError::new("not_found", "no tour")) };
        tour.at = at;
        let stop = tour.stops[at].clone();
        let of = tour.stops.len();
        self.set_tour(owner, Some(tour));
        if stop.doc {
            return self.show_doc_stop(owner, at, of, stop, args).await;
        }
        let mark = StopMark { owner: owner.to_string(), at, of, thread: stop.thread.clone() };
        let mut place = json!({ "path": stop.path, "from": owner });
        for key in PLACEMENT {
            if let Some(value) = args.get(key) {
                place[key] = value.clone();
            }
        }
        let mut out = self.place_comment(stop.comment(), &place, Some(mark)).await?;
        let shown = out.get("pane").and_then(|p| string(p, "id")).map(String::from);
        self.clear_marks(owner, shown.as_deref()).await;
        out.insert("tour".into(), json!({ "at": at + 1, "of": of }));
        out.insert("path".into(), Value::String(stop.path));
        Ok(out)
    }

    async fn show_doc_stop(&self, owner: &str, at: usize, of: usize, stop: Stop, args: &Value) -> Result<Map, KmuxError> {
        let thread = match stop.thread.clone() {
            Some(thread) => thread,
            None => {
                let agent = self.state().panes.get(owner).and_then(|p| p.terminal.clone()).unwrap_or_else(|| owner.to_string());
                let (thread, _) = kanna_threads::add_agent_thread(std::path::Path::new(&stop.path), stop.first as usize, &stop.text, Some(&agent)).map_err(bad_request)?;
                if let Some(pane) = self.state().panes.get_mut(owner) {
                    if let Some(tour) = pane.tour.as_mut() {
                        tour.stops[at].thread = Some(thread.clone());
                    }
                }
                self.changed();
                thread
            }
        };
        let mark = StopMark { owner: owner.to_string(), at, of, thread: Some(thread.clone()) };
        let placeless = PLACEMENT.iter().all(|key| *key == "wait" || args.get(*key).is_none());
        let found = if placeless { self.showing_in(PaneType::Md, &stop.path, Some(owner)).await? } else { None };
        let (id, location, opened) = match found {
            Some((id, location)) => {
                {
                    let _turn = self.0.turns.take().await;
                    self.layout(json!({ "op": "command", "cmd": "focus", "args": { "pane": id } })).await?;
                }
                if let Some(pane) = self.state().panes.get_mut(&id) {
                    pane.markup = true;
                    pane.agent = Some(owner.to_string());
                    pane.stop = Some(mark);
                }
                let snapshot = self.reload(&id, false).await;
                self.check_started(&id, &snapshot)?;
                (id, Value::Object(location), false)
            }
            None => {
                let mut open = json!({ "type": "md", "path": stop.path, "markup": true, "agent": owner, "from": owner, "tour": mark.to_json() });
                for key in PLACEMENT {
                    if let Some(value) = args.get(key) {
                        open[key] = value.clone();
                    }
                }
                let mut reply = self.open(&open).await?;
                let id = reply.get("pane").and_then(|p| string(p, "id")).unwrap_or_default().to_string();
                let location = json!({ "window": reply.remove("window").unwrap_or(Value::Null), "tab": reply.remove("tab").unwrap_or(Value::Null) });
                (id, location, true)
            }
        };
        self.clear_marks(owner, Some(&id)).await;
        let snapshot = self.state().panes.get(&id).cloned().ok_or_else(|| KmuxError::new("not_found", "the pane closed"))?;
        let mut out = reply([("pane", self.summary(&id, &snapshot)), ("first", num(stop.first as f64)), ("last", num(stop.first as f64)), ("opened", Value::Bool(opened))]);
        for key in ["window", "tab"] {
            out.insert(key.into(), location.get(key).cloned().unwrap_or(Value::Null));
        }
        out.insert("tour".into(), json!({ "at": at + 1, "of": of }));
        out.insert("path".into(), Value::String(stop.path));
        out.insert("thread".into(), Value::String(thread));
        Ok(out)
    }

    async fn refresh_mark(&self, owner: &str, at: usize) -> Map {
        let of = self.tour_of(owner).map(|t| t.stops.len()).unwrap_or(0);
        let marked: Vec<String> = self.state().panes.iter().filter(|p| p.stop.as_ref().is_some_and(|s| s.owner == owner)).map(|p| p.id.clone()).collect();
        for id in &marked {
            if let Some(pane) = self.state().panes.get_mut(id) {
                if let Some(stop) = pane.stop.as_mut() {
                    stop.at = at;
                    stop.of = of;
                }
            }
            self.reload(id, false).await;
        }
        reply([("tour", json!({ "at": at + 1, "of": of }))])
    }

    async fn end_tour(&self, owner: &str) {
        self.set_tour(owner, None);
        self.clear_marks(owner, None).await;
    }

    async fn clear_marks(&self, owner: &str, keep: Option<&str>) {
        let marked: Vec<String> = self.state().panes.iter().filter(|p| Some(p.id.as_str()) != keep && p.stop.as_ref().is_some_and(|s| s.owner == owner)).map(|p| p.id.clone()).collect();
        for id in marked {
            if let Some(pane) = self.state().panes.get_mut(&id) {
                pane.stop = None;
            }
            self.reload(&id, false).await;
        }
        self.changed();
    }

    pub(super) fn forget_tour_of(&self, owner: &str) {
        let mut s = self.state();
        let marked: Vec<String> = s.panes.iter().filter(|p| p.stop.as_ref().is_some_and(|m| m.owner == owner)).map(|p| p.id.clone()).collect();
        for id in marked {
            if let Some(pane) = s.panes.get_mut(&id) {
                pane.stop = None;
            }
        }
    }
}
