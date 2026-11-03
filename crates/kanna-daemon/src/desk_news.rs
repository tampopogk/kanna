use std::time::{Duration, Instant};

pub const SENDER: &str = crate::messages::DESK;
pub const QUIET: Duration = Duration::from_secs(2);
pub const LONGEST: Duration = Duration::from_secs(10);
pub const MERGE_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub enum Merge {
    Pending,
    Unknown,
    Into(String),
    Not(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum What {
    Picked,
    Shelved,
    Ended { on_desk: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub repo: String,
    pub task: String,
    pub short: String,
    pub name: Option<String>,
    pub what: What,
    pub merge: Merge,
    pub at: Instant,
}

impl Item {
    pub fn new(repo: &str, task: &str, short: &str, name: Option<&str>, what: What, at: Instant) -> Item {
        let merge = if matches!(what, What::Ended { .. }) { Merge::Pending } else { Merge::Unknown };
        Item { repo: repo.into(), task: task.into(), short: short.into(), name: name.map(String::from), what, merge, at }
    }

    pub fn line(&self) -> String {
        let name = match &self.name {
            Some(name) => format!("{} \"{name}\"", self.short),
            None => self.short.clone(),
        };
        match (self.what, &self.merge) {
            (What::Picked, _) => format!("{name} moved to the Desk"),
            (What::Shelved, _) => format!("{name} moved to the Shelf"),
            (What::Ended { on_desk: true }, Merge::Into(into)) => format!("{name} merged into {into} and left the Desk"),
            (What::Ended { on_desk: true }, Merge::Not(into)) => format!("{name} ended and left the Desk, not merged into {into}"),
            (What::Ended { on_desk: true }, _) => format!("{name} ended and left the Desk"),
            (What::Ended { on_desk: false }, Merge::Into(into)) => format!("{name} merged into {into} and left the Shelf"),
            (What::Ended { on_desk: false }, _) => format!("{name} was taken off the Shelf"),
        }
    }
}

#[derive(Default)]
pub struct News {
    items: Vec<Item>,
}

impl News {
    pub fn add(&mut self, item: Item) {
        self.items.push(item);
    }

    pub fn merged(&mut self, task: &str, merge: Merge) {
        for item in self.items.iter_mut().filter(|i| i.task == task && i.merge == Merge::Pending) {
            item.merge = merge.clone();
        }
    }

    fn repos(&self) -> Vec<String> {
        let mut repos: Vec<String> = vec![];
        for item in &self.items {
            if !repos.contains(&item.repo) {
                repos.push(item.repo.clone());
            }
        }
        repos
    }

    fn due_at(&self, repo: &str) -> Option<Instant> {
        let burst: Vec<&Item> = self.items.iter().filter(|i| i.repo == repo).collect();
        let first = burst.first()?.at;
        let last = burst.iter().map(|i| i.at).max()?;
        let quiet = (last + QUIET).min(first + LONGEST);
        match burst.iter().any(|i| i.merge == Merge::Pending) {
            true => Some(first + MERGE_WAIT),
            false => Some(quiet),
        }
    }

    pub fn next(&self) -> Option<Instant> {
        self.repos().iter().filter_map(|repo| self.due_at(repo)).min()
    }

    pub fn due(&mut self, now: Instant) -> Vec<(String, String)> {
        let due: Vec<String> = self.repos().into_iter().filter(|repo| self.due_at(repo).is_some_and(|at| at <= now)).collect();
        due.into_iter()
            .map(|repo| {
                let lines: Vec<String> = self.items.iter().filter(|i| i.repo == repo).map(Item::line).collect();
                self.items.retain(|i| i.repo != repo);
                (repo, lines.join("\n"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(repo: &str, task: &str, what: What, at: Instant) -> Item {
        Item::new(repo, task, &format!("task-{task}"), Some(&format!("Task {task}")), what, at)
    }

    #[test]
    fn each_change_is_one_short_line() {
        let now = Instant::now();
        let ended = |on_desk, merge| Item { merge, ..item("r", "a", What::Ended { on_desk }, now) };
        assert_eq!(item("r", "a", What::Picked, now).line(), "task-a \"Task a\" moved to the Desk");
        assert_eq!(item("r", "a", What::Shelved, now).line(), "task-a \"Task a\" moved to the Shelf");
        assert_eq!(ended(true, Merge::Into("main".into())).line(), "task-a \"Task a\" merged into main and left the Desk");
        assert_eq!(ended(true, Merge::Not("main".into())).line(), "task-a \"Task a\" ended and left the Desk, not merged into main");
        assert_eq!(ended(true, Merge::Unknown).line(), "task-a \"Task a\" ended and left the Desk");
        assert_eq!(ended(false, Merge::Into("trunk".into())).line(), "task-a \"Task a\" merged into trunk and left the Shelf");
        assert_eq!(ended(false, Merge::Not("main".into())).line(), "task-a \"Task a\" was taken off the Shelf");
        assert_eq!(Item::new("r", "a", "task-a", None, What::Picked, now).line(), "task-a moved to the Desk", "a task with no name goes by its ID");
    }

    #[test]
    fn a_burst_goes_once_it_has_been_quiet_and_each_repo_goes_on_its_own() {
        let t0 = Instant::now();
        let mut news = News::default();
        news.add(item("r", "a", What::Shelved, t0));
        news.add(item("r", "b", What::Picked, t0 + Duration::from_secs(1)));
        news.add(item("s", "c", What::Picked, t0 + Duration::from_secs(1)));
        assert_eq!(news.next(), Some(t0 + Duration::from_secs(3)));
        assert!(news.due(t0 + Duration::from_secs(2)).is_empty(), "the last change was a second ago");
        assert_eq!(
            news.due(t0 + Duration::from_secs(3)),
            [("r".to_string(), "task-a \"Task a\" moved to the Shelf\ntask-b \"Task b\" moved to the Desk".to_string()), ("s".to_string(), "task-c \"Task c\" moved to the Desk".to_string())]
        );
        assert_eq!(news.next(), None);
    }

    #[test]
    fn a_steady_trickle_still_goes_after_the_longest_wait() {
        let t0 = Instant::now();
        let mut news = News::default();
        for s in 0..12 {
            news.add(item("r", &s.to_string(), What::Shelved, t0 + Duration::from_secs(s)));
        }
        assert_eq!(news.next(), Some(t0 + LONGEST));
    }

    #[test]
    fn an_ended_task_waits_for_its_merge_to_be_known_but_not_forever() {
        let t0 = Instant::now();
        let mut news = News::default();
        news.add(item("r", "a", What::Ended { on_desk: true }, t0));
        assert!(news.due(t0 + Duration::from_secs(5)).is_empty(), "its merge isn't known yet");
        news.merged("a", Merge::Into("main".into()));
        assert_eq!(news.due(t0 + Duration::from_secs(5)), [("r".to_string(), "task-a \"Task a\" merged into main and left the Desk".to_string())]);
        news.add(item("r", "b", What::Ended { on_desk: true }, t0));
        assert_eq!(news.due(t0 + MERGE_WAIT), [("r".to_string(), "task-b \"Task b\" ended and left the Desk".to_string())]);
    }
}
