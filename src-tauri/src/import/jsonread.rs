//! A streaming reader of the objects of a JSON file: one array of objects (`[{…}, {…}]`) or
//! one object after another (NDJSON / JSON Lines). Only the current object is held in memory:
//! its bytes are cut out of the stream (string-aware, by bracket depth), checked with
//! serde_json, and split into its top-level keys and values.
//!
//! Values keep their text: a string is unescaped, a number or boolean stays as written (no
//! float rounding: `12.50` and `12345678901234567890` arrive as they are), a nested object or
//! array becomes compact JSON text, `null` is `None`.

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};

use crate::error::AppError;
use crate::types::import::JsonLayout;

use super::csvread::{Field, Record, RecordRead};

/// Largest object read (its JSON text).
pub const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;

/// One object of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonObject {
    /// 1-based line the object starts on.
    pub line: u64,
    /// Its keys in file order (a repeated key keeps its last value) with their values.
    pub entries: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    /// Inside the array; `first`: no object read yet.
    Array {
        first: bool,
    },
    Lines,
    Done,
}

pub struct JsonObjects<R: BufRead> {
    r: R,
    /// Line of the next byte.
    line: u64,
    state: State,
    layout: Option<JsonLayout>,
    /// Objects read so far.
    count: u64,
    buf: Vec<u8>,
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::Io(e.to_string())
}

impl<R: BufRead> JsonObjects<R> {
    pub fn new(r: R) -> JsonObjects<R> {
        JsonObjects {
            r,
            line: 1,
            state: State::Start,
            layout: None,
            count: 0,
            buf: Vec::new(),
        }
    }

    /// Array or lines, once the first byte was read (`None` before, or for an empty file).
    pub fn layout(&self) -> Option<JsonLayout> {
        self.layout
    }

    fn bump(&mut self) {
        if let Ok(buf) = self.r.fill_buf()
            && let Some(&b) = buf.first()
        {
            if b == b'\n' {
                self.line += 1;
            }
            self.r.consume(1);
        }
    }

    /// Skips white space; returns the next byte (not consumed).
    fn skip_ws(&mut self) -> Result<Option<u8>, AppError> {
        loop {
            let buf = self.r.fill_buf().map_err(io_err)?;
            if buf.is_empty() {
                return Ok(None);
            }
            let n = buf
                .iter()
                .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                .unwrap_or(buf.len());
            self.line += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
            let next = buf.get(n).copied();
            self.r.consume(n);
            if next.is_some() {
                return Ok(next);
            }
        }
    }

    fn not_an_object(&self, found: u8) -> AppError {
        let what = match found {
            b'[' => "an array",
            b'"' => "a string",
            b'-' | b'0'..=b'9' => "a number",
            b't' | b'f' => "a boolean",
            b'n' => "null",
            _ => "not JSON",
        };
        AppError::InvalidInput(format!(
            "item {} (line {}) is {what}, not an object: the file must hold an array of objects or one object per line",
            self.count + 1,
            self.line
        ))
    }

