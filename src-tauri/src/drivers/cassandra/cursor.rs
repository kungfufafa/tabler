//! The grid's page cursor: the driver's paging state as an opaque `String`
//! (`BrowseRequest.cursor`, `BrowsePage.next_cursor`). CQL has no OFFSET, so the next page is
//! asked for with the state the server returned for the previous one.
//!
//! A paging state only means something to the statement and values that produced it, so the
//! cursor carries a fingerprint of them: a cursor from before the filter or sort changed is
//! refused instead of paging another query from a wrong place.

use ::scylla::response::{PagingState, PagingStateResponse};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::error::AppError;
use crate::types::value::Value;

const PREFIX: &str = "cql1.";
/// Paging states are a few dozen bytes; a cursor far beyond is not ours.
const MAX_STATE: usize = 64 * 1024;
const FP_LEN: usize = 8;

/// The fingerprint of a statement and its bound values (FNV-1a: no security rests on it, it
/// only tells queries apart).
pub fn fingerprint(cql: &str, params: &[Value]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(cql.as_bytes());
    eat(&[0xff]);
    eat(format!("{params:?}").as_bytes());
    h
}

/// The cursor of the next page; None when the server says there is none.
pub fn encode(response: &PagingStateResponse, fingerprint: u64) -> Option<String> {
    let PagingStateResponse::HasMorePages { state } = response else {
        return None;
    };
    // A state without bytes would start over: no next page rather than the first one again.
    let raw = state.as_bytes_slice().filter(|b| !b.is_empty())?;
    let mut bytes = Vec::with_capacity(FP_LEN + raw.len());
    bytes.extend_from_slice(&fingerprint.to_be_bytes());
    bytes.extend_from_slice(raw);
    Some(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

/// The paging state a cursor resumes from: the start without one.
pub fn decode(cursor: Option<&str>, fingerprint: u64) -> Result<PagingState, AppError> {
    let Some(cursor) = cursor.filter(|c| !c.is_empty()) else {
        return Ok(PagingState::start());
    };
    let invalid = || AppError::InvalidInput("not a Cassandra page cursor".into());
    let body = cursor.strip_prefix(PREFIX).ok_or_else(invalid)?;
    if body.len() > (FP_LEN + MAX_STATE).div_ceil(3) * 4 {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
    if bytes.len() <= FP_LEN {
        return Err(invalid());
    }
    let (fp, state) = bytes.split_at(FP_LEN);
    if fp != fingerprint.to_be_bytes() {
        return Err(AppError::InvalidInput(
            "the page cursor belongs to another query (the filter or sort changed): reload from \
             the first page"
                .into(),
        ));
    }
    Ok(PagingState::new_from_raw_bytes(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_binds_to_its_query() {
        let fp = fingerprint("SELECT * FROM \"ks\".\"t\" LIMIT ?", &[Value::Int(300)]);
        let state = vec![0u8, 1, 2, 0xff, 0xfe];
        let more = PagingStateResponse::HasMorePages {
            state: PagingState::new_from_raw_bytes(state.clone()),
        };
        let cursor = encode(&more, fp).unwrap();
        assert!(cursor.starts_with("cql1.") && !cursor.contains(['+', '/', '=']));
        let back = decode(Some(&cursor), fp).unwrap();
        assert_eq!(back.as_bytes_slice().map(|b| b.to_vec()), Some(state));

        // Another statement or other values: refused.
        let other = fingerprint("SELECT * FROM \"ks\".\"t\" LIMIT ?", &[Value::Int(100)]);
        assert_ne!(fp, other);
        assert!(matches!(
            decode(Some(&cursor), other),
            Err(AppError::InvalidInput(m)) if m.contains("another query")
        ));
        assert_ne!(
            fp,
            fingerprint("SELECT * FROM \"ks\".\"u\" LIMIT ?", &[Value::Int(300)])
        );
    }

    #[test]
    fn start_end_and_garbage() {
        let fp = 42;
        assert_eq!(decode(None, fp).unwrap(), PagingState::start());
        assert_eq!(decode(Some(""), fp).unwrap(), PagingState::start());
        assert_eq!(encode(&PagingStateResponse::NoMorePages, fp), None);
        let empty = PagingStateResponse::HasMorePages {
            state: PagingState::new_from_raw_bytes(Vec::new()),
        };
        assert_eq!(encode(&empty, fp), None);
        let start = PagingStateResponse::HasMorePages {
            state: PagingState::start(),
        };
        assert_eq!(encode(&start, fp), None);
        for bad in [
            "x",
            "cql1.",
            "cql1.!!!",
            "cql2.AAAAAAAAAAAB",
            "cql1.AAAAAAAAAAA",
            &format!("cql1.{}", "A".repeat(200_000)),
        ] {
            assert!(
                matches!(decode(Some(bad), fp), Err(AppError::InvalidInput(_))),
                "{bad:?}"
            );
        }
    }
}
