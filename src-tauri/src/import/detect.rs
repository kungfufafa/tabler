//! Guesses for the CSV wizard: the delimiter of a file and the column types of a new table
//! ("Autodetect column datatype").

use std::io::BufReader;
use std::sync::LazyLock;

use regex::Regex;

use crate::sqlgen::Dialect;
use crate::types::import::{ImportColumnDef, ImportType};

use super::csvread::CsvReader;

/// Delimiters tried, in order of preference on a tie.
pub const DELIMITERS: [u8; 4] = *b",;\t|";

/// Records of the sample compared.
const SAMPLE_RECORDS: usize = 50;

/// The delimiter that splits the records of `sample` (the start of the file, possibly cut in the
/// middle of a record) into the same number of fields, more than one, most often. Falls back to
/// `,`.
pub fn detect_delimiter(sample: &str, quote: u8, cut: bool) -> u8 {
    let mut best = (b',', 0usize, 0usize);
    for &d in &DELIMITERS {
        let mut r = CsvReader::new(BufReader::new(sample.as_bytes()), d, quote);
        let mut counts = Vec::new();
        while counts.len() < SAMPLE_RECORDS + 1 {
            match r.next_record() {
                Ok(Some(rec)) => counts.push(rec.fields.len()),
                _ => break,
            }
        }
        // The last record of a cut sample may be partial.
        if cut && counts.len() > 1 {
            counts.pop();
        }
        counts.truncate(SAMPLE_RECORDS);
        let Some(&first) = counts.first() else {
            continue;
        };
        if first < 2 {
            continue;
        }
        let consistent = counts.iter().filter(|&&n| n == first).count();
        if (consistent, first) > (best.1, best.2) {
            best = (d, consistent, first);
        }
    }
    best.0
}

static INTEGER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[+-]?\d+$").expect("valid integer regex"));
static NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$").expect("valid numeric regex")
});
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid date regex"));
/// Date, time, optional seconds and fraction, optional UTC offset (`Z`, `+07`, `+0700`,
/// `+07:00`).
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d{2}-\d{2})[ T](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,9}))?)?(Z|[+-]\d{2}(?::?\d{2})?)?$")
        .expect("valid timestamp regex")
});

fn is_bool(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "true" | "false" | "t" | "f" | "yes" | "no" | "y" | "n"
    )
}

/// A boolean spelled the way CSV files do (`1` / `0` and `on` / `off` too).
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "yes" | "y" | "1" | "on" => Some(true),
        "false" | "f" | "no" | "n" | "0" | "off" => Some(false),
        _ => None,
    }
}

fn valid_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Leading zeros (`007`, `02134`) are codes, not numbers: importing them as numbers drops the
/// zeros.
fn has_leading_zero(s: &str) -> bool {
    let digits = s.trim_start_matches(['+', '-']);
    digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.")
}

/// Running type detection of one column.
#[derive(Debug, Clone)]
pub struct TypeGuess {
    seen: usize,
    int: bool,
    bigint: bool,
    numeric: bool,
    boolean: bool,
    date: bool,
    timestamp: bool,
    /// A timestamp with a UTC offset was seen.
    offset: bool,
    int_digits: u32,
    scale: u32,
    fraction_secs: u32,
}

impl Default for TypeGuess {
    fn default() -> Self {
        TypeGuess {
            seen: 0,
            int: true,
            bigint: true,
            numeric: true,
            boolean: true,
            date: true,
            timestamp: true,
            offset: false,
            int_digits: 0,
            scale: 0,
            fraction_secs: 0,
        }
    }
}