    /// Cuts the object starting at the next byte (`{`) out of the stream into `buf`.
    fn capture(&mut self) -> Result<u64, AppError> {
        let start_line = self.line;
        self.buf.clear();
        let (mut depth, mut in_str, mut esc) = (0usize, false, false);
        loop {
            let chunk = self.r.fill_buf().map_err(io_err)?;
            if chunk.is_empty() {
                return Err(AppError::InvalidInput(format!(
                    "the file ends inside item {} (line {start_line})",
                    self.count + 1
                )));
            }
            let mut end = None;
            for (i, &b) in chunk.iter().enumerate() {
                if b == b'\n' {
                    self.line += 1;
                }
                if in_str {
                    if esc {
                        esc = false;
                    } else if b == b'\\' {
                        esc = true;
                    } else if b == b'"' {
                        in_str = false;
                    }
                    continue;
                }
                match b {
                    b'"' => in_str = true,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            end = Some(i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let take = end.unwrap_or(chunk.len());
            self.buf.extend_from_slice(&chunk[..take]);
            self.r.consume(take);
            if self.buf.len() > MAX_OBJECT_BYTES {
                return Err(AppError::InvalidInput(format!(
                    "item {} (line {start_line}) is larger than {} MB",
                    self.count + 1,
                    MAX_OBJECT_BYTES / (1024 * 1024)
                )));
            }
            if end.is_some() {
                return Ok(start_line);
            }
        }
    }

    fn read_object(&mut self) -> Result<JsonObject, AppError> {
        let line = self.capture()?;
        self.count += 1;
        if let Err(e) = serde_json::from_slice::<serde::de::IgnoredAny>(&self.buf) {
            return Err(AppError::InvalidInput(format!(
                "item {} (line {line}) is not valid JSON: {e}",
                self.count
            )));
        }
        Ok(JsonObject {
            line,
            entries: object_entries(&self.buf),
        })
    }

    /// The next object; `None` at the end of the file.
    pub fn next_object(&mut self) -> Result<Option<JsonObject>, AppError> {
        loop {
            match self.state {
                State::Done => return Ok(None),
                State::Start => match self.skip_ws()? {
                    None => {
                        self.state = State::Done;
                        return Ok(None);
                    }
                    Some(b'[') => {
                        self.bump();
                        self.layout = Some(JsonLayout::Array);
                        self.state = State::Array { first: true };
                    }
                    Some(b'{') => {
                        self.layout = Some(JsonLayout::Lines);
                        self.state = State::Lines;
                    }
                    Some(_) => {
                        return Err(AppError::InvalidInput(
                            "the file is not a JSON array of objects, nor one JSON object per line"
                                .into(),
                        ));
                    }
                },
                State::Array { first } => {
                    let mut next = self.skip_ws()?;
                    if !first {
                        match next {
                            Some(b',') => {
                                self.bump();
                                next = self.skip_ws()?;
                            }
                            Some(b']') | None => {}
                            Some(_) => {
                                return Err(AppError::InvalidInput(format!(
                                    "line {}: expected , or ] after item {}",
                                    self.line, self.count
                                )));
                            }
                        }
                    }
                    match next {
                        // `]` (also after a trailing comma): the end, with nothing but white
                        // space after it.
                        Some(b']') => {
                            self.bump();
                            self.state = State::Done;
                            if self.skip_ws()?.is_some() {
                                return Err(AppError::InvalidInput(format!(
                                    "line {}: unexpected text after the closing ]",
                                    self.line
                                )));
                            }
                            return Ok(None);
                        }
                        Some(b'{') => {
                            self.state = State::Array { first: false };
                            return self.read_object().map(Some);
                        }
                        Some(b) => return Err(self.not_an_object(b)),
                        None => {
                            return Err(AppError::InvalidInput(
                                "the file ends before the closing ] of its array".into(),
                            ));
                        }
                    }
                }
                State::Lines => match self.skip_ws()? {
                    None => {
                        self.state = State::Done;
                        return Ok(None);
                    }
                    Some(b'{') => return self.read_object().map(Some),
                    Some(b) => return Err(self.not_an_object(b)),
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// One object's top-level entries (the text is valid JSON: checked before)
// ---------------------------------------------------------------------------------------------

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

/// End (exclusive) of the string starting at `i` (its `"`).
fn string_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// End (exclusive) of the object or array starting at `i`.
fn container_end(b: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        match b[j] {
            b'"' => {
                j = string_end(b, j);
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    b.len()
}

/// End (exclusive) of a number, `true`, `false` or `null` starting at `i`.
fn scalar_end(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
        j += 1;
    }
    j
}

fn minified(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let text = String::from_utf8_lossy(raw);
    if crate::export::writer::write_minified_json(&mut out, &text).is_err() {
        return text.into_owned();
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// The value starting at `i`: its end and its text (`None` for null).
fn value_at(b: &[u8], i: usize) -> (usize, Option<String>) {
    match b.get(i) {
        Some(b'"') => {
            let end = string_end(b, i);
            let text = serde_json::from_slice::<String>(&b[i..end]).unwrap_or_default();
            (end, Some(text))
        }
        Some(b'{' | b'[') => {
            let end = container_end(b, i);
            (end, Some(minified(&b[i..end])))
        }
        Some(_) => {
            let end = scalar_end(b, i);
            let raw = &b[i..end];
            if raw == b"null" {
                (end, None)
            } else {
                (end, Some(String::from_utf8_lossy(raw).into_owned()))
            }
        }
        None => (b.len(), None),
    }
}

/// The top-level keys and values of the object `b` (valid JSON starting with `{`).
pub fn object_entries(b: &[u8]) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut i = 1;
    while i < b.len() {
        i = skip_ws(b, i);
        match b.get(i) {
            Some(b'}') | None => break,
            Some(b',') => {
                i += 1;
                continue;
            }
            Some(b'"') => {}
            Some(_) => break,
        }
        let key_end = string_end(b, i);
        let key = serde_json::from_slice::<String>(&b[i..key_end]).unwrap_or_default();
        i = skip_ws(b, key_end);
        if b.get(i) != Some(&b':') {
            break;
        }
        i = skip_ws(b, i + 1);
        let (end, value) = value_at(b, i);
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => out.push((key, value)),
        }
        i = end;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Objects as records of fixed keys (for the import)
// ---------------------------------------------------------------------------------------------

/// Keys the import met that are not among its columns (the preview never saw them).
#[derive(Debug, Default)]
pub struct IgnoredKeys {
    /// The first few, in the order met.
    pub names: Vec<String>,
    /// More than `names` were met.
    pub more: bool,
}

const IGNORED_SHOWN: usize = 5;

/// The objects of a file as CSV-like records: one field per key of `keys`, quoted (so an
/// empty string stays one); null and missing keys are unquoted empty fields (NULL).
pub struct JsonRecords<R: BufRead> {
    objects: JsonObjects<R>,
    index: HashMap<String, usize>,
    width: usize,
    ignored: Arc<Mutex<IgnoredKeys>>,
}

impl<R: BufRead> JsonRecords<R> {
    pub fn new(r: R, keys: &[String], ignored: Arc<Mutex<IgnoredKeys>>) -> JsonRecords<R> {
        let mut index = HashMap::with_capacity(keys.len());
        for (i, k) in keys.iter().enumerate() {
            index.entry(k.clone()).or_insert(i);
        }
        JsonRecords {
            objects: JsonObjects::new(r),
            index,
            width: keys.len(),
            ignored,
        }
    }

    fn note_ignored(&self, key: &str) {
        let mut ignored = self.ignored.lock().unwrap_or_else(|p| p.into_inner());
        if ignored.names.iter().any(|k| k == key) {
            return;
        }
        if ignored.names.len() < IGNORED_SHOWN {
            ignored.names.push(key.to_string());
        } else {
            ignored.more = true;
        }
    }
}

impl<R: BufRead + Send> RecordRead for JsonRecords<R> {
    fn next_record(&mut self) -> Result<Option<Record>, AppError> {
        let Some(object) = self.objects.next_object()? else {
            return Ok(None);
        };
        let null = Field {
            text: String::new(),
            quoted: false,
        };
        let mut fields = vec![null; self.width];
        for (key, value) in object.entries {
            match self.index.get(&key) {
                Some(&i) => {
                    if let Some(text) = value {
                        fields[i] = Field { text, quoted: true };
                    }
                }
                None => self.note_ignored(&key),
            }
        }
        Ok(Some(Record {
            fields,
            line: object.line,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objects(text: &str) -> Result<Vec<JsonObject>, AppError> {
        let mut r = JsonObjects::new(text.as_bytes());
        let mut out = Vec::new();
        while let Some(o) = r.next_object()? {
            out.push(o);
        }
        Ok(out)
    }

    fn entry(k: &str, v: Option<&str>) -> (String, Option<String>) {
        (k.to_string(), v.map(str::to_string))
    }

    #[test]
    fn reads_arrays_of_objects_with_their_values_as_written() {
        let text = r#"[
  {"id": 1, "name": "Zoë \"Z\"", "price": 12.50, "big": 12345678901234567890,
   "ok": true, "none": null, "tags": ["a", "b c"], "meta": {"k": [1, 2]}, "empty": ""},
  {"id": 2, "name": "brace } in [string", "ok": false}
]"#;
        let mut r = JsonObjects::new(text.as_bytes());
        let first = r.next_object().unwrap().unwrap();
        assert_eq!(r.layout(), Some(JsonLayout::Array));
        assert_eq!(first.line, 2);
        assert_eq!(
            first.entries,
            vec![
                entry("id", Some("1")),
                entry("name", Some("Zoë \"Z\"")),
                entry("price", Some("12.50")),
                entry("big", Some("12345678901234567890")),
                entry("ok", Some("true")),
                entry("none", None),
                entry("tags", Some("[\"a\",\"b c\"]")),
                entry("meta", Some("{\"k\":[1,2]}")),
                entry("empty", Some("")),
            ]
        );
        let second = r.next_object().unwrap().unwrap();
        assert_eq!(second.line, 4);
        assert_eq!(second.entries[1], entry("name", Some("brace } in [string")));
        assert!(r.next_object().unwrap().is_none());
        assert!(r.next_object().unwrap().is_none());
    }

    #[test]
    fn reads_ndjson_and_concatenated_objects() {
        let text = "{\"a\":1}\n{\"a\":2,\"b\":\"x\"}\r\n\n  {\"a\":3} {\"a\":4}\n";
        let all = objects(text).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(
            all[1].entries,
            vec![entry("a", Some("2")), entry("b", Some("x"))]
        );
        assert_eq!(
            all.iter().map(|o| o.line).collect::<Vec<_>>(),
            vec![1, 2, 4, 4]
        );
        let mut r = JsonObjects::new(text.as_bytes());
        r.next_object().unwrap();
        assert_eq!(r.layout(), Some(JsonLayout::Lines));
        // Empty files and empty arrays hold no object.
        assert!(objects("").unwrap().is_empty());
        assert!(objects(" [ ] \n").unwrap().is_empty());
        // A trailing comma is forgiven.
        assert_eq!(objects("[{\"a\":1},]").unwrap().len(), 1);
    }

    #[test]
    fn repeated_keys_keep_their_last_value_and_escapes_are_decoded() {
        let all = objects(r#"[{"a": 1, "aé": "é\n", "a": 2}]"#).unwrap();
        assert_eq!(
            all[0].entries,
            vec![entry("a", Some("2")), entry("aé", Some("é\n"))]
        );
    }

    #[test]
    fn refuses_what_is_not_objects() {
        let err = |text: &str| objects(text).unwrap_err().to_string();
        assert!(
            err("[1, 2]").contains("is a number, not an object"),
            "{}",
            err("[1, 2]")
        );
        assert!(err("[{\"a\":1}, [2]]").contains("item 2"));
        assert!(err("\"text\"").contains("not a JSON array"));
        assert!(err("[{\"a\": 1} {\"b\": 2}]").contains("expected , or ]"));
        assert!(err("[{\"a\": 1}").contains("before the closing ]"));
        assert!(err("[{\"a\": 1}] x").contains("after the closing ]"));
        assert!(err("{\"a\": ").contains("ends inside item 1"));
        let bad = err("{\"a\": 1}\n{\"a\": tru}\n");
        assert!(bad.contains("item 2 (line 2) is not valid JSON"), "{bad}");
    }

    #[test]
    fn records_have_one_field_per_key() {
        let text = "{\"id\": 1, \"note\": \"\", \"x\": 9}\n{\"note\": null, \"y\": 1, \"z\": 2}\n";
        let keys = vec!["id".to_string(), "note".to_string()];
        let ignored = Arc::new(Mutex::new(IgnoredKeys::default()));
        let mut r = JsonRecords::new(text.as_bytes(), &keys, ignored.clone());
        let first = r.next_record().unwrap().unwrap();
        assert_eq!(
            first.fields,
            vec![
                Field {
                    text: "1".into(),
                    quoted: true
                },
                Field {
                    text: String::new(),
                    quoted: true
                },
            ]
        );
        let second = r.next_record().unwrap().unwrap();
        assert_eq!(second.line, 2);
        assert!(second.fields.iter().all(|f| f.text.is_empty() && !f.quoted));
        assert!(r.next_record().unwrap().is_none());
        assert_eq!(ignored.lock().unwrap().names, vec!["x", "y", "z"]);
    }
}
