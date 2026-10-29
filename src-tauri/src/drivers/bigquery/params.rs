//! The profile's BigQuery settings (`options.driver_params`) and the typed query parameters
//! `@p1…@pn` the client sends with a statement.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::decode::is_json_number;
use super::transport::credentials_may_go_to;
use crate::error::AppError;
use crate::types::value::Value;

/// 1 GiB: the cost guard a new profile starts with (plan §1.5 Q8).
pub const DEFAULT_MAXIMUM_BYTES_BILLED: u64 = 1 << 30;
pub const DEFAULT_API_ENDPOINT: &str = "https://bigquery.googleapis.com/bigquery/v2";

/// How the connection signs in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BigqueryAuth {
    /// A service-account key file (`keyFilePath`).
    #[default]
    ServiceAccount,
    /// Application Default Credentials: `GOOGLE_APPLICATION_CREDENTIALS` or the file of
    /// `gcloud auth application-default login`.
    ApplicationDefault,
    /// The profile's secret is an access token (Command mode: `gcloud auth print-access-token`).
    AccessToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct BigqueryParams {
    pub auth: BigqueryAuth,
    /// The service-account key file, read at connect and never stored.
    #[ts(optional = nullable)]
    pub key_file_path: Option<String>,
    /// Where jobs run (`US`, `EU`, `asia-southeast2`); needed outside the US and EU
    /// multi-regions.
    #[ts(optional = nullable)]
    pub location: Option<String>,
    /// The dataset unqualified table names resolve in.
    #[ts(optional = nullable)]
    pub default_dataset: Option<String>,
    /// Other projects listed as databases (`bigquery-public-data`).
    pub extra_projects: Vec<String>,
    /// The cost guard of every query; `None` is unlimited (the form warns).
    #[ts(type = "number | null")]
    pub maximum_bytes_billed: Option<u64>,
    pub use_query_cache: bool,
    /// Stops a job on the server after this long; `None`: BigQuery's own limit.
    #[ts(type = "number | null")]
    pub job_timeout_ms: Option<u64>,
    /// Job labels (lower-case keys and values).
    pub labels: BTreeMap<String, String>,
    /// Another REST endpoint: HTTPS, or plain HTTP to this machine (an emulator,
    /// `http://localhost:9050/bigquery/v2`).
    #[ts(optional = nullable)]
    pub api_endpoint: Option<String>,
}

impl Default for BigqueryParams {
    fn default() -> Self {
        BigqueryParams {
            auth: BigqueryAuth::default(),
            key_file_path: None,
            location: None,
            default_dataset: None,
            extra_projects: Vec::new(),
            maximum_bytes_billed: Some(DEFAULT_MAXIMUM_BYTES_BILLED),
            use_query_cache: true,
            job_timeout_ms: None,
            labels: BTreeMap::new(),
            api_endpoint: None,
        }
    }
}

