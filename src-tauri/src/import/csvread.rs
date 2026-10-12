//! A streaming CSV reader that, unlike the csv crate, tells whether each field was quoted: the
//! wizard's "Empty unquoted field as NULL" makes an unquoted empty field NULL while `""` stays an
//! empty string.
//!
//! RFC 4180 with the usual leniency: the quote character is `"` or `'` (doubled inside a quoted
//! field), records end at `\n`, `\r\n` or `\r`, a quote inside an unquoted field is literal text,
//! text after a closing quote is kept, and blank lines are skipped. Each record carries the line
//! it starts on, for error messages.

use std::io::BufRead;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub text: String,
    pub quoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub fields: Vec<Field>,
    /// 1-based line the record starts on.
    pub line: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// At the start of a field.
    Start,
    Unquoted,
    Quoted,
    /// A quote inside a quoted field: doubled (an escaped quote) or the closing one.
    QuoteSeen,
}

pub struct CsvReader<R: BufRead> {
    r: R,
    delimiter: u8,
    quote: u8,
    /// Line of the next byte.
    line: u64,
    /// The previous byte was `\r` (a following `\n` belongs to the same line break).
    last_cr: bool,
    /// The previous record ended at `\r`: a `\n` right after it is part of that line break.
    skip_lf: bool,
    done: bool,
}

impl<R: BufRead> CsvReader<R> {
    pub fn new(r: R, delimiter: u8, quote: u8) -> CsvReader<R> {
        CsvReader {
            r,
            delimiter,
            quote,
            line: 1,
            last_cr: false,
            skip_lf: false,
            done: false,
        }
    }

    /// The next record that is not a blank line; `None` at the end of the input.
    pub fn next_record(&mut self) -> Result<Option<Record>, AppError> {
        loop {
            match self.read_raw()? {
                None => return Ok(None),
                Some(rec) if rec.fields.len() == 1 && rec.fields[0] == BLANK => continue,
                Some(rec) => return Ok(Some(rec)),
            }
        }
    }

    fn read_raw(&mut self) -> Result<Option<Record>, AppError> {
        if self.done {
            return Ok(None);
        }
        let start = self.line;
        let mut fields: Vec<(Vec<u8>, bool)> = Vec::new();
        let mut field: Vec<u8> = Vec::new();
        let mut quoted = false;
        let mut state = State::Start;
        let mut any = false;
        loop {
            let buf = self
                .r
                .fill_buf()
                .map_err(|e| AppError::Io(format!("cannot read the file: {e}")))?;
            if buf.is_empty() {
                self.done = true;
                if !any {
                    return Ok(None);
                }
                fields.push((field, quoted));
                return finish(fields, start).map(Some);
            }
            let mut used = 0;
            let mut ended = false;
            for &b in buf {
                used += 1;
                // Line numbers: `\r\n`, `\n` and `\r` each end one line.
                if b == b'\n' {
                    if !self.last_cr {
                        self.line += 1;
                    }
                } else if b == b'\r' {
                    self.line += 1;
                }
                self.last_cr = b == b'\r';
                if self.skip_lf {
                    self.skip_lf = false;
                    if b == b'\n' {
                        continue;
                    }
                }
                any = true;
                let ends_record = b == b'\n' || b == b'\r';
                match state {
                    State::Start | State::Unquoted | State::QuoteSeen if b == self.delimiter => {
                        fields.push((std::mem::take(&mut field), quoted));
                        quoted = false;
                        state = State::Start;
                    }
                    State::Start | State::Unquoted | State::QuoteSeen if ends_record => {
                        self.skip_lf = b == b'\r';
                        fields.push((std::mem::take(&mut field), quoted));
                        ended = true;
                        break;
                    }
                    State::Start if b == self.quote => {
                        quoted = true;
                        state = State::Quoted;
                    }
                    State::Quoted if b == self.quote => state = State::QuoteSeen,
                    State::QuoteSeen if b == self.quote => {
                        field.push(b);
                        state = State::Quoted;
                    }
                    State::Quoted => field.push(b),
                    // Text after the closing quote, or any other byte of an unquoted field.
                    State::Start | State::Unquoted | State::QuoteSeen => {
                        field.push(b);
                        state = State::Unquoted;
                    }
                }
            }
            self.r.consume(used);
            if ended {
                return finish(fields, start).map(Some);
            }
        }
    }
}

