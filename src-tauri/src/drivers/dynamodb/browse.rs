//! What the session does with a `Client`: the catalog (every `ListTables` page, a table's
//! description, sample and TTL), grid pages from Scan or Query with the `LastEvaluatedKey` as
//! the cursor, counts, single cells, console commands, and staged edits applied one by one or
//! as transactions.
//!
//! A page asks for at most the rows still missing (`Limit` counts evaluated items, so a
//! filtered page can come back short or empty with a `LastEvaluatedKey`) and takes up to
//! `page_requests` requests before it stops with a cursor. Nothing here retries: the client
//! does.

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::attr::{Cap, Item, cell, item_from_json, item_size};
use super::catalog::{
    self, MAX_COLUMNS, MAX_TABLE_PAGES, MAX_TABLES, TABLE_PAGE, TableDescription,
};
use super::client::{CallError, Client};
use super::errors::transaction_item_errors;
use crate::error::AppError;
use crate::sqlgen::native::dynamo::{
    self as b, ConsoleCommand, EditPlan, KeySchema, PartiqlWrite, ReadRequest, ReadSpec,
};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey, SortDir,
};
use crate::types::query::ResultColumn;
use crate::types::schema::{ColumnMeta, TableMeta};
use crate::types::value::Value;

/// Every table, following `LastEvaluatedTableName` to the last page: at most a full region's
/// (pages longer than asked for and names longer than DynamoDB's are refused).
pub async fn list_tables(
    client: &Client,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<String>, AppError> {
    let mut names = Vec::new();
    let mut start: Option<String> = None;
    for _ in 0..MAX_TABLE_PAGES {
        let reply = client
            .call(
                "ListTables",
                &b::list_tables(start.as_deref(), TABLE_PAGE as u32),
                cancel,
            )
            .await?;
        let (page, next) = catalog::parse_list_tables(&reply, TABLE_PAGE)?;
        names.extend(page);
        match next {
            None => return Ok(names),
            Some(n) if start.as_deref() == Some(n.as_str()) => {
                return Err(AppError::db(format!(
                    "DynamoDB listed the tables after \u{201c}{n}\u{201d} twice"
                )));
            }
            Some(n) => start = Some(n),
        }
    }
    Err(AppError::db(format!(
        "DynamoDB listed more than {MAX_TABLES} tables, more than a region holds"
    )))
}

pub async fn describe(
    client: &Client,
    table: &str,
    cancel: Option<&CancellationToken>,
) -> Result<TableDescription, AppError> {
    let reply = client
        .call("DescribeTable", &b::describe_table(table), cancel)
        .await?;
    catalog::parse_describe_table(&reply)
}

/// The first `n` items of a table (one Scan), for its columns.
pub async fn sample(
    client: &Client,
    table: &str,
    n: u32,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<Item>, AppError> {
    let reply = client
        .call("Scan", &b::sample_scan(table, n), cancel)
        .await?;
    b::reply_items(&reply)
}

/// The TTL attribute; `None` too when the server has no TTL (DynamoDB Local, Alternator).
pub async fn ttl_attribute(
    client: &Client,
    table: &str,
    cancel: Option<&CancellationToken>,
) -> Result<Option<String>, AppError> {
    match client
        .call_detailed(
            "DescribeTimeToLive",
            &b::describe_time_to_live(table),
            cancel,
        )
        .await
    {
        Ok(reply) => Ok(catalog::parse_ttl(&reply)),
        Err(CallError::Service(e)) if !e.is_throttling() && e.status < 500 => Ok(None),
        Err(e) => Err(e.into_app_error()),
    }
}

/// A table's structure from its description, a sample of `sample_size` items and its TTL.
pub async fn table_meta(
    client: &Client,
    table: &str,
    sample_size: u32,
    cancel: Option<&CancellationToken>,
) -> Result<(TableMeta, TableDescription), AppError> {
    let desc = describe(client, table, cancel).await?;
    let items = sample(client, table, sample_size, cancel).await?;
    let ttl = ttl_attribute(client, table, cancel).await?;
    let meta = catalog::table_meta(&desc, &items, client.region(), ttl.as_deref());
    Ok((meta, desc))
}

/// Items of one grid page and where the next starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
    pub next: Option<Item>,
    pub requests: u32,
    /// Capacity units the requests consumed, when asked for.
    pub consumed: Option<f64>,
}

/// What one grid page or console command keeps at most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Item bytes ([`item_size`]): once they are in, the reads stop with their cursor.
    pub bytes: usize,
    /// A console result's cells: every item has one for every attribute shown.
    pub cells: usize,
}

