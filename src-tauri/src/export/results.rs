//! Multi-result export (the export dialog's "All results" of a multi-statement run in the SQL
//! editor): the rows of every finished result, as the renderer holds them, in one go — one
//! Excel workbook with a sheet per result, one HTML page with a heading per result, one SQL
//! script (each result's INSERTs named after its table, or `untitled`), or one CSV / JSON file
//! per result (`<name>-1.csv`, `<name>-2.csv`…) in a folder.
//!
//! It shares the machinery of the multi-table export (`tables`): the same file naming rules, a
//! temporary file per output that is moved into place only when every result was written, the
//! encoding, progress per result and cancellation. Nothing runs on the server except the one
//! statement that asks for the session time zone of a SQL script (see `session_time_zone`).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::drivers::DbSession;
use crate::error::AppError;
use crate::types::export::{
    ExportFormat, ExportLayout, ExportResultItem, ExportResultsRequest, ExportResultsResult,
    ExportTablesProgress, ExportedResult,
};

use super::encoding::{Encoded, effective_encoding, html_charset};
use super::file::{Compressed, TempTarget, validate_folder, validate_target};
use super::structure::{closing_text, script_layout};
use super::tables::{file_extension, file_stem};
use super::writer::{FormatSpec, HtmlWriter, RowWriter, SqlScript, UNTITLED_TABLE};
use super::xlsx::XlsxWriter;
use super::{
    FileOut, Plan, blocking, check_rows, export_dialect, file_out, mysql_long_lat, replaced_count,
    session_time_zone, stream_plan, sync_file,
};

/// File name stem of a file per result when the request names none.
const DEFAULT_NAME: &str = "result";

/// One file of a multi-result export and the results (indexes into the request) it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultFile {
    pub path: PathBuf,
    pub results: Vec<usize>,
}

/// Where each result goes: the one file `target` (SQL, Excel and HTML), or `<name>-<n>.<ext>` per
/// result in the folder `target` (CSV and JSON always have to).
pub fn plan_result_files(
    layout: ExportLayout,
    target: &str,
    count: usize,
    name: Option<&str>,
    format: &ExportFormat,
) -> Result<Vec<ResultFile>, AppError> {
    if count == 0 {
        return Err(AppError::InvalidInput("no results to export".into()));
    }
    match layout {
        ExportLayout::SingleFile => {
            if !matches!(
                format,
                ExportFormat::SqlInsert { .. } | ExportFormat::Xlsx { .. } | ExportFormat::Html
            ) {
                return Err(AppError::InvalidInput(
                    "CSV and JSON exports write one file per result".into(),
                ));
            }
            Ok(vec![ResultFile {
                path: validate_target(target)?,
                results: (0..count).collect(),
            }])
        }
        ExportLayout::FilePerTable => {
            let dir = validate_folder(target)?;
            let stem = file_stem(
                name.map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(DEFAULT_NAME),
            );
            let ext = file_extension(format);
            Ok((0..count)
                .map(|i| ResultFile {
                    path: dir.join(format!("{stem}-{}.{ext}", i + 1)),
                    results: vec![i],
                })
                .collect())
        }
    }
}

/// The INSERT target of a result: its table, or `untitled`.
fn insert_target(item: &ExportResultItem) -> &str {
    item.table
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(UNTITLED_TABLE)
}

/// The format of one result of a script: the INSERTs name the result's own table.
fn format_for(format: &ExportFormat, item: &ExportResultItem) -> ExportFormat {
    match format {
        ExportFormat::SqlInsert {
            batch_size,
            drop_table,
            structure,
            content,
            gzip,
            ..
        } => ExportFormat::SqlInsert {
            table_name: insert_target(item).to_string(),
            batch_size: *batch_size,
            drop_table: *drop_table,
            structure: *structure,
            content: *content,
            gzip: *gzip,
        },
        other => other.clone(),
    }
}

