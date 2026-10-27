//! cqlsh's session commands, which the session runs itself and never sends: `CONSISTENCY`,
//! `SERIAL CONSISTENCY`, `TRACING`, `PAGING` and `EXPAND`. Safe Mode's classifier reads them as
//! inspections (`dialect::cql::RULES`), so the parse is strict: a statement is one of them only
//! when it is exactly that command (one `;` may end it), a command with any other text after it
//! is an error, and anything else goes to the server, which refuses it.

use ::scylla::frame::types::{Consistency, SerialConsistency};

use crate::error::AppError;
use crate::sqlgen::dialect::cql::LEX;
use crate::sqlgen::dialect::lex::{PLAIN, TokenKind, scan};

/// What `PAGING` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    Show,
    On,
    Off,
    /// Rows per page.
    Size(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `CONSISTENCY` alone shows the level; with one, sets it.
    Consistency(Option<Consistency>),
    SerialConsistency(Option<SerialConsistency>),
    /// `TRACING` alone shows the setting.
    Tracing(Option<bool>),
    Paging(Paging),
    Expand(Option<bool>),
}

/// The command a statement is: None when it is none of them (it goes to the server), an error
/// when it is one with an argument cqlsh would refuse.
pub fn parse(statement: &str) -> Option<Result<Command, AppError>> {
    let s = scan(&LEX, PLAIN, statement);
    let mut code: Vec<_> = s
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment))
        .collect();
    // cqlsh takes the command with or without its `;` (the editor's split leaves none).
    if code.last().is_some_and(|t| s.is_punct(t, ';')) {
        code.pop();
    }
    let mut words = Vec::new();
    let mut other = false;
    for t in code {
        if t.kind == TokenKind::Word {
            words.push(s.upper(t));
        } else {
            // A string, a quoted name or punctuation: no cqlsh command has one.
            other = true;
            break;
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    if other {
        return command_name(&words).map(|name| Err(improper(name)));
    }
    let on_off = |w: &str| match w {
        "ON" => Some(true),
        "OFF" => Some(false),
        _ => None,
    };
    let command = match words.as_slice() {
        ["CONSISTENCY"] => Ok(Command::Consistency(None)),
        ["CONSISTENCY", level] => consistency(level)
            .map(|c| Command::Consistency(Some(c)))
            .ok_or_else(|| improper("CONSISTENCY")),
        ["SERIAL", "CONSISTENCY"] => Ok(Command::SerialConsistency(None)),
        ["SERIAL", "CONSISTENCY", level] => serial(level)
            .map(|c| Command::SerialConsistency(Some(c)))
            .ok_or_else(|| improper("SERIAL CONSISTENCY")),
        ["TRACING"] => Ok(Command::Tracing(None)),
        ["TRACING", w] => on_off(w)
            .map(|b| Command::Tracing(Some(b)))
            .ok_or_else(|| improper("TRACING")),
        ["EXPAND"] => Ok(Command::Expand(None)),
        ["EXPAND", w] => on_off(w)
            .map(|b| Command::Expand(Some(b)))
            .ok_or_else(|| improper("EXPAND")),
        ["PAGING"] => Ok(Command::Paging(Paging::Show)),
        ["PAGING", "ON"] => Ok(Command::Paging(Paging::On)),
        ["PAGING", "OFF"] => Ok(Command::Paging(Paging::Off)),
        ["PAGING", n] => n
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .map(|n| Command::Paging(Paging::Size(n)))
            .ok_or_else(|| improper("PAGING")),
        words => return command_name(words).map(|name| Err(improper(name))),
    };
    Some(command)
}

/// The command that words open, when they open one: `SERIAL` alone is none (`SERIAL x` goes to
/// the server, which refuses it).
fn command_name(words: &[&str]) -> Option<&'static str> {
    Some(match words {
        ["CONSISTENCY", ..] => "CONSISTENCY",
        ["SERIAL", "CONSISTENCY", ..] => "SERIAL CONSISTENCY",
        ["TRACING", ..] => "TRACING",
        ["PAGING", ..] => "PAGING",
        ["EXPAND", ..] => "EXPAND",
        _ => return None,
    })
}

fn improper(command: &str) -> AppError {
    AppError::InvalidInput(format!("Improper {command} command"))
}