impl Budget {
    /// DynamoDB's pages are at most 1 MB of items, so only a server sending far bigger ones (or
    /// items of many tiny maps, a tree node each) reaches the bytes; the cells are 10,000 rows (the editor's default) of [`MAX_COLUMNS`] attributes.
    pub const DEFAULT: Budget = Budget {
        bytes: 128 * 1024 * 1024,
        cells: 5_000_000,
    };
}

/// At most `limit` items of `read` after `start`, in at most `max_requests` requests, and no
/// more once `max_bytes` of items are in.
pub async fn read_page(
    client: &Client,
    read: &ReadRequest,
    limit: u32,
    start: Option<Item>,
    max_requests: u32,
    max_bytes: usize,
    cancel: Option<&CancellationToken>,
) -> Result<Page, AppError> {
    let limit = limit.max(1);
    let mut page = Page {
        items: Vec::new(),
        next: start,
        requests: 0,
        consumed: None,
    };
    let mut kept = 0usize;
    loop {
        let remaining = limit - page.items.len() as u32;
        let body = read.page(Some(remaining), page.next.as_ref());
        let reply = client.call(read.action, &body, cancel).await?;
        page.requests += 1;
        if let Some(units) = reply
            .get("ConsumedCapacity")
            .and_then(|c| c.get("CapacityUnits"))
            .and_then(serde_json::Value::as_f64)
        {
            *page.consumed.get_or_insert(0.0) += units;
        }
        let items = b::reply_items(&reply)?;
        // More than asked for would put the cursor past rows the page cannot show.
        if items.len() > remaining as usize {
            return Err(AppError::db(format!(
                "DynamoDB returned {} items for a Limit of {remaining}",
                items.len()
            )));
        }
        kept += items.iter().map(item_size).sum::<usize>();
        page.items.extend(items);
        page.next = b::reply_last_key(&reply)?;
        if page.next.is_none()
            || page.items.len() as u32 >= limit
            || page.requests >= max_requests.max(1)
            || kept >= max_bytes
        {
            return Ok(page);
        }
    }
}

/// How the grid reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridOptions {
    pub consistent: bool,
    pub consumed_capacity: bool,
    pub page_requests: u32,
    /// [`Budget::bytes`].
    pub max_bytes: usize,
}

/// Whether the grid's sort can be served: only by a Query on the sort key.
fn descending(req: &BrowseRequest, keys: &KeySchema) -> Result<bool, AppError> {
    match req.sort.as_slice() {
        [] => Ok(false),
        [s] if keys.range.as_ref().is_some_and(|(r, _)| *r == s.column) => {
            Ok(s.dir == SortDir::Desc)
        }
        _ => Err(AppError::Unsupported(
            "DynamoDB sorts only by the sort key, in a Query (filter the partition key with =)"
                .into(),
        )),
    }
}

/// The rows of items in these columns.
pub fn rows(columns: &[ColumnMeta], items: &[Item], cap: Cap) -> Vec<Vec<Value>> {
    items
        .iter()
        .map(|item| columns.iter().map(|c| cell(item, &c.name, cap)).collect())
        .collect()
}

/// A secondary index to read in place of the table: its name, keys and kind.
#[derive(Debug, Clone, Copy)]
pub struct IndexRead<'a> {
    pub name: &'a str,
    pub keys: &'a KeySchema,
    /// A global index (eventually consistent only), not a local one.
    pub global: bool,
}

