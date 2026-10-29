//! The console's requests, Kibana Dev Tools style: a `METHOD /path?query` line, then an optional
//! JSON body (one value, or one value per line for `_bulk` and `_msearch`), several requests per
//! tab. `#`, `//` and `/* */` comments and `"""…"""` strings are allowed as in Kibana.
//!
//! A request is parsed once into an [`EsRequest`]: the classifier reads it and the client sends
//! it, so what is judged is what goes out. The path is kept as decoded segments and written back
//! with every unsafe character percent-encoded: `.` and `..` segments (also as `%2e`) are
//! refused, since a URL parser would fold them into another endpoint, and a path can never name
//! another host.

use crate::error::AppError;
use crate::sqlgen::split::SplitStatement;

use super::json::{self, Json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
}

impl Method {
    pub fn parse(word: &str) -> Option<Method> {
        Some(match word.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "OPTIONS" => Method::Options,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
        }
    }
}

/// A request body as it will be sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// One JSON value (`application/json`).
    Json(Json),
    /// Newline-delimited JSON (`application/x-ndjson`): `_bulk`, `_msearch`.
    NdJson(Vec<Json>),
}

impl Body {
    /// Every JSON value of the body.
    pub fn values(&self) -> &[Json] {
        match self {
            Body::Json(v) => std::slice::from_ref(v),
            Body::NdJson(vs) => vs,
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            Body::Json(_) => "application/json",
            Body::NdJson(_) => "application/x-ndjson",
        }
    }

    /// The bytes on the wire (NDJSON ends with a line break, as `_bulk` requires).
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Body::Json(v) => v.to_text().into_bytes(),
            Body::NdJson(vs) => {
                let mut out = String::new();
                for v in vs {
                    out.push_str(&v.to_text());
                    out.push('\n');
                }
                out.into_bytes()
            }
        }
    }
}

/// One request: method, decoded path segments and query pairs, body.
/// Decoded query parameters in order; `None` for a bare flag (`?pretty`).
pub type QueryPairs = Vec<(String, Option<String>)>;

#[derive(Debug, Clone, PartialEq)]
pub struct EsRequest {
    pub method: Method,
    /// Decoded, non-empty, never `.` or `..`.
    pub segments: Vec<String>,
    pub query: QueryPairs,
    pub body: Option<Body>,
}

impl EsRequest {
    pub fn new(method: Method, segments: &[&str]) -> EsRequest {
        EsRequest {
            method,
            segments: segments.iter().map(|s| s.to_string()).collect(),
            query: Vec::new(),
            body: None,
        }
    }

    pub fn param(mut self, name: &str, value: impl Into<String>) -> EsRequest {
        self.query.push((name.to_string(), Some(value.into())));
        self
    }

    pub fn json(mut self, body: Json) -> EsRequest {
        self.body = Some(Body::Json(body));
        self
    }

    pub fn ndjson(mut self, lines: Vec<Json>) -> EsRequest {
        self.body = Some(Body::NdJson(lines));
        self
    }

    /// The value of a query parameter (the last one, as Elasticsearch reads them); a bare flag
    /// reads as `""`.
    pub fn query_value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_deref().unwrap_or(""))
    }

    /// The encoded path, from `/`.
    pub fn path(&self) -> String {
        let mut out = String::new();
        for s in &self.segments {
            out.push('/');
            encode_into(&mut out, s, PATH_KEEP);
        }
        if out.is_empty() {
            out.push('/');
        }
        out
    }

    /// The encoded query string without `?`, empty when there is none.
    pub fn query_string(&self) -> String {
        let mut out = String::new();
        for (i, (k, v)) in self.query.iter().enumerate() {
            if i > 0 {
                out.push('&');
            }
            encode_into(&mut out, k, QUERY_KEEP);
            if let Some(v) = v {
                out.push('=');
                encode_into(&mut out, v, QUERY_KEEP);
            }
        }
        out
    }

    /// Path and query, as appended to the cluster's base URL.
    pub fn target(&self) -> String {
        let q = self.query_string();
        if q.is_empty() {
            self.path()
        } else {
            format!("{}?{q}", self.path())
        }
    }

    /// The request as console text (what previews show and the console runs back the same).
    pub fn to_console_text(&self) -> String {
        let mut out = format!("{} {}", self.method.as_str(), self.target());
        if let Some(body) = &self.body {
            for v in body.values() {
                out.push('\n');
                out.push_str(&v.to_text());
            }
        }
        out
    }

    /// The last path segment, if any.
    pub fn last(&self) -> Option<&str> {
        self.segments.last().map(String::as_str)
    }
}

