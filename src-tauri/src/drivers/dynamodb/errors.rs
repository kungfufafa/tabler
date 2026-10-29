//! DynamoDB's error replies (`{"__type": "com.amazonaws.dynamodb.v20120810#Code", "message": …}`)
//! read into [`ServiceError`], what the client retries (throttling, a clock that drifted, an
//! expired token, server errors of requests that can be repeated), and `AppError`s: bad
//! credentials are `Auth`; everything else the server says is `Db` with the code in `detail`
//! (DynamoDB has no numeric codes for `vendor_code`). A failed condition reads like the core
//! drivers' affected-row guard; a cancelled transaction says which of its items failed and why.

use crate::error::AppError;

use super::transport::{HttpResponse, header};

/// What a failed `ConditionExpression` of an edit means for the user (the wording of
/// `drivers::check_affected`).
pub const CONDITION_FAILED: &str = "expected to affect 1 row(s) but matched 0; the item may have \
     been changed or deleted by someone else";

/// One item's outcome in a cancelled transaction (`CancellationReasons`, in request order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationReason {
    /// `None` (this item was fine), `ConditionalCheckFailed`, `TransactionConflict`,
    /// `ItemCollectionSizeLimitExceeded`, `ProvisionedThroughputExceeded`, `ThrottlingError`,
    /// `ValidationError`.
    pub code: String,
    pub message: Option<String>,
}

impl CancellationReason {
    pub fn failed(&self) -> bool {
        !self.code.is_empty() && self.code != "None"
    }

