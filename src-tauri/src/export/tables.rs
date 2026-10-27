//! Multi-table export (File ▸ Export… on several tables, ⇧⌘E without a table): every table in
//! full (no filter), either one file per table in a folder (`<table>.<ext>`) or every table in
//! one file — a SQL script (laid out by `structure`: DROPs, tables, rows, then keys and
//! triggers), an HTML page with a table per table, or an Excel workbook with one sheet per table.
//!
//! All or nothing: every file is written under a temporary name and moved into place only when
//! every table was exported; an error or a cancel leaves no file behind (nor a half-replaced
//! set of files).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::drivers::DbSession;
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::transfer::script_table;
use crate::types::export::{
    ExportFormat, ExportLayout, ExportTablesProgress, ExportTablesRequest, ExportTablesResult,
    ExportedTable,
};
use crate::types::schema::{TableMeta, TableRef};

use super::encoding::{Encoded, effective_encoding, html_charset};
use super::file::{Compressed, TempTarget, validate_folder, validate_target};
use super::structure::{ScriptTable, closing_text, context_for, same_table, script_layout};
use super::writer::{FormatSpec, HtmlWriter, RowWriter, SqlScript};
use super::xlsx::XlsxWriter;
use super::{
    FileOut, Plan, blocking, export_dialect, file_out, mysql_long_lat, replaced_count,
    session_time_zone, stream_plan, sync_file, table_query,
};

/// Longest file name stem (bytes) of a per-table file.
const STEM_MAX: usize = 200;

/// File extension of `format` (CSV with a tab delimiter is `.tsv`).
pub fn file_extension(format: &ExportFormat) -> &'static str {
    match format {
        ExportFormat::Csv { delimiter, .. } if delimiter == "\t" => "tsv",
        ExportFormat::Csv { .. } => "csv",
        ExportFormat::Json { ndjson: true, .. } => "ndjson",
        ExportFormat::Json { .. } => "json",
        ExportFormat::SqlInsert { gzip: true, .. } => "sql.gz",
        ExportFormat::SqlInsert { .. } => "sql",
        ExportFormat::Xlsx { .. } => "xlsx",
        ExportFormat::Html => "html",
    }
}

/// A table name as a file name: path separators, characters Finder or Windows refuse and
/// control characters become `_`, a leading dot too (no hidden files).
pub fn file_stem(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    out = out.trim().to_string();
    if out.starts_with('.') {
        out.replace_range(..1, "_");
    }
    if out.is_empty() {
        out = "table".into();
    }
    if out.len() > STEM_MAX {
        let mut end = STEM_MAX;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// One file of a multi-table export and the tables (indexes into the request) it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    pub path: PathBuf,
    pub tables: Vec<usize>,
}

/// Where each table goes. One file per table: `<table>.<ext>` in the folder; tables whose names
/// clash (case-insensitively, as on macOS) are named `<schema>.<table>.<ext>`, then numbered.
pub fn plan_outputs(
    layout: ExportLayout,
    target: &str,
    tables: &[TableRef],
    format: &ExportFormat,
) -> Result<Vec<OutputFile>, AppError> {
    plan_outputs_in(layout, target, tables, format, false)
}

/// `plan_outputs`; `create_folder` creates a missing folder of a file-per-table export first.
pub fn plan_outputs_in(
    layout: ExportLayout,
    target: &str,
    tables: &[TableRef],
    format: &ExportFormat,
    create_folder: bool,
) -> Result<Vec<OutputFile>, AppError> {
    if tables.is_empty() {
        return Err(AppError::InvalidInput("no tables to export".into()));
    }
    let ext = file_extension(format);
    match layout {
        ExportLayout::SingleFile => {
            if !matches!(
                format,
                ExportFormat::SqlInsert { .. } | ExportFormat::Xlsx { .. } | ExportFormat::Html
            ) {
                return Err(AppError::InvalidInput(
                    "CSV and JSON exports write one file per table".into(),
                ));
            }
            Ok(vec![OutputFile {
                path: validate_target(target)?,
                tables: (0..tables.len()).collect(),
            }])
        }
        ExportLayout::FilePerTable => {
            if create_folder {
                create_export_folder(target)?;
            }
            let dir = validate_folder(target)?;
            let plain: Vec<String> = tables.iter().map(|t| file_stem(&t.name)).collect();
            let clashes = |stem: &str| {
                plain
                    .iter()
                    .filter(|s| s.to_lowercase() == stem.to_lowercase())
                    .count()
                    > 1
            };
            let mut taken: Vec<String> = Vec::new();
            let mut out = Vec::with_capacity(tables.len());
            for (i, t) in tables.iter().enumerate() {
                let mut stem = plain[i].clone();
                if clashes(&stem)
                    && let Some(schema) = t.schema.as_deref().filter(|s| !s.is_empty())
                {
                    stem = file_stem(&format!("{schema}.{}", t.name));
                }
                let mut name = format!("{stem}.{ext}");
                let mut n = 2;
                while taken
                    .iter()
                    .any(|x| x.to_lowercase() == name.to_lowercase())
                {
                    name = format!("{stem}_{n}.{ext}");
                    n += 1;
                }
                taken.push(name.clone());
                out.push(OutputFile {
                    path: dir.join(name),
                    tables: vec![i],
                });
            }
            Ok(out)
        }
    }
}

/// Creates the folder of a file-per-table export (absolute, no `..`) and its parents.
fn create_export_folder(target: &str) -> Result<(), AppError> {
    let dir = PathBuf::from(target);
    if target.trim().is_empty() || !dir.is_absolute() {
        return Err(AppError::InvalidInput(
            "the export folder must be an absolute path".into(),
        ));
    }
    if dir
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(AppError::InvalidInput(
            "the export folder cannot contain ..".into(),
        ));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Io(format!("cannot create \"{}\": {e}", dir.display())))
}