/// Runs a multi-result export. Cancelling `cancel` stops it with `AppError::Cancelled` and leaves
/// no file behind; every file is written under a temporary name and moved into place only when
/// every result was written.
pub async fn run_export_results(
    session: &dyn DbSession,
    req: &ExportResultsRequest,
    mut progress: impl FnMut(ExportTablesProgress) + Send,
    cancel: CancellationToken,
) -> Result<ExportResultsResult, AppError> {
    let started = Instant::now();
    let dialect = export_dialect(session, &req.format)?;
    let outputs = plan_result_files(
        req.layout,
        &req.target,
        req.results.len(),
        req.name.as_deref(),
        &req.format,
    )?;
    // What can't be written is refused before any file is created.
    for item in &req.results {
        check_rows(&item.columns, &item.rows, dialect)
            .map_err(|e| AppError::InvalidInput(format!("{}: {e}", item.label)))?;
    }
    let script = SqlScript::of(&req.format);
    if let Some(s) = script
        && !s.content
    {
        return Err(AppError::InvalidInput(
            "a result has no table structure: include its content".into(),
        ));
    }
    // Validates the options once (batch size, CSV choices) with the first result's format.
    FormatSpec::from_request(&format_for(&req.format, &req.results[0]), dialect, None)?;
    let text_encoding = effective_encoding(&req.format, req.encoding);
    let replaced = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let count = req.results.len() as u32;
    let mut done_rows = 0u64;
    let mut exported: Vec<ExportedResult> = Vec::with_capacity(req.results.len());
    let mut finished: Vec<TempTarget> = Vec::with_capacity(outputs.len());
    let written = bytes.clone();
    let mut report = |index: usize, table_rows: u64, rows: u64| {
        progress(ExportTablesProgress {
            table_index: index as u32,
            table_count: count,
            table: req.results[index].label.clone(),
            table_rows,
            rows,
            bytes: written.load(Ordering::Relaxed),
        })
    };
    log::debug!(
        "export {}: {} results into {} file(s)",
        req.export_id,
        req.results.len(),
        outputs.len()
    );

    for output in &outputs {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let (target, file) = {
            let path = output.path.clone();
            blocking("file", move || TempTarget::create(&path)).await?
        };
        let path_text = output.path.to_string_lossy().into_owned();
        let out: FileOut = file_out(file, bytes.clone());
        let record = |i: usize, rows: u64, exported: &mut Vec<ExportedResult>| {
            exported.push(ExportedResult {
                label: req.results[i].label.clone(),
                path: path_text.clone(),
                rows,
            });
        };
        match &req.format {
            ExportFormat::SqlInsert { .. } => {
                let script = script.unwrap_or_default();
                let long_lat = mysql_long_lat(session, dialect).await;
                let zone = session_time_zone(session, dialect).await;
                let mut out = Encoded::new(
                    Compressed::new(out, script.gzip),
                    text_encoding,
                    replaced.clone(),
                );
                // Rows only: the session time zone around them.
                let layout = script_layout(dialect, &[], script, false, zone.as_deref());
                out.write_all(layout.opening.as_bytes())?;
                for &i in &output.results {
                    if cancel.is_cancelled() {
                        return Err(AppError::Cancelled);
                    }
                    let item = &req.results[i];
                    report(i, 0, done_rows);
                    let mut spec =
                        FormatSpec::from_request(&format_for(&req.format, item), dialect, None)?;
                    if let FormatSpec::Insert {
                        mysql_long_lat: long,
                        ..
                    } = &mut spec
                    {
                        *long = long_lat;
                    }
                    let base = done_rows;
                    let mut on_rows = |rows: u64| report(i, rows, base + rows);
                    let mut writer = RowWriter::new(spec, &mut out);
                    let rows = stream_plan(
                        session,
                        dialect,
                        &Plan::Rows {
                            columns: &item.columns,
                            rows: &item.rows,
                        },
                        &mut writer,
                        None,
                        &cancel,
                        &mut on_rows,
                        &req.export_id,
                    )
                    .await?;
                    writer.finish()?;
                    if rows > 0 {
                        out.write_all(b"\n")?;
                    }
                    done_rows += rows;
                    record(i, rows, &mut exported);
                }
                out.write_all(closing_text(&layout.closing, false).as_bytes())?;
                for line in &layout.footer {
                    out.write_all(line.as_bytes())?;
                    out.write_all(b"\n")?;
                }
                blocking("finishing", move || sync_file(out.finish()?.finish()?)).await?;
            }
            ExportFormat::Xlsx { header } => {
                let mut book = XlsxWriter::new(out, *header);
                for &i in &output.results {
                    if cancel.is_cancelled() {
                        return Err(AppError::Cancelled);
                    }
                    let item = &req.results[i];
                    report(i, 0, done_rows);
                    book.next_sheet(&item.label);
                    let base = done_rows;
                    let mut on_rows = |rows: u64| report(i, rows, base + rows);
                    let rows = stream_plan(
                        session,
                        dialect,
                        &Plan::Rows {
                            columns: &item.columns,
                            rows: &item.rows,
                        },
                        &mut book,
                        None,
                        &cancel,
                        &mut on_rows,
                        &req.export_id,
                    )
                    .await?;
                    book.end_sheet()?;
                    done_rows += rows;
                    record(i, rows, &mut exported);
                }
                blocking("finishing", move || sync_file(book.finish()?)).await?;
            }
            ExportFormat::Html
                if output.results.len() > 1 || req.layout == ExportLayout::SingleFile =>
            {
                let title = output
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Export".into());
                let mut page = HtmlWriter::new(
                    Encoded::new(out, text_encoding, replaced.clone()),
                    &title,
                    html_charset(text_encoding),
                );
                for &i in &output.results {
                    if cancel.is_cancelled() {
                        return Err(AppError::Cancelled);
                    }
                    let item = &req.results[i];
                    report(i, 0, done_rows);
                    page.next_table(&item.label)?;
                    let base = done_rows;
                    let mut on_rows = |rows: u64| report(i, rows, base + rows);
                    let rows = stream_plan(
                        session,
                        dialect,
                        &Plan::Rows {
                            columns: &item.columns,
                            rows: &item.rows,
                        },
                        &mut page,
                        None,
                        &cancel,
                        &mut on_rows,
                        &req.export_id,
                    )
                    .await?;
                    page.end_table()?;
                    done_rows += rows;
                    record(i, rows, &mut exported);
                }
                blocking("finishing", move || sync_file(page.finish()?.finish()?)).await?;
            }
            ExportFormat::Csv { .. } | ExportFormat::Json { .. } | ExportFormat::Html => {
                // One result per file.
                let i = output.results[0];
                let item = &req.results[i];
                report(i, 0, done_rows);
                let mut spec = FormatSpec::from_request(&req.format, dialect, Some(&item.label))?;
                if let FormatSpec::Html { charset, .. } = &mut spec {
                    *charset = html_charset(text_encoding);
                }
                let mut writer =
                    RowWriter::new(spec, Encoded::new(out, text_encoding, replaced.clone()));
                let base = done_rows;
                let mut on_rows = |rows: u64| report(i, rows, base + rows);
                let rows = stream_plan(
                    session,
                    dialect,
                    &Plan::Rows {
                        columns: &item.columns,
                        rows: &item.rows,
                    },
                    &mut writer,
                    None,
                    &cancel,
                    &mut on_rows,
                    &req.export_id,
                )
                .await?;
                done_rows += rows;
                record(i, rows, &mut exported);
                blocking("finishing", move || sync_file(writer.finish()?.finish()?)).await?;
            }
        }
        finished.push(target);
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let paths: Vec<String> = outputs
        .iter()
        .map(|o| o.path.to_string_lossy().into_owned())
        .collect();
    blocking("commit", move || {
        for target in finished {
            target.commit()?;
        }
        Ok(())
    })
    .await?;
    Ok(ExportResultsResult {
        results: exported,
        paths,
        rows: done_rows,
        bytes: bytes.load(Ordering::Relaxed),
        duration_ms: started.elapsed().as_millis() as u64,
        replaced: replaced_count(&replaced),
    })
}
