//! The key browser's reads: a page of keys by SCAN cursor, the count of a database or of the keys
//! a filter matches, and the whole value of one cell.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::drivers::{CancelHandle, CancelSlot, arm_cancel};
use crate::error::AppError;
use crate::sqlgen::native::redis::{self as cmd, CHANGED, Command, col, row_key};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey, TraceCategory,
};
use crate::types::schema::TableMeta;
use crate::types::value::Value;

use super::conn::{Abort, KillAction, Link, ok_reply, trace};
use super::keys::{self, KeyType};
use super::scan::{
    KeyFilter, KeyMatch, assemble, cursor, detail_commands, key_filter, meta_commands, next_token,
    parse_meta, parse_scan, scan_keys,
};
use super::session::RedisSession;
use super::values::{as_bytes, as_int, as_pairs, int_cell, json, key_value, ttl_cell};

type Resp = ::redis::Value;

/// How long one page keeps scanning for keys a filter matches before it returns what it has
/// (and the cursor to go on from): a sparse pattern over a large keyspace would otherwise give
/// page after empty page.
const PAGE_BUDGET: Duration = Duration::from_millis(500);

/// How long an estimated count samples the keyspace before it extrapolates.
const SAMPLE_BUDGET: Duration = Duration::from_millis(300);

/// The `COUNT` of each SCAN a count sends.
const COUNT_STEP: u32 = 1000;

/// The most elements `fetch_cell` loads of a container (beyond: the per-type editor pages it).
pub const FETCH_MAX_ELEMENTS: i64 = 100_000;

/// A cancel of a grid read: stop waiting (the connection is shared with other reads, so it is
/// never killed; a SCAN step or a pipelined read is short on the server).
fn arm(cancel: &CancelSlot) -> Result<(Arc<Abort>, crate::drivers::ArmedCancel<'_>), AppError> {
    let abort = Arc::new(Abort::default());
    let armed = arm_cancel(
        cancel,
        CancelHandle::Custom(Arc::new(KillAction {
            abort: abort.clone(),
            kill: None,
        })),
    )?;
    Ok((abort, armed))
}

/// One SCAN step: its reply's keys and the next cursor (`None` at the end).
async fn scan_step(
    s: &RedisSession,
    link: &Link,
    c: &Command,
) -> Result<(Vec<Vec<u8>>, Option<String>), AppError> {
    let started = Instant::now();
    let out = link.send(c).await.and_then(|r| parse_scan(&r));
    trace(
        &s.trace,
        TraceCategory::Data,
        std::slice::from_ref(c),
        started,
        &out,
        |r| Some(r.items.len() as u64),
    );
    let reply = out?;
    Ok((scan_keys(&reply), next_token(&reply.cursor)))
}

/// What a page found: its keys, the SCANs sent and where the next page starts.
struct Found {
    keys: Vec<Vec<u8>>,
    scans: Vec<String>,
    next: Option<String>,
}

/// The keys of one page: the exact key, or SCAN steps from `start` until the page is full, the
/// keyspace ends or the page's time is up.
async fn page_keys(
    s: &RedisSession,
    link: &Link,
    filter: &KeyFilter,
    start: &str,
    limit: u32,
) -> Result<Found, AppError> {
    if let KeyMatch::Exact(k) = &filter.keys {
        return Ok(Found {
            keys: vec![k.clone()],
            scans: Vec::new(),
            next: None,
        });
    }
    let count = s.params.scan_count.unwrap_or(limit);
    let started = Instant::now();
    let mut seen = HashSet::new();
    let mut found = Found {
        keys: Vec::new(),
        scans: Vec::new(),
        next: Some(start.to_string()),
    };
    while let Some(at) = found.next.take() {
        let Some(c) = filter.scan_command(&at, count, &s.gates()) else {
            break;
        };
        found.scans.push(c.text());
        let (keys, next) = scan_step(s, link, &c).await?;
        // SCAN may return a key twice: once per page.
        found
            .keys
            .extend(keys.into_iter().filter(|k| seen.insert(k.clone())));
        found.next = next;
        if found.keys.len() >= limit as usize || started.elapsed() >= PAGE_BUDGET {
            break;
        }
    }
    Ok(found)
}

