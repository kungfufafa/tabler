//! What an MCP client may see and do (TablePlus #3902 / #3909: its MCP queries first ignored the
//! connection's safe mode). Tabler's MCP server is read-only by default:
//!
//! - Connections tagged Production are hidden unless "Expose production connections" is on.
//! - A statement classified as a write (`sqlgen::classify`, the safe-mode classifier) runs only
//!   when "Allow MCP clients to change data" is on AND the connection is in Silent Mode AND it is
//!   not read-only. Anything else is refused before it reaches the server.
//! - A connection that may not be written to is also opened read-only on the server
//!   ([`session_writable`]), so a statement the classifier misjudges still cannot change data.

use std::collections::{BTreeSet, HashSet};

use crate::commands::safety::run_writes;
use crate::sqlgen::dialect::DialectRef;
use crate::sqlgen::split::{code_only, split_statements};
use crate::sqlgen::{Dialect, SqlDialect};
use crate::types::caps::SessionCaps;
use crate::types::driver::ReadOnlyMode;
use crate::types::profile::{ConnectionProfile, ConnectionTag, SafeModeLevel};

use super::prefs::McpPrefs;

/// The connection's safe-mode level (profiles saved before safe mode existed are silent, as in
/// the renderer's `effectiveLevel`).
pub fn safe_mode(profile: &ConnectionProfile) -> SafeModeLevel {
    profile.options.safe_mode.unwrap_or_default()
}

pub fn is_production(profile: &ConnectionProfile) -> bool {
    matches!(profile.options.tag, Some(ConnectionTag::Production))
}

/// Whether MCP clients see the connection at all.
pub fn exposed(profile: &ConnectionProfile, prefs: &McpPrefs) -> bool {
    !is_production(profile) || prefs.expose_production
}

/// Why writes are refused on a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBlock {
    ReadOnly,
    PrefOff,
    SafeMode(SafeModeLevel),
}

/// TablePlus's names of the safe-mode levels (`src/features/safety/levels.ts`).
pub fn level_label(level: SafeModeLevel) -> &'static str {
    match level {
        SafeModeLevel::Silent => "Silent Mode",
        SafeModeLevel::Alert => "Alert Mode 1",
        SafeModeLevel::AlertNonSelect => "Alert Mode 2",
        SafeModeLevel::Safe => "Safe Mode 1",
        SafeModeLevel::SafeNonSelect => "Safe Mode 2",
    }
}

impl WriteBlock {
    /// Why writes are refused on the connection named `name`, whose read-only mode is `mode`.
    pub fn message(self, name: &str, mode: ReadOnlyMode) -> String {
        match self {
            WriteBlock::ReadOnly => match mode {
                ReadOnlyMode::Server | ReadOnlyMode::PerRequest => format!(
                    "\u{201c}{name}\u{201d} is read-only: MCP clients can only read from it."
                ),
                // Nothing but the classifier stands behind the setting there.
                ReadOnlyMode::ClassifierOnly => format!(
                    "\u{201c}{name}\u{201d} is read-only: MCP clients can only read from it. Its \
                     server does not enforce that, so Tabler refuses every statement it cannot \
                     tell is a read."
                ),
            },
            WriteBlock::PrefOff => {
                "MCP clients can only read: \u{201c}Allow MCP clients to change \
                 data\u{201d} is off in Tabler \u{25b8} Settings \u{25b8} MCP Server."
                    .to_string()
            }
            WriteBlock::SafeMode(level) => format!(
                "\u{201c}{name}\u{201d} is in {}: MCP clients can change data only on connections \
                 in Silent Mode.",
                level_label(level)
            ),
        }
    }
}

