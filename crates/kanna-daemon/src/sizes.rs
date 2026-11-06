use std::cmp::Reverse;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub width: u16,
    pub height: u16,
}

struct Wanted {
    conn: u64,
    local: bool,
    size: Option<Size>,
    used: u64,
}

pub type Driver = (u64, Size);

#[derive(Default)]
pub struct Sizes {
    terminals: HashMap<String, Vec<Wanted>>,
    clock: u64,
}

fn driver(wanted: &[Wanted]) -> Option<Driver> {
    let last = wanted.iter().filter(|w| w.size.is_some()).max_by_key(|w| (w.used, w.local, Reverse(w.conn)))?;
    Some((last.conn, last.size?))
}

impl Sizes {
    fn wanted<'a>(wanted: &'a mut Vec<Wanted>, conn: u64, local: bool) -> &'a mut Wanted {
        match wanted.iter().position(|w| w.conn == conn) {
            Some(at) => &mut wanted[at],
            None => {
                wanted.push(Wanted { conn, local, size: None, used: 0 });
                wanted.last_mut().expect("just pushed")
            }
        }
    }

    pub fn asked(&mut self, conn: u64, local: bool, terminal: &str, size: Size) -> bool {
        let wanted = self.terminals.entry(terminal.into()).or_default();
        Self::wanted(wanted, conn, local).size = Some(size);
        driver(wanted).is_some_and(|(by, _)| by == conn)
    }

    pub fn used(&mut self, conn: u64, local: bool, terminal: &str) -> Option<Driver> {
        self.clock += 1;
        let wanted = self.terminals.entry(terminal.into()).or_default();
        let before = driver(wanted);
        Self::wanted(wanted, conn, local).used = self.clock;
        driver(wanted).filter(|after| Some(*after) != before)
    }

    pub fn left(&mut self, conn: u64, terminal: &str) -> Option<Driver> {
        let wanted = self.terminals.get_mut(terminal)?;
        let before = driver(wanted);
        wanted.retain(|w| w.conn != conn);
        let after = driver(wanted).filter(|after| Some(*after) != before);
        if wanted.is_empty() {
            self.terminals.remove(terminal);
        }
        after
    }

    pub fn gone(&mut self, conn: u64) -> Vec<(String, Driver)> {
        let mut terminals: Vec<String> = self.terminals.keys().cloned().collect();
        terminals.sort();
        terminals.into_iter().filter_map(|terminal| self.left(conn, &terminal).map(|driver| (terminal, driver))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: Size = Size { cols: 200, rows: 50, width: 2000, height: 1000 };
    const NARROW: Size = Size { cols: 80, rows: 24, width: 800, height: 480 };

    #[test]
    fn the_first_app_to_ask_sets_the_size_until_another_is_used() {
        let mut sizes = Sizes::default();
        assert!(sizes.asked(1, true, "term-1", WIDE));
        assert!(!sizes.asked(2, false, "term-1", NARROW), "a second app that only looks draws the first's size");
        assert_eq!(sizes.used(2, false, "term-1"), Some((2, NARROW)), "typing takes the size");
        assert_eq!(sizes.used(2, false, "term-1"), None, "and it isn't taken twice");
        assert!(!sizes.asked(1, true, "term-1", NARROW), "resizing a view doesn't take the size from who used it last");
        assert_eq!(sizes.used(1, true, "term-1"), Some((1, NARROW)));
        assert_eq!(sizes.used(2, false, "term-1"), Some((2, NARROW)));
        assert!(sizes.asked(2, false, "term-1", WIDE), "who used it last follows its own view");
    }

    #[test]
    fn the_last_usage_wins_back_and_forth() {
        let mut sizes = Sizes::default();
        sizes.asked(1, true, "term-1", WIDE);
        sizes.asked(2, false, "term-1", NARROW);
        assert_eq!(sizes.used(2, false, "term-1"), Some((2, NARROW)));
        assert_eq!(sizes.used(1, true, "term-1"), Some((1, WIDE)));
        assert_eq!(sizes.used(2, false, "term-1"), Some((2, NARROW)));
    }

    #[test]
    fn each_terminal_follows_its_own_usage() {
        let mut sizes = Sizes::default();
        for terminal in ["term-1", "term-2"] {
            sizes.asked(1, true, terminal, WIDE);
            sizes.asked(2, false, terminal, NARROW);
        }
        assert_eq!(sizes.used(2, false, "term-1"), Some((2, NARROW)));
        assert!(sizes.asked(1, true, "term-2", WIDE), "using one terminal leaves the other's size alone");
    }

    #[test]
    fn when_the_app_that_set_the_size_leaves_the_last_one_used_takes_over() {
        let mut sizes = Sizes::default();
        sizes.asked(1, true, "term-1", WIDE);
        sizes.asked(2, false, "term-1", NARROW);
        sizes.asked(3, false, "term-1", Size { cols: 100, ..NARROW });
        sizes.used(2, false, "term-1");
        sizes.used(3, false, "term-1");
        assert_eq!(sizes.left(1, "term-1"), None, "one that wasn't setting the size changes nothing");
        assert_eq!(sizes.gone(3), vec![("term-1".to_string(), (2, NARROW))]);
        assert_eq!(sizes.left(2, "term-1"), None);
        assert!(sizes.terminals.is_empty());
    }

    #[test]
    fn with_no_usage_the_machines_own_app_sets_the_size() {
        let mut sizes = Sizes::default();
        assert!(sizes.asked(4, false, "term-1", NARROW));
        assert!(sizes.asked(5, true, "term-1", WIDE), "this machine's own app, though it came later");
        assert!(!sizes.asked(6, false, "term-1", NARROW));
    }

    #[test]
    fn usage_before_a_size_counts_once_the_size_arrives() {
        let mut sizes = Sizes::default();
        sizes.asked(1, true, "term-1", WIDE);
        assert_eq!(sizes.used(2, false, "term-1"), None);
        assert!(sizes.asked(2, false, "term-1", NARROW));
    }
}
