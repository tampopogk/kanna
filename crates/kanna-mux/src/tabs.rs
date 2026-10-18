use crate::model::{normalize, pane_ids, split_at, Axis, Child, Node, Split};
use crate::pane::{Pane, Panes};

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub root: Option<Node>,
    pub last_focus: Option<String>,
    pub number: usize,
    pub namer: Option<String>,
    pub renamed: bool,
}

impl Tab {
    pub(crate) fn new(id: String, number: usize) -> Tab {
        Tab { id, title: format!("Tab {number}"), root: None, last_focus: None, number, namer: None, renamed: false }
    }

    pub(crate) fn rename(&mut self, title: &str) {
        self.title = title.into();
        self.renamed = true;
    }

    pub(crate) fn retitle(&mut self, panes: &Panes) -> bool {
        let ids = pane_ids(self.root.as_ref());
        if !self.namer.as_ref().is_some_and(|n| ids.contains(n)) {
            self.namer = ids.into_iter().next();
        }
        if self.renamed {
            return false;
        }
        let title = self.namer.as_deref().and_then(|n| panes.get(n)).and_then(Pane::tab_name).unwrap_or_else(|| format!("Tab {}", self.number));
        let changed = title != self.title;
        self.title = title;
        changed
    }

    pub fn panes(&self) -> Vec<String> {
        pane_ids(self.root.as_ref())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub active: Option<String>,
    pub focused: Option<String>,
    pub zoom: bool,
    pub count: usize,
}

impl Tabs {
    pub(crate) fn rename(&mut self, from: &str, to: &str) {
        fn walk(node: &mut Node, from: &str, to: &str) {
            match node {
                Node::Pane(id) if id == from => *id = to.into(),
                Node::Pane(_) => {}
                Node::Split(split) => split.kids.iter_mut().for_each(|kid| walk(&mut kid.node, from, to)),
            }
        }
        let rename = |id: &mut Option<String>| {
            if id.as_deref() == Some(from) {
                *id = Some(to.into());
            }
        };
        rename(&mut self.focused);
        for tab in &mut self.tabs {
            if let Some(root) = &mut tab.root {
                walk(root, from, to);
            }
            rename(&mut tab.last_focus);
            rename(&mut tab.namer);
        }
    }

    pub(crate) fn active_index(&self) -> Option<usize> {
        self.tabs.iter().position(|t| Some(&t.id) == self.active.as_ref()).or(if self.tabs.is_empty() { None } else { Some(0) })
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_index().map(|i| &self.tabs[i])
    }

    pub fn shown(&self) -> Vec<String> {
        pane_ids(self.active_tab().and_then(|t| t.root.as_ref()))
    }

    pub fn panes(&self) -> Vec<String> {
        self.tabs.iter().flat_map(Tab::panes).collect()
    }

    pub fn position(&self, tab: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == tab)
    }

    pub fn zoomed(&self) -> Option<&String> {
        self.focused.as_ref().filter(|_| self.zoom)
    }

    pub(crate) fn locate(&self, id: &str) -> Option<(usize, Option<Vec<usize>>, usize)> {
        self.tabs.iter().enumerate().find_map(|(t, tab)| locate_in(tab.root.as_ref(), id).map(|(parent, index)| (t, parent, index)))
    }

    pub(crate) fn show(&mut self, t: usize) {
        let ids = self.tabs[t].panes();
        self.active = Some(self.tabs[t].id.clone());
        self.focused = self.tabs[t].last_focus.clone().filter(|l| ids.contains(l)).or_else(|| ids.first().cloned());
    }

    pub(crate) fn focus(&mut self, id: &str) {
        if let Some((t, _, _)) = self.locate(id) {
            self.active = Some(self.tabs[t].id.clone());
            self.tabs[t].last_focus = Some(id.into());
        }
        self.focused = Some(id.into());
    }

    pub(crate) fn settle_focus(&mut self, beside: Option<&String>) {
        let ids = self.shown();
        if self.focused.as_ref().is_some_and(|f| ids.contains(f) || Some(f) == beside) {
            return;
        }
        let last = self.active_tab().and_then(|t| t.last_focus.clone()).filter(|l| ids.contains(l));
        self.focused = last.or_else(|| ids.first().cloned()).or_else(|| beside.cloned());
    }

