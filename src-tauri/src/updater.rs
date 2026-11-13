//! In-app updates, on the Tauri updater plugin: a check reads the `latest.json` of the build's
//! channel, the update is downloaded and its signature verified against the public key in
//! tauri.conf.json, and installing swaps the app (macOS) or runs the installer (Windows) before
//! the restart. The flow, the two channels and the release side are in docs/updates.md.
//!
//! The renderer drives it through `commands::update`; nothing here is reachable from the page
//! except through those commands (the plugin's own commands are not granted in the capability).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tauri::AppHandle;
use tauri_plugin_updater::{Error as PluginError, Update, UpdaterExt};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::types::update::{UpdateChannel, UpdateCheck, UpdateProgress};

/// A check gives up after this long (a download has no limit: it can be cancelled).
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// A progress message goes out at most once per this many bytes received.
pub const PROGRESS_STEP: u64 = 256 * 1024;

const DEV_BUILD: &str = "This is a development build: it does not update itself.";
const NO_ADDRESS: &str = "This build was made without an update address: it does not update \
     itself. Download the newest installer from the releases page instead.";
const NO_KEY: &str = "This build has no release key to verify updates with: it does not update \
     itself. Download the newest installer from the releases page instead.";

/// The short hash of the commit this build was made from (build.rs): CI builds and release builds
/// made in a git checkout have one, a debug build has none.
pub fn build_commit() -> Option<&'static str> {
    let commit = env!("TABLER_COMMIT");
    (!commit.is_empty()).then_some(commit)
}

/// "0.1.7 (1a2b3c4), nightly channel": the running build, for the log's first line.
pub fn describe_build(version: &str, channel: UpdateChannel) -> String {
    let channel = match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Nightly => "nightly",
    };
    match build_commit() {
        Some(commit) => format!("{version} ({commit}), {channel} channel"),
        None => format!("{version}, {channel} channel"),
    }
}

/// Where the rolling `nightly` release keeps its files: a build whose update address is there
/// follows every build of `main`.
const NIGHTLY_RELEASE: &str = "/releases/download/nightly/";

/// The update addresses of the configuration (`plugins.updater.endpoints`).
fn endpoints(plugins: &HashMap<String, serde_json::Value>) -> Vec<&str> {
    plugins
        .get("updater")
        .and_then(|updater| updater.get("endpoints"))
        .and_then(|endpoints| endpoints.as_array())
        .map(|list| list.iter().filter_map(|e| e.as_str()).collect())
        .unwrap_or_default()
}

/// The channel a build follows, which is where it asks for updates: CI gives the automatic
/// builds of `main` the address of the rolling `nightly` release and a release tagged by hand the
/// address of the latest numbered release (dev/scripts/release-config.mjs). Every version is a
/// plain `MAJOR.MINOR.PATCH`, so the version itself does not say.
pub fn channel(plugins: &HashMap<String, serde_json::Value>) -> UpdateChannel {
    if endpoints(plugins)
        .iter()
        .any(|endpoint| endpoint.contains(NIGHTLY_RELEASE))
    {
        UpdateChannel::Nightly
    } else {
        UpdateChannel::Stable
    }
}

/// Why this build cannot update itself, if it cannot: a debug build (its binary sits in
/// `target/`, not in an installed app), one whose configuration has no update address (a build
/// made without the signing key, a fork's for instance, gets none: it must not follow the
/// upstream releases), or one without the public key that verifies a download.
pub fn unsupported_reason(
    debug_build: bool,
    has_endpoints: bool,
    has_pubkey: bool,
) -> Option<&'static str> {
    if debug_build {
        Some(DEV_BUILD)
    } else if !has_endpoints {
        Some(NO_ADDRESS)
    } else if !has_pubkey {
        Some(NO_KEY)
    } else {
        None
    }
}

/// Whether `plugins.updater.pubkey` of the configuration holds a key. Without one no download
/// could ever be verified, so the build does not look for updates at all.
pub fn has_pubkey(plugins: &HashMap<String, serde_json::Value>) -> bool {
    plugins
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .is_some_and(|key| !key.trim().is_empty())
}

