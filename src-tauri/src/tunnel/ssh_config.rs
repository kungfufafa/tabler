//! A small `~/.ssh/config` reader for the SSH tunnel: the `HostName`, `User`, `Port`,
//! `IdentityFile`, `IdentityAgent`, `ProxyJump` and `ProxyCommand` that OpenSSH would use for a
//! host (alias). Values typed in the connection form win; the config only fills what the form
//! leaves empty (TablePlus does the same).
//!
//! Supported: `Host` blocks with `*` / `?` patterns and `!` negations, settings before the first
//! block (they apply to every host), `Key value` and `Key=value`, quoted values, `#` comments and
//! the `%h %d %u %r %%` tokens (`%p %n` too in `ProxyCommand`). `ProxyJump` and `ProxyCommand`
//! compete: whichever comes first wins, and `none` turns both off (OpenSSH's rules).
//!
//! `Include` is followed where it stands, so the first value found still wins: an `Include`
//! before the first `Host` line applies to every host, one inside a `Host` / `Match` block is part
//! of that block (and is not read at all when the block does not apply). Relative paths are taken
//! from `~/.ssh`, `~/` from the home directory; `*`, `?` and `[...]` patterns expand in lexical
//! order (a leading `.` is not matched by a wildcard, as in glob(3)); several patterns may follow
//! one `Include`. As in OpenSSH, the `Host` / `Match` lines of an included file do not carry over
//! to the file that included it, and nesting stops at depth 16. A cycle, a missing or unreadable
//! file, or a directory is skipped (logged at debug level); so are the files beyond the 256th a
//! lookup would read (a snippet that includes itself twice per level would read 2^16 files).
//!
//! `Match` blocks are evaluated for `all`, `host` (the `HostName` found so far, else the alias),
//! `originalhost` (the alias as typed), `user` (the SSH user of the form, else a `User` found so
//! far, else the local user) and `localuser`, each optionally negated (`!host`), with comma
//! separated pattern lists (`!` negates a pattern) and several criteria on one line (all must
//! hold). The other criteria (`exec`, `localnetwork`, `canonical`, `final`, `tagged`, `command`,
//! `sessiontype`, ...) cannot be evaluated here, so a block that uses one never matches.

use std::path::{Component, Path, PathBuf};

/// Include nesting OpenSSH allows (`READCONF_MAX_DEPTH`).
const MAX_INCLUDE_DEPTH: usize = 16;
/// Files one lookup reads through `Include` (diamond-shaped includes multiply).
const MAX_INCLUDED_FILES: usize = 256;

/// How the SSH server is reached: directly, through jump hosts, or through a command's stdio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxy {
    /// `ProxyJump a,b`: connect to `a`, then to `b` through `a`, then to the target through `b`.
    Jump(Vec<JumpHost>),
    /// `ProxyCommand …`: the raw command line; tokens are expanded at connect time
    /// ([`expand_proxy_command`]).
    Command(String),
}

/// One hop of a `ProxyJump` list: `[user@]host[:port]` or `ssh://[user@]host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpHost {
    pub user: Option<String>,
    /// A host name, address or `~/.ssh/config` alias (its own HostName / User / Port apply).
    pub host: String,
    pub port: Option<u16>,
}

/// `IdentityAgent`: which SSH agent socket key authentication uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityAgent {
    /// `none`: no agent.
    Disabled,
    /// `SSH_AUTH_SOCK` or `$VAR`: the socket path in that environment variable.
    Env(String),
    /// A socket path (tokens and `~` already expanded), e.g. 1Password's agent.
    Path(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostConfig {
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    pub identity_agent: Option<IdentityAgent>,
    /// `None`: a direct connection (no proxy set, or `none`).
    pub proxy: Option<Proxy>,
}

/// Where the config lives: the main file, the directory relative `Include` paths start from
/// (`~/.ssh`) and the directory `~` stands for.
#[derive(Debug, Clone)]
pub struct ConfigFiles {
    pub config: PathBuf,
    pub ssh_dir: PathBuf,
    pub home: Option<PathBuf>,
}

impl ConfigFiles {
    /// The user's `~/.ssh/config`, if there is a home directory.
    pub fn user() -> Option<Self> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)?;
        let ssh_dir = home.join(".ssh");
        Some(Self {
            config: ssh_dir.join("config"),
            ssh_dir,
            home: Some(home),
        })
    }
}

/// The settings of `~/.ssh/config` that apply to `host` (nothing when the file is missing).
pub fn for_host(host: &str) -> HostConfig {
    for_host_as(host, None)
}

/// [`for_host`] for a connection that signs in as `user` (what `Match user` tests; the local
/// user when `None`).
pub fn for_host_as(host: &str, user: Option<&str>) -> HostConfig {
    match ConfigFiles::user() {
        Some(files) => for_host_in(&files, host, user),
        None => HostConfig::default(),
    }
}

/// [`for_host_as`] with explicit locations (tests; no `HOME` involved).
pub fn for_host_in(files: &ConfigFiles, host: &str, user: Option<&str>) -> HostConfig {
    match std::fs::read_to_string(&files.config) {
        Ok(text) => {
            let mut resolver = Resolver::new(host, user, Some(files));
            if let Ok(main) = files.config.canonicalize() {
                resolver.stack.push(main);
            }
            resolver.read(&text, 0);
            resolver.finish()
        }
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("cannot read {}: {e}", files.config.display());
            }
            HostConfig::default()
        }
    }
}

/// Splits a config line into its keyword, its raw argument text (`ProxyCommand` takes it as
/// is) and its arguments (`Key value…` or `Key=value…`).
fn split_line(line: &str) -> Option<(String, String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..end].to_ascii_lowercase();
    let mut rest = line[end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    Some((keyword, rest.to_string(), split_args(rest)))
}

/// Whitespace-separated arguments; double quotes group words.
fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut current));
                    has = false;
                }
            }
            c => {
                current.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(current);
    }
    out
}