const BLANK: Field = Field {
    text: String::new(),
    quoted: false,
};

fn finish(fields: Vec<(Vec<u8>, bool)>, line: u64) -> Result<Record, AppError> {
    let fields = fields
        .into_iter()
        .map(|(bytes, quoted)| match String::from_utf8(bytes) {
            Ok(text) => Ok(Field { text, quoted }),
            Err(_) => Err(AppError::InvalidInput(format!(
                "line {line} is not valid UTF-8: choose the file's encoding"
            ))),
        })
        .collect::<Result<_, _>>()?;
    Ok(Record { fields, line })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn records(text: &str, delimiter: u8, quote: u8) -> Vec<Record> {
        // A tiny buffer cuts the input everywhere.
        let mut r = CsvReader::new(
            BufReader::with_capacity(3, text.as_bytes()),
            delimiter,
            quote,
        );
        let mut out = Vec::new();
        while let Some(rec) = r.next_record().unwrap() {
            out.push(rec);
        }
        out
    }

    fn texts(recs: &[Record]) -> Vec<Vec<(&str, bool)>> {
        recs.iter()
            .map(|r| {
                r.fields
                    .iter()
                    .map(|f| (f.text.as_str(), f.quoted))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn quoting_and_empties() {
        let recs = records("a,b,c\n1,\"x, \"\"y\"\"\",\n2,\"\",z\n", b',', b'"');
        assert_eq!(
            texts(&recs),
            vec![
                vec![("a", false), ("b", false), ("c", false)],
                vec![("1", false), ("x, \"y\"", true), ("", false)],
                vec![("2", false), ("", true), ("z", false)],
            ]
        );
        assert_eq!(recs.iter().map(|r| r.line).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn line_breaks_inside_and_between_records() {
        let recs = records(
            "id;note\r\n1;'multi\r\nline'\r2;'it''s'\n\n\r\n3;end",
            b';',
            b'\'',
        );
        assert_eq!(
            texts(&recs),
            vec![
                vec![("id", false), ("note", false)],
                vec![("1", false), ("multi\r\nline", true)],
                vec![("2", false), ("it's", true)],
                vec![("3", false), ("end", false)],
            ]
        );
        // Lines: 1, 2 (spans to 3), 4, then two blank lines, 7.
        assert_eq!(
            recs.iter().map(|r| r.line).collect::<Vec<_>>(),
            [1, 2, 4, 7]
        );
    }

    #[test]
    fn lenient_cases() {
        // A quote inside an unquoted field is text; text after a closing quote is kept.
        let recs = records("ab\"c,\"d\"e\n", b',', b'"');
        assert_eq!(texts(&recs), vec![vec![("ab\"c", false), ("de", true)]]);
        // No line break at the end; a lone quoted field running to the end.
        let recs = records("1,2", b',', b'"');
        assert_eq!(texts(&recs), vec![vec![("1", false), ("2", false)]]);
        let recs = records("\"open", b',', b'"');
        assert_eq!(texts(&recs), vec![vec![("open", true)]]);
        // Tabs; trailing delimiter makes an empty last field.
        let recs = records("a\tb\t\n", b'\t', b'"');
        assert_eq!(
            texts(&recs),
            vec![vec![("a", false), ("b", false), ("", false)]]
        );
        // An empty quoted field alone on a line is not a blank line.
        let recs = records("\"\"\n\n", b',', b'"');
        assert_eq!(texts(&recs), vec![vec![("", true)]]);
        assert!(records("", b',', b'"').is_empty());
        assert!(records("\n\r\n", b',', b'"').is_empty());
    }

    #[test]
    fn invalid_utf8_names_the_line() {
        let bytes: &[u8] = b"a\nb\xFF\n";
        let mut r = CsvReader::new(BufReader::new(bytes), b',', b'"');
        assert!(r.next_record().unwrap().is_some());
        let err = r.next_record().unwrap_err();
        assert!(err.to_string().contains("line 2"), "{err}");
    }
}
