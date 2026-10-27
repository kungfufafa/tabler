//! JSON import (File ▸ Import ▸ From JSON…): an array of objects, or one object per line
//! (NDJSON), into a table or a new one. The preview reads the first objects: every key seen is a
//! column, with its type detected as for a CSV column. The import goes through the CSV wizard's
//! machinery (`csvimport::import_records`): the file is streamed object by object, inserted in
//! chunks with the same insert options, failures and reports.

use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::drivers::DbSession;
use crate::error::AppError;
use crate::types::caps::Feature;
use crate::types::import::{
    CsvColumn, CsvImportProgress, CsvImportReport, JsonImportRequest, JsonPreview, TextEncoding,
};
use crate::types::schema::TableMeta;

use super::csvimport::{
    CHUNK_CELLS, PREVIEW_ROWS, RecordInput, SAMPLE_ROWS, file_name, import_records, open_file,
    plan_for, sniff,
};
use super::csvread::RecordRead;
use super::detect::TypeGuess;
use super::jsonread::{IgnoredKeys, JsonObjects, JsonRecords};
use super::text::{CountingReader, Utf8Reader, detect_encoding, is_utf8_prefix};

const FILE_BUFFER: usize = 256 * 1024;

/// The encoding to read the file with: the one chosen, else detected from its start.
fn resolve_encoding(path: &str, encoding: Option<TextEncoding>) -> Result<TextEncoding, AppError> {
    match encoding {
        Some(TextEncoding::Utf8) => {
            if !is_utf8_prefix(&sniff(path)?) {
                return Err(AppError::InvalidInput(
                    "the file is not valid UTF-8: choose its encoding".into(),
                ));
            }
            Ok(TextEncoding::Utf8)
        }
        Some(e) => Ok(e),
        None => Ok(detect_encoding(&sniff(path)?)),
    }
}

type JsonFile = BufReader<Utf8Reader<CountingReader<std::fs::File>>>;

fn open_json(
    path: &str,
    encoding: TextEncoding,
    bytes: Arc<AtomicU64>,
) -> Result<(JsonFile, u64), AppError> {
    let (file, size) = open_file(path)?;
    let decoded = Utf8Reader::new(CountingReader::new(file, bytes), encoding);
    Ok((BufReader::with_capacity(FILE_BUFFER, decoded), size))
}

/// A key as a column name: trimmed, `column_<n>` when empty.
fn column_name(key: &str, i: usize) -> String {
    let name = key.trim();
    if name.is_empty() {
        format!("column_{}", i + 1)
    } else {
        name.to_string()
    }
}

/// Reads the preview (blocking: call it off the async workers).
pub fn preview_json(path: &str, encoding: Option<TextEncoding>) -> Result<JsonPreview, AppError> {
    let encoding = resolve_encoding(path, encoding)?;
    let (file, size) = open_json(path, encoding, Arc::new(AtomicU64::new(0)))?;
    let mut objects = JsonObjects::new(file);
    let mut sample = Vec::new();
    while sample.len() < SAMPLE_ROWS {
        match objects.next_object()? {
            Some(o) => sample.push(o),
            None => break,
        }
    }
    let more = sample.len() > PREVIEW_ROWS || objects.next_object()?.is_some();
    let mut keys: Vec<String> = Vec::new();
    for object in &sample {
        for (key, _) in &object.entries {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    let mut guesses = vec![TypeGuess::default(); keys.len()];
    let rows: Vec<Vec<Option<String>>> = sample
        .iter()
        .map(|object| {
            keys.iter()
                .map(|k| {
                    object
                        .entries
                        .iter()
                        .find(|(key, _)| key == k)
                        .and_then(|(_, v)| v.clone())
                })
                .collect()
        })
        .collect();
    for row in &rows {
        for (g, v) in guesses.iter_mut().zip(row) {
            if let Some(v) = v {
                g.add(v);
            }
        }
    }
    let columns = keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let name = column_name(key, i);
            CsvColumn {
                detected: guesses[i].column(&name),
                name,
            }
        })
        .collect();
    Ok(JsonPreview {
        path: path.to_string(),
        file_name: file_name(Path::new(path)),
        size,
        encoding,
        layout: objects.layout(),
        keys,
        columns,
        rows: rows.into_iter().take(PREVIEW_ROWS).collect(),
        sampled_rows: sample.len() as u32,
        more,
    })
}

/// The note about keys the import met that the preview did not list.
fn ignored_note(ignored: &IgnoredKeys) -> Option<String> {
    if ignored.names.is_empty() {
        return None;
    }
    let names = ignored
        .names
        .iter()
        .map(|k| format!("\"{k}\""))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Keys that were not in the previewed objects were not imported: {names}{}.",
        if ignored.more { " and more" } else { "" }
    ))
}