    /// The error of this item, in the user's words.
    pub fn text(&self) -> String {
        match self.code.as_str() {
            "ConditionalCheckFailed" => CONDITION_FAILED.to_string(),
            "TransactionConflict" => {
                "another request changed the item at the same time; try again".to_string()
            }
            code => match &self.message {
                Some(m) if !m.is_empty() => format!("{code}: {m}"),
                _ => code.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    pub status: u16,
    /// The exception name without its namespace (`ConditionalCheckFailedException`); empty when
    /// the reply named none.
    pub code: String,
    pub message: String,
    pub cancellation_reasons: Vec<CancellationReason>,
    pub request_id: Option<String>,
}

/// Errors that say the request was throttled: it did nothing and can be repeated after a pause.
const THROTTLING: &[&str] = &[
    "ProvisionedThroughputExceededException",
    "ThrottlingException",
    "Throttling",
    "RequestLimitExceeded",
    "TransactionInProgressException",
];

/// Errors that mean the credentials were refused.
const AUTH: &[&str] = &[
    "UnrecognizedClientException",
    "InvalidSignatureException",
    "IncompleteSignatureException",
    "MissingAuthenticationTokenException",
    "MissingAuthenticationToken",
    "InvalidClientTokenId",
    "SignatureDoesNotMatch",
    "ExpiredTokenException",
    "ExpiredToken",
];

impl ServiceError {
    /// The error of a reply that is not a success. A body that is not DynamoDB's JSON (a proxy's
    /// HTML page) keeps a short excerpt as the message.
    pub fn from_response(resp: &HttpResponse) -> ServiceError {
        let json: Option<serde_json::Value> = serde_json::from_slice(&resp.body).ok();
        let field = |k: &str| {
            json.as_ref()
                .and_then(|j| j.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let raw_code = field("__type")
            .or_else(|| field("code"))
            .or_else(|| resp.header(header::ERROR_TYPE).map(str::to_string))
            .unwrap_or_default();
        // `ns#Code`, and the header's `Code:http://…` form.
        let code = raw_code
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let message = field("message")
            .or_else(|| field("Message"))
            .unwrap_or_else(|| {
                if json.is_some() {
                    String::new()
                } else {
                    excerpt(&resp.body)
                }
            });
        let cancellation_reasons = json
            .as_ref()
            .and_then(|j| j.get("CancellationReasons"))
            .and_then(|r| r.as_array())
            .map(|reasons| {
                reasons
                    .iter()
                    .map(|r| CancellationReason {
                        code: r
                            .get("Code")
                            .and_then(|c| c.as_str())
                            .unwrap_or("None")
                            .to_string(),
                        message: r
                            .get("Message")
                            .and_then(|m| m.as_str())
                            .map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default();
        ServiceError {
            status: resp.status,
            code,
            message,
            cancellation_reasons,
            request_id: resp.header(header::REQUEST_ID).map(str::to_string),
        }
    }

    pub fn is_throttling(&self) -> bool {
        THROTTLING.contains(&self.code.as_str()) || self.status == 429
    }

    /// A server-side failure: the request may or may not have been applied.
    pub fn is_server_error(&self) -> bool {
        self.status >= 500
            || matches!(
                self.code.as_str(),
                "InternalServerError" | "ServiceUnavailable"
            )
    }

    /// The signature's time is too far from the server's.
    pub fn is_clock_skew(&self) -> bool {
        let m = self.message.to_ascii_lowercase();
        matches!(
            self.code.as_str(),
            "InvalidSignatureException" | "RequestExpired" | "RequestTimeTooSkewed"
        ) && (m.contains("signature expired")
            || m.contains("signature not yet current")
            || self.code != "InvalidSignatureException")
    }

    /// Temporary credentials ran out.
    pub fn is_expired_token(&self) -> bool {
        matches!(self.code.as_str(), "ExpiredTokenException" | "ExpiredToken")
    }

    pub fn is_condition_failed(&self) -> bool {
        self.code == "ConditionalCheckFailedException"
    }

    pub fn is_transaction_canceled(&self) -> bool {
        self.code == "TransactionCanceledException"
    }

    fn said(&self) -> String {
        if self.message.is_empty() {
            self.code.clone()
        } else {
            self.message.clone()
        }
    }

    pub fn into_app_error(self) -> AppError {
        if AUTH.contains(&self.code.as_str()) || (self.code.is_empty() && self.status == 403) {
            return AppError::Auth(format!(
                "DynamoDB refused the credentials ({}): {}",
                if self.code.is_empty() {
                    format!("HTTP {}", self.status)
                } else {
                    self.code.clone()
                },
                self.said()
            ));
        }
        if self.code.is_empty() && matches!(self.status, 502..=504) {
            return AppError::Connect(format!(
                "DynamoDB is unavailable (HTTP {}): {}",
                self.status, self.message
            ));
        }
        let (message, hint) = if self.is_condition_failed() {
            (CONDITION_FAILED.to_string(), None)
        } else if self.is_transaction_canceled() && !self.cancellation_reasons.is_empty() {
            let failed: Vec<String> = self
                .cancellation_reasons
                .iter()
                .enumerate()
                .filter(|(_, r)| r.failed())
                .map(|(i, r)| format!("item {}: {}", i + 1, r.text()))
                .collect();
            (
                format!(
                    "the transaction was cancelled and nothing was applied ({})",
                    failed.join("; ")
                ),
                None,
            )
        } else if self.is_throttling() {
            (
                format!("DynamoDB throttled the request: {}", self.said()),
                Some(
                    "The table's capacity or the account's request rate was exceeded. Retry in a \
                     moment, or raise the table's capacity."
                        .to_string(),
                ),
            )
        } else if self.code == "AccessDeniedException" {
            (
                self.said(),
                Some("The IAM policy of these credentials does not allow this action.".into()),
            )
        } else if self.code.is_empty() {
            (
                format!("DynamoDB answered HTTP {}: {}", self.status, self.message),
                None,
            )
        } else {
            (self.said(), None)
        };
        AppError::Db {
            message,
            detail: (!self.code.is_empty()).then_some(self.code),
            hint,
            sqlstate: None,
            vendor_code: None,
            position: None,
            statement_index: None,
        }
    }
}

/// At most this many characters of a body that is not JSON go into a message.
const EXCERPT_CHARS: usize = 200;

fn excerpt(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut plain = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                plain.push(' ');
            }
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > EXCERPT_CHARS {
        format!(
            "{}…",
            collapsed.chars().take(EXCERPT_CHARS).collect::<String>()
        )
    } else {
        collapsed
    }
}

/// The error of each item of a cancelled transaction, in request order: the item's own reason,
/// or that nothing was applied because of another item.
pub fn transaction_item_errors(reasons: &[CancellationReason], items: usize) -> Vec<String> {
    let first_failed = reasons.iter().position(CancellationReason::failed);
    (0..items)
        .map(|i| match reasons.get(i) {
            Some(r) if r.failed() => r.text(),
            _ => match first_failed {
                Some(f) => format!(
                    "not applied: the transaction was cancelled because item {} failed",
                    f + 1
                ),
                None => "not applied: the transaction was cancelled".to_string(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, body: serde_json::Value) -> ServiceError {
        ServiceError::from_response(&HttpResponse::json(status, &body))
    }

    #[test]
    fn codes_and_messages() {
        let e = reply(
            400,
            serde_json::json!({
                "__type": "com.amazonaws.dynamodb.v20120810#ResourceNotFoundException",
                "message": "Requested resource not found"
            }),
        );
        assert_eq!(e.code, "ResourceNotFoundException");
        match e.into_app_error() {
            AppError::Db {
                message,
                detail,
                vendor_code,
                ..
            } => {
                assert_eq!(message, "Requested resource not found");
                assert_eq!(detail.as_deref(), Some("ResourceNotFoundException"));
                assert_eq!(vendor_code, None);
            }
            other => panic!("{other:?}"),
        }
        let header_only = ServiceError::from_response(&HttpResponse {
            status: 400,
            headers: vec![(
                header::ERROR_TYPE.into(),
                "ValidationException:http://internal.amazon.com/coral/".into(),
            )],
            body: br#"{"Message":"1 validation error"}"#.to_vec(),
        });
        assert_eq!(header_only.code, "ValidationException");
        assert_eq!(header_only.message, "1 validation error");
    }

    #[test]
    fn credentials_errors_are_auth() {
        for code in [
            "UnrecognizedClientException",
            "MissingAuthenticationTokenException",
            "ExpiredTokenException",
        ] {
            let e = reply(
                400,
                serde_json::json!({"__type": format!("com.amazon.coral.service#{code}"), "message": "no"}),
            );
            assert!(matches!(e.into_app_error(), AppError::Auth(_)), "{code}");
        }
        let proxy = ServiceError::from_response(&HttpResponse {
            status: 403,
            headers: vec![],
            body: b"<html><body>Forbidden</body></html>".to_vec(),
        });
        assert_eq!(proxy.message, "Forbidden");
        assert!(matches!(proxy.into_app_error(), AppError::Auth(_)));
        let gateway = ServiceError::from_response(&HttpResponse {
            status: 502,
            headers: vec![],
            body: b"<h1>Bad Gateway</h1>".to_vec(),
        });
        assert!(gateway.is_server_error());
        assert!(matches!(gateway.into_app_error(), AppError::Connect(_)));
    }

    #[test]
    fn retry_classes() {
        let throttled = reply(
            400,
            serde_json::json!({"__type": "com.amazonaws.dynamodb.v20120810#ProvisionedThroughputExceededException", "message": "slow down"}),
        );
        assert!(throttled.is_throttling() && !throttled.is_server_error());
        match throttled.into_app_error() {
            AppError::Db { hint, .. } => assert!(hint.unwrap().contains("capacity")),
            other => panic!("{other:?}"),
        }
        let internal = reply(
            500,
            serde_json::json!({"__type": "com.amazonaws.dynamodb.v20120810#InternalServerError"}),
        );
        assert!(internal.is_server_error() && !internal.is_throttling());
        let skew = reply(
            400,
            serde_json::json!({"__type": "com.amazon.coral.service#InvalidSignatureException", "message": "Signature expired: 20261029T000000Z is now earlier than 20261029T100000Z (20261029T095500Z - 5 min.)"}),
        );
        assert!(skew.is_clock_skew());
        let bad_sig = reply(
            400,
            serde_json::json!({"__type": "com.amazon.coral.service#InvalidSignatureException", "message": "The request signature we calculated does not match"}),
        );
        assert!(!bad_sig.is_clock_skew());
        let expired = reply(
            400,
            serde_json::json!({"__type": "com.amazon.coral.service#ExpiredTokenException", "message": "The security token included in the request is expired"}),
        );
        assert!(expired.is_expired_token());
    }

    #[test]
    fn conditions_and_cancelled_transactions() {
        let cond = reply(
            400,
            serde_json::json!({"__type": "com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException", "message": "The conditional request failed"}),
        );
        assert!(cond.is_condition_failed());
        assert!(
            cond.into_app_error()
                .to_string()
                .contains("changed or deleted by someone else")
        );
        let tx = reply(
            400,
            serde_json::json!({
                "__type": "com.amazonaws.dynamodb.v20120810#TransactionCanceledException",
                "Message": "Transaction cancelled, please refer cancellation reasons for specific reasons [None, ConditionalCheckFailed]",
                "CancellationReasons": [{"Code": "None"}, {"Code": "ConditionalCheckFailed", "Message": "The conditional request failed"}]
            }),
        );
        assert!(tx.is_transaction_canceled());
        assert_eq!(tx.cancellation_reasons.len(), 2);
        let items = transaction_item_errors(&tx.cancellation_reasons, 3);
        assert!(items[0].contains("because item 2 failed"), "{}", items[0]);
        assert_eq!(items[1], CONDITION_FAILED);
        assert!(items[2].starts_with("not applied"));
        let text = tx.into_app_error().to_string();
        assert!(
            text.contains("item 2:") && text.contains("nothing was applied"),
            "{text}"
        );
        assert_eq!(
            transaction_item_errors(&[], 1),
            ["not applied: the transaction was cancelled"]
        );
    }
}
