//! In-app updates (`crate::updater`): which releases a build follows, what a check found, and
//! how far a download is.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The releases a build follows: the numbered releases tagged by hand, or the rolling `nightly`
/// pre-release that every build of `main` goes to (`updater::channel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateChannel {
    Stable,
    Nightly,
}

/// What a check for updates found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum UpdateCheck {
    /// This build does not update itself (a development build, or one made without an update
    /// address); `reason` says why, for the dialog.
    Unsupported { reason: String },
    /// The check at launch was already made by another window of this run.
    Skipped,
    /// `version` (the running one) is the newest of its channel.
    UpToDate { version: String },
    /// A newer build is published.
    Available {
        version: String,
        current_version: String,
        /// What changed, as plain text.
        notes: Option<String>,
        /// When it was published (RFC 3339), as the release says.
        date: Option<String>,
        /// It was already downloaded in this run: installing needs no second download.
        downloaded: bool,
    },
}

/// Sent through the progress channel while an update downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateProgress {
    /// Bytes received so far.
    pub downloaded: u64,
    /// The size of the download, when the server says.
    pub total: Option<u64>,
}
