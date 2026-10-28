//! The console's command lines, read the way redis-cli reads them (its `sdssplitargs`):
//! arguments split at whitespace; `"…"` takes the escapes `\n` `\r` `\t` `\b` `\a` `\xHH` and
//! `\<any>`; `'…'` is literal except `\'`; a quote may start in the middle of a word
//! (`a"b c"` is `ab c`) and a closing quote must be followed by whitespace or the end. Arguments
//! are bytes (`\xff` is one byte), sent as argv, never re-joined into text.
//!
//! The editor text holds one command per line, or several separated by `;`; both are code only
//! outside quotes, so a quoted value may hold `;` and line breaks.

use crate::sqlgen::split::SplitStatement;

/// Why a command line cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// A quote is not closed.
    Unbalanced,
    /// A closing quote is followed by something other than whitespace.
    AfterQuote,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CliError::Unbalanced => "Invalid argument(s): unbalanced quotes",
            CliError::AfterQuote => {
                "Invalid argument(s): a closing quote must be followed by a space"
            }
        })
    }
}

/// C's `isspace`: what separates arguments and may follow a closing quote.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Splits one command line into its arguments.
pub fn tokenize(line: &str) -> Result<Vec<Vec<u8>>, CliError> {
    let p = line.as_bytes();
    let mut i = 0;
    let mut args = Vec::new();
    loop {
        while i < p.len() && is_space(p[i]) {
            i += 1;
        }
        if i >= p.len() {
            return Ok(args);
        }
        let mut cur = Vec::new();
        let (mut dq, mut sq) = (false, false);
        loop {
            let c = p.get(i).copied();
            if dq {
                match c {
                    None => return Err(CliError::Unbalanced),
                    Some(b'\\')
                        if p.get(i + 1) == Some(&b'x')
                            && p.get(i + 2).copied().and_then(hex).is_some()
                            && p.get(i + 3).copied().and_then(hex).is_some() =>
                    {
                        let hi = p.get(i + 2).copied().and_then(hex).unwrap_or(0);
                        let lo = p.get(i + 3).copied().and_then(hex).unwrap_or(0);
                        cur.push((hi << 4) | lo);
                        i += 3;
                    }
                    Some(b'\\') if i + 1 < p.len() => {
                        i += 1;
                        cur.push(match p[i] {
                            b'n' => b'\n',
                            b'r' => b'\r',
                            b't' => b'\t',
                            b'b' => 0x08,
                            b'a' => 0x07,
                            other => other,
                        });
                    }
                    Some(b'"') => {
                        if p.get(i + 1).is_some_and(|&n| !is_space(n)) {
                            return Err(CliError::AfterQuote);
                        }
                        i += 1;
                        break;
                    }
                    Some(b) => cur.push(b),
                }
            } else if sq {
                match c {
                    None => return Err(CliError::Unbalanced),
                    Some(b'\\') if p.get(i + 1) == Some(&b'\'') => {
                        i += 1;
                        cur.push(b'\'');
                    }
                    Some(b'\'') => {
                        if p.get(i + 1).is_some_and(|&n| !is_space(n)) {
                            return Err(CliError::AfterQuote);
                        }
                        i += 1;
                        break;
                    }
                    Some(b) => cur.push(b),
                }
            } else {
                match c {
                    None | Some(b' ' | b'\n' | b'\r' | b'\t') => break,
                    Some(b'"') => dq = true,
                    Some(b'\'') => sq = true,
                    Some(b) => cur.push(b),
                }
            }
            i += 1;
        }
        args.push(cur);
    }
}

