use std::process::Command;

fn main() {
    build_commit();
    tauri_build::build()
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
