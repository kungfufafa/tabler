use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::driver::KindId;
use crate::types::profile::Driver;
use crate::types::update::UpdateChannel;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
    pub debug: bool,
    /// The short hash of the commit this build was made from (a development build has none).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub commit: Option<String>,
    /// The releases this build follows when it updates itself.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub channel: Option<UpdateChannel>,
    /// The drivers this build has.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub drivers: Option<Vec<Driver>>,
    /// The databases of the new-connection picker this build can connect to, in its order.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub kinds: Option<Vec<KindId>>,
}