/// Characters a path segment keeps as typed: RFC 3986 unreserved plus `*` and `,` (index
/// patterns and lists) and `:` (a remote cluster's index), as Kibana sends them.
const PATH_KEEP: &[u8] = b"-._~*,:";
/// Characters a query key or value keeps: unreserved plus `*` and `,`.
const QUERY_KEEP: &[u8] = b"-._~*,";

fn encode_into(out: &mut String, s: &str, keep: &[u8]) {
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || keep.contains(&b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from(b"0123456789ABCDEF"[usize::from(b >> 4)]));
            out.push(char::from(b"0123456789ABCDEF"[usize::from(b & 15)]));
        }
    }
}

/// Percent-decodes `s` (no `+` for space, as in a path). Malformed escapes and text that is not
/// UTF-8 once decoded are errors.
pub fn percent_decode(s: &str) -> Result<String, AppError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| {
                    AppError::InvalidInput(format!(
                        "malformed percent escape in \u{201c}{s}\u{201d}"
                    ))
                })?;
            out.push(hex);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| {
        AppError::InvalidInput(format!("\u{201c}{s}\u{201d} does not decode to UTF-8 text"))
    })
}

/// Splits a typed path (`/idx/_search?q=x`, `_cat/indices`) into decoded segments and query.
pub fn parse_target(target: &str) -> Result<(Vec<String>, QueryPairs), AppError> {
    let bad = |why: &str| AppError::InvalidInput(format!("path \u{201c}{target}\u{201d}: {why}"));
    if target.contains("://") {
        return Err(bad(
            "give the path only; the console sends every request to this connection",
        ));
    }
    if target
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return Err(bad(
            "spaces, control characters and backslashes are not allowed",
        ));
    }
    if target.contains('#') {
        return Err(bad("`#` must be written %23"));
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let mut segments = Vec::new();
    for raw in path.split('/').filter(|s| !s.is_empty()) {
        let seg = percent_decode(raw)?;
        if seg == "." || seg == ".." {
            return Err(bad("`.` and `..` segments are not allowed"));
        }
        segments.push(seg);
    }
    let mut pairs = Vec::new();
    if let Some(q) = query {
        for part in q.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = match part.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (part, None),
            };
            // `+` is a space in a query string.
            let decode = |s: &str| percent_decode(&s.replace('+', "%20"));
            pairs.push((decode(k)?, v.map(decode).transpose()?));
        }
    }
    Ok((segments, pairs))
}

/// Whether a request with these path segments takes newline-delimited JSON.
pub fn takes_ndjson(segments: &[String]) -> bool {
    segments
        .iter()
        .any(|s| matches!(s.as_str(), "_bulk" | "_msearch" | "_fleet_msearch"))
        || segments
            .windows(2)
            .any(|w| w[0] == "_monitoring" && w[1] == "bulk")
}

/// Parses one console request (a statement of [`split`]).
pub fn parse_request(text: &str) -> Result<EsRequest, AppError> {
    let mut lines = text.split_inclusive('\n');
    let mut consumed = 0;
    let request_line = loop {
        let Some(line) = lines.next() else {
            return Err(AppError::InvalidInput(
                "expected a request such as `GET /_search`".into(),
            ));
        };
        consumed += line.len();
        let t = line.trim();
        if t.is_empty() || is_comment_line(t) {
            continue;
        }
        break t;
    };
    let mut words = request_line.split_whitespace();
    let method_word = words.next().unwrap_or_default();
    let method = Method::parse(method_word).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "\u{201c}{method_word}\u{201d} is not a request method (GET, POST, PUT, DELETE, HEAD)"
        ))
    })?;
    let target = words
        .next()
        .ok_or_else(|| AppError::InvalidInput(format!("{} needs a path", method.as_str())))?;
    if let Some(extra) = words.next() {
        return Err(AppError::InvalidInput(format!(
            "unexpected \u{201c}{extra}\u{201d} after the path (the body goes on the next line)"
        )));
    }
    let (segments, query) = parse_target(target)?;
    let body_text = &text[consumed..];
    let values = json::parse_console(body_text).map_err(|e| {
        let (line, col) = e.line_col(body_text);
        AppError::InvalidInput(format!(
            "request body: {} (line {}, column {col})",
            e.message,
            line + text[..consumed].matches('\n').count()
        ))
    })?;
    let body = if values.is_empty() {
        None
    } else if takes_ndjson(&segments) {
        Some(Body::NdJson(values))
    } else if values.len() == 1 {
        values.into_iter().next().map(Body::Json)
    } else {
        return Err(AppError::InvalidInput(format!(
            "{} takes one JSON body, found {}",
            method.as_str(),
            values.len()
        )));
    };
    Ok(EsRequest {
        method,
        segments,
        query,
        body,
    })
}