impl BigqueryParams {
    /// From `options.driver_params` (missing fields take their defaults).
    pub fn from_value(v: Option<&serde_json::Value>) -> Result<BigqueryParams, AppError> {
        match v {
            None | Some(serde_json::Value::Null) => Ok(BigqueryParams::default()),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| AppError::InvalidInput(format!("BigQuery settings: {e}"))),
        }
    }

    /// The checks of a profile being saved or tested.
    pub fn validate(&self) -> Result<(), AppError> {
        let bad = |m: String| Err(AppError::InvalidInput(m));
        let set = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.trim().is_empty());
        if self.auth == BigqueryAuth::ServiceAccount && !set(&self.key_file_path) {
            return bad("BigQuery: choose the service account's key file (JSON).".into());
        }
        if self.maximum_bytes_billed == Some(0) {
            return bad(
                "BigQuery: Maximum bytes billed must be more than 0 (or empty for no limit)."
                    .into(),
            );
        }
        // The form sends numbers through JavaScript.
        const SAFE: u64 = (1 << 53) - 1;
        if self.maximum_bytes_billed.is_some_and(|b| b > SAFE)
            || self.job_timeout_ms.is_some_and(|t| t > SAFE)
        {
            return bad("BigQuery: the number is too large.".into());
        }
        if let Some(d) = self
            .default_dataset
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            && !d.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return bad(format!(
                "BigQuery: \u{201c}{d}\u{201d} is no dataset name (letters, digits and underscores)."
            ));
        }
        for (k, v) in &self.labels {
            let ok = |s: &str, first: bool| {
                s.chars().count() <= 63
                    && s.chars()
                        .all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
                    && (!first || s.starts_with(|c: char| c.is_lowercase()))
            };
            if !ok(k, true) || !ok(v, false) {
                return bad(format!(
                    "BigQuery: the label \u{201c}{k}\u{201d} must use lower-case letters, digits, \
                     _ and - (63 at most, the key starting with a letter)."
                ));
            }
        }
        // Every call carries the bearer token, so the same rule as a token endpoint's.
        if let Some(e) = self
            .api_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            && !credentials_may_go_to(e)
        {
            return bad(format!(
                "BigQuery: the API endpoint \u{201c}{e}\u{201d} must be an https URL (plain http \
                 only to this machine)."
            ));
        }
        Ok(())
    }

    /// The REST base URL, without a trailing slash.
    pub fn endpoint(&self) -> String {
        self.api_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(DEFAULT_API_ENDPOINT)
            .trim_end_matches('/')
            .to_string()
    }

    pub fn location(&self) -> Option<&str> {
        self.location
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
    }

    pub fn default_dataset(&self) -> Option<&str> {
        self.default_dataset
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }
}

/// The named parameters of `@p1…@pn` (`dialect::bigquery` placeholders) for `values`.
pub fn query_parameters(values: &[Value]) -> Result<Vec<serde_json::Value>, AppError> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| query_parameter(&format!("p{}", i + 1), v))
        .collect()
}

/// One `QueryParameter`: its name, type and value. NULL is a typed parameter without a value.
pub fn query_parameter(name: &str, v: &Value) -> Result<serde_json::Value, AppError> {
    Ok(serde_json::json!({
        "name": name,
        "parameterType": parameter_type(v)?,
        "parameterValue": parameter_value(v)?,
    }))
}

fn scalar_type(ty: &str) -> serde_json::Value {
    serde_json::json!({ "type": ty })
}

fn parameter_type(v: &Value) -> Result<serde_json::Value, AppError> {
    Ok(match v {
        Value::Null => scalar_type("STRING"),
        Value::Bool(_) => scalar_type("BOOL"),
        Value::Int(_) => scalar_type("INT64"),
        Value::BigInt(s) if s.trim().parse::<i64>().is_ok() => scalar_type("INT64"),
        Value::BigInt(_) => scalar_type("BIGNUMERIC"),
        Value::Float(_) => scalar_type("FLOAT64"),
        Value::Decimal(s) => scalar_type(numeric_type(s)),
        Value::Text(_) | Value::Uuid(_) | Value::Enum(_) => scalar_type("STRING"),
        Value::Bytes(_) => scalar_type("BYTES"),
        Value::Date(_) => scalar_type("DATE"),
        Value::Time(_) => scalar_type("TIME"),
        Value::Timestamp(_) => scalar_type("DATETIME"),
        Value::TimestampTz(_) => scalar_type("TIMESTAMP"),
        Value::Interval(_) => scalar_type("INTERVAL"),
        Value::Json(_) => scalar_type("JSON"),
        Value::Other(o)
            if o.ty.eq_ignore_ascii_case("geometry") || o.ty.eq_ignore_ascii_case("geography") =>
        {
            scalar_type("GEOGRAPHY")
        }
        Value::Other(_) => scalar_type("STRING"),
        Value::Array(items) => {
            let first = items.iter().find(|i| !i.is_null());
            let element = match first {
                Some(Value::Array(_)) => {
                    return Err(AppError::InvalidInput(
                        "BigQuery has no arrays of arrays".into(),
                    ));
                }
                Some(e) => parameter_type(e)?,
                None => scalar_type("STRING"),
            };
            serde_json::json!({ "type": "ARRAY", "arrayType": element })
        }
        Value::Default => {
            return Err(AppError::InvalidInput(
                "DEFAULT is a keyword, not a parameter value".into(),
            ));
        }
    })
}

