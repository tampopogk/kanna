//! Multiline draft buffer with a character cursor.

#[derive(Debug, Default, Clone)]
pub struct Composer {
    text: String,
    /// Byte offset of the cursor; always on a char boundary.
    cursor: usize,
}

impl Composer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        let s = s
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    pub fn newline(&mut self) {
        self.insert_char('\n');
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    pub fn backspace(&mut self) {
        if let Some(p) = self.prev_boundary() {
            self.text.replace_range(p..self.cursor, "");
            self.cursor = p;
        }
    }

    pub fn delete(&mut self) {
        if let Some(n) = self.next_boundary() {
            self.text.replace_range(self.cursor..n, "");
        }
    }

    pub fn left(&mut self) {
        if let Some(p) = self.prev_boundary() {
            self.cursor = p;
        }
    }

    pub fn right(&mut self) {
        if let Some(n) = self.next_boundary() {
            self.cursor = n;
        }
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    /// Moves to the previous logical line. Returns false when already on the first.
    pub fn up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let col = self.text[start..self.cursor].chars().count();
        let prev_start = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = advance(&self.text, prev_start, start - 1, col);
        true
    }

    /// Moves to the next logical line. Returns false when already on the last.
    pub fn down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let col = self.text[self.line_start()..self.cursor].chars().count();
        let next_start = end + 1;
        let next_end = self.text[next_start..]
            .find('\n')
            .map_or(self.text.len(), |i| next_start + i);
        self.cursor = advance(&self.text, next_start, next_end, col);
        true
    }

    pub fn delete_word_back(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|c: char| c.is_whitespace() && c != '\n');
        let start = trimmed
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |i| i + 1);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn kill_to_line_start(&mut self) {
        let start = self.line_start();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }
}

fn advance(text: &str, from: usize, limit: usize, cols: usize) -> usize {
    let mut pos = from;
    for (n, c) in text[from..limit].chars().enumerate() {
        if n == cols {
            break;
        }
        pos += c.len_utf8();
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_cursor() {
        let mut c = Composer::default();
        for ch in "héllo".chars() {
            c.insert_char(ch);
        }
        c.left();
        c.left();
        c.backspace();
        assert_eq!(c.text(), "hélo");
        c.home();
        c.delete();
        assert_eq!(c.text(), "élo");
        c.end();
        c.newline();
        c.insert_str("line2\r\nline3");
        assert_eq!(c.text(), "élo\nline2\nline3");
        assert_eq!(c.line_count(), 3);
        assert!(c.up());
        assert!(c.up());
        assert!(!c.up());
        assert!(c.down());
        c.home();
        c.insert_char('>');
        assert_eq!(c.text(), "élo\n>line2\nline3");
    }

    #[test]
    fn delete_word() {
        let mut c = Composer::default();
        c.set("fix the parser  ");
        c.delete_word_back();
        assert_eq!(c.text(), "fix the ");
    }
}