/// Runs a JSON import on `session` (see `csvimport::run_csv_import` for the reporting).
pub async fn run_json_import(
    session: &dyn DbSession,
    req: &JsonImportRequest,
    meta: Option<&TableMeta>,
    progress: impl FnMut(CsvImportProgress) + Send,
    cancel: CancellationToken,
) -> Result<CsvImportReport, AppError> {
    json_import_in_chunks(session, req, meta, progress, cancel, CHUNK_CELLS).await
}

async fn json_import_in_chunks(
    session: &dyn DbSession,
    req: &JsonImportRequest,
    meta: Option<&TableMeta>,
    progress: impl FnMut(CsvImportProgress) + Send,
    cancel: CancellationToken,
    chunk_cells: usize,
) -> Result<CsvImportReport, AppError> {
    if req.keys.is_empty() {
        return Err(AppError::InvalidInput(
            "the JSON file has no keys to import".into(),
        ));
    }
    if req.mapping.len() > req.keys.len() {
        return Err(AppError::InvalidInput(
            "the mapping names more columns than the file has keys".into(),
        ));
    }
    let plan = plan_for(
        session.core_dialect(Feature::ImportJson)?,
        &req.target,
        &req.mapping,
        req.mode,
        meta,
    )?;
    let ignored = Arc::new(Mutex::new(IgnoredKeys::default()));
    let open = |bytes: Arc<AtomicU64>| {
        let encoding = resolve_encoding(&req.path, req.encoding)?;
        let (file, size) = open_json(&req.path, encoding, bytes)?;
        let records = JsonRecords::new(file, &req.keys, ignored.clone());
        Ok((Box::new(records) as Box<dyn RecordRead>, size))
    };
    // JSON null and missing keys are unquoted empty fields: NULL; strings are quoted fields.
    let input = RecordInput {
        header: false,
        empty_as_null: true,
    };
    let mut report = import_records(
        session,
        plan,
        meta,
        input,
        open,
        progress,
        cancel,
        chunk_cells,
    )
    .await?;
    if let Some(note) = ignored_note(&ignored.lock().unwrap_or_else(|p| p.into_inner())) {
        report.notes.push(note);
    }
    Ok(report)
}