/// Splits editor text into commands: at line breaks and `;` outside quotes. Offsets are
/// characters (the editor's unit), each command trimmed; blank ones are skipped.
pub fn split_console(text: &str) -> Vec<SplitStatement> {
    #[derive(PartialEq)]
    enum Q {
        No,
        Double,
        Single,
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut q = Q::No;
    let mut start = 0;
    let mut i = 0;
    let flush = |out: &mut Vec<SplitStatement>, from: usize, to: usize| {
        let mut s = from;
        let mut e = to;
        while s < e && chars[s].is_whitespace() {
            s += 1;
        }
        while e > s && chars[e - 1].is_whitespace() {
            e -= 1;
        }
        if s < e {
            let line = 1 + chars[..s].iter().filter(|&&c| c == '\n').count() as u32;
            out.push(SplitStatement {
                sql: chars[s..e].iter().collect(),
                start: s,
                end: e,
                line,
            });
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match q {
            Q::Double => match c {
                '\\' => i += 1,
                '"' => q = Q::No,
                _ => {}
            },
            Q::Single => match c {
                '\\' if chars.get(i + 1) == Some(&'\'') => i += 1,
                '\'' => q = Q::No,
                _ => {}
            },
            Q::No => match c {
                '"' => q = Q::Double,
                '\'' => q = Q::Single,
                '\n' | ';' => {
                    flush(&mut out, start, i);
                    start = i + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    flush(&mut out, start, chars.len().max(start));
    out
}

/// The command name of an argv, upper-case; `None` for an empty line or a name that is not text.
pub fn command_name(argv: &[Vec<u8>]) -> Option<String> {
    let name = std::str::from_utf8(argv.first()?).ok()?;
    (!name.is_empty()).then(|| name.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        tokenize(line)
            .unwrap()
            .into_iter()
            .map(|a| String::from_utf8_lossy(&a).into_owned())
            .collect()
    }

    #[test]
    fn plain_words() {
        assert_eq!(args("  GET   user:1 \t"), ["GET", "user:1"]);
        assert_eq!(args(""), Vec::<String>::new());
        assert_eq!(args(" \t\r\n"), Vec::<String>::new());
        assert_eq!(args("SET k a\\nb"), ["SET", "k", "a\\nb"]);
    }

    #[test]
    fn double_quotes_take_escapes() {
        assert_eq!(
            args(r#"SET "my key" "a\nb\t\"c\"""#),
            ["SET", "my key", "a\nb\t\"c\""]
        );
        assert_eq!(
            tokenize(r#"SET k "\x00\xFF\x4a\xzz""#).unwrap()[2],
            b"\x00\xff\x4axzz"
        );
        assert_eq!(args(r#"ECHO "\a\b\q\\""#)[1].as_bytes(), b"\x07\x08q\\");
        assert_eq!(args(r#"SET k """#), ["SET", "k", ""]);
        assert_eq!(args(r#"SET k "caf\xc3\xa9""#)[2], "café");
    }

    #[test]
    fn single_quotes_are_literal() {
        assert_eq!(args(r#"SET k 'a\nb "c"'"#), ["SET", "k", "a\\nb \"c\""]);
        assert_eq!(args(r"SET k 'it\'s'"), ["SET", "k", "it's"]);
        assert_eq!(args("SET k ''"), ["SET", "k", ""]);
    }

    #[test]
    fn quotes_inside_words_and_errors() {
        // A closing quote followed by a letter is an error in redis-cli.
        assert_eq!(tokenize(r#"SET a"b c"d"#), Err(CliError::AfterQuote));
        assert_eq!(args(r#"SET a"b c""#), ["SET", "ab c"]);
        assert_eq!(tokenize(r#"GET "abc"#), Err(CliError::Unbalanced));
        assert_eq!(tokenize("GET 'abc"), Err(CliError::Unbalanced));
        assert_eq!(tokenize(r#"GET "abc\"#), Err(CliError::Unbalanced));
        assert_eq!(tokenize("GET 'a'b"), Err(CliError::AfterQuote));
        assert_eq!(args("GET \"a\"\t"), ["GET", "a"]);
        assert!(CliError::Unbalanced.to_string().contains("unbalanced"));
    }

    #[test]
    fn splitting_at_lines_and_semicolons() {
        let text = "GET a\n\n  SET b \"x;y\nz\" ; DEL c;\r\nPING";
        let parts = split_console(text);
        let sqls: Vec<&str> = parts.iter().map(|s| s.sql.as_str()).collect();
        assert_eq!(sqls, ["GET a", "SET b \"x;y\nz\"", "DEL c", "PING"]);
        assert_eq!(parts[0].line, 1);
        assert_eq!(parts[1].line, 3);
        assert_eq!(parts[3].line, 5);
        let chars: Vec<char> = text.chars().collect();
        for p in &parts {
            assert_eq!(chars[p.start..p.end].iter().collect::<String>(), p.sql);
        }
        // Escaped quotes do not end a string; single quotes only end at an unescaped quote.
        let parts = split_console(r#"SET k "a\";b"; SET j 'it\'s;x'; GET z"#);
        let sqls: Vec<&str> = parts.iter().map(|s| s.sql.as_str()).collect();
        assert_eq!(sqls, [r#"SET k "a\";b""#, r#"SET j 'it\'s;x'"#, "GET z"]);
        assert_eq!(tokenize(sqls[0]).unwrap()[2], b"a\";b");
        // Offsets count characters, not bytes.
        let parts = split_console("ECHO é;GET b");
        assert_eq!((parts[1].start, parts[1].end), (7, 12));
        assert!(split_console(" ;\n ; ").is_empty());
        // An unclosed quote runs to the end; the tokenizer then reports it.
        let parts = split_console("GET \"a\nb");
        assert_eq!(parts.len(), 1);
        assert_eq!(tokenize(&parts[0].sql), Err(CliError::Unbalanced));
    }

    #[test]
    fn names() {
        assert_eq!(
            command_name(&tokenize("hgetall h").unwrap()).as_deref(),
            Some("HGETALL")
        );
        assert_eq!(command_name(&[]), None);
        assert_eq!(command_name(&[b"\xff".to_vec()]), None);
        assert_eq!(command_name(&[Vec::new()]), None);
    }
}