/// One grid page of `meta`'s table (`keys`: its primary key), or of one of its indexes, and
/// the items it shows (the session remembers a table's for the edits).
pub async fn browse(
    client: &Client,
    meta: &TableMeta,
    keys: &KeySchema,
    index: Option<IndexRead<'_>>,
    req: &BrowseRequest,
    opts: &GridOptions,
    cancel: Option<&CancellationToken>,
) -> Result<(BrowsePage, Vec<Item>), AppError> {
    let started = Instant::now();
    let read_keys = index.map_or(keys, |i| i.keys);
    // Strongly consistent reads are not available on a global index (DynamoDB refuses them).
    let consistent = opts.consistent && index.is_none_or(|i| !i.global);
    let read = b::read_request(&ReadSpec {
        table: &meta.table.name,
        index: index.map(|i| i.name),
        keys: read_keys,
        columns: &meta.columns,
        filter: req.filter.as_ref(),
        descending: descending(req, read_keys)?,
        consistent,
        consumed_capacity: opts.consumed_capacity,
    })?;
    if !req.sort.is_empty() && read.action != "Query" {
        return Err(AppError::Unsupported(
            "DynamoDB sorts only a Query: filter the partition key with =, the sort key with at \
             most one condition, and the other columns without Any column"
                .into(),
        ));
    }
    let start = req.cursor.as_deref().map(b::decode_cursor).transpose()?;
    let page = read_page(
        client,
        &read,
        req.limit,
        start,
        opts.page_requests,
        opts.max_bytes,
        cancel,
    )
    .await?;
    let mut columns = meta.columns.clone();
    catalog::merge_columns(&mut columns, &page.items);
    if let Some(wanted) = &req.columns {
        columns = wanted
            .iter()
            .filter_map(|w| columns.iter().find(|c| c.name == *w).cloned())
            .collect();
    }
    let shown = BrowsePage {
        rows: rows(&columns, &page.items, Cap::Grid),
        columns,
        pk_columns: keys.names().into_iter().map(str::to_string).collect(),
        identity: RowIdentity::Pk,
        row_ids: None,
        offset: req.offset,
        limit: req.limit,
        sql: read.text(),
        duration_ms: started.elapsed().as_millis() as u64,
        next_cursor: page.next.as_ref().map(b::encode_cursor),
    };
    Ok((shown, page.items))
}

/// The table's item count: DynamoDB's estimate (refreshed about every six hours) unless an
/// exact count is asked for or a filter is set; then every page is read with `Select: COUNT`
/// (the whole table's read capacity).
pub async fn count(
    client: &Client,
    meta: &TableMeta,
    keys: &KeySchema,
    filter: Option<&FilterGroup>,
    exact: bool,
    consistent: bool,
    cancel: Option<&CancellationToken>,
) -> Result<RowCount, AppError> {
    let filtered = filter.is_some_and(|f| !f.items.is_empty());
    if !exact
        && !filtered
        && let Some(n) = meta.row_estimate
    {
        return Ok(RowCount {
            count: n,
            exact: false,
        });
    }
    let read = b::read_request(&ReadSpec {
        table: &meta.table.name,
        index: None,
        keys,
        columns: &meta.columns,
        filter,
        descending: false,
        consistent,
        consumed_capacity: false,
    })?
    .count();
    let mut total: i64 = 0;
    let mut start: Option<Item> = None;
    loop {
        let reply = client
            .call(read.action, &read.page(None, start.as_ref()), cancel)
            .await?;
        let n = reply
            .get("Count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        // A hostile server's counts must not wrap (or panic in a debug build).
        total = total
            .checked_add(n)
            .filter(|t| n >= 0 && *t >= 0)
            .ok_or_else(|| {
                AppError::db(format!(
                    "DynamoDB returned a Count of {n} after {total} items"
                ))
            })?;
        match b::reply_last_key(&reply)? {
            Some(k) if Some(&k) != start.as_ref() => start = Some(k),
            Some(_) => {
                return Err(AppError::db(
                    "DynamoDB returned the same LastEvaluatedKey twice",
                ));
            }
            None => {
                return Ok(RowCount {
                    count: total,
                    exact: true,
                });
            }
        }
    }
}

/// One attribute of one item, whole.
pub async fn fetch_cell(
    client: &Client,
    table: &str,
    keys: &KeySchema,
    key: &RowKey,
    column: &str,
    consistent: bool,
    cancel: Option<&CancellationToken>,
) -> Result<Value, AppError> {
    let key = b::item_key(key, keys)?;
    let reply = client
        .call(
            "GetItem",
            &b::get_attribute(table, &key, column, consistent),
            cancel,
        )
        .await?;
    match b::reply_item(&reply)? {
        Some(item) => Ok(cell(&item, column, Cap::Full)),
        None => Err(AppError::NotFound(
            "the item no longer exists (deleted by someone else)".into(),
        )),
    }
}

// --- the console ------------------------------------------------------------------------------

/// What a console command printed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConsoleResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
    /// A PartiQL write: without items back it reports what it changed rather than an empty grid.
    pub write: bool,
    /// Items a write changed, when its reply proves it (`PartiqlWrite::affected`).
    pub affected: Option<u64>,
    pub notices: Vec<String>,
}

