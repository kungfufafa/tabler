//! The REST client of the D1 driver. Every call is one HTTPS request: there is no connection,
//! no transaction across calls and no cursor, so the catalog statements of a call go out as one
//! batch, and a staged commit as one batch with its guards.

use std::sync::Arc;

use crate::drivers::TxStatement;
use crate::drivers::sqlite_http::catalog::{CatalogQuery, Rows};
use crate::drivers::sqlite_http::transport::{HttpRequest, Transport};
use crate::error::AppError;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

use super::api::{
    D1Target, DatabaseInfo, ENGINE, RawBody, RawQuery, RawResult, is_uuid, list_url, one_statement,
    parse_reply,
};
use super::batch::{tx_batch, tx_error, tx_outcome};
use super::decode::{ParamEncoding, encode_params, exec_outcome, plain_rows, result_set};

/// Databases per page of the list.
const PAGE: u32 = 100;
/// Pages read at most (10,000 databases; the free plan allows 10).
const MAX_PAGES: u32 = 100;

pub struct D1Client {
    transport: Arc<dyn Transport>,
    target: D1Target,
    encoding: ParamEncoding,
}

impl D1Client {
    pub fn new(
        transport: Arc<dyn Transport>,
        target: D1Target,
        encoding: ParamEncoding,
    ) -> D1Client {
        D1Client {
            transport,
            target,
            encoding,
        }
    }

    pub fn target(&self) -> &D1Target {
        &self.target
    }

    async fn raw(&self, body: &RawBody, sql: Option<&str>) -> Result<Vec<RawResult>, AppError> {
        let reply = self
            .transport
            .send(HttpRequest::post_json(self.target.raw_url()?, body)?)
            .await?;
        let results: Vec<RawResult> = parse_reply(&reply, sql)?;
        if results.iter().any(|r| r.success == Some(false)) {
            return Err(AppError::db(format!(
                "{ENGINE} reported a failed statement without an error"
            )));
        }
        Ok(results)
    }

    /// One statement; a text of several is refused before it is sent (`one_statement`).
    pub async fn statement(&self, sql: &str, params: &[Value]) -> Result<RawResult, AppError> {
        one_statement(sql)?;
        let body = RawBody::Single(RawQuery {
            sql: sql.to_string(),
            params: encode_params(sql, params, self.encoding)?,
        });
        self.raw(&body, Some(sql))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::db(format!("{ENGINE} returned no result")))
    }

    /// Rows of a query: `kinds` are the columns' kinds when the caller knows them (a browse).
    pub async fn query(
        &self,
        sql: &str,
        params: &[Value],
        kinds: Option<&[ValueKind]>,
        max_rows: Option<usize>,
    ) -> Result<ResultSet, AppError> {
        let result = self.statement(sql, params).await?;
        Ok(result
            .results
            .as_ref()
            .map(|r| result_set(r, kinds, max_rows))
            .unwrap_or_default())
    }

    pub async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        Ok(exec_outcome(sql, &self.statement(sql, params).await?))
    }

    /// The catalog statements of one call, as one batch.
    pub async fn catalog(&self, queries: &[CatalogQuery]) -> Result<Vec<Rows>, AppError> {
        let batch = queries
            .iter()
            .map(|q| {
                Ok(RawQuery {
                    sql: q.sql.to_string(),
                    params: encode_params(q.sql, &q.params, self.encoding)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let results = self.raw(&RawBody::Batch { batch }, None).await?;
        if results.len() != queries.len() {
            return Err(AppError::db(format!(
                "{ENGINE} answered {} result(s) to {} statement(s)",
                results.len(),
                queries.len()
            )));
        }
        Ok(results.iter().map(plain_rows).collect())
    }

    /// The staged changes as one atomic batch, guarded.
    pub async fn transaction(
        &self,
        statements: &[TxStatement],
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let batch = tx_batch(statements, self.encoding)?;
        let body = RawBody::Batch {
            batch: batch.queries.clone(),
        };
        let results = self
            .raw(&body, None)
            .await
            .map_err(|e| tx_error(statements, e))?;
        Ok(tx_outcome(statements, &batch, &results))
    }

    /// The database's name, version and size.
    pub async fn database(&self) -> Result<DatabaseInfo, AppError> {
        let reply = self
            .transport
            .send(HttpRequest::get(self.target.database_url()?))
            .await?;
        parse_reply(&reply, None)
    }

    /// The account's databases (those named `name`, when given), every page.
    pub async fn list_databases(&self, name: Option<&str>) -> Result<Vec<DatabaseInfo>, AppError> {
        list_databases(
            self.transport.as_ref(),
            &self.target.api_base,
            &self.target.account_id,
            name,
            PAGE,
        )
        .await
    }
}

/// Every page of the account's databases.
pub async fn list_databases(
    transport: &dyn Transport,
    api_base: &str,
    account_id: &str,
    name: Option<&str>,
    per_page: u32,
) -> Result<Vec<DatabaseInfo>, AppError> {
    let mut all = Vec::new();
    for page in 1..=MAX_PAGES {
        let reply = transport
            .send(HttpRequest::get(list_url(
                api_base, account_id, name, page, per_page,
            )?))
            .await?;
        let found: Vec<DatabaseInfo> = parse_reply(&reply, None)?;
        let short = found.len() < per_page as usize;
        all.extend(found);
        if short {
            break;
        }
    }
    Ok(all)
}

/// The UUID of the database a profile names: a UUID as it is, a name looked up (names are
/// unique in an account, but change on rename, so the session keeps the UUID).
pub async fn resolve_database(
    transport: &dyn Transport,
    api_base: &str,
    account_id: &str,
    name_or_id: &str,
) -> Result<String, AppError> {
    let wanted = name_or_id.trim();
    if is_uuid(wanted) {
        return Ok(wanted.to_string());
    }
    list_databases(transport, api_base, account_id, Some(wanted), PAGE)
        .await?
        .into_iter()
        .find(|d| d.name == wanted)
        .map(|d| d.uuid)
        .ok_or_else(|| AppError::NotFound(format!("{ENGINE} database \u{201c}{wanted}\u{201d}")))
}
