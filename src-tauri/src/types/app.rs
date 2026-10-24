use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::driver::KindId;
use crate::types::profile::Driver;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
    pub debug: bool,
    /// The drivers this build has.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub drivers: Option<Vec<Driver>>,
    /// The databases of the new-connection picker this build can connect to, in its order.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub kinds: Option<Vec<KindId>>,
}
