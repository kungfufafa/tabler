//! The grid's cursors (`BrowseRequest.cursor` / `BrowsePage.next_cursor`): where the next page of
//! a table starts. A plain page of a native table is read for free with `tabledata.list`, and its
//! cursor is the page token; a sorted or filtered page (or any page of a view) is the result of
//! one query job, whose later pages `jobs.getQueryResults` reads without billing again, so its
//! cursor names the job and the page token.
//!
//! A cursor carries a fingerprint of what it pages (the table, or the query and its values): one
//! handed back for another table, filter or sort is refused rather than read from the wrong
//! place. Cursors are opaque text to the renderer (base64url of JSON) and decoding them is total.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use super::jobs::JobReference;
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "camelCase")]
pub enum Cursor {
    /// The next `tabledata.list` page.
    Data { fp: String, token: String },
    /// The next page of a finished query job.
    Job {
        fp: String,
        project: String,
        location: Option<String>,
        job: String,
        token: String,
    },
}

impl Cursor {
    pub fn data(fp: &str, token: &str) -> Cursor {
        Cursor::Data {
            fp: fp.to_string(),
            token: token.to_string(),
        }
    }

    pub fn job(fp: &str, job: &JobReference, token: &str) -> Cursor {
        Cursor::Job {
            fp: fp.to_string(),
            project: job.project_id.clone(),
            location: job.location.clone(),
            job: job.job_id.clone(),
            token: token.to_string(),
        }
    }

    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap_or_default())
    }

    /// The cursor `text` holds, when it pages what `fp` names.
    pub fn decode(text: &str, fp: &str) -> Result<Cursor, AppError> {
        let bytes = URL_SAFE_NO_PAD.decode(text.trim()).map_err(|_| stale())?;
        let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| stale())?;
        let (Cursor::Data { fp: of, .. } | Cursor::Job { fp: of, .. }) = &cursor;
        if of != fp {
            return Err(stale());
        }
        Ok(cursor)
    }
}

/// The error of a cursor that pages something else than the page asked for.
pub fn stale() -> AppError {
    AppError::InvalidInput(
        "BigQuery: this page no longer matches the table, filter or sort; reload the first page"
            .into(),
    )
}

/// What a cursor pages, as a short stable hash of its parts (FNV-1a, 64 bits): the same in every
/// process, so a cursor outlives nothing but its own grid.
pub fn fingerprint(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        // Each part's length first, so ("ab", "c") and ("a", "bc") differ.
        for b in (part.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(part.as_bytes())
        {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_and_refuse_another_page() {
        let fp = fingerprint(&["data", "p", "d", "t"]);
        let c = Cursor::data(&fp, "tok/1");
        assert_eq!(Cursor::decode(&c.encode(), &fp).unwrap(), c);
        let job = JobReference {
            project_id: "p".into(),
            job_id: "tabler_1".into(),
            location: Some("EU".into()),
        };
        let j = Cursor::job("f2", &job, "page2");
        assert_eq!(Cursor::decode(&j.encode(), "f2").unwrap(), j);
        // Another table, filter or sort; junk.
        for (text, fp) in [
            (c.encode(), "other".to_string()),
            ("not base64!".to_string(), fp.clone()),
            (URL_SAFE_NO_PAD.encode(b"{\"k\":\"nope\"}"), fp.clone()),
            (String::new(), fp.clone()),
        ] {
            let e = Cursor::decode(&text, &fp).unwrap_err();
            assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
        }
    }

    #[test]
    fn fingerprints_are_stable_and_tell_parts_apart() {
        assert_eq!(fingerprint(&["a", "b"]), fingerprint(&["a", "b"]));
        assert_ne!(fingerprint(&["ab", "c"]), fingerprint(&["a", "bc"]));
        assert_eq!(fingerprint(&[]).len(), 16);
    }
}
