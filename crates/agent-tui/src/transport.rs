//! Newline-delimited JSON framing for harness stdout.
//!
//! Chunks from a pipe can split a record anywhere, carry several records, use
//! CRLF, or end without a trailing newline. The framer buffers partial input
//! and yields one [`Record`] per non-blank line. A line that is not valid JSON
//! still becomes a record (with `parsed` set to the error) so the caller can
//! surface it as a diagnostic while later records keep flowing.

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Record {
    /// 1-based line number in the stream, counting blank lines.
    pub line_no: u64,
    /// The original text of the line, without its line terminator.
    pub raw: String,
    pub parsed: Result<Value, String>,
}

#[derive(Debug, Default)]
pub struct JsonlFramer {
    buf: Vec<u8>,
    line_no: u64,
}

impl JsonlFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; returns every record completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Record> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, b) in chunk.iter().enumerate() {
            if *b == b'\n' {
                self.buf.extend_from_slice(&chunk[start..i]);
                start = i + 1;
                self.line_no += 1;
                let line = std::mem::take(&mut self.buf);
                if let Some(rec) = make_record(self.line_no, &line) {
                    out.push(rec);
                }
            }
        }
        self.buf.extend_from_slice(&chunk[start..]);
        out
    }

    /// Call at EOF: parses a final line that had no newline.
    pub fn finish(&mut self) -> Option<Record> {
        if self.buf.is_empty() {
            return None;
        }
        self.line_no += 1;
        let line = std::mem::take(&mut self.buf);
        make_record(self.line_no, &line)
    }

    /// Bytes currently buffered waiting for a newline.
    pub fn pending_len(&self) -> usize {
        self.buf.len()
    }
}

fn make_record(line_no: u64, bytes: &[u8]) -> Option<Record> {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let raw = String::from_utf8_lossy(bytes).into_owned();
    if raw.trim().is_empty() {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(&raw).map_err(|e| e.to_string());
    Some(Record {
        line_no,
        raw,
        parsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(recs: &[Record]) -> Vec<Value> {
        recs.iter().map(|r| r.parsed.clone().unwrap()).collect()
    }

    #[test]
    fn record_split_across_chunks() {
        let mut f = JsonlFramer::new();
        assert!(f.push(br#"{"a":"#).is_empty());
        assert!(f.push(br#"1,"b":"x"#).is_empty());
        let recs = f.push(b"y\"}\n");
        assert_eq!(values(&recs), vec![serde_json::json!({"a":1,"b":"xy"})]);
        assert_eq!(recs[0].line_no, 1);
    }

    #[test]
    fn several_records_in_one_chunk() {
        let mut f = JsonlFramer::new();
        let recs = f.push(b"{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":");
        assert_eq!(recs.len(), 3);
        assert_eq!(f.pending_len(), 5);
        let recs = f.push(b"4}\n");
        assert_eq!(values(&recs), vec![serde_json::json!({"n":4})]);
        assert_eq!(recs[0].line_no, 4);
    }

    #[test]
    fn crlf_and_blank_lines() {
        let mut f = JsonlFramer::new();
        let recs = f.push(b"{\"n\":1}\r\n\r\n   \n{\"n\":2}\r\n");
        assert_eq!(
            values(&recs),
            vec![serde_json::json!({"n":1}), serde_json::json!({"n":2})]
        );
        assert_eq!(recs[0].raw, "{\"n\":1}");
        // Blank lines still count toward line numbers.
        assert_eq!(recs[1].line_no, 4);
    }

    #[test]
    fn final_line_without_newline_parsed_at_eof() {
        let mut f = JsonlFramer::new();
        assert!(f.push(b"{\"n\":1}\n{\"last\":true}").len() == 1);
        let rec = f.finish().unwrap();
        assert_eq!(rec.parsed.unwrap(), serde_json::json!({"last":true}));
        assert!(f.finish().is_none());
    }

    #[test]
    fn incomplete_json_at_eof_is_malformed_not_dropped() {
        let mut f = JsonlFramer::new();
        f.push(b"{\"type\":\"result\"");
        let rec = f.finish().unwrap();
        assert!(rec.parsed.is_err());
        assert_eq!(rec.raw, "{\"type\":\"result\"");
    }

    #[test]
    fn malformed_record_followed_by_valid_ones() {
        let mut f = JsonlFramer::new();
        let recs = f.push(b"{\"n\":1}\nnot json at all\n{\"n\":3}\n");
        assert_eq!(recs.len(), 3);
        assert!(recs[0].parsed.is_ok());
        assert!(recs[1].parsed.is_err());
        assert_eq!(recs[1].raw, "not json at all");
        assert_eq!(recs[1].line_no, 2);
        assert_eq!(recs[2].parsed.clone().unwrap(), serde_json::json!({"n":3}));
    }

    #[test]
    fn byte_at_a_time() {
        let mut f = JsonlFramer::new();
        let input = b"{\"text\":\"h\xc3\xa9llo\"}\n{\"n\":2}\n";
        let mut recs = Vec::new();
        for b in input.iter() {
            recs.extend(f.push(&[*b]));
        }
        assert_eq!(
            values(&recs),
            vec![
                serde_json::json!({"text":"héllo"}),
                serde_json::json!({"n":2})
            ]
        );
    }
}
