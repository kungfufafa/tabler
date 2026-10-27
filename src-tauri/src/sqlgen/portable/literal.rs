//! Values as SQL literals of an extension dialect, for display only (Preview SQL, the trace):
//! execution always binds parameters.

use crate::sqlgen::dialect::lex::{Scan, TokenKind, scan};
use crate::sqlgen::dialect::spec::{DialectSpec, PlaceholderStyle};
use crate::sqlgen::portable::split::first_reading;
use crate::types::value::Value;

/// Renders a `Value` as a literal of `spec`.
pub fn literal(spec: &DialectSpec, v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Default => "DEFAULT".into(),
        Value::Bool(b) => spec.knobs.bool_literal[usize::from(*b)].into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::BigInt(s) | Value::Decimal(s) => s.clone(),
        Value::Bytes(b) => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .unwrap_or_default();
            let hex: String = bytes.iter().map(|x| format!("{x:02x}")).collect();
            (spec.knobs.bytes_literal)(&hex)
        }
        other => (spec.quote_literal)(&other.as_text().unwrap_or_default()),
    }
}

/// Replaces the placeholders of `sql` (in `spec`'s style; strings, quoted names and comments
/// skipped) with the literals of `params`. Only for display; a placeholder without a value stays.
pub fn inline_params(spec: &DialectSpec, sql: &str, params: &[Value]) -> String {
    let scan = scan(spec.lex, first_reading(spec.lex), sql);
    let mut out = String::with_capacity(sql.len());
    let mut next_positional = 0;
    let mut i = 0;
    while i < scan.tokens.len() {
        let t = scan.tokens[i];
        if let Some((n, end)) = placeholder_at(spec.placeholder_style, &scan, i) {
            let n = n.unwrap_or_else(|| {
                next_positional += 1;
                next_positional
            });
            if let Some(v) = n.checked_sub(1).and_then(|k| params.get(k)) {
                out.push_str(&literal(spec, v));
                i = end;
                continue;
            }
        }
        out.extend(&scan.chars[t.start..t.end]);
        i += 1;
    }
    out
}

/// The placeholder starting at token `i`, if there is one: its number (None for a positional
/// `?`) and the index of the token after it.
fn placeholder_at(
    style: PlaceholderStyle,
    scan: &Scan,
    i: usize,
) -> Option<(Option<usize>, usize)> {
    let toks = &scan.tokens;
    let t = toks[i];
    let punct = |c: char| scan.is_punct(&t, c);
    // The word right after token `i`, touching it.
    let word_after = |k: usize| {
        toks.get(k + 1)
            .filter(|w| w.kind == TokenKind::Word && w.start == toks[k].end)
            .map(|w| scan.text(w))
    };
    let number = |s: &str| s.parse::<usize>().ok();
    match style {
        PlaceholderStyle::Question if punct('?') => Some((None, i + 1)),
        PlaceholderStyle::Dollar if punct('$') => Some((Some(number(&word_after(i)?)?), i + 2)),
        PlaceholderStyle::QuestionNumbered if punct('?') => {
            Some((Some(number(&word_after(i)?)?), i + 2))
        }
        PlaceholderStyle::Colon if punct(':') => Some((Some(number(&word_after(i)?)?), i + 2)),
        PlaceholderStyle::AtP | PlaceholderStyle::AtName if punct('@') => {
            let w = word_after(i)?;
            let digits = w.strip_prefix('P').or_else(|| w.strip_prefix('p'))?;
            Some((Some(number(digits)?), i + 2))
        }
        PlaceholderStyle::Typed if punct('{') => {
            // `{p1:Type}`: the whole braces.
            let w = word_after(i)?;
            let n = number(w.strip_prefix('p')?)?;
            let close = (i + 2..toks.len()).find(|&k| scan.is_punct(&toks[k], '}'))?;
            Some((Some(n), close + 1))
        }
        _ => None,
    }
}