fn is_comment_line(t: &str) -> bool {
    t.starts_with('#') || t.starts_with("//")
}

/// Whether a line (trimmed at the start) opens a request: a method word, then a space or the end.
fn is_request_line(t: &str) -> bool {
    let word: String = t.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let rest = &t[word.len()..];
    Method::parse(&word).is_some() && (rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// Splits console text into requests. A request starts at a line whose first word is a method,
/// outside strings and comments; its body runs to the next such line. Text before the first
/// request that is not a comment is a statement of its own, which then fails to parse.
pub fn split(text: &str) -> Vec<SplitStatement> {
    let chars: Vec<char> = text.chars().collect();
    let mut starts: Vec<usize> = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    // Lexical state of the body text, so a method word inside a """ string or a comment does
    // not start a request.
    let mut in_triple = false;
    let mut in_block = false;
    while i < chars.len() {
        let c = chars[i];
        if line_start && !in_triple && !in_block {
            let mut j = i;
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                j += 1;
            }
            let line_end = chars[j..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |n| j + n);
            let line: String = chars[j..line_end].iter().collect();
            if is_request_line(&line) {
                starts.push(j);
            }
        }
        line_start = false;
        if in_triple {
            if chars[i..].starts_with(&['"', '"', '"']) {
                in_triple = false;
                i += 3;
                continue;
            }
        } else if in_block {
            if chars[i..].starts_with(&['*', '/']) {
                in_block = false;
                i += 2;
                continue;
            }
        } else if chars[i..].starts_with(&['"', '"', '"']) {
            in_triple = true;
            i += 3;
            continue;
        } else if c == '"' {
            // A plain string ends at its quote or, unterminated, at the line's end.
            i += 1;
            while i < chars.len() && chars[i] != '"' && chars[i] != '\n' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            if i < chars.len() && chars[i] == '"' {
                i += 1;
            }
            continue;
        } else if c == '#' || (c == '/' && chars.get(i + 1) == Some(&'/')) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            in_block = true;
            i += 2;
            continue;
        }
        if c == '\n' {
            line_start = true;
        }
        i += 1;
    }

    let mut out = Vec::new();
    // Leading text that is not blank or comments.
    let first = starts.first().copied().unwrap_or(chars.len());
    let lead: String = chars[..first].iter().collect();
    if lead.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && !is_comment_line(t) && !t.starts_with("/*")
    }) {
        push_statement(&mut out, &chars, 0, first);
    }
    for (n, &s) in starts.iter().enumerate() {
        let e = starts.get(n + 1).copied().unwrap_or(chars.len());
        push_statement(&mut out, &chars, s, e);
    }
    out
}