/// Items as the result grid of `out`: the attributes in the order they were first seen, typed by
/// what was seen (`S|N`). Like the grid, it shows at most [`MAX_COLUMNS`] attributes, and at
/// most the rows that fit `max_cells`: every item has a cell for every attribute, so the items
/// of a sparse table (attributes that differ per item) would otherwise make rows times
/// attributes of missing cells. A notice says what was left out.
pub fn items_grid(items: &[Item], max_cells: usize, out: &mut ConsoleResult) {
    let attributes = catalog::sampled_attributes(items);
    let hidden = attributes.len().saturating_sub(MAX_COLUMNS);
    let columns: Vec<ResultColumn> = attributes
        .into_iter()
        .take(MAX_COLUMNS)
        .map(|(name, types)| ResultColumn {
            name,
            data_type: catalog::type_label(&types),
            kind: catalog::kind_of(&types),
            nullable: None,
            table: None,
            origin: None,
        })
        .collect();
    if hidden > 0 {
        out.notices.push(format!(
            "The items have more than {MAX_COLUMNS} attributes: {hidden} are not shown (name the \
             ones you need in the SELECT list or a ProjectionExpression)"
        ));
    }
    let fit = max_cells / columns.len().max(1);
    if items.len() > fit {
        out.truncated = true;
        out.notices.push(format!(
            "Only the first {fit} items are shown: with {} attributes each, more would not fit \
             the result's {max_cells} cells",
            columns.len()
        ));
    }
    out.rows = items
        .iter()
        .take(fit)
        .map(|i| {
            columns
                .iter()
                .map(|c| cell(i, &c.name, Cap::Grid))
                .collect()
        })
        .collect();
    out.columns = columns;
}

/// The column of a reply shown whole.
fn reply_column() -> ResultColumn {
    ResultColumn {
        name: "reply".into(),
        data_type: "M".into(),
        kind: crate::types::schema::ValueKind::Json,
        nullable: None,
        table: None,
        origin: None,
    }
}

/// Requests a PartiQL `SELECT` may take to fill its rows (pages can be empty).
pub const MAX_STATEMENT_PAGES: usize = 100;

/// Runs one console command: a PartiQL statement page by page up to `max_rows`, an API
/// request once (its items as a grid, any other reply as JSON), within [`Budget::DEFAULT`].
pub async fn run_command(
    client: &Client,
    cmd: &ConsoleCommand,
    max_rows: usize,
    cancel: Option<&CancellationToken>,
) -> Result<ConsoleResult, AppError> {
    run_command_within(client, cmd, max_rows, Budget::DEFAULT, cancel).await
}

/// [`run_command`] keeping at most `budget`: a PartiQL statement stops reading pages once its
/// bytes are in, and the result shows the rows its cells hold.
pub async fn run_command_within(
    client: &Client,
    cmd: &ConsoleCommand,
    max_rows: usize,
    budget: Budget,
    cancel: Option<&CancellationToken>,
) -> Result<ConsoleResult, AppError> {
    let mut out = ConsoleResult::default();
    match cmd {
        ConsoleCommand::Partiql(statement) => {
            let mut items = Vec::new();
            let mut kept = 0usize;
            let mut token: Option<String> = None;
            for _ in 0..MAX_STATEMENT_PAGES {
                let (action, body) = b::command_request(cmd, token.as_deref());
                let reply = client.call_quiet(&action, &body, cancel).await?;
                let page = b::reply_items(&reply)?;
                kept += page.iter().map(item_size).sum::<usize>();
                items.extend(page);
                token = reply
                    .get("NextToken")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                if token.is_none() || items.len() >= max_rows {
                    break;
                }
                if kept >= budget.bytes {
                    out.notices.push(format!(
                        "Stopped after {} MB of items: narrow the statement with a WHERE clause \
                         or the attributes it selects",
                        budget.bytes >> 20
                    ));
                    break;
                }
            }
            if items.len() > max_rows || token.is_some() {
                out.truncated = true;
                items.truncate(max_rows);
            }
            let write = b::partiql_write(statement);
            out.write = write != PartiqlWrite::Nothing;
            out.affected = write.affected(items.len());
            items_grid(&items, budget.cells, &mut out);
        }
        ConsoleCommand::Api { action, body } => {
            let reply = client.call_quiet(action, body, cancel).await?;
            let items = match (reply.get("Items"), reply.get("Item")) {
                (Some(_), _) => Some(b::reply_items(&reply)?),
                (None, Some(i)) => Some(vec![item_from_json(i)?]),
                _ => None,
            };
            match items {
                Some(mut items) => {
                    if items.len() > max_rows {
                        out.truncated = true;
                        items.truncate(max_rows);
                    }
                    if let Some(k) = b::reply_last_key(&reply)? {
                        out.notices.push(format!(
                            "More items follow: pass \"ExclusiveStartKey\": {}",
                            b::item_text(&k)
                        ));
                    }
                    items_grid(&items, budget.cells, &mut out);
                }
                None => {
                    out.columns = vec![reply_column()];
                    out.rows = vec![vec![Value::Json(reply.to_string())]];
                }
            }
        }
    }
    Ok(out)
}

