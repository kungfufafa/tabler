//! Crash and error reports to Sentry: panics of the backend, and the unhandled errors of the
//! renderer, which `tauri-plugin-sentry` passes through IPC (the webview needs no network access
//! of its own, so the production CSP stays closed).
//!
//! What leaves the machine is the error, its stack trace, the app version and the OS / device
//! context. `send_default_pii` is off, and the breadcrumbs that can hold what the user typed or
//! what a server answered (the console, text inputs) are dropped: this is a database client.

use sentry::Breadcrumb;
use tauri_plugin_sentry::Sentry as SentryPlugin;

/// Overrides where reports go: a DSN sends them there (debug builds included), an empty value
/// turns reporting off.
pub const DSN_ENV: &str = "TABLER_SENTRY_DSN";

/// The `tabler` project of the release builds. A DSN only allows sending events, it is not a
/// secret (it ships inside every installer).
const RELEASE_DSN: &str = "https://e36e29a8354c3cf7ddfe902bcc5e3db6@o4510944541016064.ingest.de.sentry.io/4512413446963280";

/// `tabler@<version>`, the release every event is tagged with.
const RELEASE: &str = concat!("tabler@", env!("CARGO_PKG_VERSION"));

/// Keeps reporting alive for as long as it is held (dropping it flushes what is still queued),
/// and carries the Tauri plugin of the same client.
pub struct Telemetry {
    _guard: sentry::ClientInitGuard,
    plugin: SentryPlugin,
}

impl Telemetry {
    /// The plugin for `tauri::Builder::plugin`: it injects the browser SDK into every webview and
    /// takes its events and breadcrumbs over IPC.
    pub fn plugin(&self) -> SentryPlugin {
        self.plugin.clone()
    }
}

/// The DSN to report to: `env` (the value of [`DSN_ENV`]) wins, and without it only a release
/// build reports. `None` is "do not report".
pub fn resolve_dsn(env: Option<&str>, release_build: bool) -> Option<String> {
    match env.map(str::trim) {
        Some("") => None,
        Some(dsn) => Some(dsn.to_string()),
        None => release_build.then(|| RELEASE_DSN.to_string()),
    }
}

/// Drops the breadcrumbs that can carry SQL text, values or server messages.
pub fn keep_breadcrumb(breadcrumb: Breadcrumb) -> Option<Breadcrumb> {
    match breadcrumb.category.as_deref() {
        Some("console" | "ui.input") => None,
        _ => Some(breadcrumb),
    }
}

/// Starts reporting, or returns `None` when it is off (a debug build without [`DSN_ENV`]). Call
/// it once, before the Tauri builder: the plugin needs the client to exist.
pub fn init() -> Option<Telemetry> {
    let env = std::env::var(DSN_ENV).ok();
    let dsn = resolve_dsn(env.as_deref(), !cfg!(debug_assertions))?;
    let plugin = SentryPlugin::new();
    let options = sentry::ClientOptions::new()
        .add_integration(plugin.clone())
        .dsn(&dsn)
        .release(RELEASE)
        .environment(if cfg!(debug_assertions) {
            "development"
        } else {
            "production"
        })
        .send_default_pii(false)
        .auto_session_tracking(true)
        .before_breadcrumb(keep_breadcrumb);
    let guard = sentry::init(options);
    guard.is_enabled().then_some(Telemetry {
        _guard: guard,
        plugin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debug_build_reports_only_when_told_where() {
        assert_eq!(resolve_dsn(None, false), None);
        assert_eq!(
            resolve_dsn(Some("https://key@example.test/1"), false).as_deref(),
            Some("https://key@example.test/1")
        );
    }

    #[test]
    fn a_release_build_reports_unless_turned_off() {
        assert_eq!(resolve_dsn(None, true).as_deref(), Some(RELEASE_DSN));
        assert_eq!(resolve_dsn(Some(""), true), None);
        assert_eq!(resolve_dsn(Some("  "), true), None);
    }

    #[test]
    fn console_and_input_breadcrumbs_never_leave() {
        let crumb = |category: &str| Breadcrumb {
            category: Some(category.to_string()),
            ..Default::default()
        };
        assert!(keep_breadcrumb(crumb("console")).is_none());
        assert!(keep_breadcrumb(crumb("ui.input")).is_none());
        assert!(keep_breadcrumb(crumb("ui.click")).is_some());
        assert!(keep_breadcrumb(Breadcrumb::default()).is_some());
    }
}
