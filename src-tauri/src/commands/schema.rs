use std::sync::Arc;

use tauri::State;
use uuid::Uuid;

use crate::drivers::DbSession;
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::{MYSQL_COLLATIONS_SQL, MYSQL_ENGINES_SQL, PG_USER_TYPES_SQL};
use crate::state::{AppState, Connection};
use crate::types::caps::Feature;
use crate::types::schema::{
    CharsetInfo, CompletionMetadata, DatabaseInfo, SchemaInfo, StructureChoices, TableMeta,
    TableRef, TableSummary, UserTypeInfo,
};
use crate::types::value::Value;

#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<DatabaseInfo>, AppError> {
    state
        .any_session(connection_id)
        .await?
        .list_databases()
        .await
}

#[tauri::command]
pub async fn list_schemas(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<SchemaInfo>, AppError> {
    state.any_session(connection_id).await?.list_schemas().await
}

#[tauri::command]
pub async fn list_tables(
    state: State<'_, AppState>,
    connection_id: Uuid,
    schema: String,
) -> Result<Vec<TableSummary>, AppError> {
    state
        .any_session(connection_id)
        .await?
        .list_tables(&schema)
        .await
}

/// Table metadata, cached per connection until `refresh` or invalidation.
#[tauri::command]
pub async fn get_table(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
    refresh: Option<bool>,
) -> Result<TableMeta, AppError> {
    let conn = state.connection(connection_id).await?;
    table_on(&conn, table, refresh.unwrap_or(false)).await
}

/// The body of [`get_table`] on an open connection.
pub(crate) async fn table_on(
    conn: &Connection,
    table: TableRef,
    refresh: bool,
) -> Result<TableMeta, AppError> {
    let session = conn.any_session_for(Feature::StructureView).await?;
    if !refresh && let Some(meta) = conn.schema_cache.read().await.get(&table) {
        return Ok((**meta).clone());
    }
    let meta = session.table_meta(&table).await?;
    conn.schema_cache
        .write()
        .await
        .insert(table, Arc::new(meta.clone()));
    Ok(meta)
}

#[tauri::command]
pub async fn get_view_definition(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
) -> Result<Option<String>, AppError> {
    let conn = state.connection(connection_id).await?;
    view_definition_on(&conn, &table).await
}

/// The body of [`get_view_definition`] on an open connection.
pub(crate) async fn view_definition_on(
    conn: &Connection,
    table: &TableRef,
) -> Result<Option<String>, AppError> {
    conn.any_session_for(Feature::ViewDefinition)
        .await?
        .view_definition(table)
        .await
}

#[tauri::command]
pub async fn get_completion_metadata(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<CompletionMetadata, AppError> {
    state
        .any_session(connection_id)
        .await?
        .completion_metadata()
        .await
}

#[tauri::command]
pub async fn invalidate_schema_cache(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: Option<TableRef>,
) -> Result<(), AppError> {
    state
        .connection(connection_id)
        .await?
        .invalidate(table.as_ref())
        .await;
    Ok(())
}

/// What the structure editor's pickers offer: PG user-defined types, MySQL engines and
/// character sets with their collations.
#[tauri::command]
pub async fn get_structure_choices(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<StructureChoices, AppError> {
    let session = state.session(connection_id).await?;
    structure_choices(session.as_ref()).await
}

pub(crate) async fn structure_choices(
    session: &dyn DbSession,
) -> Result<StructureChoices, AppError> {
    let mut out = StructureChoices::default();
    match session.core_dialect(Feature::StructureEdit)? {
        Dialect::Postgres => {
            let rows = session.query(PG_USER_TYPES_SQL, &[]).await?.rows;
            out.user_types = rows
                .iter()
                .filter_map(|r| match (text(r, 0), text(r, 1)) {
                    (Some(name), Some(kind)) => Some(UserTypeInfo { name, kind }),
                    _ => None,
                })
                .collect();
        }
        Dialect::Mysql => {
            let engines = session.query(MYSQL_ENGINES_SQL, &[]).await?.rows;
            out.engines = engines.iter().filter_map(|r| text(r, 0)).collect();
            let rows = session.query(MYSQL_COLLATIONS_SQL, &[]).await?.rows;
            out.charsets = group_collations(rows.iter().filter_map(|r| {
                Some((
                    text(r, 0)?,
                    text(r, 1)?,
                    text(r, 2).is_some_and(|d| is_yes(&d)),
                ))
            }));
        }
        Dialect::Sqlite => {}
    }
    Ok(out)
}

fn text(row: &[Value], i: usize) -> Option<String> {
    match row.get(i)? {
        Value::Text(s) => Some(s.clone()),
        Value::Int(n) => Some(n.to_string()),
        _ => None,
    }
}

fn is_yes(s: &str) -> bool {
    s.eq_ignore_ascii_case("yes")
}

/// `(charset, collation, is default)` rows (ordered by charset) as character sets with their
/// collations, the default one first.
fn group_collations(rows: impl Iterator<Item = (String, String, bool)>) -> Vec<CharsetInfo> {
    let mut out: Vec<CharsetInfo> = Vec::new();
    for (charset, collation, default) in rows {
        let at = match out.iter().position(|c| c.name == charset) {
            Some(i) => i,
            None => {
                out.push(CharsetInfo {
                    name: charset,
                    default_collation: None,
                    collations: Vec::new(),
                });
                out.len() - 1
            }
        };
        let set = &mut out[at];
        if default && set.default_collation.is_none() {
            set.default_collation = Some(collation.clone());
            set.collations.insert(0, collation);
        } else {
            set.collations.push(collation);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_collations_by_charset() {
        let rows = [
            ("latin1", "latin1_bin", false),
            ("latin1", "latin1_swedish_ci", true),
            ("utf8mb4", "utf8mb4_0900_ai_ci", true),
            ("utf8mb4", "utf8mb4_bin", false),
        ]
        .into_iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c));
        let sets = group_collations(rows);
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].name, "latin1");
        assert_eq!(
            sets[0].default_collation.as_deref(),
            Some("latin1_swedish_ci")
        );
        assert_eq!(sets[0].collations, ["latin1_swedish_ci", "latin1_bin"]);
        assert_eq!(sets[1].collations, ["utf8mb4_0900_ai_ci", "utf8mb4_bin"]);
        assert!(is_yes("Yes") && !is_yes(""));
    }
}