#[cfg(test)]
pub(crate) async fn json_import_with_chunks(
    session: &dyn DbSession,
    req: &JsonImportRequest,
    meta: Option<&TableMeta>,
    chunk_cells: usize,
) -> Result<CsvImportReport, AppError> {
    json_import_in_chunks(
        session,
        req,
        meta,
        |_| {},
        CancellationToken::new(),
        chunk_cells,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::import::{ImportType, JsonLayout};
    use std::io::Write;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn previews_keys_types_and_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::from("[\n");
        for i in 1..=150 {
            text.push_str(&format!(
                "  {{\"id\": {i}, \"name\": \"Item {i}\", \"price\": {i}.25, \"active\": {}, \"tags\": [\"a\"]}},\n",
                i % 2 == 0
            ));
        }
        text.push_str("  {\"id\": 151, \"name\": null, \"extra\": \"\"}\n]\n");
        let path = write(dir.path(), "items.json", text.as_bytes());
        let p = preview_json(&path, None).unwrap();
        assert_eq!(p.file_name, "items.json");
        assert_eq!(p.layout, Some(JsonLayout::Array));
        assert_eq!(p.encoding, TextEncoding::Utf8);
        assert_eq!(p.keys, ["id", "name", "price", "active", "tags", "extra"]);
        let types: Vec<ImportType> = p.columns.iter().map(|c| c.detected.ty).collect();
        assert_eq!(
            types,
            [
                ImportType::Integer,
                ImportType::Text,
                ImportType::Numeric,
                ImportType::Boolean,
                ImportType::Text,
                ImportType::Text
            ]
        );
        assert_eq!(p.rows.len(), PREVIEW_ROWS);
        assert_eq!(p.sampled_rows, 151);
        assert!(p.more);
        assert_eq!(
            p.rows[0],
            vec![
                Some("1".into()),
                Some("Item 1".into()),
                Some("1.25".into()),
                Some("false".into()),
                Some("[\"a\"]".into()),
                None
            ]
        );

        // NDJSON with a byte order mark, an empty key.
        let path = write(
            dir.path(),
            "rows.ndjson",
            b"\xEF\xBB\xBF{\"\": 1, \"b\": \"x\"}\n{\"b\": \"y\"}\n",
        );
        let p = preview_json(&path, None).unwrap();
        assert_eq!(p.layout, Some(JsonLayout::Lines));
        assert_eq!(p.keys, ["", "b"]);
        assert_eq!(p.columns[0].name, "column_1");
        assert_eq!(p.rows[1], vec![None, Some("y".into())]);
        assert!(!p.more);

        // Not objects.
        let path = write(dir.path(), "bad.json", b"[1, 2, 3]");
        assert!(preview_json(&path, None).is_err());
        let path = write(dir.path(), "latin.json", b"[{\"a\": \"Zo\xEB\"}]");
        assert!(preview_json(&path, Some(TextEncoding::Utf8)).is_err());
        let p = preview_json(&path, None).unwrap();
        assert_eq!(p.encoding, TextEncoding::Windows1252);
        assert_eq!(p.rows[0][0].as_deref(), Some("Zoë"));
        assert!(preview_json("relative.json", None).is_err());
    }

    async fn session(dir: &Path) -> crate::drivers::sqlite::LiteSession {
        let profile: crate::types::profile::ConnectionProfile =
            serde_json::from_value(serde_json::json!({
                "id": uuid::Uuid::new_v4(),
                "name": "json import test",
                "driver": "sqlite",
                "sqlitePath": dir.join("db.sqlite").to_string_lossy(),
            }))
            .unwrap();
        crate::drivers::sqlite::LiteSession::open(&profile)
            .await
            .unwrap()
    }

    fn def(name: &str, ty: ImportType) -> crate::types::import::ImportColumnDef {
        crate::types::import::ImportColumnDef {
            name: name.into(),
            ty,
            precision: None,
            scale: None,
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn imports_objects_into_a_new_table() {
        use crate::types::import::{ImportTarget, InsertMode};
        use crate::types::schema::TableRef;
        use crate::types::value::Value;
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path()).await;
        let path = write(
            dir.path(),
            "people.json",
            br#"[{"id": 1, "name": "Ann", "score": 9.5, "tags": ["x", 1]},
                 {"id": 2, "name": "", "score": null, "later": true},
                 {"name": "no id"}]"#,
        );
        let req = JsonImportRequest {
            import_id: "j".into(),
            path: path.clone(),
            encoding: None,
            keys: strings(&["id", "name", "score", "tags"]),
            target: ImportTarget::Create {
                table: TableRef::new(Some("main"), "people"),
                columns: vec![
                    def("id", ImportType::Integer),
                    def("name", ImportType::Text),
                    def("score", ImportType::Numeric),
                    def("tags", ImportType::Text),
                ],
            },
            mapping: vec![
                Some("id".into()),
                Some("name".into()),
                Some("score".into()),
                Some("tags".into()),
            ],
            mode: InsertMode::StopOnError,
        };
        let report = json_import_with_chunks(&s, &req, None, 1000).await.unwrap();
        assert_eq!(report.failure, None);
        assert_eq!(report.rows_imported, 3);
        assert!(report.created_table && report.single_transaction);
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].contains("\"later\""), "{:?}", report.notes);
        let rows = s
            .query(
                "SELECT id, name, score, tags FROM people ORDER BY rowid",
                &[],
            )
            .await
            .unwrap()
            .rows;
        assert_eq!(rows[0][1], Value::Text("Ann".into()));
        assert_eq!(rows[0][3], Value::Text("[\"x\",1]".into()));
        // An empty string stays one; null and a missing key are NULL.
        assert_eq!(rows[1][1], Value::Text(String::new()));
        assert_eq!(rows[1][2], Value::Null);
        assert_eq!(rows[2][0], Value::Null);

        // Into the existing table, in chunks of one row: a bad value names its line.
        let path = write(
            dir.path(),
            "more.ndjson",
            b"{\"id\": 10, \"name\": \"a\"}\n{\"id\": 11, \"name\": \"b\"}\n{\"id\": \"x\"\n",
        );
        let meta = s
            .table_meta(&TableRef::new(Some("main"), "people"))
            .await
            .unwrap();
        let more = JsonImportRequest {
            path,
            keys: strings(&["id", "name"]),
            target: ImportTarget::Existing {
                table: TableRef::new(Some("main"), "people"),
            },
            mapping: vec![Some("id".into()), Some("name".into())],
            ..req
        };
        let report = json_import_with_chunks(&s, &more, Some(&meta), 2)
            .await
            .unwrap();
        assert_eq!(report.rows_imported, 2);
        let failure = report.failure.expect("the cut object fails");
        assert!(
            failure.message.contains("ends inside item 3"),
            "{failure:?}"
        );
    }

    #[test]
    fn notes_keys_the_preview_did_not_see() {
        let mut ignored = IgnoredKeys::default();
        assert_eq!(ignored_note(&ignored), None);
        ignored.names = vec!["x".into(), "y".into()];
        assert_eq!(
            ignored_note(&ignored).unwrap(),
            "Keys that were not in the previewed objects were not imported: \"x\", \"y\"."
        );
        ignored.more = true;
        assert!(ignored_note(&ignored).unwrap().ends_with("\"y\" and more."));
    }
}