/// NUMERIC holds 29 integer and 9 fraction digits; anything larger goes as BIGNUMERIC.
fn numeric_type(s: &str) -> &'static str {
    let s = s.trim().trim_start_matches('-');
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let plain = !s.contains(['e', 'E']);
    if plain && int.trim_start_matches('0').len() <= 29 && frac.len() <= 9 {
        "NUMERIC"
    } else {
        "BIGNUMERIC"
    }
}

fn parameter_value(v: &Value) -> Result<serde_json::Value, AppError> {
    let scalar = |s: String| serde_json::json!({ "value": s });
    Ok(match v {
        Value::Null => serde_json::json!({ "value": null }),
        Value::Bool(b) => scalar(b.to_string()),
        Value::Int(n) => scalar(n.to_string()),
        Value::Float(f) => scalar(f.to_string()),
        Value::BigInt(s) | Value::Decimal(s) => {
            let s = s.trim();
            if !is_json_number(s) {
                return Err(AppError::InvalidInput(format!(
                    "\u{201c}{s}\u{201d} is not a number"
                )));
            }
            scalar(s.to_string())
        }
        Value::Bytes(b) if b.truncated => {
            return Err(AppError::InvalidInput(
                "the value was cut for display; load it whole before using it".into(),
            ));
        }
        Value::Bytes(b) => scalar(b.base64.clone()),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if item.is_null() {
                    return Err(AppError::InvalidInput(
                        "BigQuery arrays cannot hold NULL".into(),
                    ));
                }
                out.push(parameter_value(item)?);
            }
            serde_json::json!({ "arrayValues": out })
        }
        Value::Default => {
            return Err(AppError::InvalidInput(
                "DEFAULT is a keyword, not a parameter value".into(),
            ));
        }
        other => scalar(other.as_text().unwrap_or_default()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::{BytesValue, OtherValue};
    use serde_json::json;

    #[test]
    fn params_default_and_validate() {
        let p = BigqueryParams::from_value(None).unwrap();
        assert_eq!(p.maximum_bytes_billed, Some(DEFAULT_MAXIMUM_BYTES_BILLED));
        assert!(p.use_query_cache);
        assert_eq!(p.endpoint(), DEFAULT_API_ENDPOINT);
        assert!(p.validate().is_err(), "a key file is needed");
        let p = BigqueryParams::from_value(Some(&json!({
            "auth": "applicationDefault",
            "location": " asia-southeast2 ",
            "defaultDataset": "shop",
            "extraProjects": ["bigquery-public-data"],
            "maximumBytesBilled": null,
            "labels": {"team": "data-1"},
            "apiEndpoint": "http://localhost:9050/bigquery/v2/"
        })))
        .unwrap();
        p.validate().unwrap();
        assert_eq!(p.location(), Some("asia-southeast2"));
        assert_eq!(p.default_dataset(), Some("shop"));
        assert_eq!(p.maximum_bytes_billed, None);
        assert_eq!(p.endpoint(), "http://localhost:9050/bigquery/v2");
        for bad in [
            json!({"auth": "accessToken", "maximumBytesBilled": 0}),
            json!({"auth": "accessToken", "defaultDataset": "a.b"}),
            json!({"auth": "accessToken", "labels": {"Team": "x"}}),
            json!({"auth": "accessToken", "labels": {"t": "X Y"}}),
            json!({"auth": "accessToken", "apiEndpoint": "ftp://x"}),
            json!({"auth": "accessToken", "apiEndpoint": "http://bq.example/bigquery/v2"}),
            json!({"auth": "accessToken", "apiEndpoint": "http://localhost.example.com/v2"}),
            json!({"auth": "accessToken", "apiEndpoint": "http://127.0.0.1.nip.io/v2"}),
            json!({"auth": "accessToken", "apiEndpoint": "http://10.0.0.5:9050/bigquery/v2"}),
            json!({"auth": "accessToken", "jobTimeoutMs": 9007199254740992u64}),
        ] {
            let p = BigqueryParams::from_value(Some(&bad)).unwrap();
            assert!(p.validate().is_err(), "{bad}");
        }
        assert!(BigqueryParams::from_value(Some(&json!({"auth": "magic"}))).is_err());
        let ok = BigqueryParams::from_value(Some(&json!({"keyFilePath": "C:\\k.json"}))).unwrap();
        ok.validate().unwrap();
        for e in [
            "https://bq.example/bigquery/v2",
            "http://127.0.0.1:9050/bigquery/v2",
            "http://[::1]:9050",
        ] {
            let p =
                BigqueryParams::from_value(Some(&json!({"auth": "accessToken", "apiEndpoint": e})))
                    .unwrap();
            p.validate().unwrap();
        }
    }

    #[test]
    fn values_become_typed_parameters() {
        let ps = query_parameters(&[
            Value::Null,
            Value::Bool(true),
            Value::Int(-7),
            Value::BigInt("9007199254740993".into()),
            Value::BigInt("99999999999999999999".into()),
            Value::Float(1.5),
            Value::Decimal("12.345".into()),
            Value::Decimal("1234567890123456789012345678901.5".into()),
            Value::Text("it's".into()),
            Value::from_bytes(&[0, 255]),
            Value::Date("2024-01-01".into()),
            Value::Timestamp("2024-01-01 10:00:00".into()),
            Value::TimestampTz("2024-01-01 10:00:00+07".into()),
            Value::Json("{\"a\":1}".into()),
            Value::Other(OtherValue {
                ty: "geometry".into(),
                text: "POINT(1 2)".into(),
            }),
            Value::Array(vec![Value::Int(1), Value::Int(2)]),
            Value::Array(vec![]),
        ])
        .unwrap();
        let ty = |i: usize| ps[i]["parameterType"].clone();
        let val = |i: usize| ps[i]["parameterValue"].clone();
        assert_eq!(ps[0]["name"], "p1");
        assert_eq!(val(0), json!({"value": null}));
        assert_eq!(
            (ty(1), val(1)),
            (json!({"type": "BOOL"}), json!({"value": "true"}))
        );
        assert_eq!(
            (ty(2), val(2)),
            (json!({"type": "INT64"}), json!({"value": "-7"}))
        );
        assert_eq!(ty(3), json!({"type": "INT64"}));
        assert_eq!(ty(4), json!({"type": "BIGNUMERIC"}));
        assert_eq!(val(5), json!({"value": "1.5"}));
        assert_eq!(ty(6), json!({"type": "NUMERIC"}));
        assert_eq!(ty(7), json!({"type": "BIGNUMERIC"}));
        assert_eq!(val(8), json!({"value": "it's"}));
        assert_eq!(
            (ty(9), val(9)),
            (json!({"type": "BYTES"}), json!({"value": "AP8="}))
        );
        assert_eq!(ty(10), json!({"type": "DATE"}));
        assert_eq!(ty(11), json!({"type": "DATETIME"}));
        assert_eq!(ty(12), json!({"type": "TIMESTAMP"}));
        assert_eq!(ty(13), json!({"type": "JSON"}));
        assert_eq!(ty(14), json!({"type": "GEOGRAPHY"}));
        assert_eq!(
            (ty(15), val(15)),
            (
                json!({"type": "ARRAY", "arrayType": {"type": "INT64"}}),
                json!({"arrayValues": [{"value": "1"}, {"value": "2"}]})
            )
        );
        assert_eq!(
            ty(16),
            json!({"type": "ARRAY", "arrayType": {"type": "STRING"}})
        );
        assert_eq!(ps[16]["name"], "p17");
    }

    #[test]
    fn values_bigquery_cannot_take_are_refused() {
        let truncated = Value::Bytes(BytesValue {
            base64: "AA==".into(),
            len: 1 << 20,
            truncated: true,
        });
        for v in [
            Value::Default,
            truncated,
            Value::Array(vec![Value::Array(vec![Value::Int(1)])]),
            Value::Array(vec![Value::Int(1), Value::Null]),
            Value::Decimal("1; DROP".into()),
        ] {
            assert!(query_parameters(std::slice::from_ref(&v)).is_err(), "{v:?}");
        }
    }
}