/// One element of a parsed wildcard pattern.
enum Piece {
    /// `*`: any run of characters.
    Star,
    /// `?`: one character.
    Any,
    Lit(char),
    /// `[abc]`, `[a-z]`, `[!abc]`: one character from (or, negated, not from) the ranges.
    Set {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl Piece {
    fn accepts(&self, c: char) -> bool {
        match self {
            Piece::Star => true,
            Piece::Any => true,
            Piece::Lit(l) => *l == c,
            Piece::Set { negated, ranges } => {
                ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi) != *negated
            }
        }
    }
}

/// The bracket expression starting at `chars[start] == '['`, and the index after its `]`
/// (`None`: no closing bracket, the `[` is a plain character).
fn parse_set(chars: &[char], start: usize) -> Option<(Piece, usize)> {
    let mut i = start + 1;
    let negated = matches!(chars.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let first = i;
    let mut ranges = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        // A `]` right after the opening bracket is a member, not the end.
        if c == ']' && i > first {
            return Some((Piece::Set { negated, ranges }, i + 1));
        }
        match (chars.get(i + 1), chars.get(i + 2)) {
            (Some('-'), Some(end)) if *end != ']' => {
                ranges.push((c, *end));
                i += 3;
            }
            _ => {
                ranges.push((c, c));
                i += 1;
            }
        }
    }
    None
}

fn parse_pattern(pattern: &str, fold: bool, brackets: bool) -> Vec<Piece> {
    let chars: Vec<char> = if fold {
        pattern.to_lowercase().chars().collect()
    } else {
        pattern.chars().collect()
    };
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push(Piece::Star),
            '?' => out.push(Piece::Any),
            '[' if brackets => {
                if let Some((set, next)) = parse_set(&chars, i) {
                    out.push(set);
                    i = next;
                    continue;
                }
                out.push(Piece::Lit('['));
            }
            c => out.push(Piece::Lit(c)),
        }
        i += 1;
    }
    out
}

/// Wildcard match: `*` (any run), `?` (one character) and, with `brackets`, `[...]`; `fold`
/// ignores the case. (`Host` / `Match` patterns have no brackets, file names do.)
fn wildcard(pattern: &str, text: &str, fold: bool, brackets: bool) -> bool {
    let p = parse_pattern(pattern, fold, brackets);
    let t: Vec<char> = if fold {
        text.to_lowercase().chars().collect()
    } else {
        text.chars().collect()
    };
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some(Piece::Star) => {
                star = Some((pi, ti));
                pi += 1;
                continue;
            }
            Some(piece) if piece.accepts(t[ti]) => {
                pi += 1;
                ti += 1;
                continue;
            }
            _ => {}
        }
        if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|piece| matches!(piece, Piece::Star))
}

/// Whether a pattern list selects `text`: one pattern matches and no negated (`!`) one does.
fn patterns_match(patterns: &[String], text: &str, fold: bool) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if wildcard(negated, text, fold, false) {
                return false;
            }
        } else if wildcard(pattern, text, fold, false) {
            matched = true;
        }
    }
    matched
}

/// Whether a `Host` line's patterns select `host`: one pattern matches and no negated one does.
fn host_matches(patterns: &[String], host: &str) -> bool {
    patterns_match(patterns, host, true)
}