/// Counts the bytes of a download and says when the page should hear about it: the first chunk,
/// then every [`PROGRESS_STEP`] bytes, and the last chunk when the size is known.
#[derive(Debug, Default)]
pub struct ProgressMeter {
    downloaded: u64,
    reported: Option<u64>,
}

impl ProgressMeter {
    pub fn add(&mut self, chunk: usize, total: Option<u64>) -> Option<UpdateProgress> {
        self.downloaded += chunk as u64;
        let due = match self.reported {
            None => true,
            Some(reported) => {
                self.downloaded - reported >= PROGRESS_STEP || total == Some(self.downloaded)
            }
        };
        due.then(|| {
            self.reported = Some(self.downloaded);
            UpdateProgress {
                downloaded: self.downloaded,
                total,
            }
        })
    }

    /// The whole download, for the message that ends it.
    pub fn done(&self) -> UpdateProgress {
        UpdateProgress {
            downloaded: self.downloaded,
            total: Some(self.downloaded),
        }
    }
}

/// The update the last check found, and its package once downloaded and verified.
struct Found {
    update: Update,
    package: Option<Arc<Vec<u8>>>,
}

/// Managed state of the updater: one per app, shared by every window.
#[derive(Default)]
pub struct UpdaterState {
    /// The check at launch was made (each window asks; the first one's counts).
    launch_checked: AtomicBool,
    found: Mutex<Option<Found>>,
    /// The download under way, to cancel it.
    download: Mutex<Option<CancellationToken>>,
}

impl UpdaterState {
    fn found(&self) -> MutexGuard<'_, Option<Found>> {
        self.found.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn download(&self) -> MutexGuard<'_, Option<CancellationToken>> {
        self.download.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether this is the first check at launch of this run (later ones are skipped).
    pub fn begin_launch_check(&self) -> bool {
        !self.launch_checked.swap(true, Ordering::SeqCst)
    }
}

/// Ends the download slot when the download returns, however it does.
struct DownloadSlot<'a>(&'a UpdaterState);

impl Drop for DownloadSlot<'_> {
    fn drop(&mut self) {
        *self.0.download() = None;
    }
}

fn check_failure(e: PluginError) -> AppError {
    let message = match &e {
        PluginError::ReleaseNotFound => "No release information was found at the update address. \
             A new build may be publishing right now: try again in a few minutes."
            .to_string(),
        PluginError::TargetNotFound(_) | PluginError::TargetsNotFound(_) => {
            "The newest release has no build for this computer.".to_string()
        }
        other => format!("Could not check for updates: {other}"),
    };
    log::warn!("update check failed: {e}");
    AppError::Internal(message)
}

fn download_failure(e: PluginError) -> AppError {
    let message = match &e {
        PluginError::Minisign(_)
        | PluginError::Base64(_)
        | PluginError::SignatureUtf8(_)
        | PluginError::SignedVersionMismatch { .. }
        | PluginError::MissingSignedVersion => format!(
            "The downloaded update is not signed by Tabler's release key, so it was not \
             installed: {e}"
        ),
        other => format!("Could not download the update: {other}"),
    };
    log::warn!("update download failed: {e}");
    AppError::Internal(message)
}

/// Whether an install failed because the app sits where it cannot be replaced: run straight
/// from the disk image, or from the read-only copy macOS runs a quarantined app from (App
/// Translocation) until it has been moved to Applications.
fn not_replaceable(e: &PluginError) -> bool {
    match e {
        PluginError::TempDirNotOnSameMountPoint => true,
        PluginError::Io(io) => io.kind() == std::io::ErrorKind::ReadOnlyFilesystem,
        _ => false,
    }
}

fn install_failure(e: PluginError) -> AppError {
    log::error!("update install failed: {e}");
    let message = if not_replaceable(&e) {
        "Could not install the update: Tabler cannot be replaced where it is running from. Move \
         it to the Applications folder, open it from there and try again."
            .to_string()
    } else {
        format!("Could not install the update: {e}")
    };
    AppError::Internal(message)
}