/// A page of keys of a database: their type, TTL, size, memory and value preview.
pub async fn browse(
    s: &RedisSession,
    meta: &TableMeta,
    req: &BrowseRequest,
    cancel: CancelSlot,
) -> Result<BrowsePage, AppError> {
    let started = Instant::now();
    let db = s.db_of(&meta.table)?;
    let filter = key_filter(req.filter.as_ref())?;
    let start = cursor(req.cursor.as_deref())?;
    let limit = req.limit.max(1);
    let (abort, _armed) = arm(&cancel)?;
    let work = s.read(db, |link| {
        let (filter, start) = (&filter, &start);
        async move {
            let found = page_keys(s, &link, filter, start, limit).await?;
            let meta_cmds = meta_commands(&found.keys);
            let t = Instant::now();
            let replies = link.pipeline(&meta_cmds).await;
            trace(
                &s.trace,
                TraceCategory::Data,
                &meta_cmds,
                t,
                &replies,
                |r| Some(r.len() as u64),
            );
            let rows = parse_meta(&found.keys, &replies?, filter);
            let memory = s.params.memory_column.unwrap_or(true);
            let plan = detail_commands(&rows, &s.limits, memory);
            let t = Instant::now();
            let details = link.pipeline(&plan.commands).await;
            trace(
                &s.trace,
                TraceCategory::Data,
                &plan.commands,
                t,
                &details,
                |r| Some(r.len() as u64),
            );
            Ok::<_, AppError>((assemble(&rows, &plan, &details?), found))
        }
    });
    let (rows, found) = abort.run(work).await?;
    let sql = match &filter.keys {
        KeyMatch::Exact(k) => cmd::key_type(k).text(),
        _ => found.scans.join("\n"),
    };
    Ok(BrowsePage {
        columns: keys::columns(),
        rows,
        pk_columns: keys::primary_key(),
        identity: RowIdentity::Pk,
        row_ids: None,
        offset: req.offset,
        limit: req.limit,
        sql,
        duration_ms: started.elapsed().as_millis() as u64,
        next_cursor: found.next,
    })
}

/// How many keys a database holds (`DBSIZE`), or how many a filter matches: an exact key is
/// `EXISTS` (and its `TYPE` for a type filter); a pattern or a type scans the keyspace, all of
/// it when `exact`, otherwise for a moment, extrapolated from the share of the keyspace it saw.
pub async fn count(
    s: &RedisSession,
    meta: &TableMeta,
    filter: Option<&FilterGroup>,
    exact: bool,
    cancel: CancelSlot,
) -> Result<RowCount, AppError> {
    let db = s.db_of(&meta.table)?;
    let filter = key_filter(filter)?;
    let (abort, _armed) = arm(&cancel)?;
    let work = s.read(db, |link| {
        let filter = &filter;
        async move { count_on(s, &link, filter, exact).await }
    });
    abort.run(work).await
}

async fn count_on(
    s: &RedisSession,
    link: &Link,
    filter: &KeyFilter,
    exact: bool,
) -> Result<RowCount, AppError> {
    let one = |c: Command| async move {
        let started = Instant::now();
        let out = link.send(&c).await.and_then(ok_reply);
        trace(
            &s.trace,
            TraceCategory::Data,
            std::slice::from_ref(&c),
            started,
            &out,
            |_| None,
        );
        out
    };
    match (&filter.keys, &filter.ty) {
        (KeyMatch::All, None) => {
            let n = as_int(&one(cmd::dbsize()).await?).unwrap_or(0);
            return Ok(RowCount {
                count: n,
                exact: true,
            });
        }
        (KeyMatch::Exact(k), ty) => {
            let found = match ty {
                None => as_int(&one(cmd::exists(k)).await?).unwrap_or(0) > 0,
                Some(_) => {
                    let t = KeyType::from_reply(&one(cmd::key_type(k)).await?);
                    t.exists() && filter.matches_type(&t)
                }
            };
            return Ok(RowCount {
                count: i64::from(found),
                exact: true,
            });
        }
        _ => {}
    }
    let gates = s.gates();
    // Before 6.0 SCAN has no TYPE: the keys a step finds are typed one by one.
    let typed_after = filter.ty.is_some() && !gates.scan_type;
    let started = Instant::now();
    let mut at = "0".to_string();
    let (mut matched, mut steps) = (0i64, 0i64);
    while let Some(c) = filter.scan_command(&at, COUNT_STEP, &gates) {
        let (keys, next) = scan_step(s, link, &c).await?;
        steps += 1;
        matched += if typed_after {
            let types: Vec<Command> = keys.iter().map(|k| cmd::key_type(k)).collect();
            link.pipeline(&types)
                .await?
                .iter()
                .filter(|r| filter.matches_type(&KeyType::from_reply(r)))
                .count() as i64
        } else {
            keys.len() as i64
        };
        match next {
            None => {
                return Ok(RowCount {
                    count: matched,
                    exact: true,
                });
            }
            Some(n) => at = n,
        }
        if !exact && started.elapsed() >= SAMPLE_BUDGET {
            break;
        }
    }
    // Each step looks at about COUNT_STEP keys of the keyspace.
    let total = as_int(&one(cmd::dbsize()).await?).unwrap_or(0);
    let seen = (steps * i64::from(COUNT_STEP)).max(1);
    let estimate = if seen >= total {
        matched
    } else {
        ((matched as f64) * (total as f64) / (seen as f64)).round() as i64
    };
    Ok(RowCount {
        count: estimate.clamp(matched, total.max(matched)),
        exact: false,
    })
}