impl TypeGuess {
    /// Adds a non-empty value (empty fields and NULLs say nothing about the type).
    pub fn add(&mut self, value: &str) {
        let v = value.trim();
        if v.is_empty() {
            return;
        }
        self.seen += 1;
        let leading_zero = has_leading_zero(v);
        if self.int || self.bigint {
            let integral = INTEGER.is_match(v) && !leading_zero;
            self.int &= integral && v.parse::<i32>().is_ok();
            self.bigint &= integral && v.parse::<i64>().is_ok();
        }
        if self.numeric {
            if NUMERIC.is_match(v) && !leading_zero {
                let (int_digits, scale) = numeric_digits(v);
                self.int_digits = self.int_digits.max(int_digits);
                self.scale = self.scale.max(scale);
            } else {
                self.numeric = false;
            }
        }
        self.boolean &= is_bool(v);
        self.date &= DATE.is_match(v) && valid_date(v);
        if self.timestamp {
            match TIMESTAMP.captures(v) {
                Some(c) if valid_date(&c[1]) => {
                    let frac = c.get(5).map_or(0, |m| m.as_str().len() as u32);
                    self.fraction_secs = self.fraction_secs.max(frac);
                    self.offset |= c.get(6).is_some();
                }
                _ => self.timestamp = DATE.is_match(v) && valid_date(v),
            }
        }
    }

    /// The column definition for a new table named `name`.
    pub fn column(&self, name: &str) -> ImportColumnDef {
        let def = |ty, precision, scale| ImportColumnDef {
            name: name.to_string(),
            ty,
            precision,
            scale,
        };
        if self.seen == 0 {
            return def(ImportType::Text, None, None);
        }
        if self.int {
            def(ImportType::Integer, None, None)
        } else if self.bigint {
            def(ImportType::Bigint, None, None)
        } else if self.numeric {
            let scale = self.scale.min(30);
            def(
                ImportType::Numeric,
                Some((self.int_digits + scale).clamp(1, 65)),
                Some(scale),
            )
        } else if self.boolean {
            def(ImportType::Boolean, None, None)
        } else if self.date {
            def(ImportType::Date, None, None)
        } else if self.timestamp {
            // Any offset: a column that keeps it (PostgreSQL would drop it from a `timestamp`).
            def(
                if self.offset {
                    ImportType::TimestampTz
                } else {
                    ImportType::Timestamp
                },
                None,
                Some(self.fraction_secs.min(6)).filter(|s| *s > 0),
            )
        } else {
            def(ImportType::Text, None, None)
        }
    }
}