// --- edits ------------------------------------------------------------------------------------

/// What became of one staged change.
#[derive(Debug, Clone)]
pub enum Outcome {
    Applied,
    Failed(AppError),
    /// Not sent, or rolled back with the transaction it was in.
    NotApplied(String),
}

impl Outcome {
    /// The error text of a change that was not applied (`None`: applied).
    pub fn error(&self) -> Option<String> {
        match self {
            Outcome::Applied => None,
            Outcome::Failed(e) => Some(e.to_string()),
            Outcome::NotApplied(why) => Some(why.clone()),
        }
    }
}

/// Records `outcome` for `change`; a change's failure outranks its other item's "not applied".
fn set_outcome(out: &mut Vec<(usize, Outcome)>, change: usize, outcome: Outcome) {
    match out.iter_mut().find(|(c, _)| *c == change) {
        Some((_, Outcome::Failed(_))) => {}
        Some((_, o)) => *o = outcome,
        None => out.push((change, outcome)),
    }
}

/// Applies each change with its own request, in order, stopping at the first that fails: the
/// ones before it are applied, the ones after it are not sent.
pub async fn apply_each(
    client: &Client,
    plan: &EditPlan,
    cancel: Option<&CancellationToken>,
) -> Vec<(usize, Outcome)> {
    let mut out = Vec::with_capacity(plan.ops.len());
    let mut failed = false;
    for op in &plan.ops {
        if failed {
            out.push((
                op.change,
                Outcome::NotApplied("not applied: an earlier change failed".into()),
            ));
            continue;
        }
        let (action, body) = op.request();
        match client.call(action, &body, cancel).await {
            Ok(_) => out.push((op.change, Outcome::Applied)),
            Err(e) => {
                failed = true;
                out.push((op.change, Outcome::Failed(e)));
            }
        }
    }
    out
}

/// Applies the changes as transactions of at most 100 items (one for most commits). A
/// cancelled transaction applies none of its changes; the reasons say which failed. Changes of
/// later transactions are not sent once one fails, earlier ones stay applied.
pub async fn apply_atomic(
    client: &Client,
    plan: &EditPlan,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<(usize, Outcome)>, AppError> {
    let txs = plan.transactions()?;
    let mut by_change: Vec<(usize, Outcome)> = Vec::new();
    let mut failed = false;
    for tx in &txs {
        if failed {
            for c in &tx.changes {
                let why = "not applied: an earlier transaction failed".to_string();
                set_outcome(&mut by_change, *c, Outcome::NotApplied(why));
            }
            continue;
        }
        match client
            .call_detailed("TransactWriteItems", &tx.body, cancel)
            .await
        {
            Ok(_) => {
                for c in &tx.changes {
                    set_outcome(&mut by_change, *c, Outcome::Applied);
                }
            }
            Err(CallError::Service(e)) if e.is_transaction_canceled() => {
                failed = true;
                let texts = transaction_item_errors(&e.cancellation_reasons, tx.changes.len());
                for ((i, c), text) in tx.changes.iter().enumerate().zip(texts) {
                    let reason_failed = e.cancellation_reasons.get(i).is_some_and(|r| r.failed());
                    let outcome = if reason_failed {
                        Outcome::Failed(AppError::db(text))
                    } else {
                        Outcome::NotApplied(text)
                    };
                    set_outcome(&mut by_change, *c, outcome);
                }
            }
            Err(e) => {
                failed = true;
                let e = e.into_app_error();
                for c in &tx.changes {
                    set_outcome(&mut by_change, *c, Outcome::Failed(e.clone()));
                }
            }
        }
    }
    Ok(by_change)
}