/// One cell of a key in full: the whole value (a string with `GET`, a container whole when it
/// has at most `FETCH_MAX_ELEMENTS` elements, in the preview's form), or its TTL, size, exact
/// memory or type. The key must still have the type its row was loaded with.
pub async fn fetch_cell(
    s: &RedisSession,
    meta: &TableMeta,
    key: &RowKey,
    column: &str,
) -> Result<Value, AppError> {
    let db = s.db_of(&meta.table)?;
    let (k, loaded) = row_key(key)?;
    let read = |c: Command| async move { s.read_one(db, TraceCategory::Data, &c).await };
    let ty = KeyType::from_reply(&read(cmd::key_type(&k)).await?);
    if !ty.exists() {
        return Err(AppError::NotFound(
            "the key no longer exists (changed by someone else)".into(),
        ));
    }
    if loaded.as_deref().is_some_and(|t| t != ty.name()) {
        return Err(AppError::InvalidInput(CHANGED.into()));
    }
    match column {
        col::KEY => Ok(key_value(&k)),
        col::TYPE => Ok(Value::Text(ty.name().to_string())),
        col::TTL => Ok(ttl_cell(as_int(&read(cmd::pttl(&k)).await?).unwrap_or(-1))),
        col::SIZE => match ty.size_command(&k) {
            Some(c) => Ok(int_cell(&read(c).await?)),
            None => Ok(Value::Null),
        },
        col::MEMORY => Ok(int_cell(&read(cmd::memory_usage(&k, Some(0))).await?)),
        col::VALUE => whole_value(&ty, &k, read).await,
        other => Err(AppError::InvalidInput(format!(
            "a Redis key has no column \u{201c}{other}\u{201d}"
        ))),
    }
}

async fn whole_value<F, Fut>(ty: &KeyType, k: &[u8], read: F) -> Result<Value, AppError>
where
    F: Fn(Command) -> Fut,
    Fut: std::future::Future<Output = Result<Resp, AppError>>,
{
    if let Some(size) = ty.size_command(k)
        && *ty != KeyType::String
    {
        let n = as_int(&read(size).await?).unwrap_or(0);
        if n > FETCH_MAX_ELEMENTS {
            return Err(AppError::InvalidInput(format!(
                "the {} has {n} elements: open it in the value editor, which reads it page by page",
                ty.name()
            )));
        }
    }
    let all = keys::Limits {
        bytes: u64::MAX,
        elements: u32::MAX,
    };
    Ok(match ty {
        KeyType::String => match as_bytes(&read(cmd::get(k)).await?) {
            Some(b) => match String::from_utf8(b) {
                Ok(text) => Value::Text(text),
                Err(e) => Value::from_bytes_uncapped(e.as_bytes()),
            },
            None => Value::Null,
        },
        KeyType::Json => keys::preview_cell(ty, &read(cmd::json_get(k, None)).await?, None, &all),
        KeyType::List => {
            keys::preview_cell(ty, &read(cmd::list_range(k, 0, -1)).await?, None, &all)
        }
        KeyType::ZSet => {
            keys::preview_cell(ty, &read(cmd::zset_range(k, 0, -1)).await?, None, &all)
        }
        KeyType::Stream => keys::preview_cell(
            ty,
            &read(cmd::stream_range_rev(k, "+", "-", u32::MAX)).await?,
            None,
            &all,
        ),
        KeyType::Set => Value::Json(json(&read(cmd::set_members(k)).await?)),
        KeyType::Hash => {
            let reply = read(cmd::hash_all(k)).await?;
            let pairs: Vec<(Resp, Resp)> = as_pairs(&reply)
                .unwrap_or_default()
                .into_iter()
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect();
            Value::Json(json(&Resp::Map(pairs)))
        }
        KeyType::Other(name) => {
            return Err(AppError::Unsupported(format!(
                "Redis: a {name} key has no standard read command; use the console"
            )));
        }
        KeyType::None => Value::Null,
    })
}