fn local_user() -> Option<String> {
    ["USER", "LOGNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.is_empty()))
}

/// Expands `%h` (the host name), `%d` (home), `%u` (local user), `%r` (remote user), `%%`.
fn expand_tokens(value: &str, host: &str, user: Option<&str>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(host),
            Some('d') => out.push_str(&std::env::var("HOME").unwrap_or_default()),
            Some('u') => out.push_str(&local_user().unwrap_or_default()),
            Some('r') => out.push_str(user.unwrap_or_default()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The settings of config `text` that apply to `host`: for each keyword the first value found
/// wins (OpenSSH's rule); every `IdentityFile` is collected. `Include` lines are ignored (there
/// is no `~/.ssh` to resolve them against: see [`for_host_in`]).
pub fn resolve(text: &str, host: &str) -> HostConfig {
    resolve_as(text, host, None)
}

/// [`resolve`] for a connection that signs in as `user` (what `Match user` tests).
pub fn resolve_as(text: &str, host: &str, user: Option<&str>) -> HostConfig {
    let mut resolver = Resolver::new(host, user, None);
    resolver.read(text, 0);
    resolver.finish()
}

/// Reads config text line by line, following `Include`, and keeps the first value of each keyword.
struct Resolver<'a> {
    /// The alias as typed.
    host: &'a str,
    /// The SSH user of the connection form.
    form_user: Option<&'a str>,
    /// Where `Include` paths start from; `None`: `Include` is ignored.
    files: Option<&'a ConfigFiles>,
    out: HostConfig,
    raw_host_name: Option<String>,
    raw_identities: Vec<String>,
    raw_agent: Option<String>,
    /// The first of ProxyJump / ProxyCommand decides (`none` included: it turns both off).
    raw_proxy: Option<(bool, String)>,
    /// Whether the lines being read apply to `host`. Settings before the first Host / Match apply
    /// to every host.
    active: bool,
    /// The files being read, outermost first (canonical paths): the cycle check.
    stack: Vec<PathBuf>,
    /// Included files read so far.
    included: usize,
}

impl<'a> Resolver<'a> {
    fn new(host: &'a str, form_user: Option<&'a str>, files: Option<&'a ConfigFiles>) -> Self {
        Self {
            host,
            form_user,
            files,
            out: HostConfig::default(),
            raw_host_name: None,
            raw_identities: Vec::new(),
            raw_agent: None,
            raw_proxy: None,
            active: true,
            stack: Vec::new(),
            included: 0,
        }
    }

    /// Reads `text`, the file at include nesting `depth` (0: the main file).
    fn read(&mut self, text: &str, depth: usize) {
        for line in text.lines() {
            let Some((keyword, raw, args)) = split_line(line) else {
                continue;
            };
            match keyword.as_str() {
                "host" => {
                    self.active = host_matches(&args, self.host);
                    continue;
                }
                "match" => {
                    self.active = self.match_applies(&args);
                    continue;
                }
                // An inactive block's includes cannot switch anything on (OpenSSH reads them
                // "parse only"), so they are not read.
                "include" => {
                    if self.active {
                        self.include(&args, depth);
                    }
                    continue;
                }
                _ => {}
            }
            if !self.active {
                continue;
            }
            let Some(value) = args.first().cloned() else {
                continue;
            };
            match keyword.as_str() {
                "hostname" if self.raw_host_name.is_none() => self.raw_host_name = Some(value),
                "user" if self.out.user.is_none() => self.out.user = Some(value),
                "port" if self.out.port.is_none() => {
                    self.out.port = value.parse().ok().filter(|p| *p != 0)
                }
                "identityfile" => self.raw_identities.push(value),
                "identityagent" if self.raw_agent.is_none() => self.raw_agent = Some(value),
                "proxyjump" if self.raw_proxy.is_none() => {
                    self.raw_proxy = Some((true, args.join(",")))
                }
                "proxycommand" if self.raw_proxy.is_none() => {
                    self.raw_proxy = Some((false, raw.trim().into()))
                }
                _ => {}
            }
        }
    }

    /// `Include pattern...` met at nesting `depth`: reads each matching file where the line is.
    fn include(&mut self, patterns: &[String], depth: usize) {
        let Some(files) = self.files else {
            return;
        };
        if depth + 1 > MAX_INCLUDE_DEPTH {
            log::debug!("ssh config: Include nested deeper than {MAX_INCLUDE_DEPTH}, skipped");
            return;
        }
        // The Host / Match lines of an included file must not decide how the including file goes
        // on (OpenSSH restores the state after each file).
        let outer = self.active;
        for pattern in patterns {
            for path in include_paths(pattern, files) {
                self.include_file(&path, depth);
                self.active = outer;
            }
        }
    }

    fn include_file(&mut self, path: &Path, depth: usize) {
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                log::debug!("ssh config: cannot include {}: {e}", path.display());
                return;
            }
        };
        if !canonical.is_file() {
            log::debug!("ssh config: {} is not a file, skipped", canonical.display());
            return;
        }
        if self.stack.contains(&canonical) {
            log::debug!(
                "ssh config: {} includes itself, skipped",
                canonical.display()
            );
            return;
        }
        if self.included >= MAX_INCLUDED_FILES {
            log::debug!("ssh config: more than {MAX_INCLUDED_FILES} included files, skipped");
            return;
        }
        self.included += 1;
        let text = match std::fs::read_to_string(&canonical) {
            Ok(t) => t,
            Err(e) => {
                log::debug!("ssh config: cannot read {}: {e}", canonical.display());
                return;
            }
        };
        self.stack.push(canonical);
        self.read(&text, depth + 1);
        self.stack.pop();
    }

    /// Whether a `Match` line selects the host: every criterion must hold, an unsupported one
    /// (or a missing argument) never does.
    fn match_applies(&self, args: &[String]) -> bool {
        if args.is_empty() {
            return false;
        }
        let mut i = 0;
        while i < args.len() {
            let (negated, name) = match args[i].strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, args[i].as_str()),
            };
            i += 1;
            let key = name.to_ascii_lowercase();
            let holds = match key.as_str() {
                "all" => true,
                "host" | "originalhost" | "user" | "localuser" => {
                    let Some(list) = args.get(i) else {
                        return false;
                    };
                    i += 1;
                    let patterns: Vec<String> = list
                        .split(',')
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect();
                    match key.as_str() {
                        "host" => patterns_match(&patterns, &self.current_host(), true),
                        "originalhost" => patterns_match(&patterns, self.host, true),
                        "user" => patterns_match(&patterns, &self.current_user(), false),
                        _ => patterns_match(&patterns, &local_user().unwrap_or_default(), false),
                    }
                }
                // exec, localnetwork, canonical, final, tagged, command, sessiontype, ...
                _ => return false,
            };
            if holds == negated {
                return false;
            }
        }
        true
    }

    /// What `Match host` tests: the `HostName` found so far, else the alias.
    fn current_host(&self) -> String {
        match &self.raw_host_name {
            Some(h) => expand_tokens(h, self.host, None),
            None => self.host.to_string(),
        }
    }

    /// What `Match user` tests: the form's user, else a `User` found so far, else the local user.
    fn current_user(&self) -> String {
        self.form_user
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or_else(|| self.out.user.clone())
            .or_else(local_user)
            .unwrap_or_default()
    }

    fn finish(self) -> HostConfig {
        let Resolver {
            host,
            mut out,
            raw_host_name,
            raw_identities,
            raw_agent,
            raw_proxy,
            ..
        } = self;
        out.host_name = raw_host_name.map(|h| expand_tokens(&h, host, None));
        let real_host = out.host_name.clone().unwrap_or_else(|| host.to_string());
        out.identity_files = raw_identities
            .iter()
            .map(|f| expand_tokens(f, &real_host, out.user.as_deref()))
            .collect();
        out.identity_agent = raw_agent.map(|a| identity_agent(&a, &real_host, out.user.as_deref()));
        out.proxy = match raw_proxy {
            Some((_, v)) if v.eq_ignore_ascii_case("none") || v.is_empty() => None,
            Some((true, v)) => {
                let hops = parse_proxy_jump(&v);
                (!hops.is_empty()).then_some(Proxy::Jump(hops))
            }
            Some((false, v)) => Some(Proxy::Command(v)),
            None => None,
        };
        out
    }
}

/// The files an `Include` argument names: `~` and `~/` from the home directory, a relative path
/// from `~/.ssh`, wildcards expanded in lexical order.
fn include_paths(pattern: &str, files: &ConfigFiles) -> Vec<PathBuf> {
    let path = if let Some(rest) = pattern.strip_prefix('~') {
        match (&files.home, rest) {
            (Some(home), "") => home.clone(),
            (Some(home), rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
            // `~user/...`, or no home directory.
            _ => {
                log::debug!("ssh config: cannot expand the Include path {pattern}");
                return Vec::new();
            }
        }
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        files.ssh_dir.join(pattern)
    };
    expand_glob(&path)
}

fn has_wildcard(name: &str) -> bool {
    name.contains(['*', '?', '['])
}

/// `path` with the wildcards of its components expanded (`*`, `?`, `[...]`, case-sensitive, a
/// leading `.` only matched by a pattern that starts with one), each directory's entries in
/// lexical order. A path without wildcards comes back as it is, existing or not.
fn expand_glob(path: &Path) -> Vec<PathBuf> {
    let mut found = vec![PathBuf::new()];
    for component in path.components() {
        let pattern = match component {
            Component::Normal(name) => name.to_str().filter(|n| has_wildcard(n)),
            _ => None,
        };
        let Some(pattern) = pattern else {
            for p in &mut found {
                p.push(component.as_os_str());
            }
            continue;
        };
        let mut next = Vec::new();
        for dir in &found {
            let listed = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir.as_path()
            };
            let Ok(entries) = std::fs::read_dir(listed) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.') || pattern.starts_with('.'))
                .filter(|n| wildcard(pattern, n, false, true))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| dir.join(n)));
        }
        found = next;
        if found.is_empty() {
            break;
        }
    }
    found
}