/// `None` when MCP clients may change data on the connection.
pub fn write_block(profile: &ConnectionProfile, prefs: &McpPrefs) -> Option<WriteBlock> {
    if profile.options.read_only {
        return Some(WriteBlock::ReadOnly);
    }
    if !prefs.allow_writes {
        return Some(WriteBlock::PrefOff);
    }
    match safe_mode(profile) {
        SafeModeLevel::Silent => None,
        level => Some(WriteBlock::SafeMode(level)),
    }
}

/// Whether the connection's MCP session is opened writable on the server.
pub fn session_writable(profile: &ConnectionProfile, prefs: &McpPrefs) -> bool {
    write_block(profile, prefs).is_none()
}

/// Why a MySQL `USE <db>` is refused over MCP.
pub const USE_REFUSED: &str = "USE is not supported over MCP: it would change which database \
     later statements run in. Pass the database argument of run_query instead.";

/// Why SQL with very many different `/*!NNNNN … */` comments is refused over MCP.
pub const COMMENTS_REFUSED: &str = "This SQL has too many different version-conditional comments \
     (/*!NNNNN … */) to be checked safely. Remove them, or run it in smaller parts.";

/// Most different version numbers of conditional comments that one text may carry: the text is
/// read once for each, and a handful is all a real script (a mysqldump header) has.
const MAX_COMMENT_VERSIONS: usize = 16;

/// Most characters read in all for the versions of one text (its length times the number of
/// readings): a long text with many versions would otherwise cost seconds in a call.
const MAX_READING_CHARS: usize = 4 * 1024 * 1024;

/// Whether `code` (SQL with its strings and comments blanked) starts with `USE`: after any
/// whitespace, and the version number a conditional comment (`/*!50000 USE db */`) leaves behind.
fn starts_with_use(code: &str) -> bool {
    let rest = code.trim_start_matches(|c: char| c.is_whitespace() || c.is_ascii_digit());
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    word.eq_ignore_ascii_case("USE")
}

/// A comment that opens with a character that makes the server read it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    /// `/*+ … */`: an optimizer hint, which to the server is never more than a comment.
    Hint,
    /// `/*!NNNNN … */`: MySQL runs the body as SQL when the server is at least version `NNNNN`
    /// (no digits: always).
    Conditional(u64),
    /// `/*M!NNNNNN … */`: the same for MariaDB; MySQL reads it as a plain comment.
    MariaDb(u64),
}

/// The number written right at `from` (0 when there are no digits; it saturates).
fn number_at(chars: &[char], from: usize) -> u64 {
    chars[from.min(chars.len())..]
        .iter()
        .map_while(|c| c.to_digit(10))
        .fold(0u64, |n, d| {
            n.saturating_mul(10).saturating_add(u64::from(d))
        })
}

/// The opener that starts at `chars[i]` and how many chars its marker (`/*!`, `/*M!`, `/*+`) has.
/// Found by the characters alone, with no regard for strings or comments around it: where it is
/// not a real opener, rewriting it changes no more than the content of a string or a comment.
fn opener_at(chars: &[char], i: usize) -> Option<(Opener, usize)> {
    if chars.get(i) != Some(&'/') || chars.get(i + 1) != Some(&'*') {
        return None;
    }
    match chars.get(i + 2)? {
        '+' => Some((Opener::Hint, 3)),
        '!' => Some((Opener::Conditional(number_at(chars, i + 3)), 3)),
        m if m.eq_ignore_ascii_case(&'m') && chars.get(i + 3) == Some(&'!') => {
            Some((Opener::MariaDb(number_at(chars, i + 4)), 4))
        }
        _ => None,
    }
}

/// The different versions the conditional comments of `chars` name.
fn comment_versions(chars: &[char]) -> BTreeSet<u64> {
    (0..chars.len())
        .filter_map(|i| match opener_at(chars, i) {
            Some((Opener::Conditional(n) | Opener::MariaDb(n), _)) => Some(n),
            _ => None,
        })
        .collect()
}