    pub(crate) fn tidy(&mut self) {
        let at = self.tabs.iter().position(|t| Some(&t.id) == self.active.as_ref()).unwrap_or(0);
        self.tabs.retain(|t| t.root.is_some());
        if !self.tabs.iter().any(|t| Some(&t.id) == self.active.as_ref()) {
            self.active = if self.tabs.is_empty() { None } else { Some(self.tabs[at.min(self.tabs.len() - 1)].id.clone()) };
        }
    }

    pub(crate) fn remove(&mut self, id: &str) -> bool {
        let Some((t, parent, index)) = self.locate(id) else { return false };
        let tab = &mut self.tabs[t];
        match &parent {
            Some(path) => {
                let parent = split_at(tab.root.as_mut().unwrap(), path);
                let gone = parent.kids.remove(index).size;
                let count = parent.kids.len() as f64;
                for kid in &mut parent.kids {
                    kid.size = if gone < 1.0 { kid.size / (1.0 - gone) } else { 1.0 / count };
                }
                tab.root = tab.root.take().map(normalize);
            }
            None => tab.root = None,
        }
        true
    }

    pub(crate) fn insert(&mut self, node: Node, beside: &str, t: usize, axis: Axis, after: bool, size: f64) {
        let tab = &mut self.tabs[t];
        let Some((parent, index)) = locate_in(tab.root.as_ref(), beside) else { return };
        if let Some(path) = &parent {
            let split = split_at(tab.root.as_mut().unwrap(), path);
            if split.axis == axis {
                let share = split.kids[index].size;
                split.kids[index].size = share * (1.0 - size);
                split.kids.insert(index + after as usize, Child { node, size: share * size });
                return;
            }
        }
        let mut kids = vec![Child { node: Node::Pane(beside.into()), size: 1.0 - size }, Child { node, size }];
        if !after {
            kids.reverse();
        }
        let split = Node::Split(Split { axis, kids });
        match &parent {
            Some(path) => split_at(tab.root.as_mut().unwrap(), path).kids[index].node = split,
            None => tab.root = Some(split),
        }
    }

    pub(crate) fn append(&mut self, id: &str, t: usize) {
        let tab = &mut self.tabs[t];
        let pane = Node::Pane(id.into());
        tab.root = Some(match tab.root.take() {
            None => pane,
            Some(Node::Split(mut split)) if split.axis == Axis::Row => {
                let count = split.kids.len() as f64;
                for kid in &mut split.kids {
                    kid.size *= count / (count + 1.0);
                }
                split.kids.push(Child { node: pane, size: 1.0 / (count + 1.0) });
                Node::Split(split)
            }
            Some(root) => Node::Split(Split { axis: Axis::Row, kids: vec![Child { node: root, size: 0.5 }, Child { node: pane, size: 0.5 }] }),
        });
    }

    pub(crate) fn set_at(&mut self, t: usize, parent: Option<&[usize]>, index: usize, id: &str) {
        let tab = &mut self.tabs[t];
        match parent {
            Some(path) => split_at(tab.root.as_mut().unwrap(), path).kids[index].node = Node::Pane(id.into()),
            None => tab.root = Some(Node::Pane(id.into())),
        }
    }
}

pub(crate) fn locate_in(root: Option<&Node>, id: &str) -> Option<(Option<Vec<usize>>, usize)> {
    fn find(split: &Split, id: &str, here: &mut Vec<usize>) -> Option<(Option<Vec<usize>>, usize)> {
        for (i, kid) in split.kids.iter().enumerate() {
            match &kid.node {
                Node::Pane(pane) if pane == id => return Some((Some(here.clone()), i)),
                Node::Pane(_) => {}
                Node::Split(inner) => {
                    here.push(i);
                    if let Some(found) = find(inner, id, here) {
                        return Some(found);
                    }
                    here.pop();
                }
            }
        }
        None
    }
    match root? {
        Node::Pane(pane) => (pane == id).then_some((None, 0)),
        Node::Split(split) => find(split, id, &mut vec![]),
    }
}