/// An `IdentityAgent` value: `none`, `SSH_AUTH_SOCK`, `$VAR` or a socket path (`~` and the
/// `%d %h %u %r` tokens expanded).
fn identity_agent(value: &str, host: &str, user: Option<&str>) -> IdentityAgent {
    if value.eq_ignore_ascii_case("none") {
        return IdentityAgent::Disabled;
    }
    if value == "SSH_AUTH_SOCK" {
        return IdentityAgent::Env("SSH_AUTH_SOCK".into());
    }
    if let Some(var) = value.strip_prefix('$').filter(|v| !v.is_empty()) {
        return IdentityAgent::Env(var.trim_matches(|c| c == '{' || c == '}').to_string());
    }
    IdentityAgent::Path(expand_tokens(value, host, user))
}

impl IdentityAgent {
    /// The socket path to connect to; `None` for `none` or an unset variable.
    pub fn socket(&self) -> Option<String> {
        match self {
            IdentityAgent::Disabled => None,
            IdentityAgent::Env(var) => std::env::var(var).ok().filter(|v| !v.is_empty()),
            IdentityAgent::Path(p) => Some(p.clone()),
        }
    }
}

/// Parses a `ProxyJump` list (`a,user@b:2222,ssh://c`); entries that are not a host are skipped.
pub fn parse_proxy_jump(value: &str) -> Vec<JumpHost> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(parse_jump_host)
        .collect()
}

fn parse_jump_host(spec: &str) -> Option<JumpHost> {
    let spec = spec.strip_prefix("ssh://").unwrap_or(spec);
    let spec = spec.trim_end_matches('/');
    let (user, rest) = match spec.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()).filter(|u| !u.is_empty()), r),
        None => (None, spec),
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        // [IPv6]:port
        let (addr, after) = inner.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p.parse::<u16>().ok()?),
            None if after.is_empty() => None,
            None => return None,
        };
        (addr.to_string(), port)
    } else {
        match rest.rsplit_once(':') {
            // A bare IPv6 address has several colons and no port.
            Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p.parse::<u16>().ok()?)),
            _ => (rest.to_string(), None),
        }
    };
    if host.is_empty() {
        return None;
    }
    Some(JumpHost {
        user,
        host,
        port: port.filter(|p| *p != 0),
    })
}

/// What `ProxyCommand` tokens stand for.
pub struct ProxyTokens<'a> {
    /// `%h`: the host name connected to (after `HostName`).
    pub host: &'a str,
    /// `%p`.
    pub port: u16,
    /// `%r`: the remote user.
    pub user: &'a str,
    /// `%n`: the host as typed (the alias).
    pub alias: &'a str,
}

/// Longest value a `%h` / `%r` / `%n` token may have (a DNS name is at most 253 characters).
const PROXY_TOKEN_MAX: usize = 255;

/// Characters besides letters and digits that a value substituted into a `ProxyCommand` may
/// contain: host names, IPv4 / IPv6 addresses with a zone (`fe80::1%eth0`) and user names
/// (`user@domain`, `first+tag`, `a=b`, `a,b`).
const PROXY_TOKEN_EXTRA: &str = "._-:@+=,%";

