//! A small `~/.ssh/config` reader for the SSH tunnel: the `HostName`, `User`, `Port` and
//! `IdentityFile` that OpenSSH would use for a host (alias). Values typed in the connection form
//! win; the config only fills what the form leaves empty (TablePlus does the same).
//!
//! Supported: `Host` blocks with `*` / `?` patterns and `!` negations, settings before the first
//! block (they apply to every host), `Key value` and `Key=value`, quoted values, `#` comments and
//! the `%h %d %u %r %%` tokens. `Match` blocks are skipped and `Include` is ignored.

use std::path::PathBuf;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostConfig {
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
}

/// `~/.ssh/config`, if there is a home directory.
pub fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".ssh").join("config"))
}

/// The settings of `~/.ssh/config` that apply to `host` (nothing when the file is missing).
pub fn for_host(host: &str) -> HostConfig {
    let Some(path) = default_config_path() else {
        return HostConfig::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => resolve(&text, host),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("cannot read {}: {e}", path.display());
            }
            HostConfig::default()
        }
    }
}

/// Splits a config line into its keyword and arguments (`Key value…` or `Key=value…`).
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
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
    Some((keyword, split_args(rest)))
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

/// Glob match with `*` (any run) and `?` (one character), case-insensitive.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Whether a `Host` line's patterns select `host`: one pattern matches and no negated one does.
fn host_matches(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob(negated, host) {
                return false;
            }
        } else if glob(pattern, host) {
            matched = true;
        }
    }
    matched
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
/// wins (OpenSSH's rule); every `IdentityFile` is collected.
pub fn resolve(text: &str, host: &str) -> HostConfig {
    let mut out = HostConfig::default();
    let mut raw_host_name: Option<String> = None;
    let mut raw_identities = Vec::new();
    // Settings before the first Host / Match apply to every host.
    let mut active = true;
    for line in text.lines() {
        let Some((keyword, args)) = split_line(line) else {
            continue;
        };
        match keyword.as_str() {
            "host" => {
                active = host_matches(&args, host);
                continue;
            }
            "match" => {
                active = false;
                continue;
            }
            _ => {}
        }
        if !active {
            continue;
        }
        let Some(value) = args.first().cloned() else {
            continue;
        };
        match keyword.as_str() {
            "hostname" if raw_host_name.is_none() => raw_host_name = Some(value),
            "user" if out.user.is_none() => out.user = Some(value),
            "port" if out.port.is_none() => out.port = value.parse().ok().filter(|p| *p != 0),
            "identityfile" => raw_identities.push(value),
            _ => {}
        }
    }
    out.host_name = raw_host_name.map(|h| expand_tokens(&h, host, None));
    let real_host = out.host_name.clone().unwrap_or_else(|| host.to_string());
    out.identity_files = raw_identities
        .iter()
        .map(|f| expand_tokens(f, &real_host, out.user.as_deref()))
        .collect();
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
        let c = resolve(CONFIG, "other.example.org");
        assert_eq!(c.host_name, None);
        assert_eq!(c.user.as_deref(), Some("fallback"));
        assert_eq!(c.port, None);

        let c = resolve("Host db\n  HostName %h.internal\n", "db");
        assert_eq!(c.host_name.as_deref(), Some("db.internal"));
        assert_eq!(resolve("", "x"), HostConfig::default());
    }

    #[test]
    fn globs() {
        assert!(glob("*", "anything"));
        assert!(glob("prod-?", "prod-1"));
        assert!(!glob("prod-?", "prod-12"));
        assert!(glob("*.example.com", "a.b.example.com"));
        assert!(!glob("*.example.com", "example.com"));
        assert!(glob("a*b*c", "aXXbYYc"));
    }
}
