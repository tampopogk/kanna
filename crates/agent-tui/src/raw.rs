//! Bounded log of every protocol record in both directions, kept verbatim so
//! transcript entries can be inspected as the original JSON.

use std::collections::VecDeque;

pub type RawId = u64;

/// Records longer than this are truncated for display, with a notice.
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

#[derive(Debug, Clone)]
pub struct RawRecord {
    pub id: RawId,
    pub dir: Dir,
    /// Line number in the harness's stdout (incoming only).
    pub line_no: Option<u64>,
    pub text: String,
    /// Bytes dropped from `text` because the record exceeded the display limit.
    pub truncated: usize,
    pub malformed: bool,
}

#[derive(Debug)]
pub struct RawLog {
    records: VecDeque<RawRecord>,
    next_id: RawId,
    max: usize,
    pub discarded: u64,
}

impl Default for RawLog {
    fn default() -> Self {
        Self::with_capacity(MAX_RECORDS)
    }
}

impl RawLog {
    pub fn with_capacity(max: usize) -> Self {
        Self {
            records: VecDeque::new(),
            next_id: 1,
            max,
            discarded: 0,
        }
    }

    pub fn push(&mut self, dir: Dir, line_no: Option<u64>, text: &str, malformed: bool) -> RawId {
        let id = self.next_id;
        self.next_id += 1;
        let (text, truncated) = if text.len() > MAX_RECORD_BYTES {
            let mut cut = MAX_RECORD_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            (text[..cut].to_string(), text.len() - cut)
        } else {
            (text.to_string(), 0)
        };
        self.records.push_back(RawRecord {
            id,
            dir,
            line_no,
            text,
            truncated,
            malformed,
        });
        while self.records.len() > self.max {
            self.records.pop_front();
            self.discarded += 1;
        }
        id
    }

    pub fn get(&self, id: RawId) -> Option<&RawRecord> {
        let first = self.records.front()?.id;
        if id < first {
            return None;
        }
        self.records.get((id - first) as usize)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &RawRecord> {
        self.records.iter()
    }

    pub fn last_id(&self) -> Option<RawId> {
        self.records.back().map(|r| r.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_and_addressable() {
        let mut log = RawLog::with_capacity(3);
        let ids: Vec<_> = (0..5)
            .map(|i| log.push(Dir::In, Some(i), &format!("{{\"n\":{i}}}"), false))
            .collect();
        assert_eq!(log.len(), 3);
        assert_eq!(log.discarded, 2);
        assert!(log.get(ids[0]).is_none());
        assert_eq!(log.get(ids[4]).unwrap().text, "{\"n\":4}");
    }

    #[test]
    fn oversized_record_truncated_with_notice() {
        let mut log = RawLog::default();
        let big = "é".repeat(MAX_RECORD_BYTES);
        let id = log.push(Dir::In, None, &big, false);
        let r = log.get(id).unwrap();
        assert!(r.text.len() <= MAX_RECORD_BYTES);
        assert_eq!(r.text.len() + r.truncated, big.len());
    }
}
