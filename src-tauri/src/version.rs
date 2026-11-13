//! The version the app shows (Welcome window, About): the release it was built as.
//!
//! A release or nightly build gets its version from CI, written into tauri.conf.json, Cargo.toml
//! and package.json before the build (`dev/scripts/build-version.mjs`). A local build keeps the
//! repository's base version (0.1.0), which said nothing about the code it was built from. So
//! `build.rs` reads the release tags (`vX.Y.Z`) at build time, and a build whose own version is
//! older than the latest release shows that release instead: the release itself when the build is
//! of its commit, else the next one marked `-dev` (code after the last release, not released yet).

/// MAJOR.MINOR.PATCH of a plain version (no pre-release, no build metadata); None otherwise.
fn plain(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The version to show. `package`: the build's own (tauri.conf.json); `latest_release`: the highest
/// release tag when it was built; `head_release`: the release tag on the commit it was built from.
pub fn display_version(
    package: &str,
    latest_release: Option<&str>,
    head_release: Option<&str>,
) -> String {
    let Some(latest) = latest_release.and_then(|l| plain(l).map(|p| (l, p))) else {
        return package.to_string();
    };
    match plain(package) {
        // CI wrote the version (a release, or the nightly after the latest release): it is right.
        Some(own) if own >= latest.1 => return package.to_string(),
        Some(_) => {}
        // A pre-release or otherwise unusual version was set on purpose: keep it.
        None => return package.to_string(),
    }
    if let Some(head) = head_release.filter(|h| plain(h).is_some()) {
        return head.to_string();
    }
    let (major, minor, patch) = latest.1;
    format!("{major}.{minor}.{}-dev", patch + 1)
}

/// The version this build shows (see the module docs).
pub fn app_version(package: &str) -> String {
    display_version(
        package,
        option_env!("TABLER_LATEST_RELEASE"),
        option_env!("TABLER_HEAD_RELEASE"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_builds_keep_their_own_version() {
        // The nightly after v0.1.2, and the release v0.1.2 itself.
        assert_eq!(display_version("0.1.3", Some("0.1.2"), None), "0.1.3");
        assert_eq!(
            display_version("0.1.2", Some("0.1.2"), Some("0.1.2")),
            "0.1.2"
        );
        assert_eq!(
            display_version("1.0.0-rc.1", Some("0.1.2"), None),
            "1.0.0-rc.1"
        );
    }

    #[test]
    fn local_builds_follow_the_releases() {
        // Built from the release's own commit: that release.
        assert_eq!(
            display_version("0.1.0", Some("0.1.2"), Some("0.1.2")),
            "0.1.2"
        );
        // Built from later code: the next release, not released yet.
        assert_eq!(display_version("0.1.0", Some("0.1.2"), None), "0.1.3-dev");
        assert_eq!(display_version("0.1.0", Some("0.1.9"), None), "0.1.10-dev");
    }

    #[test]
    fn without_release_tags_the_own_version_stays() {
        assert_eq!(display_version("0.1.0", None, None), "0.1.0");
        assert_eq!(display_version("0.1.0", Some("nope"), None), "0.1.0");
    }
}