/// Digits before and after the point of a number in the `NUMERIC` form, with its exponent
/// applied (`1.5e-3` is 0.0015: no integer digits, scale 4), each capped at 1,000.
fn numeric_digits(v: &str) -> (u32, u32) {
    let mut parts = v.splitn(2, ['e', 'E']);
    let mantissa = parts.next().unwrap_or(v).trim_start_matches(['+', '-']);
    let exponent: i64 = parts
        .next()
        .and_then(|e| e.parse::<i64>().ok())
        .unwrap_or(0)
        .clamp(-1000, 1000);
    let (int_part, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let int_part = int_part.trim_start_matches('0');
    let int_digits = if int_part.is_empty() {
        // 0.0012e3 = 1.2: digits move from the fraction (leading zeros do not count).
        let lead = frac.len() - frac.trim_start_matches('0').len();
        (exponent - lead as i64).max(0)
    } else {
        (int_part.len() as i64 + exponent).max(0)
    };
    let scale = (frac.len() as i64 - exponent).max(0);
    (int_digits.min(1000) as u32, scale.min(1000) as u32)
}

/// Why a value would not be stored as it is in a column the import creates as `ty` (`None`: it
/// would, or the server reports it itself):
/// - a date column drops the time of day of a timestamp (PostgreSQL silently; MySQL with a note);
/// - PostgreSQL drops the UTC offset of a value stored in a `timestamp` (without time zone).
///
/// The types come from the first rows; a later row like this fails (or is skipped) instead.
pub fn created_value_problem(ty: ImportType, dialect: Dialect, value: &str) -> Option<String> {
    let v = value.trim();
    match ty {
        ImportType::Date if dialect != Dialect::Sqlite => {
            let c = TIMESTAMP.captures(v)?;
            let midnight = [c.get(2), c.get(3), c.get(4), c.get(5)]
                .iter()
                .flatten()
                .all(|m| m.as_str().bytes().all(|b| b == b'0'));
            (!midnight || c.get(6).is_some())
                .then(|| format!("\"{v}\" has a time of day, but the new column is a date"))
        }
        ImportType::Timestamp if dialect == Dialect::Postgres => {
            let c = TIMESTAMP.captures(v)?;
            c.get(6).map(|_| {
                format!(
                    "\"{v}\" has a UTC offset, but the new column is a timestamp without time \
                     zone (choose timestamptz)"
                )
            })
        }
        _ => None,
    }
}

/// A timestamp with a UTC offset as the same moment in UTC, `YYYY-MM-DD HH:MM:SS[.fraction]`
/// (MySQL's DATETIME keeps no offset and refuses `Z`); `None` for a value without an offset or
/// one that is not such a timestamp.
pub fn timestamp_in_utc(value: &str) -> Option<String> {
    let v = value.trim();
    let c = TIMESTAMP.captures(v)?;
    let offset = c.get(6)?.as_str();
    let seconds = if offset == "Z" {
        0
    } else {
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let digits: String = offset[1..].chars().filter(char::is_ascii_digit).collect();
        let hours: i64 = digits.get(..2)?.parse().ok()?;
        let minutes: i64 = digits
            .get(2..)
            .filter(|m| !m.is_empty())
            .map_or(Some(0), |m| m.parse().ok())?;
        sign * (hours * 3600 + minutes * 60)
    };
    let date = chrono::NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").ok()?;
    let sec: u32 = c.get(4).map_or(Some(0), |m| m.as_str().parse().ok())?;
    let time = chrono::NaiveTime::from_hms_opt(c[2].parse().ok()?, c[3].parse().ok()?, sec)?;
    let utc = date.and_time(time) - chrono::Duration::seconds(seconds);
    let mut out = utc.format("%Y-%m-%d %H:%M:%S").to_string();
    if let Some(frac) = c.get(5) {
        out.push('.');
        out.push_str(frac.as_str());
    }
    Some(out)
}

/// Detects a column's type from its values.
pub fn guess<'a>(values: impl IntoIterator<Item = &'a str>, name: &str) -> ImportColumnDef {
    let mut g = TypeGuess::default();
    for v in values {
        g.add(v);
    }
    g.column(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(values: &[&str]) -> ImportType {
        guess(values.iter().copied(), "c").ty
    }

    #[test]
    fn detects_column_types() {
        assert_eq!(ty(&["1", "-2", "", "+30"]), ImportType::Integer);
        assert_eq!(ty(&["1", "3000000000"]), ImportType::Bigint);
        assert_eq!(ty(&["1", "99999999999999999999"]), ImportType::Numeric);
        assert_eq!(ty(&["1.5", "2", "-.25", "1e3"]), ImportType::Numeric);
        assert_eq!(ty(&["true", "False", "yes", "N"]), ImportType::Boolean);
        // 0 / 1 are numbers first.
        assert_eq!(ty(&["0", "1"]), ImportType::Integer);
        assert_eq!(ty(&["2026-01-31", "1999-12-01"]), ImportType::Date);
        assert_eq!(ty(&["2026-02-30"]), ImportType::Text);
        assert_eq!(
            ty(&["2026-01-31", "2026-01-31 10:20:30", "2026-01-31T10:20"]),
            ImportType::Timestamp
        );
        // An offset anywhere: a column that keeps it.
        assert_eq!(
            ty(&["2026-01-31 10:20:30+07:00", "2026-01-31 10:20"]),
            ImportType::TimestampTz
        );
        assert_eq!(ty(&["2026-01-31T10:20:30Z"]), ImportType::TimestampTz);
        // Dates that show a time somewhere are timestamps.
        assert_eq!(
            ty(&["2026-01-31", "2026-02-01 08:00"]),
            ImportType::Timestamp
        );
        assert_eq!(ty(&["007", "12"]), ImportType::Text);
        assert_eq!(ty(&["0", "0.5"]), ImportType::Numeric);
        assert_eq!(ty(&["1", "x"]), ImportType::Text);
        assert_eq!(ty(&["", ""]), ImportType::Text);
        assert_eq!(ty(&[]), ImportType::Text);
    }

    #[test]
    fn exponents_move_digits() {
        assert_eq!(numeric_digits("1.5e-3"), (0, 4));
        assert_eq!(numeric_digits("12.5E1"), (3, 0));
        assert_eq!(numeric_digits("1e3"), (4, 0));
        assert_eq!(numeric_digits("0.0012e3"), (1, 1));
        assert_eq!(numeric_digits("-0.05"), (0, 2));
        assert_eq!(numeric_digits("1e-9999"), (0, 1000));
        let d = guess(["1.5e-3", "2"], "x");
        assert_eq!(
            (d.ty, d.precision, d.scale),
            (ImportType::Numeric, Some(5), Some(4))
        );
    }

    #[test]
    fn values_a_new_column_would_change() {
        use Dialect::*;
        let date = |d, v| created_value_problem(ImportType::Date, d, v);
        assert!(date(Postgres, "2026-01-02 10:20").is_some());
        assert!(date(Mysql, "2026-01-02T10:20:30.5").is_some());
        assert!(date(Postgres, "2026-01-02 00:00:00.000").is_none());
        assert!(date(Postgres, "2026-01-02 00:00Z").is_some());
        assert!(date(Postgres, "2026-01-02").is_none());
        assert!(date(Sqlite, "2026-01-02 10:20").is_none());
        let ts = |d, v| created_value_problem(ImportType::Timestamp, d, v);
        assert!(ts(Postgres, "2026-01-02 10:20+07").is_some());
        assert!(ts(Postgres, "2026-01-02 10:20").is_none());
        assert!(ts(Mysql, "2026-01-02 10:20+07:00").is_none());
        assert!(
            created_value_problem(ImportType::TimestampTz, Postgres, "2026-01-02 10:20Z").is_none()
        );
        assert!(created_value_problem(ImportType::Text, Postgres, "2026-01-02 10:20").is_none());
    }

    #[test]
    fn offsets_to_utc() {
        assert_eq!(
            timestamp_in_utc("2026-01-02 03:20:30.123+07:00").as_deref(),
            Some("2026-01-01 20:20:30.123")
        );
        assert_eq!(
            timestamp_in_utc("2026-12-31T23:30Z").as_deref(),
            Some("2026-12-31 23:30:00")
        );
        assert_eq!(
            timestamp_in_utc("2026-12-31 23:30:00-0530").as_deref(),
            Some("2027-01-01 05:00:00")
        );
        assert_eq!(
            timestamp_in_utc("2026-12-31 23:30:00-05").as_deref(),
            Some("2027-01-01 04:30:00")
        );
        assert_eq!(timestamp_in_utc("2026-12-31 23:30:00"), None);
        assert_eq!(timestamp_in_utc("2026-02-30 10:00Z"), None);
        assert_eq!(timestamp_in_utc("x"), None);
    }

    #[test]
    fn numeric_and_timestamp_sizes() {
        let d = guess(["12.5", "-1234.125", "0.1"], "price");
        assert_eq!(d.name, "price");
        assert_eq!((d.precision, d.scale), (Some(7), Some(3)));
        let t = guess(["2026-01-31 10:20:30.123", "2026-01-31 10:20:30.5"], "at");
        assert_eq!((t.ty, t.scale), (ImportType::Timestamp, Some(3)));
        let t = guess(["2026-01-31 10:20:30"], "at");
        assert_eq!(t.scale, None);
    }

    #[test]
    fn booleans() {
        assert_eq!(parse_bool(" Yes "), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn detects_delimiters() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n", b'"', false), b',');
        assert_eq!(
            detect_delimiter("a;b;c\n1;2,5;3\n4;5;6\n", b'"', false),
            b';'
        );
        assert_eq!(detect_delimiter("a\tb\n1\t\"x,y\"\n", b'"', false), b'\t');
        assert_eq!(detect_delimiter("a|b\n1|2\n", b'"', false), b'|');
        // Commas inside quotes do not count.
        assert_eq!(
            detect_delimiter("name;note\nx;\"a, b, c\"\ny;\"d, e\"\n", b'"', false),
            b';'
        );
        // A single column: the default.
        assert_eq!(detect_delimiter("value\n1\n2\n", b'"', false), b',');
        // A cut last record does not count.
        assert_eq!(detect_delimiter("a;b\n1;2\n3;4\n5", b'"', true), b';');
    }
}