/// Whether `value` is safe to paste into a `ProxyCommand` that runs through `/bin/sh -c`: the SSH
/// host, user and alias come from the profile (an imported file or a link can set them) and
/// `~/.ssh/config` patterns match them as typed, so `x$(cmd).corp` would otherwise run `cmd`
/// before any host key or password check. The value must be non-empty, at most 255 characters,
/// must not start with `-` (it would be taken for an option) and may only contain letters
/// (Unicode), digits and `. _ - : @ + = , %`: whitespace, control characters and every shell
/// metacharacter are refused. The value is not shell-quoted instead because templates may
/// already quote the token (`"%h"`). `what` names the value in the message (`SSH host`).
pub fn check_proxy_token(what: &str, value: &str) -> Result<(), String> {
    let shown = printable_prefix(value);
    let tail = "cannot be used with the ProxyCommand from ~/.ssh/config";
    if value.is_empty() {
        return Err(format!("The {what} is empty and {tail}"));
    }
    if value.chars().count() > PROXY_TOKEN_MAX {
        return Err(format!(
            "The {what} {shown} is longer than {PROXY_TOKEN_MAX} characters and {tail}"
        ));
    }
    if value.starts_with('-') {
        return Err(format!(
            "The {what} {shown} starts with a dash, which the ProxyCommand would take for an \
             option, and {tail}"
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_alphanumeric() || PROXY_TOKEN_EXTRA.contains(c))
    {
        return Err(format!(
            "The {what} {shown} {tail}: it contains characters the shell would interpret"
        ));
    }
    Ok(())
}

/// `value` for an error message: control characters removed, cut to 60 characters, quoted.
fn printable_prefix(value: &str) -> String {
    let clean: String = value.chars().filter(|c| !c.is_control()).collect();
    let mut shown: String = clean.chars().take(60).collect();
    if clean.chars().count() > 60 {
        shown.push('\u{2026}');
    }
    format!("\u{201c}{shown}\u{201d}")
}

/// Which of `%h %r %n` a `ProxyCommand` uses (a `%%` is a literal percent sign, as in
/// [`expand_proxy_command`]).
fn used_tokens(command: &str) -> [bool; 3] {
    let mut used = [false; 3];
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        match chars.next() {
            Some('h') => used[0] = true,
            Some('r') => used[1] = true,
            Some('n') => used[2] = true,
            _ => {}
        }
    }
    used
}

/// Refuses to expand `command` when a token it uses (`%h`, `%r`, `%n`) holds something the shell
/// would interpret ([`check_proxy_token`]); the error is the message to show.
pub fn check_proxy_command(command: &str, t: &ProxyTokens<'_>) -> Result<(), String> {
    let [host, user, alias] = used_tokens(command);
    if host {
        check_proxy_token("SSH host", t.host)?;
    }
    if user {
        check_proxy_token("SSH user", t.user)?;
    }
    if alias {
        check_proxy_token("SSH host alias", t.alias)?;
    }
    Ok(())
}

/// Expands `%h %p %r %n %d %u %%` in a `ProxyCommand`. The caller checks the tokens first
/// ([`check_proxy_command`]): they are pasted as they are.
pub fn expand_proxy_command(command: &str, t: &ProxyTokens<'_>) -> String {
    let mut out = String::with_capacity(command.len());
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(t.host),
            Some('p') => out.push_str(&t.port.to_string()),
            Some('r') => out.push_str(t.user),
            Some('n') => out.push_str(t.alias),
            Some('d') => out.push_str(&std::env::var("HOME").unwrap_or_default()),
            Some('u') => out.push_str(&local_user().unwrap_or_default()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The local user name, OpenSSH's default remote user.
pub fn default_user() -> Option<String> {
    local_user()
}

/// The keys OpenSSH tries when nothing names one, in the order checked here.
pub const DEFAULT_IDENTITIES: [&str; 3] = ["~/.ssh/id_ed25519", "~/.ssh/id_ecdsa", "~/.ssh/id_rsa"];

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
# global defaults
ServerAliveInterval 30

Host bastion prod-*
    HostName bastion.example.com
    User deploy
    Port=2222
    IdentityFile "~/.ssh/deploy key"

Host prod-db !prod-db-old
    HostName %h.internal
    User dbadmin

Match host *.example.org
    User ignored

Host *
    User fallback
    IdentityFile ~/.ssh/id_%r
"#;

    #[test]
    fn first_matching_value_wins() {
        let c = resolve(CONFIG, "bastion");
        assert_eq!(c.host_name.as_deref(), Some("bastion.example.com"));
        assert_eq!(c.user.as_deref(), Some("deploy"));
        assert_eq!(c.port, Some(2222));
        assert_eq!(
            c.identity_files,
            [
                "~/.ssh/deploy key".to_string(),
                "~/.ssh/id_deploy".to_string()
            ]
        );

        // `prod-*` matches first, so its HostName / User win over the later block.
        let c = resolve(CONFIG, "prod-db");
        assert_eq!(c.host_name.as_deref(), Some("bastion.example.com"));
        assert_eq!(c.user.as_deref(), Some("deploy"));
    }

    #[test]
    fn negations_wildcards_and_match_blocks() {
        let c = resolve(CONFIG, "PROD-DB-OLD");
        // Only `prod-*` and `*` apply (the negation drops the second block).
        assert_eq!(c.user.as_deref(), Some("deploy"));
        // `Match host *.example.org` is evaluated now (it used to be skipped: the user was
        // "fallback"): it comes before `Host *`, so its User wins.
        let c = resolve(CONFIG, "other.example.org");
        assert_eq!(c.host_name, None);
        assert_eq!(c.user.as_deref(), Some("ignored"));
        assert_eq!(c.port, None);
        let c = resolve(CONFIG, "other.example.net");
        assert_eq!(c.user.as_deref(), Some("fallback"));

        let c = resolve("Host db\n  HostName %h.internal\n", "db");
        assert_eq!(c.host_name.as_deref(), Some("db.internal"));
        assert_eq!(resolve("", "x"), HostConfig::default());
    }

    #[test]
    fn proxy_jump_lists() {
        let c = resolve(
            "Host db\n  ProxyJump deploy@bastion:2222,ssh://ops@[2001:db8::1]:22,inner\n",
            "db",
        );
        assert_eq!(
            c.proxy,
            Some(Proxy::Jump(vec![
                JumpHost {
                    user: Some("deploy".into()),
                    host: "bastion".into(),
                    port: Some(2222),
                },
                JumpHost {
                    user: Some("ops".into()),
                    host: "2001:db8::1".into(),
                    port: Some(22),
                },
                JumpHost {
                    user: None,
                    host: "inner".into(),
                    port: None,
                },
            ]))
        );
        assert_eq!(
            parse_proxy_jump("a, ,b:x,::1"),
            vec![
                JumpHost {
                    user: None,
                    host: "a".into(),
                    port: None
                },
                JumpHost {
                    user: None,
                    host: "::1".into(),
                    port: None
                },
            ]
        );
        assert!(parse_proxy_jump("@:22").is_empty());
    }

    #[test]
    fn proxy_command_is_raw_and_competes_with_proxy_jump() {
        let text = "Host a\n  ProxyCommand ssh -W %h:%p \"jump host\"\n  ProxyJump ignored\n\
                    Host b\n  ProxyJump j1\n  ProxyCommand ignored\n\
                    Host c\n  ProxyJump none\nHost *\n  ProxyJump late\n";
        assert_eq!(
            resolve(text, "a").proxy,
            Some(Proxy::Command("ssh -W %h:%p \"jump host\"".into()))
        );
        assert!(matches!(resolve(text, "b").proxy, Some(Proxy::Jump(h)) if h[0].host == "j1"));
        // `none` is the first value for c: it wins over the later `Host *` jump.
        assert_eq!(resolve(text, "c").proxy, None);
        assert!(matches!(resolve(text, "z").proxy, Some(Proxy::Jump(h)) if h[0].host == "late"));
        assert_eq!(
            expand_proxy_command(
                "nc -X connect -x proxy:8080 %h %p # %r@%n %%",
                &ProxyTokens {
                    host: "db.internal",
                    port: 2200,
                    user: "deploy",
                    alias: "db",
                }
            ),
            "nc -X connect -x proxy:8080 db.internal 2200 # deploy@db %"
        );
    }

    #[test]
    fn proxy_tokens_accept_plain_hosts_and_users() {
        for ok in [
            "bastion-1.corp.internal",
            "10.0.0.5",
            "fe80::1%eth0",
            "2001:db8::1",
            "a.b@c.d",
            "first+tag",
            "a=b,c",
            "db_01",
            "M\u{fc}ller",
            "\u{6570}\u{636e}\u{5e93}.example",
        ] {
            assert_eq!(check_proxy_token("SSH host", ok), Ok(()), "{ok}");
        }
        assert_eq!(
            check_proxy_token("SSH host", &"a".repeat(255)),
            Ok(()),
            "255 characters are allowed"
        );
    }

    #[test]
    fn proxy_tokens_refuse_what_the_shell_would_interpret() {
        let too_long = "a".repeat(256);
        for bad in [
            "",
            "$(id)",
            "x$(touch /tmp/pwn).corp",
            "x$(touch${IFS}/tmp/pwn).corp",
            "`id`",
            "a`id`b",
            "a;b",
            "a b",
            "a\tb",
            "a|b",
            "a&b",
            "a&&b",
            "a>b",
            "a<b",
            "a(b",
            "a)b",
            "a{b",
            "a}b",
            "a[b",
            "a]b",
            "a*b",
            "a?b",
            "a!b",
            "a#b",
            "a~b",
            "a\\b",
            "a'b",
            "a\"b",
            "a\nb",
            "a\rb",
            "a\0b",
            "a\u{85}b",
            "a\u{2028}b",
            "-oProxyCommand=x",
            "-",
            "--",
            too_long.as_str(),
        ] {
            assert!(
                check_proxy_token("SSH host", bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn the_refusal_names_the_value_without_control_characters_cut_short() {
        let err = check_proxy_token("SSH user", "$(touch /tmp/x)\n").unwrap_err();
        assert!(err.starts_with("The SSH user \u{201c}$(touch /tmp/x)\u{201d} cannot be used"));
        assert!(err.contains("ProxyCommand"), "{err}");
        assert!(
            err.contains("characters the shell would interpret"),
            "{err}"
        );
        assert!(!err.contains('\n'));
        let long = format!("$({})", "x".repeat(200));
        let err = check_proxy_token("SSH host", &long).unwrap_err();
        assert!(err.len() < 250, "{err}");
        assert!(err.contains('\u{2026}'), "{err}");
        assert!(
            check_proxy_token("SSH host", "")
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            check_proxy_token("SSH host", "-x")
                .unwrap_err()
                .contains("dash")
        );
    }

    #[test]
    fn only_the_tokens_a_proxy_command_uses_are_checked() {
        let bad = |host: &'static str, user: &'static str, alias: &'static str| ProxyTokens {
            host,
            port: 22,
            user,
            alias,
        };
        let evil = "$(id)";
        // %h
        let e = check_proxy_command("ssh -W %h:%p jump", &bad(evil, "u", "a")).unwrap_err();
        assert!(e.starts_with("The SSH host "), "{e}");
        // %r
        let e = check_proxy_command("ssh -l %r jump", &bad("h", evil, "a")).unwrap_err();
        assert!(e.starts_with("The SSH user "), "{e}");
        // %n
        let e = check_proxy_command("jump %n", &bad("h", "u", evil)).unwrap_err();
        assert!(e.starts_with("The SSH host alias "), "{e}");
        // A token the command does not use is not looked at (the user chose that command).
        assert_eq!(
            check_proxy_command("ssh -W %h:%p jump", &bad("h", evil, evil)),
            Ok(())
        );
        assert_eq!(
            check_proxy_command("nc proxy 8080", &bad(evil, evil, evil)),
            Ok(())
        );
        // `%%h` is a literal `%h`, not the host.
        assert_eq!(
            check_proxy_command("echo %%h", &bad(evil, "u", "a")),
            Ok(())
        );
        assert!(check_proxy_command("echo %%%h", &bad(evil, "u", "a")).is_err());
        // Plain values pass.
        assert_eq!(
            check_proxy_command(
                "ssh -W %h:%p -l %r %n",
                &bad("bastion-1.corp.internal", "deploy", "db")
            ),
            Ok(())
        );
    }

    #[test]
    fn identity_agents() {
        let c = resolve(
            "Host op\n  IdentityAgent \"~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock\"\n\
             Host env\n  IdentityAgent SSH_AUTH_SOCK\nHost var\n  IdentityAgent $MY_AGENT\n\
             Host off\n  IdentityAgent none\nHost tok\n  IdentityAgent /run/%h.sock\n",
            "op",
        );
        assert_eq!(
            c.identity_agent,
            Some(IdentityAgent::Path(
                "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock".into()
            ))
        );
        let agent = |host: &str| {
            resolve(
                "Host env\n IdentityAgent SSH_AUTH_SOCK\nHost var\n IdentityAgent ${MY_AGENT}\n\
                 Host off\n IdentityAgent none\nHost tok\n HostName t.example\n IdentityAgent /run/%h.sock\n",
                host,
            )
            .identity_agent
        };
        assert_eq!(
            agent("env"),
            Some(IdentityAgent::Env("SSH_AUTH_SOCK".into()))
        );
        assert_eq!(agent("var"), Some(IdentityAgent::Env("MY_AGENT".into())));
        assert_eq!(agent("off"), Some(IdentityAgent::Disabled));
        assert_eq!(agent("off").unwrap().socket(), None);
        assert_eq!(
            agent("tok"),
            Some(IdentityAgent::Path("/run/t.example.sock".into()))
        );
        assert_eq!(agent("other"), None);
    }

    #[test]
    fn globs() {
        // The `Host` / `Match` matcher: `*` and `?`, case-insensitive.
        let glob = |pattern: &str, text: &str| wildcard(pattern, text, true, false);
        assert!(glob("*", "anything"));
        assert!(glob("prod-?", "prod-1"));
        assert!(!glob("prod-?", "prod-12"));
        assert!(glob("*.example.com", "a.b.example.com"));
        assert!(!glob("*.example.com", "example.com"));
        assert!(glob("a*b*c", "aXXbYYc"));
    }

    // ---- Include and Match ----

    use std::fs;

    /// A fake home: `~/.ssh` with a `config`, plus helpers to add files below it.
    struct Home {
        dir: tempfile::TempDir,
    }

    impl Home {
        fn new(config: &str) -> Self {
            let home = Home {
                dir: tempfile::tempdir().unwrap(),
            };
            home.write(".ssh/config", config);
            home
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn files(&self) -> ConfigFiles {
            // Canonical: macOS's temp dir is behind a /var -> /private/var symlink.
            let home = self.dir.path().canonicalize().unwrap();
            ConfigFiles {
                config: home.join(".ssh/config"),
                ssh_dir: home.join(".ssh"),
                home: Some(home),
            }
        }

        fn lookup(&self, host: &str) -> HostConfig {
            for_host_in(&self.files(), host, None)
        }
    }

    #[test]
    fn an_alias_defined_only_in_an_included_file_resolves() {
        let home = Home::new("Include config.d/*\n\nHost direct\n  HostName direct.example\n");
        home.write(
            ".ssh/config.d/10-orb",
            "# OrbStack\nHost orb-box\n  HostName 192.168.139.5\n  User orb\n  Port 32222\n  \
             IdentityFile ~/.orbstack/ssh/id_ed25519\n  ProxyJump hop@jump.example:2200\n",
        );
        // A hidden file is not picked up by `*`, a directory is skipped.
        home.write(".ssh/config.d/.hidden", "Host orb-box\n  User hidden\n");
        fs::create_dir_all(home.dir.path().join(".ssh/config.d/sub")).unwrap();

        let c = home.lookup("orb-box");
        assert_eq!(c.host_name.as_deref(), Some("192.168.139.5"));
        assert_eq!(c.user.as_deref(), Some("orb"));
        assert_eq!(c.port, Some(32222));
        assert_eq!(c.identity_files, ["~/.orbstack/ssh/id_ed25519".to_string()]);
        assert_eq!(
            c.proxy,
            Some(Proxy::Jump(vec![JumpHost {
                user: Some("hop".into()),
                host: "jump.example".into(),
                port: Some(2200),
            }]))
        );
        // The main file's own blocks still work, and other aliases see nothing of orb-box.
        assert_eq!(
            home.lookup("direct").host_name.as_deref(),
            Some("direct.example")
        );
        assert_eq!(home.lookup("elsewhere"), HostConfig::default());
        // Without the Include the alias is unknown (the old behaviour).
        let text = fs::read_to_string(home.files().config).unwrap();
        assert_eq!(resolve(&text, "orb-box"), HostConfig::default());
    }

    #[test]
    fn an_include_before_the_first_host_applies_to_every_host() {
        let home = Home::new("INCLUDE defaults.conf\nHost a\n  HostName a.example\n  User later\n");
        home.write(
            ".ssh/defaults.conf",
            "User shared\nPort 2200\nIdentityFile ~/.ssh/shared\n",
        );
        for host in ["a", "anything"] {
            let c = home.lookup(host);
            // First value wins: the included User / Port come before the block's.
            assert_eq!(c.user.as_deref(), Some("shared"), "{host}");
            assert_eq!(c.port, Some(2200), "{host}");
            assert_eq!(c.identity_files, ["~/.ssh/shared".to_string()], "{host}");
        }
        assert_eq!(home.lookup("a").host_name.as_deref(), Some("a.example"));
    }

    #[test]
    fn an_include_inside_a_block_belongs_to_that_block_and_does_not_leak() {
        let home =
            Home::new("Host a\n  Include inner.conf\n  Port 2222\nHost b\n  HostName b.example\n");
        // The included file ends inside its own `Host zzz` block; that must not decide what the
        // including file's later `Port` does.
        home.write(
            ".ssh/inner.conf",
            "User inner\nHost zzz\n  Compression yes\n",
        );
        let a = home.lookup("a");
        assert_eq!(a.user.as_deref(), Some("inner"));
        assert_eq!(a.port, Some(2222));
        let b = home.lookup("b");
        assert_eq!(b.user, None, "b is not in the block that includes the file");
        assert_eq!(b.host_name.as_deref(), Some("b.example"));
        // The included file's `Host zzz` selects zzz inside it.
        home.write(".ssh/config", "Include inner2.conf\n");
        home.write(".ssh/inner2.conf", "Host zzz\n  User only-zzz\n");
        assert_eq!(home.lookup("zzz").user.as_deref(), Some("only-zzz"));
        assert_eq!(home.lookup("other").user, None);
    }

    #[test]
    fn include_paths_are_relative_to_ssh_tilde_absolute_or_globs_in_lexical_order() {
        let home = Home::new("");
        let abs = home.dir.path().join("elsewhere/abs.conf");
        let config = format!(
            "Include conf.d/[0-2]*.conf ~/tilde/t.conf {} \"conf.d/with space.conf\"\nHost *\n",
            abs.display()
        );
        home.write(".ssh/config", &config);
        home.write(".ssh/conf.d/20-b.conf", "IdentityFile b\n");
        home.write(".ssh/conf.d/10-a.conf", "IdentityFile a\n");
        home.write(
            ".ssh/conf.d/30-c.conf",
            "IdentityFile not-matched-by-the-class\n",
        );
        home.write(".ssh/conf.d/with space.conf", "IdentityFile spaced\n");
        home.write("tilde/t.conf", "IdentityFile tilde\n");
        home.write("elsewhere/abs.conf", "IdentityFile absolute\n");
        assert_eq!(
            home.lookup("x").identity_files,
            ["a", "b", "tilde", "absolute", "spaced"]
        );
    }

    #[test]
    fn include_cycles_terminate_and_a_file_may_be_included_twice() {
        let home = Home::new("Include a.conf\nIdentityFile main\n");
        home.write(".ssh/a.conf", "IdentityFile a\nInclude b.conf\n");
        home.write(
            ".ssh/b.conf",
            "IdentityFile b\nInclude a.conf\nInclude config\nInclude b.conf\n",
        );
        // a -> b -> (a, config, b are all being read): each file once on the way down.
        assert_eq!(home.lookup("x").identity_files, ["a", "b", "main"]);

        // Not a cycle: the same snippet from two places.
        home.write(
            ".ssh/config",
            "Host one\n  Include snip\nHost two\n  Include snip\n  Include snip\n",
        );
        home.write(".ssh/snip", "IdentityFile s\n");
        assert_eq!(home.lookup("one").identity_files, ["s"]);
        assert_eq!(home.lookup("two").identity_files, ["s", "s"]);
    }

    #[test]
    fn include_depth_is_limited_to_16() {
        let home = Home::new("Include f1.conf\n");
        for i in 1..=20 {
            home.write(
                &format!(".ssh/f{i}.conf"),
                &format!("IdentityFile id{i}\nInclude f{}.conf\n", i + 1),
            );
        }
        let ids = home.lookup("x").identity_files;
        assert_eq!(ids.len(), 16, "{ids:?}");
        assert_eq!(ids.first().map(String::as_str), Some("id1"));
        assert_eq!(ids.last().map(String::as_str), Some("id16"));
    }

    #[test]
    fn a_diamond_of_includes_is_cut_off_by_the_file_budget() {
        let home = Home::new("Include d1.conf\n");
        for i in 1..=15 {
            home.write(
                &format!(".ssh/d{i}.conf"),
                &format!("Include d{}.conf d{}.conf\n", i + 1, i + 1),
            );
        }
        home.write(".ssh/d16.conf", "IdentityFile leaf\n");
        // 2^15 reads of the leaf without a budget; this returns (and is bounded).
        let ids = home.lookup("x").identity_files;
        assert!(ids.len() <= MAX_INCLUDED_FILES, "{}", ids.len());
    }

    #[test]
    fn missing_unreadable_and_directory_includes_are_ignored() {
        let home = Home::new(
            "Include nope.conf missing/*.conf ~/nodir/x ~other/x conf.d\nInclude binary.conf\n\
             Host a\n  User kept\n",
        );
        fs::create_dir_all(home.dir.path().join(".ssh/conf.d")).unwrap();
        // Not UTF-8: unreadable as text, skipped.
        fs::write(
            home.dir.path().join(".ssh/binary.conf"),
            [0xff, 0xfe, 0x00, 0x80],
        )
        .unwrap();
        assert_eq!(home.lookup("a").user.as_deref(), Some("kept"));
        // No config file at all.
        let empty = Home::new("");
        fs::remove_file(empty.files().config).unwrap();
        assert_eq!(empty.lookup("a"), HostConfig::default());
    }

    #[test]
    fn an_include_inside_a_block_that_does_not_apply_is_not_read() {
        let home = Home::new("Host other\n  Include snip\nHost *\n  Port 22\n");
        // If it were read, `Host a` in it would switch the block on for `a`.
        home.write(".ssh/snip", "Host a\n  User leaked\n");
        assert_eq!(home.lookup("a").user, None);
    }

    #[test]
    fn match_all_host_originalhost_and_user() {
        let text = "Match host web*\n  Port 8022\n\
                    Match originalhost db1,db2\n  User dba\n\
                    Match all\n  IdentityFile everyone\n";
        let c = resolve(text, "web1");
        assert_eq!(c.port, Some(8022));
        assert_eq!(c.user, None);
        assert_eq!(c.identity_files, ["everyone"]);
        let c = resolve(text, "db2");
        assert_eq!(c.port, None);
        assert_eq!(c.user.as_deref(), Some("dba"));
        assert_eq!(c.identity_files, ["everyone"]);
        // The host name matches case-insensitively, as for `Host`.
        assert_eq!(resolve(text, "WEB9").port, Some(8022));
        // No match: only `Match all`.
        let c = resolve(text, "cache");
        assert_eq!((c.port, c.user), (None, None));
        assert_eq!(c.identity_files, ["everyone"]);
    }

    #[test]
    fn match_host_tests_the_host_name_found_so_far_and_originalhost_the_alias() {
        let text = "Host jump\n  HostName web7.corp\n  Port 1\n\
                    Match host web*\n  User by-host\n\
                    Match originalhost web*\n  IdentityFile by-alias\n";
        let c = resolve(text, "jump");
        assert_eq!(
            c.user.as_deref(),
            Some("by-host"),
            "HostName web7.corp is web*"
        );
        assert!(c.identity_files.is_empty(), "the alias `jump` is not web*");
        let c = resolve(text, "web2");
        assert_eq!(c.user.as_deref(), Some("by-host"));
        assert_eq!(c.identity_files, ["by-alias"]);
    }

    #[test]
    fn match_user_uses_the_form_user_negation_lists_and_several_criteria() {
        let text = "Match user deploy,ops\n  IdentityFile team\n\
                    Match !user deploy\n  IdentityFile not-deploy\n\
                    Match host web* !user root\n  IdentityFile web-non-root\n\
                    Match host web*,!web9 originalhost web*\n  IdentityFile list\n";
        let ids = |host: &str, user: Option<&str>| resolve_as(text, host, user).identity_files;
        assert_eq!(
            ids("web1", Some("deploy")),
            ["team", "web-non-root", "list"]
        );
        assert_eq!(
            ids("web1", Some("ops")),
            ["team", "not-deploy", "web-non-root", "list"]
        );
        // root is excluded by `!user root`; web9 by the negated host pattern.
        assert_eq!(ids("web1", Some("root")), ["not-deploy", "list"]);
        assert_eq!(
            ids("web9", Some("ops")),
            ["team", "not-deploy", "web-non-root"]
        );
        assert_eq!(ids("db", Some("deploy")), ["team"]);
        // `User` set earlier in the config counts when the form has none; user names are
        // case-sensitive.
        let text = "Host a\n  User deploy\nMatch user deploy\n  IdentityFile by-config-user\n\
                    Match user DEPLOY\n  IdentityFile case\n";
        assert_eq!(resolve(text, "a").identity_files, ["by-config-user"]);
        // The form's user wins over a `User` from the config.
        assert!(resolve_as(text, "a", Some("bob")).identity_files.is_empty());
    }

    #[test]
    fn unsupported_match_criteria_never_match() {
        let text = "Match exec true\n  User from-exec\n\
                    Match localnetwork 192.168.0.0/16\n  User from-network\n\
                    Match canonical host x\n  User canonical\n\
                    Match final all\n  User final\n\
                    Match host a exec true\n  User mixed\n\
                    Match\n  User empty\n\
                    Match host\n  User no-argument\n\
                    Match frobnicate yes\n  User unknown\n\
                    Match all\n  Port 22\n";
        let c = resolve(text, "a");
        assert_eq!(c.user, None);
        assert_eq!(c.port, Some(22));
    }

    #[test]
    fn match_user_through_an_included_file_and_the_explicit_path_lookup() {
        let home = Home::new("Include extra.conf\n");
        home.write(
            ".ssh/extra.conf",
            "Match user alice\n  IdentityFile ~/.ssh/alice\n",
        );
        let files = home.files();
        assert_eq!(
            for_host_in(&files, "h", Some("alice")).identity_files,
            ["~/.ssh/alice"]
        );
        assert!(
            for_host_in(&files, "h", Some("bob"))
                .identity_files
                .is_empty()
        );
    }

    #[test]
    fn wildcard_brackets_and_case() {
        assert!(wildcard("[0-2]*.conf", "10-a.conf", false, true));
        assert!(!wildcard("[0-2]*.conf", "30-a.conf", false, true));
        assert!(wildcard("[!0-2]*", "30-a", false, true));
        assert!(wildcard("[^0-2]*", "30-a", false, true));
        assert!(wildcard("[]]x", "]x", false, true));
        assert!(
            wildcard("[ab", "[ab", false, true),
            "an unclosed bracket is literal"
        );
        assert!(
            !wildcard("A*", "abc", false, true),
            "file names are case-sensitive"
        );
        assert!(wildcard("A*", "abc", true, false));
        // No brackets for Host / Match patterns: `[::1]` is literal text.
        assert!(wildcard("[::1]", "[::1]", true, false));
    }
}