/// `chars` as a server at `version` reads its comments. The splitter keeps `/*!` as code and reads
/// every other comment as plain text, so each opener is written the way the splitter takes it:
/// a comment whose body the server runs as `/*!` (the version digits that follow stay), anything
/// else as a plain `/*`. Hints are always plain comments. `/*M!` runs only on MariaDB.
fn read_as(chars: &[char], mariadb: bool, version: u64) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match opener_at(chars, i) {
            Some((opener, marker_len)) => {
                out.push_str(match opener {
                    Opener::Hint => "/* ",
                    Opener::Conditional(n) if n <= version => "/*!",
                    Opener::Conditional(_) => "/* ",
                    Opener::MariaDb(n) if mariadb && n <= version => "/*!",
                    Opener::MariaDb(_) => "/*M!",
                });
                i += marker_len;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// Whether one statement of `text` (read as one of [`read_as`]) starts with `USE`, or holds a `;`
/// that makes it do. What the splitter leaves of a conditional comment in the code is its body and
/// its closing `*/`, which can only be a comment's (a `*/` outside a string or a comment closes
/// nothing else): blanked, `/*!50000 */ USE db` and `/*!*/USE db` start with `USE` as they do for
/// the server.
fn has_use_statement(text: &str, dialect: Dialect) -> bool {
    split_statements(text, dialect).iter().any(|statement| {
        code_only(&statement.sql, dialect)
            .replace("*/", " ")
            .split(';')
            .any(starts_with_use)
    })
}

/// What [`database_switch`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabaseSwitch {
    No,
    Yes,
    /// There are too many different conditional comments to read the text every way.
    Unverifiable,
}

/// Whether `sql` holds a MySQL / MariaDB `USE <db>` statement. A session is pooled between calls,
/// so the new database would stay for every later statement while the answer and the history still
/// name the one the connection opened on. `USE` is a read to the classifier, so the write gate does
/// not stop it. Statement by statement as the app splits them (a custom `DELIMITER` included); the
/// code of each is also cut at every `;`, since a conditional comment holds statements the
/// splitter cannot see.
///
/// The server reads the text in ways the splitter cannot know, so every one of them is tried and
/// one `USE` among them refuses the text:
///
/// - With and without backslash escapes in strings (`NO_BACKSLASH_ESCAPES`): a `; USE db` hidden
///   in a string under one reading is a statement under the other.
/// - `/*!NNNNN … */` runs as code only on a server of at least that version, and a comment whose
///   body is run or skipped changes where the next string or comment ends. One server version
///   decides all of them at once, so the text is read at each version a comment names (and below
///   all of them): `/*!99999 ' */ ; /*!50000 USE db */` is a comment and a `USE` on a server in
///   between. The text is also read as MariaDB does, where `/*M!NNNNNN … */` is code too.
/// - An optimizer hint `/*+ … */` is a comment to the server, but not to the splitter:
///   `/*+ ' */ ; USE db ; '` is a statement and a string to the splitter.
///
/// This copies the server's lexer, so it can only be a first line: it refuses the plain and the
/// common disguised forms early, with a clear message. What it cannot read is caught after the run
/// (`tools::run_query` asks the session which database it is on, and closes a session that moved
/// instead of pooling it).
fn database_switch(sql: &str, dialect: impl Into<SqlDialect>) -> DatabaseSwitch {
    let dialect = dialect.into();
    let mysql = match dialect.resolve() {
        DialectRef::Core(Dialect::Mysql) => Dialect::Mysql,
        DialectRef::Core(Dialect::Postgres | Dialect::Sqlite) => return DatabaseSwitch::No,
        // An extension dialect: the statements its spec reads as switching the database.
        DialectRef::Ext(spec) => {
            let switches = crate::sqlgen::sql::split_statements(dialect, sql)
                .iter()
                .any(|statement| (spec.knobs.switch_db)(&statement.sql));
            return if switches {
                DatabaseSwitch::Yes
            } else {
                DatabaseSwitch::No
            };
        }
    };
    let no_backslash = sql.replace('\\', " ");
    let mut seen: HashSet<String> = HashSet::new();
    for text in [sql, no_backslash.as_str()] {
        let chars: Vec<char> = text.chars().collect();
        let mut versions = comment_versions(&chars);
        if versions.len() > MAX_COMMENT_VERSIONS
            || (!versions.is_empty() && (versions.len() + 1) * chars.len() > MAX_READING_CHARS)
        {
            return DatabaseSwitch::Unverifiable;
        }
        versions.insert(0);
        for &version in &versions {
            for mariadb in [false, true] {
                let reading = read_as(&chars, mariadb, version);
                if seen.contains(&reading) {
                    continue;
                }
                if has_use_statement(&reading, mysql) {
                    return DatabaseSwitch::Yes;
                }
                seen.insert(reading);
            }
        }
    }
    DatabaseSwitch::No
}

/// Refuses `sql` (one or more statements) when one of them writes and writes are not allowed, and
/// a MySQL `USE` always (see [`database_switch`]).
pub fn check_query(
    profile: &ConnectionProfile,
    prefs: &McpPrefs,
    sql: &str,
    dialect: impl Into<SqlDialect>,
) -> Result<(), String> {
    let dialect = dialect.into();
    match database_switch(sql, dialect) {
        DatabaseSwitch::No => {}
        DatabaseSwitch::Yes => return Err(USE_REFUSED.to_string()),
        DatabaseSwitch::Unverifiable => return Err(COMMENTS_REFUSED.to_string()),
    }
    match write_block(profile, prefs) {
        Some(block) if run_writes(sql, dialect, None, false) => Err(format!(
            "Refused: this statement changes data or the server. {}",
            block.message(
                &profile.name,
                SessionCaps::for_profile(profile).safety.read_only
            )
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{Driver, ProfileOptions, SslMode};
    use uuid::Uuid;

    fn profile(
        tag: Option<ConnectionTag>,
        level: Option<SafeModeLevel>,
        ro: bool,
    ) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "db".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some("/tmp/x.sqlite".into()),
            ssh: None,
            options: ProfileOptions {
                tag,
                safe_mode: level,
                read_only: ro,
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    fn prefs(allow_writes: bool, expose_production: bool) -> McpPrefs {
        McpPrefs {
            enabled: true,
            allow_writes,
            expose_production,
            ..McpPrefs::default()
        }
    }

    const LEVELS: [Option<SafeModeLevel>; 6] = [
        None,
        Some(SafeModeLevel::Silent),
        Some(SafeModeLevel::Alert),
        Some(SafeModeLevel::AlertNonSelect),
        Some(SafeModeLevel::Safe),
        Some(SafeModeLevel::SafeNonSelect),
    ];

    #[test]
    fn write_gate_matrix() {
        let tags = [
            None,
            Some(ConnectionTag::Development),
            Some(ConnectionTag::Production),
            Some(ConnectionTag::Custom("prod-ish".into())),
        ];
        for allow in [false, true] {
            for level in LEVELS {
                for ro in [false, true] {
                    for tag in &tags {
                        let p = profile(tag.clone(), level, ro);
                        let pr = prefs(allow, true);
                        let silent = matches!(level, None | Some(SafeModeLevel::Silent));
                        let writable = allow && silent && !ro;
                        assert_eq!(
                            session_writable(&p, &pr),
                            writable,
                            "allow={allow} level={level:?} ro={ro} tag={tag:?}"
                        );
                        let insert =
                            check_query(&p, &pr, "INSERT INTO t VALUES (1)", Dialect::Sqlite);
                        assert_eq!(insert.is_ok(), writable);
                        // Reads always pass the gate.
                        assert!(
                            check_query(&p, &pr, "SELECT 1; SELECT 2", Dialect::Sqlite).is_ok()
                        );
                        // One write among reads refuses the whole text.
                        let mixed =
                            check_query(&p, &pr, "SELECT 1; DELETE FROM t", Dialect::Sqlite);
                        assert_eq!(mixed.is_ok(), writable);
                    }
                }
            }
        }
    }

    #[test]
    fn write_block_reasons_come_in_order() {
        let p = profile(None, Some(SafeModeLevel::Safe), true);
        assert_eq!(
            write_block(&p, &prefs(true, false)),
            Some(WriteBlock::ReadOnly)
        );
        let p = profile(None, Some(SafeModeLevel::Safe), false);
        assert_eq!(
            write_block(&p, &prefs(false, false)),
            Some(WriteBlock::PrefOff)
        );
        assert_eq!(
            write_block(&p, &prefs(true, false)),
            Some(WriteBlock::SafeMode(SafeModeLevel::Safe))
        );
        let err =
            check_query(&p, &prefs(true, false), "DROP TABLE t", Dialect::Sqlite).unwrap_err();
        assert!(err.contains("Safe Mode 1"), "{err}");
        let err = check_query(
            &p,
            &prefs(false, false),
            "UPDATE t SET a = 1",
            Dialect::Sqlite,
        )
        .unwrap_err();
        assert!(err.contains("Allow MCP clients to change data"), "{err}");
    }

    #[test]
    fn use_is_refused_for_mysql_whatever_the_connection_allows() {
        let dialect = Dialect::Mysql;
        let every_gate = [
            // Reads only, and writes allowed on a silent connection.
            (profile(None, None, false), prefs(false, false)),
            (
                profile(None, Some(SafeModeLevel::Silent), false),
                prefs(true, false),
            ),
            (
                profile(None, Some(SafeModeLevel::Silent), true),
                prefs(true, false),
            ),
            (
                profile(None, Some(SafeModeLevel::Safe), false),
                prefs(true, false),
            ),
        ];
        for (p, pr) in &every_gate {
            for sql in [
                "USE other",
                "use other",
                "  USE `other`;",
                "USE\n  other",
                "SELECT 1; USE other",
                "SELECT 1;\n-- next\nUSE other;",
                "/* hi */ USE other",
                "# c\nUSE other",
                "/*!50000 USE other */",
                "/*!USE other*/",
                "SELECT 1; /*!40101 USE other */",
                "SELECT '\\'; USE other; --'",
                "DELIMITER //\nUSE other//",
                "(SELECT 1); USE other",
                // MariaDB's executable comment: code there, a plain comment to the splitter.
                "SELECT 1 /*M!50000 ; USE other */",
                "SELECT 1; /*M!100100 USE other */",
                "/*M!USE other*/",
                "SELECT 1 /*m!50000 ; USE other */",
                "SELECT 1 /*M!50000 ;\n USE other */ ; SELECT 2",
                "DELIMITER //\nSELECT 1 /*M!50000 ; USE other *///",
                // An executable comment with no body, or one that ends before the `USE`: the `*/`
                // it leaves in the code is not the statement's first word.
                "/*!50000 */ USE other",
                "/*!*/USE other",
                "/*! */ USE other",
                "/*!50000 */\nUSE other",
                "SELECT 1; /*!*/ USE other",
                "/*!50000 */ /*!50000 USE other */",
                "/*!12345*/  USE other",
                "/*!50000 */ /*M!50000 */ USE other",
                "/*M!50000 */ USE other",
                "/*m!*/USE other",
                "/*!50000 /* c */ USE other */",
                "SELECT 1 /*!50000 */; USE other",
                // An optimizer hint is a comment to the server, wherever it stands.
                "/*+ x */ USE other",
                "/*+*/USE other",
                "SELECT 1; /*+ hint */ USE other",
                "/*+ NO_ICP(t) */ /*!50000 */ USE other",
                "/*+ ' */ ; USE other ; '",
                "/*+ # */ USE other",
                // The server's version decides which conditional comments run, so a quote or a
                // line comment in one of them must not hide a `USE` from the version that skips it.
                "/*!99999 ' */ ; /*!50000 USE other */",
                "/*!99999 # */ ; USE other",
                "/*!99999 -- */ ; USE other",
                "SELECT 1 /*!50000 ; */ /*!99999 FOO */ /*!99998 BAR */ USE other",
                "/*!99999 ' */ ; /*M!50000 USE other */",
                "/*M!99999 ' */ ; USE other ; '",
                "/*!99999 ' */ /*!99998 ' */ /*!99997 ' */ ; USE other",
            ] {
                let err = check_query(p, pr, sql, dialect).expect_err(sql);
                assert_eq!(err, USE_REFUSED, "{sql}");
            }
        }
        assert!(USE_REFUSED.starts_with("USE is not supported over MCP"));
        assert!(USE_REFUSED.ends_with("Pass the database argument of run_query instead."));
    }

    #[test]
    fn statements_that_only_mention_use_still_pass() {
        let p = profile(None, None, false);
        let pr = prefs(false, false);
        for sql in [
            "SELECT 1",
            "SELECT 1; SELECT 2",
            "SELECT * FROM use_log",
            "SELECT * FROM `use`",
            "SELECT 'USE other'",
            "SELECT 1 -- USE other",
            "/* USE other */ SELECT 1",
            "SELECT * FROM t USE INDEX (i)",
            "SELECT * FROM t WHERE note = 'x; USE other'",
            "SELECT used FROM users",
            "SELECT 1; SELECT user FROM mysql.user",
            "SHOW DATABASES",
            "SELECT 1 /*M!50000 , 2 */",
            "SELECT 1 /*M! -- USE other */",
            "SELECT '/*M!50000 ; USE other */'",
            "SELECT 1 /*!50000 */",
            "SELECT 1 /*!50000 , 2 */",
            "SELECT /*+ NO_ICP(t) */ 1",
            "SELECT 1 /*+ x */",
            // A hint is a comment to the server: what is in it never runs.
            "SELECT /*+ ; USE other */ 1",
            "/*+ USE other */ SELECT 1",
            "SELECT 1 /*!50000 */ -- USE other",
        ] {
            assert!(check_query(&p, &pr, sql, Dialect::Mysql).is_ok(), "{sql}");
        }
    }

    /// Conditional comments and hints with nothing in them that starts a statement are no `USE`.
    /// (The write classifier reads a statement that opens with one as a write, so only the USE
    /// check is asked here: `SELECT 1; /*!50000 */ SELECT 2` stays a refusal on a read-only
    /// connection for that reason, not for a `USE`.)
    #[test]
    fn conditional_comments_that_start_no_statement_are_no_use() {
        for sql in [
            "/*!50000 */ SELECT 1",
            "SELECT 1 /*!50000 */",
            "/*!*/ SELECT 1",
            "SELECT 1; /*!50000 */ SELECT 2",
            "/*!40101 SET @a = 1 */; SELECT 1",
            "SELECT 1 /*!50000 , 2 */",
            "/*!50000 */ /*!50000 */ SELECT 1",
            "SELECT /*+ NO_ICP(t) */ 1",
            "SELECT 1 /*+ x */",
            "/*+ x */ SELECT 1",
            "SELECT /*+ ; USE other */ 1",
            "/*+ USE other */ SELECT 1",
            "/*+ ' */ SELECT 1 -- '",
            "SELECT 1 /*!50000 */ -- USE other",
            "/*!99999 ' */ SELECT 1 /*!50000 ' */",
            "SELECT * FROM t /*!50000 USE INDEX (i) */",
        ] {
            assert_eq!(
                database_switch(sql, Dialect::Mysql),
                DatabaseSwitch::No,
                "{sql}"
            );
        }
    }

    #[test]
    fn conditional_comments_are_read_at_every_version_they_name() {
        // `/*!99999 … */` is a comment on any server below that version, and `/*M!` is one on
        // MySQL: each text below is a `USE` only at the versions in the second column.
        let chars = |s: &str| s.chars().collect::<Vec<char>>();
        let text = chars("/*!99999 A */ /*!50000 B */ /*M!60000 C */ /*+ D */ /*!E */");
        assert_eq!(
            comment_versions(&text).into_iter().collect::<Vec<_>>(),
            vec![0, 50000, 60000, 99999]
        );
        assert_eq!(
            read_as(&text, false, 0),
            "/* 99999 A */ /* 50000 B */ /*M!60000 C */ /*  D */ /*!E */"
        );
        assert_eq!(
            read_as(&text, false, 50000),
            "/* 99999 A */ /*!50000 B */ /*M!60000 C */ /*  D */ /*!E */"
        );
        assert_eq!(
            read_as(&text, true, 60000),
            "/* 99999 A */ /*!50000 B */ /*!60000 C */ /*  D */ /*!E */"
        );
        assert_eq!(
            read_as(&text, true, 99999),
            "/*!99999 A */ /*!50000 B */ /*!60000 C */ /*  D */ /*!E */"
        );
        // Not an opener: left alone.
        let plain = chars("/* a */ /*/ b */ /*M c */ / * ! 5");
        assert_eq!(
            read_as(&plain, true, u64::MAX),
            plain.iter().collect::<String>()
        );
        // A version too long for a number saturates, and no digits is version 0.
        let long = chars("/*!99999999999999999999999999 */");
        assert_eq!(
            comment_versions(&long).into_iter().collect::<Vec<_>>(),
            vec![u64::MAX]
        );
    }

    #[test]
    fn too_many_different_versions_are_refused_not_skipped() {
        let with = |n: u64| {
            let comments: String = (1..=n).map(|v| format!("/*!{} */", 40000 + v)).collect();
            format!("SELECT 1 {comments}")
        };
        let p = profile(None, None, false);
        let pr = prefs(false, false);
        assert!(check_query(&p, &pr, &with(MAX_COMMENT_VERSIONS as u64), Dialect::Mysql).is_ok());
        assert_eq!(
            check_query(
                &p,
                &pr,
                &with(MAX_COMMENT_VERSIONS as u64 + 1),
                Dialect::Mysql
            ),
            Err(COMMENTS_REFUSED.to_string())
        );
        // A long text is read at fewer versions: 4 versions of a text of 1 MiB are too much work.
        let long = |versions: u64| {
            let comments: String = (1..=versions)
                .map(|v| format!("/*!{} */", 50000 + v))
                .collect();
            format!("{}SELECT 1 {comments}", "SELECT 1;\n".repeat(100_000))
        };
        // (The USE check alone: the write classifier would spend most of the time on the text.)
        assert_eq!(
            database_switch(&long(1), Dialect::Mysql),
            DatabaseSwitch::No
        );
        assert_eq!(
            database_switch(&long(8), Dialect::Mysql),
            DatabaseSwitch::Unverifiable
        );
        // Without conditional comments a text of any length is read once.
        let plain = "SELECT 1;\n".repeat(450_000);
        assert_eq!(database_switch(&plain, Dialect::Mysql), DatabaseSwitch::No);
        // The same version over and over is one version.
        let same = format!("SELECT 1 {}", "/*!50000 */".repeat(500));
        assert!(check_query(&p, &pr, &same, Dialect::Mysql).is_ok());
        // Other servers have no such comments.
        for dialect in [Dialect::Postgres, Dialect::Sqlite] {
            let many = with(MAX_COMMENT_VERSIONS as u64 + 5);
            assert!(check_query(&p, &pr, &many, dialect).is_ok());
        }
    }

    #[test]
    fn use_is_not_special_for_postgres_and_sqlite() {
        let p = profile(None, None, false);
        let pr = prefs(false, false);
        for dialect in [Dialect::Postgres, Dialect::Sqlite] {
            // The classifier treats USE as a read, and nothing else changes for these dialects.
            assert_eq!(database_switch("USE other", dialect), DatabaseSwitch::No);
            assert!(check_query(&p, &pr, "USE other", dialect).is_ok());
            assert!(check_query(&p, &pr, "SELECT 1; USE other", dialect).is_ok());
        }
    }

    #[test]
    fn an_extension_dialect_switches_databases_as_its_spec_says() {
        use crate::sqlgen::dialect::testing::override_spec;
        use crate::sqlgen::portable::testing::ANSI_TEST;

        let _vertica = override_spec(SqlDialect::Vertica, &ANSI_TEST);
        let v = SqlDialect::Vertica;
        let p = profile(None, None, false);
        let pr = prefs(false, false);
        // The spec's `switch_db` reads each statement its splitter finds.
        assert_eq!(
            database_switch("SELECT 1; USE other", v),
            DatabaseSwitch::Yes
        );
        assert_eq!(
            check_query(&p, &pr, "use other", v).unwrap_err(),
            USE_REFUSED
        );
        assert_eq!(
            database_switch("SELECT 'x; USE other'", v),
            DatabaseSwitch::No
        );
        assert!(check_query(&p, &pr, "SELECT 1; SELECT 2", v).is_ok());
        // Its classifier decides the writes.
        assert!(check_query(&p, &pr, "DELETE FROM t", v).is_err());
    }

    #[test]
    fn the_read_only_message_says_when_only_the_classifier_enforces_it() {
        use crate::sqlgen::dialect::testing::override_spec;
        use crate::sqlgen::portable::testing::ANSI_TEST;

        let pr = prefs(true, false);
        let core = check_query(
            &profile(None, None, true),
            &pr,
            "DELETE FROM t",
            Dialect::Sqlite,
        )
        .unwrap_err();
        assert!(
            core.ends_with("is read-only: MCP clients can only read from it."),
            "{core}"
        );
        let _vertica = override_spec(SqlDialect::Vertica, &ANSI_TEST);
        let mut vertica = profile(None, None, true);
        vertica.driver = crate::types::profile::Driver::Vertica;
        let err = check_query(&vertica, &pr, "DELETE FROM t", SqlDialect::Vertica).unwrap_err();
        assert!(err.contains("Its server does not enforce that"), "{err}");
    }

    #[test]
    fn production_connections_are_hidden_unless_exposed() {
        for level in LEVELS {
            for ro in [false, true] {
                let prod = profile(Some(ConnectionTag::Production), level, ro);
                assert!(!exposed(&prod, &prefs(true, false)));
                assert!(exposed(&prod, &prefs(false, true)));
                for tag in [
                    None,
                    Some(ConnectionTag::Staging),
                    Some(ConnectionTag::Custom("production-like".into())),
                ] {
                    let p = profile(tag, level, ro);
                    assert!(exposed(&p, &prefs(false, false)));
                }
            }
        }
    }

    #[test]
    fn classifier_cases_that_matter_for_mcp() {
        let p = profile(None, None, false);
        let pr = prefs(false, false);
        for read in [
            "SELECT * FROM t",
            "  -- comment\nselect 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "EXPLAIN SELECT 1",
            "PRAGMA table_info(t)",
        ] {
            assert!(
                check_query(&p, &pr, read, Dialect::Sqlite).is_ok(),
                "{read}"
            );
        }
        for write in [
            "CREATE TABLE t (a int)",
            "SELECT 1 INTO x",
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
            "PRAGMA journal_mode = DELETE",
            "SET search_path = x",
            "SELECT set_config('default_transaction_read_only', 'off', false)",
            "BEGIN",
        ] {
            assert!(
                check_query(&p, &pr, write, Dialect::Postgres).is_err(),
                "{write}"
            );
        }
    }
}