fn consistency(level: &str) -> Option<Consistency> {
    Some(match level {
        "ANY" => Consistency::Any,
        "ONE" => Consistency::One,
        "TWO" => Consistency::Two,
        "THREE" => Consistency::Three,
        "QUORUM" => Consistency::Quorum,
        "ALL" => Consistency::All,
        "LOCAL_QUORUM" => Consistency::LocalQuorum,
        "EACH_QUORUM" => Consistency::EachQuorum,
        "LOCAL_ONE" => Consistency::LocalOne,
        "SERIAL" => Consistency::Serial,
        "LOCAL_SERIAL" => Consistency::LocalSerial,
        _ => return None,
    })
}

fn serial(level: &str) -> Option<SerialConsistency> {
    match level {
        "SERIAL" => Some(SerialConsistency::Serial),
        "LOCAL_SERIAL" => Some(SerialConsistency::LocalSerial),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(sql: &str) -> Command {
        match parse(sql) {
            Some(Ok(c)) => c,
            other => panic!("{sql:?}: {other:?}"),
        }
    }

    fn refused(sql: &str) {
        assert!(
            matches!(parse(sql), Some(Err(AppError::InvalidInput(_)))),
            "{sql:?}: {:?}",
            parse(sql)
        );
    }

    #[test]
    fn session_commands() {
        assert_eq!(ok("CONSISTENCY"), Command::Consistency(None));
        assert_eq!(
            ok("consistency local_quorum"),
            Command::Consistency(Some(Consistency::LocalQuorum))
        );
        assert_eq!(
            ok("  CONSISTENCY ONE -- c"),
            Command::Consistency(Some(Consistency::One))
        );
        assert_eq!(
            ok("SERIAL CONSISTENCY LOCAL_SERIAL"),
            Command::SerialConsistency(Some(SerialConsistency::LocalSerial))
        );
        assert_eq!(ok("serial consistency"), Command::SerialConsistency(None));
        assert_eq!(ok("TRACING ON"), Command::Tracing(Some(true)));
        assert_eq!(ok("tracing"), Command::Tracing(None));
        assert_eq!(ok("PAGING 50"), Command::Paging(Paging::Size(50)));
        assert_eq!(ok("PAGING OFF"), Command::Paging(Paging::Off));
        assert_eq!(ok("EXPAND OFF"), Command::Expand(Some(false)));
        // One `;` may end the command, as in cqlsh.
        assert_eq!(
            ok("CONSISTENCY ONE;"),
            Command::Consistency(Some(Consistency::One))
        );
        assert_eq!(
            ok("SERIAL CONSISTENCY SERIAL ; -- c"),
            Command::SerialConsistency(Some(SerialConsistency::Serial))
        );
        assert_eq!(ok("PAGING 10;"), Command::Paging(Paging::Size(10)));
        assert_eq!(ok("tracing;"), Command::Tracing(None));
    }

    #[test]
    fn strict_about_arguments() {
        refused("CONSISTENCY FIVE");
        refused("CONSISTENCY ONE TWO");
        refused("CONSISTENCY 'ONE'");
        refused("SERIAL CONSISTENCY QUORUM");
        refused("TRACING MAYBE");
        refused("PAGING 0");
        refused("PAGING -1");
        refused("PAGING 99999999999");
        refused("EXPAND ON; DELETE FROM t");
        refused("CONSISTENCY ONE DELETE FROM t");
        refused("CONSISTENCY ONE;;");
        refused("CONSISTENCY FIVE;");
        refused("TRACING ON, OFF");
        // The error names the command, SERIAL CONSISTENCY whole.
        for (sql, name) in [
            ("SERIAL CONSISTENCY QUORUM", "SERIAL CONSISTENCY"),
            ("SERIAL CONSISTENCY SERIAL, x", "SERIAL CONSISTENCY"),
            ("SERIAL CONSISTENCY 'SERIAL'", "SERIAL CONSISTENCY"),
            ("PAGING 'x'", "PAGING"),
        ] {
            match parse(sql) {
                Some(Err(AppError::InvalidInput(m))) => {
                    assert_eq!(m, format!("Improper {name} command"), "{sql:?}");
                }
                other => panic!("{sql:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn other_statements_go_to_the_server() {
        for sql in [
            "SELECT * FROM t",
            "DELETE FROM t WHERE k = 1",
            "SERIAL",
            "SERIAL x",
            // With or without a `;` or other punctuation, alike.
            "SERIAL x;",
            "SERIAL;",
            "SERIAL 'x'",
            ";",
            "'CONSISTENCY'",
            "\"TRACING\" ON",
            "",
            "-- CONSISTENCY ONE",
        ] {
            assert!(parse(sql).is_none(), "{sql:?}");
        }
    }
}
