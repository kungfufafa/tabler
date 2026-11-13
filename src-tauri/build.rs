use std::path::PathBuf;
use std::process::Command;

fn main() {
    build_commit();
    release_tags();
    tauri_build::build()
}

/// `git` in the repository, its trimmed output; None when git or the repository is not there.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// MAJOR.MINOR.PATCH of a `vX.Y.Z` tag; None for any other tag (`nightly`, `v1.0.0-rc.1`).
fn tag_version(tag: &str) -> Option<(u64, u64, u64, String)> {
    let version = tag.strip_prefix('v')?;
    let mut parts = version.split('.');
    let parsed = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts
        .next()
        .is_none()
        .then(|| (parsed.0, parsed.1, parsed.2, version.to_string()))
}

fn highest<'a>(tags: impl Iterator<Item = &'a str>) -> Option<String> {
    tags.filter_map(tag_version).max().map(|t| t.3)
}

/// The release tags the app's shown version follows (`src/version.rs`): the latest release, and
/// the release of the commit built, if any. Best effort: without git the build keeps its own.
fn release_tags() {
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let dir = PathBuf::from(git_dir);
        // A new tag, a fetch of tags, or another branch / commit checked out. Only paths that
        // exist: Cargo reruns the script on every build for one that does not.
        for path in ["HEAD", "packed-refs", "refs/tags"] {
            let path = dir.join(path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let Some(all) = git(&["tag", "--list", "v*"]) else {
        return;
    };
    if let Some(latest) = highest(all.lines()) {
        println!("cargo:rustc-env=TABLER_LATEST_RELEASE={latest}");
    }
    if let Some(head) =
        git(&["tag", "--list", "v*", "--points-at", "HEAD"]).and_then(|tags| highest(tags.lines()))
    {
        println!("cargo:rustc-env=TABLER_HEAD_RELEASE={head}");
    }
}

/// `TABLER_COMMIT` for the crate: the short hash of the commit this build is made from, shown in
/// the About dialog next to the version (`updater::build_commit`). CI names it (`GITHUB_SHA`, or
/// `TABLER_COMMIT` to override); a release build made by hand asks git. A debug build asks nothing
/// and reports none: following `HEAD` there would recompile the crate after every commit.
fn build_commit() {
    println!("cargo:rerun-if-env-changed=TABLER_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let named = ["TABLER_COMMIT", "GITHUB_SHA"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()));
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let commit = named.or_else(|| release.then(git_head).flatten());
    let short: String = commit.unwrap_or_default().trim().chars().take(7).collect();
    // Anything but a hash (a branch name in the variable, say) is left out.
    let short = if short.len() == 7 && short.chars().all(|c| c.is_ascii_hexdigit()) {
        short.to_ascii_lowercase()
    } else {
        String::new()
    };
    println!("cargo:rustc-env=TABLER_COMMIT={short}");
}

/// `HEAD` of the checkout this crate sits in, and a rebuild whenever it moves (the reflog gets a
/// line for every commit, checkout and pull).
fn git_head() -> Option<String> {
    let reflog = std::path::Path::new("../.git/logs/HEAD");
    if reflog.exists() {
        println!("cargo:rerun-if-changed={}", reflog.display());
    }
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
