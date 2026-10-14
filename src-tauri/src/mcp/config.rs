//! One-click install into Cursor (`~/.cursor/mcp.json`, section `mcpServers`) and VS Code (the
//! user `mcp.json`, section `servers`): Tabler's server entry is added to, or replaced in, the
//! file's text. Everything else stays as it was: other servers, key order, indentation, comments
//! and trailing commas (VS Code reads its config as JSON with comments). The edited text is
//! parsed again and must equal the old document plus the entry, or nothing is written.

use serde_json::{Map, Value as Json};

/// Nesting deeper than this is refused (a config file never needs it).
const MAX_DEPTH: usize = 256;

/// The result of a merge: the new text, and whether it differs from the old one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    pub text: String,
    pub changed: bool,
}

/// `entry` as a JSON object (its fields in order).
pub fn entry_json(entry: &[(&str, Json)]) -> Json {
    Json::Object(
        entry
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<Map<String, Json>>(),
    )
}

/// The text with comments blanked out and trailing commas removed: plain JSON.
pub fn strip_jsonc(text: &str) -> Result<String, String> {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let mut in_string = false;
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    if b[i] != b'\r' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..]
                    .find("*/")
                    .map(|p| i + 2 + p + 2)
                    .ok_or("a /* comment is not closed")?;
                for (j, byte) in out.iter_mut().enumerate().take(end).skip(i) {
                    if !matches!(b[j], b'\n' | b'\r') {
                        *byte = b' ';
                    }
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    // Trailing commas, now that no comment can sit between them and the bracket.
    let mut in_string = false;
    let mut i = 0;
    while i < out.len() {
        let c = out[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
        } else if c == b'"' {
            in_string = true;
        } else if c == b',' {
            let next = out[i + 1..].iter().find(|x| !x.is_ascii_whitespace());
            if matches!(next, Some(b'}') | Some(b']')) {
                out[i] = b' ';
            }
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

/// Parses JSON with comments and trailing commas.
pub fn parse_jsonc(text: &str) -> Result<Json, String> {
    let plain = strip_jsonc(text)?;
    serde_json::from_str(&plain).map_err(|e| format!("it is not valid JSON ({e})"))
}

#[derive(Debug)]
struct Member {
    key: String,
    key_start: usize,
    value: (usize, usize),
    /// Position of the comma after the value.
    comma: Option<usize>,
}

#[derive(Debug)]
struct Obj {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// Walks JSON-with-comments text, recording where objects, keys and values are.
struct Scanner<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl<'a> Scanner<'a> {
    fn new(s: &'a str, at: usize) -> Scanner<'a> {
        Scanner {
            s,
            b: s.as_bytes(),
            i: at,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn trivia(&mut self) -> Result<(), String> {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with("//") {
                self.i = self.s[self.i..]
                    .find('\n')
                    .map_or(self.b.len(), |p| self.i + p);
            } else if self.s[self.i..].starts_with("/*") {
                let end = self.s[self.i + 2..]
                    .find("*/")
                    .ok_or("a /* comment is not closed")?;
                self.i += 2 + end + 2;
            } else {
                return Ok(());
            }
        }
    }

    fn unexpected(&self, wanted: &str) -> String {
        let line = self.s[..self.i.min(self.s.len())].matches('\n').count() + 1;
        match self.peek() {
            Some(_) => format!("expected {wanted} on line {line}"),
            None => format!("expected {wanted}, but the file ends"),
        }
    }

    fn string(&mut self) -> Result<(usize, usize), String> {
        let start = self.i;
        self.i += 1;
        while let Some(c) = self.peek() {
            match c {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return Ok((start, self.i));
                }
                _ => self.i += 1,
            }
        }
        Err("a string is not closed".into())
    }

    fn value(&mut self) -> Result<(usize, usize), String> {
        self.trivia()?;
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("the file is nested too deeply".into());
        }
        let span = match self.peek() {
            Some(b'{') => {
                let o = self.object()?;
                Ok((o.open, o.close + 1))
            }
            Some(b'[') => self.array(),
            Some(b'"') => self.string(),
            Some(_) => {
                let start = self.i;
                while self
                    .peek()
                    .is_some_and(|c| !c.is_ascii_whitespace() && !b",:{}[]/\"".contains(&c))
                {
                    self.i += 1;
                }
                if self.i == start {
                    Err(self.unexpected("a value"))
                } else {
                    Ok((start, self.i))
                }
            }
            None => Err(self.unexpected("a value")),
        };
        self.depth -= 1;
        span
    }

    fn array(&mut self) -> Result<(usize, usize), String> {
        let start = self.i;
        self.i += 1;
        loop {
            self.trivia()?;
            if self.peek() == Some(b']') {
                self.i += 1;
                return Ok((start, self.i));
            }
            self.value()?;
            self.trivia()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {}
                _ => return Err(self.unexpected("',' or ']'")),
            }
        }
    }

    /// At `{`: the object's members (keys decoded) and where it opens and closes.
    fn object(&mut self) -> Result<Obj, String> {
        let open = self.i;
        self.i += 1;
        let mut members = Vec::new();
        loop {
            self.trivia()?;
            match self.peek() {
                Some(b'}') => {
                    let close = self.i;
                    self.i += 1;
                    return Ok(Obj {
                        open,
                        close,
                        members,
                    });
                }
                Some(b'"') => {}
                _ => return Err(self.unexpected("a key in quotes or '}'")),
            }
            let (ks, ke) = self.string()?;
            let key: String = serde_json::from_str(&self.s[ks..ke]).map_err(|e| e.to_string())?;
            self.trivia()?;
            if self.peek() != Some(b':') {
                return Err(self.unexpected("':'"));
            }
            self.i += 1;
            let value = self.value()?;
            self.trivia()?;
            let comma = match self.peek() {
                Some(b',') => {
                    self.i += 1;
                    Some(self.i - 1)
                }
                Some(b'}') => None,
                _ => return Err(self.unexpected("',' or '}'")),
            };
            members.push(Member {
                key,
                key_start: ks,
                value,
                comma,
            });
        }
    }
}

/// The whitespace in front of `pos` on its line, or `None` when something else comes first.
fn line_indent(s: &str, pos: usize) -> Option<&str> {
    let start = s[..pos].rfind('\n').map_or(0, |p| p + 1);
    let indent = &s[start..pos];
    indent
        .bytes()
        .all(|c| c == b' ' || c == b'\t')
        .then_some(indent)
}

fn quoted(s: &str) -> String {
    Json::String(s.to_string()).to_string()
}

fn render_entry(entry: &[(&str, Json)], base: &str, unit: &str, nl: &str) -> String {
    let mut out = String::from("{");
    for (i, (k, v)) in entry.iter().enumerate() {
        out.push_str(nl);
        out.push_str(base);
        out.push_str(unit);
        out.push_str(&quoted(k));
        out.push_str(": ");
        out.push_str(&v.to_string());
        if i + 1 < entry.len() {
            out.push(',');
        }
    }
    out.push_str(nl);
    out.push_str(base);
    out.push('}');
    out
}

fn render_compact(entry: &[(&str, Json)]) -> String {
    let fields: Vec<String> = entry
        .iter()
        .map(|(k, v)| format!("{}: {v}", quoted(k)))
        .collect();
    format!("{{{}}}", fields.join(", "))
}

fn splice(s: &str, from: usize, to: usize, with: &str) -> String {
    format!("{}{with}{}", &s[..from], &s[to..])
}

struct Style<'a> {
    unit: &'a str,
    nl: &'a str,
}

/// Adds `key: value` as the last member of `obj`. `value(indent)` renders the value for a member
/// line indented by `indent`; `compact` is used inside one-line objects.
fn insert_member(
    s: &str,
    obj: &Obj,
    container_indent: Option<&str>,
    style: &Style<'_>,
    key: &str,
    value: impl Fn(&str) -> String,
    compact: impl Fn() -> String,
) -> String {
    let (unit, nl) = (style.unit, style.nl);
    let Some(last) = obj.members.last() else {
        let inner = &s[obj.open + 1..obj.close];
        return match container_indent {
            Some(ci) => {
                let mi = format!("{ci}{unit}");
                let member = format!("{nl}{mi}{key}: {}", value(&mi));
                if inner.trim().is_empty() {
                    splice(s, obj.open + 1, obj.close, &format!("{member}{nl}{ci}"))
                } else {
                    // Only comments inside: they stay, after the new member.
                    splice(s, obj.open + 1, obj.open + 1, &member)
                }
            }
            None => splice(
                s,
                obj.open + 1,
                obj.open + 1,
                &format!("{key}: {}", compact()),
            ),
        };
    };
    let member = match line_indent(s, last.key_start) {
        Some(mi) => format!("{nl}{mi}{key}: {}", value(mi)),
        None => format!(" {key}: {}", compact()),
    };
    match last.comma {
        // A trailing comma (JSON with comments): the new member goes after it.
        Some(c) => splice(s, c + 1, c + 1, &member),
        None => splice(s, last.value.1, last.value.1, &format!(",{member}")),
    }
}

/// The file's indentation unit, from the first indented member of the root object.
fn indent_unit(s: &str, root: &Obj) -> String {
    root.members
        .iter()
        .filter_map(|m| line_indent(s, m.key_start))
        .find(|i| !i.is_empty())
        .map(|i| {
            if i.contains('\t') {
                "\t".to_string()
            } else {
                i.to_string()
            }
        })
        .unwrap_or_else(|| "  ".to_string())
}

/// Adds the server `name` with `entry` to the `section` object of a config file (`text` empty:
/// a new file), or replaces an existing entry of that name.
pub fn merge_server(
    text: &str,
    section: &str,
    name: &str,
    entry: &[(&str, Json)],
) -> Result<Merge, String> {
    let wanted = entry_json(entry);
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    if body.trim().is_empty() {
        let unit = "  ";
        let server = render_entry(entry, &unit.repeat(2), unit, nl);
        let new = format!(
            "{{{nl}{unit}{}: {{{nl}{unit}{unit}{}: {server}{nl}{unit}}}{nl}}}{nl}",
            quoted(section),
            quoted(name)
        );
        return Ok(Merge {
            text: new,
            changed: true,
        });
    }
    let before = parse_jsonc(body)?;
    let Json::Object(root_map) = &before else {
        return Err("it does not hold a JSON object".into());
    };
    if let Some(existing) = root_map.get(section) {
        if !existing.is_object() {
            return Err(format!("its \u{201c}{section}\u{201d} is not an object"));
        }
        if existing.get(name) == Some(&wanted) {
            return Ok(Merge {
                text: text.to_string(),
                changed: false,
            });
        }
    }

    let mut scan = Scanner::new(body, 0);
    scan.trivia()?;
    if scan.peek() != Some(b'{') {
        return Err("it does not hold a JSON object".into());
    }
    let root = scan.object()?;
    scan.trivia()?;
    if scan.i < body.len() {
        return Err(scan.unexpected("the end of the file"));
    }
    let unit = indent_unit(body, &root);
    let style = Style { unit: &unit, nl };
    let server_key = quoted(name);
    let edited = match root.members.iter().rev().find(|m| m.key == section) {
        Some(sec) => {
            let obj = Scanner::new(body, sec.value.0).object()?;
            match obj.members.iter().rev().find(|m| m.key == name) {
                Some(m) => {
                    let rendered = match line_indent(body, m.key_start) {
                        Some(ind) => render_entry(entry, ind, &unit, nl),
                        None => render_compact(entry),
                    };
                    splice(body, m.value.0, m.value.1, &rendered)
                }
                None => insert_member(
                    body,
                    &obj,
                    line_indent(body, sec.key_start),
                    &style,
                    &server_key,
                    |mi| render_entry(entry, mi, &unit, nl),
                    || render_compact(entry),
                ),
            }
        }
        None => insert_member(
            body,
            &root,
            line_indent(body, root.open),
            &style,
            &quoted(section),
            |mi| {
                let inner = format!("{mi}{unit}");
                format!(
                    "{{{nl}{inner}{server_key}: {}{nl}{mi}}}",
                    render_entry(entry, &inner, &unit, nl)
                )
            },
            || format!("{{{server_key}: {}}}", render_compact(entry)),
        ),
    };

    // Safety net: the new text must be the old document plus (exactly) the entry.
    let mut expected = before.clone();
    expected[section][name] = wanted;
    let after =
        parse_jsonc(&edited).map_err(|e| format!("Tabler could not add its entry safely: {e}"))?;
    if after != expected {
        return Err("Tabler could not add its entry without changing other settings".into());
    }
    Ok(Merge {
        text: format!("{bom}{edited}"),
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> Vec<(&'static str, Json)> {
        vec![
            (
                "command",
                json!("/Applications/Tabler.app/Contents/MacOS/tabler"),
            ),
            ("args", json!(["--mcp"])),
        ]
    }

    fn merge(text: &str) -> Merge {
        merge_server(text, "mcpServers", "tabler", &entry()).unwrap()
    }

    #[test]
    fn creates_a_new_file() {
        for text in ["", "  \n", "\u{feff}"] {
            let m = merge(text);
            assert!(m.changed);
            assert_eq!(
                m.text,
                "{\n  \"mcpServers\": {\n    \"tabler\": {\n      \"command\": \
                 \"/Applications/Tabler.app/Contents/MacOS/tabler\",\n      \"args\": \
                 [\"--mcp\"]\n    }\n  }\n}\n"
            );
            assert_eq!(
                parse_jsonc(&m.text).unwrap()["mcpServers"]["tabler"],
                entry_json(&entry())
            );
        }
    }

    #[test]
    fn adds_the_section_to_an_object() {
        assert_eq!(
            merge("{}").text,
            "{\n  \"mcpServers\": {\n    \"tabler\": {\n      \"command\": \
             \"/Applications/Tabler.app/Contents/MacOS/tabler\",\n      \"args\": [\"--mcp\"]\n    \
             }\n  }\n}"
        );
        let m = merge("{\n    \"theme\": \"dark\"\n}\n");
        assert_eq!(
            m.text,
            "{\n    \"theme\": \"dark\",\n    \"mcpServers\": {\n        \"tabler\": {\n            \
             \"command\": \"/Applications/Tabler.app/Contents/MacOS/tabler\",\n            \"args\": \
             [\"--mcp\"]\n        }\n    }\n}\n"
        );
    }

    #[test]
    fn appends_next_to_other_servers_keeping_their_text() {
        let text = "{\n  \"mcpServers\": {\n    \"zeta\": { \"command\": \"z\" },\n    \"alpha\": {\n      \
                    \"url\": \"https://example.com/mcp\"\n    }\n  },\n  \"other\": 1\n}\n";
        let m = merge(text);
        assert!(m.changed);
        assert!(m.text.starts_with(
            "{\n  \"mcpServers\": {\n    \"zeta\": { \"command\": \"z\" },\n    \"alpha\": {\n      \
             \"url\": \"https://example.com/mcp\"\n    },\n    \"tabler\": {\n      \"command\""
        ));
        assert!(
            m.text
                .ends_with("\"args\": [\"--mcp\"]\n    }\n  },\n  \"other\": 1\n}\n")
        );
        let doc = parse_jsonc(&m.text).unwrap();
        assert_eq!(doc["mcpServers"]["zeta"], json!({"command": "z"}));
        assert_eq!(doc["other"], json!(1));
    }

    #[test]
    fn replaces_an_old_entry_and_leaves_an_identical_one_alone() {
        let text = "{\"mcpServers\": {\n  \"tabler\": {\n    \"command\": \"/old/tabler\",\n    \"args\": [\"--mcp\"]\n  },\n  \"b\": {}\n}}";
        let m = merge(text);
        assert!(m.changed);
        assert!(
            m.text
                .contains("\"command\": \"/Applications/Tabler.app/Contents/MacOS/tabler\"")
        );
        assert!(!m.text.contains("/old/tabler"));
        assert!(m.text.contains("\"b\": {}"));
        let again = merge(&m.text);
        assert!(!again.changed);
        assert_eq!(again.text, m.text);
        // Field order does not matter for "already there".
        let reordered = "{\"mcpServers\": {\"tabler\": {\"args\": [\"--mcp\"], \"command\": \
                         \"/Applications/Tabler.app/Contents/MacOS/tabler\"}}}";
        assert!(!merge(reordered).changed);
    }

    #[test]
    fn keeps_comments_and_trailing_commas() {
        let text = "// VS Code user MCP servers\n{\n\t\"servers\": {\n\t\t// my server\n\t\t\"mine\": {\n\t\t\t\"type\": \"stdio\", /* inline */\n\t\t\t\"command\": \"x\",\n\t\t},\n\t},\n\t\"inputs\": [],\n}\n";
        let vs = vec![
            ("type", json!("stdio")),
            ("command", json!("/T/tabler")),
            ("args", json!(["--mcp"])),
        ];
        let m = merge_server(text, "servers", "tabler", &vs).unwrap();
        assert!(m.changed);
        assert!(m.text.contains("// VS Code user MCP servers"));
        assert!(m.text.contains("// my server"));
        assert!(m.text.contains("/* inline */"));
        assert!(m.text.contains(
            "\t\t},\n\t\t\"tabler\": {\n\t\t\t\"type\": \"stdio\",\n\t\t\t\"command\": \"/T/tabler\",\n\t\t\t\"args\": [\"--mcp\"]\n\t\t}\n\t},"
        ));
        let doc = parse_jsonc(&m.text).unwrap();
        assert_eq!(doc["servers"]["mine"]["command"], json!("x"));
        assert_eq!(doc["inputs"], json!([]));
    }

    #[test]
    fn empty_sections_and_comment_only_objects() {
        let m = merge("{\n  \"mcpServers\": {}\n}");
        assert_eq!(
            m.text,
            "{\n  \"mcpServers\": {\n    \"tabler\": {\n      \"command\": \
             \"/Applications/Tabler.app/Contents/MacOS/tabler\",\n      \"args\": [\"--mcp\"]\n    \
             }\n  }\n}"
        );
        let m = merge("{\n  \"mcpServers\": {\n    // add servers here\n  }\n}");
        assert!(m.text.contains("// add servers here"));
        assert!(parse_jsonc(&m.text).unwrap()["mcpServers"]["tabler"].is_object());
    }

    #[test]
    fn one_line_files_crlf_and_bom() {
        let m = merge("{\"mcpServers\": {\"a\": {\"command\": \"a\"}}}");
        assert_eq!(
            m.text,
            "{\"mcpServers\": {\"a\": {\"command\": \"a\"}, \"tabler\": {\"command\": \
             \"/Applications/Tabler.app/Contents/MacOS/tabler\", \"args\": [\"--mcp\"]}}}"
        );
        let m = merge("{\"x\": true}");
        assert!(parse_jsonc(&m.text).unwrap()["mcpServers"]["tabler"].is_object());

        let m = merge("\u{feff}{\r\n  \"x\": 1\r\n}\r\n");
        assert!(m.text.starts_with('\u{feff}'));
        assert!(!m.text.replace("\r\n", "").contains('\n'), "{:?}", m.text);
        assert!(parse_jsonc(m.text.trim_start_matches('\u{feff}')).unwrap()["x"] == json!(1));
    }

    #[test]
    fn refuses_what_it_cannot_merge_safely() {
        for (text, msg) in [
            ("[]", "JSON object"),
            ("{\"mcpServers\": []}", "not an object"),
            ("{\"mcpServers\": \"x\"}", "not an object"),
            ("{\"a\": }", "valid JSON"),
            ("{\"a\": 1} /* open", "not closed"),
            ("{\"a\": 1} {}", "valid JSON"),
            ("{\"a\": \"x}", "valid JSON"),
        ] {
            let err = merge_server(text, "mcpServers", "tabler", &entry()).unwrap_err();
            assert!(err.contains(msg), "{text}: {err}");
        }
    }

    #[test]
    fn strings_that_look_like_comments_are_kept() {
        let text = "{\"note\": \"http://x // not a comment, /* nor this */\", \"mcpServers\": {}}";
        let m = merge(text);
        assert_eq!(
            parse_jsonc(&m.text).unwrap()["note"],
            json!("http://x // not a comment, /* nor this */")
        );
    }

    #[test]
    fn duplicate_section_keys_use_the_last_one() {
        let m = merge("{\"mcpServers\": {\"a\": {}}, \"mcpServers\": {\"b\": {}}}");
        let doc = parse_jsonc(&m.text).unwrap();
        assert!(doc["mcpServers"]["b"].is_object());
        assert!(doc["mcpServers"]["tabler"].is_object());
    }
}