/// A table's heading in a multi-table HTML page: `schema.table`, or the name alone.
fn table_heading(table: &TableRef) -> String {
    match table.schema.as_deref().filter(|s| !s.is_empty()) {
        Some(schema) => format!("{schema}.{}", table.name),
        None => table.name.clone(),
    }
}

/// Writes one table's INSERTs into `out`. Returns the rows written.
#[allow(clippy::too_many_arguments)]
async fn write_sql_rows<W: Write + Send>(
    session: &dyn DbSession,
    dialect: Dialect,
    meta: &TableMeta,
    batch_size: u32,
    mysql_long_lat: bool,
    out: &mut W,
    cancel: &CancellationToken,
    on_rows: &mut (dyn FnMut(u64) + Send),
    export_id: &str,
) -> Result<u64, AppError> {
    let query = table_query(dialect, meta, None, None, &[], None, 0, true)?;
    let spec = FormatSpec::Insert {
        dialect,
        table: script_table(dialect, &meta.table),
        batch_size,
        overriding_system_value: query.identity_always,
        mysql_long_lat,
    };
    let mut writer = RowWriter::new(spec, &mut *out);
    let rows = stream_plan(
        session,
        dialect,
        &Plan::Query(query),
        &mut writer,
        None,
        cancel,
        on_rows,
        export_id,
    )
    .await?;
    writer.finish()?;
    if rows > 0 {
        out.write_all(b"\n")?;
    }
    Ok(rows)
}