/// Asks the channel's `latest.json` for a newer build. `launch`: the quiet check a window makes
/// when it opens, which only the first window of a run gets to make.
pub async fn check(
    app: &AppHandle,
    state: &UpdaterState,
    launch: bool,
) -> Result<UpdateCheck, AppError> {
    if launch && !state.begin_launch_check() {
        return Ok(UpdateCheck::Skipped);
    }
    // Windows: the installer ends the process without the exit events, so what
    // `RunEvent::Exit` owes the user's work runs here first.
    let cleanup_app = app.clone();
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .on_before_exit(move || {
            before_installer_exit(&cleanup_app);
        })
        .build();
    let has_endpoints = !matches!(updater, Err(PluginError::EmptyEndpoints));
    let has_pubkey = has_pubkey(&app.config().plugins.0);
    if let Some(reason) = unsupported_reason(cfg!(debug_assertions), has_endpoints, has_pubkey) {
        log::info!("update check: {reason}");
        return Ok(UpdateCheck::Unsupported {
            reason: reason.to_string(),
        });
    }
    let updater = updater.map_err(check_failure)?;
    let current = app.package_info().version.to_string();
    let Some(update) = updater.check().await.map_err(check_failure)? else {
        *state.found() = None;
        log::info!("update check: {current} is up to date");
        return Ok(UpdateCheck::UpToDate { version: current });
    };
    log::info!(
        "update check: {} is available (running {current})",
        update.version
    );
    let mut found = state.found();
    // The package of the same version, downloaded after an earlier check, is kept.
    let package = found
        .take()
        .filter(|f| f.update.version == update.version)
        .and_then(|f| f.package);
    let result = UpdateCheck::Available {
        version: update.version.clone(),
        current_version: current,
        notes: update
            .body
            .clone()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty()),
        date: update
            .raw_json
            .get("pub_date")
            .and_then(|d| d.as_str())
            .map(str::to_string),
        downloaded: package.is_some(),
    };
    *found = Some(Found { update, package });
    Ok(result)
}

/// Downloads the update the last check found and verifies its signature; `on_progress` hears how
/// far it is. A download already made is not made again. [`cancel`] stops it (`AppError::Cancelled`).
pub async fn download(
    state: &UpdaterState,
    on_progress: impl Fn(UpdateProgress) + Send,
) -> Result<(), AppError> {
    let update = {
        let found = state.found();
        let Some(found) = found.as_ref() else {
            return Err(AppError::InvalidInput(
                "Check for updates first: no update was found.".into(),
            ));
        };
        if let Some(package) = &found.package {
            let size = package.len() as u64;
            on_progress(UpdateProgress {
                downloaded: size,
                total: Some(size),
            });
            return Ok(());
        }
        found.update.clone()
    };
    let token = CancellationToken::new();
    {
        let mut slot = state.download();
        if slot.is_some() {
            return Err(AppError::InvalidInput(
                "The update is already downloading.".into(),
            ));
        }
        *slot = Some(token.clone());
    }
    let _slot = DownloadSlot(state);
    let mut meter = ProgressMeter::default();
    let package = tokio::select! {
        package = update.download(
            |chunk, total| {
                if let Some(progress) = meter.add(chunk, total) {
                    on_progress(progress);
                }
            },
            || {},
        ) => package.map_err(download_failure)?,
        () = token.cancelled() => {
            log::info!("update download cancelled");
            return Err(AppError::Cancelled);
        }
    };
    on_progress(meter.done());
    log::info!(
        "update {} downloaded and verified ({} bytes)",
        update.version,
        package.len()
    );
    // A later check may have found a newer build meanwhile: its package is not this one.
    if let Some(found) = state.found().as_mut()
        && found.update.version == update.version
    {
        found.package = Some(Arc::new(package));
    }
    Ok(())
}

/// Stops the download under way, if any.
pub fn cancel(state: &UpdaterState) {
    if let Some(token) = state.download().as_ref() {
        token.cancel();
    }
}