fn push_statement(out: &mut Vec<SplitStatement>, chars: &[char], start: usize, end: usize) {
    let mut s = start;
    while s < end && chars[s].is_whitespace() {
        s += 1;
    }
    let mut e = end;
    while e > s && chars[e - 1].is_whitespace() {
        e -= 1;
    }
    if s >= e {
        return;
    }
    let line = chars[..s].iter().filter(|&&c| c == '\n').count() as u32 + 1;
    out.push(SplitStatement {
        sql: chars[s..e].iter().collect(),
        start: s,
        end: e,
        line,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(text: &str) -> EsRequest {
        parse_request(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn request_lines_and_bodies() {
        let r = req("GET /logs-*/_search?size=5&q=user:kimchy\n{\"query\":{\"match_all\":{}}}");
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.segments, ["logs-*", "_search"]);
        assert_eq!(r.query_value("size"), Some("5"));
        assert_eq!(r.query_value("q"), Some("user:kimchy"));
        assert_eq!(r.target(), "/logs-*/_search?size=5&q=user%3Akimchy");
        assert!(matches!(r.body, Some(Body::Json(_))));
        // No leading slash, lower-case method, a bare flag.
        let r = req("get _cat/indices?v");
        assert_eq!(r.segments, ["_cat", "indices"]);
        assert_eq!(r.query_value("v"), Some(""));
        assert_eq!(r.target(), "/_cat/indices?v");
        assert_eq!(req("GET /").target(), "/");
    }

    #[test]
    fn bulk_bodies_are_ndjson() {
        let r = req("POST _bulk\n{\"index\":{\"_index\":\"a\"}}\n{\"f\":1}\n");
        let Some(Body::NdJson(lines)) = &r.body else {
            panic!("{r:?}")
        };
        assert_eq!(lines.len(), 2);
        assert_eq!(
            String::from_utf8(r.body.as_ref().unwrap().to_bytes()).unwrap(),
            "{\"index\":{\"_index\":\"a\"}}\n{\"f\":1}\n"
        );
        assert!(parse_request("POST /a/_search\n{}\n{}").is_err());
    }

    #[test]
    fn paths_are_decoded_and_reencoded() {
        let r = req("GET /my%20index/_doc/a%2Fb");
        assert_eq!(r.segments, ["my index", "_doc", "a/b"]);
        assert_eq!(r.path(), "/my%20index/_doc/a%2Fb");
        let r = req("GET /%3Clogs-%7Bnow%2Fd%7D%3E/_search");
        assert_eq!(r.segments[0], "<logs-{now/d}>");
        assert_eq!(r.path(), "/%3Clogs-%7Bnow%2Fd%7D%3E/_search");
        let r = req("GET /caf\u{e9}/_search");
        assert_eq!(r.path(), "/caf%C3%A9/_search");
        // `#` in a query value is encoded, never a fragment.
        let r = req("GET /a/_search?q=a%23b");
        assert_eq!(r.target(), "/a/_search?q=a%23b");
    }

    #[test]
    fn dangerous_paths_are_refused() {
        for bad in [
            "GET /a/../_bulk",
            "GET /a/%2e%2e/_bulk",
            "GET /a/%2E/x",
            "GET ./x",
            "GET http://other:9200/_search",
            "GET /a\\..\\b",
            "GET /a#frag",
            "GET /a%zz",
            "GET /a%ff",
            "GET",
            "FETCH /a",
            "GET /a extra",
        ] {
            assert!(parse_request(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn splitting_follows_method_lines() {
        let text = "# setup\nGET _cluster/health\n\nPUT /a\n{\n  \"settings\": {}\n}\n\n# next\nPOST _sql?format=json\n{\"query\": \"\"\"\nDELETE FROM x\nGET /y\n\"\"\"}\n";
        let parts = split(text);
        let sqls: Vec<&str> = parts.iter().map(|s| s.sql.as_str()).collect();
        assert_eq!(sqls.len(), 3, "{sqls:?}");
        assert_eq!(sqls[0], "GET _cluster/health");
        assert!(sqls[1].starts_with("PUT /a\n{") && sqls[1].ends_with("# next"));
        assert!(sqls[2].contains("DELETE FROM x\nGET /y"));
        assert_eq!(parts[0].line, 2);
        assert_eq!(parts[1].line, 4);
        // Offsets are characters.
        let t = "# é\nGET /a";
        let p = split(t);
        assert_eq!(p[0].start, 4);
        assert_eq!(&t.chars().skip(p[0].start).collect::<String>(), "GET /a");
    }

    #[test]
    fn comments_and_strings_do_not_split() {
        let text = "POST /a/_search\n{\n  /* GET /b\n  */\n  \"q\": \"x\" // GET /c\n}\n";
        assert_eq!(split(text).len(), 1);
        let text = "POST /a/_doc\n{\"msg\": \"x\"}\nDELETE /a";
        assert_eq!(split(text).len(), 2);
    }

    #[test]
    fn leading_junk_is_a_statement_that_fails() {
        let parts = split("hello\nGET /a");
        assert_eq!(parts.len(), 2);
        assert!(parse_request(&parts[0].sql).is_err());
        assert!(split("  \n// x\n").is_empty());
    }

    #[test]
    fn body_errors_name_the_line() {
        let e = parse_request("POST /a/_search\n{\n  \"q\": x\n}").unwrap_err();
        assert!(e.to_string().contains("line 3"), "{e}");
    }

    #[test]
    fn console_text_round_trips() {
        let r = req("POST /a/_update/1?if_seq_no=3&if_primary_term=1\n{\"doc\":{\"a\":1}}");
        assert_eq!(parse_request(&r.to_console_text()).unwrap(), r);
        let r = EsRequest::new(Method::Get, &["a b", "_search"]).param("q", "x y&z");
        assert_eq!(r.target(), "/a%20b/_search?q=x%20y%26z");
        assert_eq!(parse_request(&r.to_console_text()).unwrap(), r);
    }
}