fn script_lines(lines: &[String]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Runs a multi-table export. `metas` holds each requested table's metadata, in request order.
pub async fn run_export_tables(
    session: &dyn DbSession,
    req: &ExportTablesRequest,
    metas: &[Arc<TableMeta>],
    mut progress: impl FnMut(ExportTablesProgress) + Send,
    cancel: CancellationToken,
) -> Result<ExportTablesResult, AppError> {
    let started = Instant::now();
    let dialect = export_dialect(session, &req.format)?;
    if metas.len() != req.tables.len() {
        return Err(AppError::internal(
            "table metadata does not match the request",
        ));
    }
    let outputs = plan_outputs_in(
        req.layout,
        &req.target,
        &req.tables,
        &req.format,
        req.create_folder,
    )?;
    let text_encoding = effective_encoding(&req.format, req.encoding);
    let replaced = Arc::new(AtomicU64::new(0));
    let script = SqlScript::of(&req.format);
    if let Some(s) = script
        && !(s.content || s.structure || s.drop_table)
    {
        return Err(AppError::InvalidInput(
            "nothing to export: include the table structure or its content".into(),
        ));
    }
    let batch_size = match &req.format {
        ExportFormat::SqlInsert { batch_size, .. } => {
            if *batch_size == 0 {
                return Err(AppError::InvalidInput(
                    "the INSERT batch size must be at least 1".into(),
                ));
            }
            *batch_size
        }
        _ => 0,
    };
    let count = req.tables.len() as u32;
    let bytes = Arc::new(AtomicU64::new(0));
    let mut done_rows = 0u64;
    let mut exported: Vec<ExportedTable> = Vec::with_capacity(req.tables.len());
    let mut finished: Vec<TempTarget> = Vec::with_capacity(outputs.len());
    let names: Vec<String> = req.tables.iter().map(|t| t.name.clone()).collect();
    let written = bytes.clone();
    let mut report = |index: usize, table_rows: u64, rows: u64| {
        progress(ExportTablesProgress {
            table_index: index as u32,
            table_count: count,
            table: names[index].clone(),
            table_rows,
            rows,
            bytes: written.load(Ordering::Relaxed),
        })
    };
    log::debug!(
        "export {}: {} tables into {} file(s)",
        req.export_id,
        req.tables.len(),
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
        let order: Vec<usize> = output.tables.clone();
        let record = |i: usize, rows: u64, exported: &mut Vec<ExportedTable>| {
            exported.push(ExportedTable {
                table: metas[i].table.clone(),
                path: path_text.clone(),
                rows,
            });
        };
        match &req.format {
            ExportFormat::SqlInsert { .. } => {
                let script = script.unwrap_or_default();
                let long_lat = script.content && mysql_long_lat(session, dialect).await;
                let zone = if script.content {
                    session_time_zone(session, dialect).await
                } else {
                    None
                };
                let mut out = Encoded::new(
                    Compressed::new(out, script.gzip),
                    text_encoding,
                    replaced.clone(),
                );
                let wants_header = script.drop_table || script.structure || order.len() > 1;
                let mut tables = Vec::with_capacity(order.len());
                for &i in &order {
                    if cancel.is_cancelled() {
                        return Err(AppError::Cancelled);
                    }
                    let meta = metas[i].as_ref();
                    let context =
                        context_for(session, dialect, meta, script.structure, order.len() > 1)
                            .await;
                    tables.push(ScriptTable { meta, context });
                }
                let layout = script_layout(dialect, &tables, script, wants_header, zone.as_deref());
                out.write_all(layout.opening.as_bytes())?;
                for &k in &layout.order {
                    let i = order[k];
                    report(i, 0, done_rows);
                    let rows = if script.content && layout.rows[k] {
                        let base = done_rows;
                        let mut on_rows = |rows: u64| report(i, rows, base + rows);
                        write_sql_rows(
                            session,
                            dialect,
                            &metas[i],
                            batch_size,
                            long_lat,
                            &mut out,
                            &cancel,
                            &mut on_rows,
                            &req.export_id,
                        )
                        .await?
                    } else {
                        0
                    };
                    done_rows += rows;
                    record(i, rows, &mut exported);
                }
                out.write_all(closing_text(&layout.closing, false).as_bytes())?;
                out.write_all(script_lines(&layout.footer).as_bytes())?;
                blocking("finishing", move || sync_file(out.finish()?.finish()?)).await?;
            }
            ExportFormat::Xlsx { header } => {
                let mut book = XlsxWriter::new(out, *header);
                for &i in &order {
                    report(i, 0, done_rows);
                    book.next_sheet(&metas[i].table.name);
                    let query = table_query(dialect, &metas[i], None, None, &[], None, 0, false)?;
                    let base = done_rows;
                    let mut on_rows = |rows: u64| report(i, rows, base + rows);
                    let rows = stream_plan(
                        session,
                        dialect,
                        &Plan::Query(query),
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
            ExportFormat::Html if order.len() > 1 => {
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
                for &i in &order {
                    if cancel.is_cancelled() {
                        return Err(AppError::Cancelled);
                    }
                    report(i, 0, done_rows);
                    page.next_table(&table_heading(&metas[i].table))?;
                    let query = table_query(dialect, &metas[i], None, None, &[], None, 0, false)?;
                    let base = done_rows;
                    let mut on_rows = |rows: u64| report(i, rows, base + rows);
                    let rows = stream_plan(
                        session,
                        dialect,
                        &Plan::Query(query),
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
                // One table per file (the plan refuses a single CSV / JSON file).
                let i = order[0];
                report(i, 0, done_rows);
                let meta = &metas[i];
                let mut spec =
                    FormatSpec::from_request(&req.format, dialect, Some(&meta.table.name))?;
                if let FormatSpec::Html { charset, .. } = &mut spec {
                    *charset = html_charset(text_encoding);
                }
                let query = table_query(dialect, meta, None, None, &[], None, 0, false)?;
                let mut writer =
                    RowWriter::new(spec, Encoded::new(out, text_encoding, replaced.clone()));
                let base = done_rows;
                let mut on_rows = |rows: u64| report(i, rows, base + rows);
                let rows = stream_plan(
                    session,
                    dialect,
                    &Plan::Query(query),
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
    // Tables in request order.
    exported.sort_by_key(|e| {
        req.tables
            .iter()
            .position(|t| same_table(t, &e.table))
            .unwrap_or(usize::MAX)
    });
    let bytes = bytes.load(Ordering::Relaxed);
    Ok(ExportTablesResult {
        tables: exported,
        paths,
        rows: done_rows,
        bytes,
        duration_ms: started.elapsed().as_millis() as u64,
        replaced: replaced_count(&replaced),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv() -> ExportFormat {
        ExportFormat::Csv {
            delimiter: ",".into(),
            header: true,
            quote: Default::default(),
            null_as: String::new(),
            line_break: "\n".into(),
            decimal: ".".into(),
            line_break_to_space: false,
        }
    }

    fn sql(gzip: bool) -> ExportFormat {
        ExportFormat::SqlInsert {
            table_name: String::new(),
            batch_size: 100,
            drop_table: true,
            structure: true,
            content: true,
            gzip,
        }
    }

    #[test]
    fn plans_one_file_per_table_without_clashes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_string_lossy().into_owned();
        let tables = vec![
            TableRef::new(Some("public"), "users"),
            TableRef::new(Some("audit"), "users"),
            TableRef::new(Some("public"), "Orders"),
            TableRef::new(Some("public"), "orders"),
            TableRef::new(None, "a/b"),
            TableRef::new(None, ".hidden"),
        ];
        let plan = plan_outputs(ExportLayout::FilePerTable, &folder, &tables, &csv()).unwrap();
        let names: Vec<String> = plan
            .iter()
            .map(|o| o.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "public.users.csv",
                "audit.users.csv",
                "public.Orders.csv",
                "public.orders_2.csv",
                "a_b.csv",
                "_hidden.csv"
            ]
        );
        assert!(plan.iter().all(|o| o.path.parent() == Some(dir.path())));
        assert_eq!(plan[3].tables, vec![3]);
        // Extensions follow the format.
        let one = [TableRef::new(None, "t")];
        let ext = |f: &ExportFormat| {
            plan_outputs(ExportLayout::FilePerTable, &folder, &one, f).unwrap()[0]
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(ext(&sql(true)), "t.sql.gz");
        assert_eq!(ext(&ExportFormat::Xlsx { header: true }), "t.xlsx");
        assert_eq!(
            ext(&ExportFormat::Json {
                ndjson: true,
                pretty: false
            }),
            "t.ndjson"
        );
    }

    #[test]
    fn plans_single_files_for_sql_and_excel_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("all.sql").to_string_lossy().into_owned();
        let tables = vec![TableRef::new(None, "a"), TableRef::new(None, "b")];
        let plan = plan_outputs(ExportLayout::SingleFile, &file, &tables, &sql(false)).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].tables, vec![0, 1]);
        assert!(plan_outputs(ExportLayout::SingleFile, &file, &tables, &csv()).is_err());
        assert!(plan_outputs(ExportLayout::SingleFile, &file, &[], &sql(false)).is_err());
        // HTML: one page with a table per table.
        assert!(
            plan_outputs(
                ExportLayout::SingleFile,
                &file,
                &tables,
                &ExportFormat::Html
            )
            .is_ok()
        );
        assert!(plan_outputs(ExportLayout::FilePerTable, "relative", &tables, &csv()).is_err());
        let missing = dir.path().join("nope").to_string_lossy().into_owned();
        assert!(plan_outputs(ExportLayout::FilePerTable, &missing, &tables, &csv()).is_err());
    }

    #[test]
    fn creates_the_folder_of_a_file_per_table_export_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("tabler_dev").join("csv");
        let target = nested.to_string_lossy().into_owned();
        let tables = [TableRef::new(Some("public"), "users")];
        assert!(plan_outputs(ExportLayout::FilePerTable, &target, &tables, &csv()).is_err());
        let plan =
            plan_outputs_in(ExportLayout::FilePerTable, &target, &tables, &csv(), true).unwrap();
        assert!(nested.is_dir());
        assert_eq!(plan[0].path, nested.join("users.csv"));
        let up = dir.path().join("a").join("..").join("b");
        assert!(
            plan_outputs_in(
                ExportLayout::FilePerTable,
                &up.to_string_lossy(),
                &tables,
                &csv(),
                true
            )
            .is_err()
        );
        assert_eq!(
            table_heading(&TableRef::new(Some("public"), "users")),
            "public.users"
        );
        assert_eq!(table_heading(&TableRef::new(None, "t")), "t");
        assert_eq!(file_extension(&ExportFormat::Html), "html");
    }

    #[test]
    fn stems() {
        assert_eq!(file_stem("Order Items"), "Order Items");
        assert_eq!(file_stem("x:y*z?"), "x_y_z_");
        assert_eq!(file_stem("  "), "table");
        assert_eq!(file_stem(&"é".repeat(150)).len(), 200);
    }
}