/// Installs the downloaded update: on macOS the new `.app` takes the place of the running one
/// (the caller then restarts); on Windows the installer starts and the process ends here.
/// Off the main thread: macOS may need to ask for an administrator password, and that dialog is
/// run on the main thread while this waits for it.
pub async fn install(state: &UpdaterState) -> Result<(), AppError> {
    let (update, package) = {
        let found = state.found();
        match found.as_ref() {
            Some(Found {
                update,
                package: Some(package),
            }) => (update.clone(), package.clone()),
            _ => {
                return Err(AppError::InvalidInput(
                    "The update has not been downloaded yet.".into(),
                ));
            }
        }
    };
    log::info!("installing update {}", update.version);
    tauri::async_runtime::spawn_blocking(move || update.install(package.as_slice()))
        .await
        .map_err(|e| AppError::Internal(format!("Could not install the update: {e}")))?
        .map_err(install_failure)?;
    *state.found() = None;
    Ok(())
}

/// Windows: runs before the installer is started and the process ends (`std::process::exit`, no
/// exit events). On a thread of its own, since the cleanup blocks on the async runtime and this
/// is called from one of its blocking threads.
#[cfg_attr(not(windows), allow(dead_code))]
fn before_installer_exit(app: &AppHandle) {
    let cleanup_app = app.clone();
    let cleanup = std::thread::spawn(move || crate::exit_cleanup(&cleanup_app));
    if cleanup.join().is_err() {
        log::warn!("the cleanup before the update's installer failed");
    }
    app.cleanup_before_exit();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugins(json: serde_json::Value) -> HashMap<String, serde_json::Value> {
        json.as_object()
            .expect("an object")
            .clone()
            .into_iter()
            .collect()
    }

    #[test]
    fn the_update_address_names_the_channel() {
        let nightly = plugins(serde_json::json!({ "updater": { "endpoints": [
            "https://github.com/kungfufafa/tabler/releases/download/nightly/latest.json"
        ] } }));
        assert_eq!(channel(&nightly), UpdateChannel::Nightly);
        let stable = plugins(serde_json::json!({ "updater": { "endpoints": [
            "https://github.com/kungfufafa/tabler/releases/latest/download/latest.json"
        ] } }));
        assert_eq!(channel(&stable), UpdateChannel::Stable);
        // A build without an address (it does not update itself) or without the plugin's
        // configuration is not a nightly.
        let none = plugins(serde_json::json!({ "updater": { "endpoints": [] } }));
        assert_eq!(channel(&none), UpdateChannel::Stable);
        assert_eq!(channel(&HashMap::new()), UpdateChannel::Stable);
    }

    /// The address tauri.conf.json carries (a build made by hand) is the stable channel's, and
    /// what `channel` looks for is what the release script writes for a nightly.
    #[test]
    fn the_channels_match_the_release_script_and_the_configuration() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(
            channel(&plugins(conf["plugins"].clone())),
            UpdateChannel::Stable
        );
        let script = include_str!("../../dev/scripts/release-config.mjs");
        assert!(
            script.contains("/releases/download/nightly/latest.json"),
            "the nightly address of release-config.mjs changed: update NIGHTLY_RELEASE"
        );
        assert!(NIGHTLY_RELEASE.ends_with("/nightly/"));
    }

    #[test]
    fn the_log_names_the_build() {
        let nightly = describe_build("0.1.7", UpdateChannel::Nightly);
        assert!(nightly.starts_with("0.1.7"), "{nightly}");
        assert!(nightly.ends_with(", nightly channel"), "{nightly}");
        assert!(describe_build("0.2.0", UpdateChannel::Stable).ends_with(", stable channel"));
    }

    #[test]
    fn a_debug_build_or_one_without_an_address_does_not_update_itself() {
        assert_eq!(unsupported_reason(true, true, true), Some(DEV_BUILD));
        assert_eq!(unsupported_reason(true, false, false), Some(DEV_BUILD));
        assert_eq!(unsupported_reason(false, false, true), Some(NO_ADDRESS));
        assert_eq!(unsupported_reason(false, true, false), Some(NO_KEY));
        assert_eq!(unsupported_reason(false, true, true), None);
    }

    #[test]
    fn the_public_key_must_be_in_the_configuration() {
        assert!(!has_pubkey(&plugins(serde_json::json!({}))));
        assert!(!has_pubkey(&plugins(serde_json::json!({ "updater": {} }))));
        assert!(!has_pubkey(&plugins(
            serde_json::json!({ "updater": { "pubkey": "  " } })
        )));
        assert!(has_pubkey(&plugins(
            serde_json::json!({ "updater": { "pubkey": "dW50cnVzdGVk" } })
        )));
    }

    #[test]
    fn only_the_first_window_checks_at_launch() {
        let state = UpdaterState::default();
        assert!(state.begin_launch_check());
        assert!(!state.begin_launch_check());
        assert!(!state.begin_launch_check());
    }

    #[test]
    fn progress_is_reported_in_steps_and_at_the_end() {
        let total = Some(PROGRESS_STEP * 2 + 10);
        let mut meter = ProgressMeter::default();
        // The first chunk is always reported, so the bar leaves zero at once.
        assert_eq!(
            meter.add(1000, total),
            Some(UpdateProgress {
                downloaded: 1000,
                total
            })
        );
        // Small chunks are counted, not reported, until a step's worth has come in.
        assert_eq!(meter.add(1000, total), None);
        let step = meter.add(PROGRESS_STEP as usize, total).expect("a step");
        assert_eq!(step.downloaded, PROGRESS_STEP + 2000);
        assert_eq!(meter.add(100, total), None);
        // The last chunk is reported whatever its size.
        let rest = (PROGRESS_STEP * 2 + 10 - (PROGRESS_STEP + 2100)) as usize;
        let last = meter.add(rest, total).expect("the end");
        assert_eq!(Some(last.downloaded), total);
        assert_eq!(meter.done().total, total);
    }

    #[test]
    fn progress_without_a_known_size_still_moves() {
        let mut meter = ProgressMeter::default();
        assert!(meter.add(10, None).is_some());
        assert!(meter.add(10, None).is_none());
        assert!(meter.add(PROGRESS_STEP as usize, None).is_some());
        assert_eq!(
            meter.done(),
            UpdateProgress {
                downloaded: PROGRESS_STEP + 20,
                total: Some(PROGRESS_STEP + 20)
            }
        );
    }

    #[test]
    fn nothing_to_download_or_install_before_a_check_found_an_update() {
        let state = UpdaterState::default();
        let err = shell_block_on(download(&state, |_| {})).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        let err = shell_block_on(install(&state)).unwrap_err();
        assert!(
            matches!(&err, AppError::InvalidInput(m) if m == "The update has not been downloaded yet."),
            "{err:?}"
        );
        // Cancelling with no download under way is a no-op.
        cancel(&state);
    }

    fn shell_block_on<F: std::future::Future>(fut: F) -> F::Output {
        tauri::async_runtime::block_on(fut)
    }

    #[test]
    fn failures_read_as_sentences() {
        let e = check_failure(PluginError::ReleaseNotFound);
        assert!(e.to_string().starts_with("No release information"), "{e}");
        let e = check_failure(PluginError::TargetsNotFound(vec!["darwin-aarch64".into()]));
        assert_eq!(
            e.to_string(),
            "The newest release has no build for this computer."
        );
        let e = download_failure(PluginError::MissingSignedVersion);
        assert!(
            e.to_string().contains("not signed by Tabler's release key"),
            "{e}"
        );
        let e = install_failure(PluginError::TempDirNotOnSameMountPoint);
        assert!(
            e.to_string().contains("Move it to the Applications folder"),
            "{e}"
        );
        let read_only = std::io::Error::from(std::io::ErrorKind::ReadOnlyFilesystem);
        let e = install_failure(PluginError::Io(read_only));
        assert!(
            e.to_string().contains("Move it to the Applications folder"),
            "{e}"
        );
        let e = install_failure(PluginError::InvalidUpdaterFormat);
        assert_eq!(
            e.to_string(),
            "Could not install the update: invalid updater binary format"
        );
        let e = download_failure(PluginError::Network("status 404".into()));
        assert!(
            e.to_string().starts_with("Could not download the update"),
            "{e}"
        );
    }
}
