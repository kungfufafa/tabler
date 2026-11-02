//! The session's own calls (`session_call`) for the per-type value editor, the New key dialog,
//! the database menu and the server panel. Reads: `serverInfo`, `keyPage`, `slowLog`,
//! `previewEdits`. Writes: `editKey`, `newKey`, `copyKey`, `flushDb`. Container edits go through
//! the same guarded apply as the grid's (`apply::apply_edits`), one result per edit.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::drivers::native::{CallKind, unknown_call};
use crate::error::AppError;
use crate::sqlgen::native::redis::{
    self as cmd, Command, Guard, KeyEdit, NewKey, key_arg, preview, ttl_ms, value_arg,
};
use crate::types::browse::TraceCategory;
use crate::types::profile::Driver;
use crate::types::value::Value;

use super::apply::{Outcome, apply_edits};
use super::info::Info;
use super::keys::KeyType;
use super::scan::{cursor, next_token, parse_scan};
use super::session::{RedisSession, read_only_error};
use super::values::{
    as_int, as_items, as_pairs, as_scored, as_text, bytes_value, cell, int_cell, json, key_value,
    number_cell, string_preview, ttl_cell,
};

type Resp = ::redis::Value;

/// Elements a `keyPage` reads when the call names no count.
const PAGE_COUNT: u32 = 100;
/// The most elements one `keyPage` reads.
const MAX_PAGE_COUNT: u32 = 10_000;

/// Whether a call only reads; anything unknown changes something.
pub fn call_kind(method: &str) -> CallKind {
    match method {
        "serverInfo" | "keyPage" | "slowLog" | "previewEdits" => CallKind::Read,
        _ => CallKind::Write,
    }
}

// --- arguments and replies (ts-exported for the renderer) --------------------------------------

/// A key of a database, as the grid holds it (text, or bytes when not UTF-8).
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisKeyArgs {
    pub db: u32,
    pub key: Value,
    /// The type the key was loaded with; the call fails when it changed.
    #[serde(rename = "type")]
    pub ty: String,
}

/// `keyPage`: a page of a key's elements from `cursor` (none: the first).
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisPageArgs {
    #[serde(flatten)]
    pub key: RedisKeyArgs,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub count: Option<u32>,
}

/// One element of a container: a hash field and its value, a list index and its element, a set
/// member, a sorted set member and its score, a stream entry id and its fields (JSON).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisElement {
    pub name: Value,
    pub value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisKeyPage {
    pub key: Value,
    #[serde(rename = "type")]
    pub ty: String,
    /// Seconds until it expires; null without expiry.
    pub ttl: Value,
    /// Bytes of a string, elements of a container.
    pub size: Value,
    /// A string's or a RedisJSON document's value (a string beyond the preview is cut and
    /// marked truncated).
    pub value: Option<Value>,
    pub elements: Vec<RedisElement>,
    /// Where the next page starts; none after the last.
    pub next_cursor: Option<String>,
}

/// One edit of the value editor.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename_all_fields = "camelCase")]
pub enum RedisEdit {
    /// A string's whole new value (its TTL kept).
    StringSet {
        value: Value,
    },
    /// A field's new value (`HSET`).
    HashSet {
        field: Value,
        value: Value,
    },
    /// A new field; refused when it exists.
    HashAdd {
        field: Value,
        value: Value,
    },
    HashDelete {
        field: Value,
    },
    /// Renames a field that holds `value` (as loaded).
    HashRename {
        from: Value,
        to: Value,
        value: Value,
    },
    /// Replaces the element at `index`, which holds `old` (as loaded).
    ListSet {
        index: i64,
        old: Value,
        value: Value,
    },
    ListPush {
        values: Vec<Value>,
        head: bool,
    },
    ListInsert {
        before: bool,
        pivot: Value,
        value: Value,
    },
    /// Deletes the element at `index`, which holds `old` (as loaded).
    ListDelete {
        index: i64,
        old: Value,
    },
    SetAdd {
        member: Value,
    },
    SetRemove {
        member: Value,
    },
    SetRename {
        from: Value,
        to: Value,
    },
    /// A new member; refused when it exists.
    ZsetAdd {
        member: Value,
        score: String,
    },
    /// A member's new score.
    ZsetScore {
        member: Value,
        score: String,
    },
    ZsetRemove {
        member: Value,
    },
    /// A new entry; `id` none for the server's next.
    StreamAdd {
        id: Option<String>,
        fields: Vec<RedisField>,
    },
    StreamDelete {
        id: String,
    },
    JsonSet {
        path: String,
        doc: String,
    },
    JsonDelete {
        path: String,
    },
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisField {
    pub name: Value,
    pub value: Value,
}

/// `editKey` / `previewEdits`: edits of one key, applied in order.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisEditArgs {
    #[serde(flatten)]
    pub key: RedisKeyArgs,
    pub edits: Vec<RedisEdit>,
}

/// The first value of a new key.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum RedisNewValue {
    String {
        value: Value,
    },
    Hash {
        fields: Vec<RedisField>,
    },
    List {
        values: Vec<Value>,
    },
    Set {
        members: Vec<Value>,
    },
    Zset {
        members: Vec<RedisScored>,
    },
    Stream {
        id: Option<String>,
        fields: Vec<RedisField>,
    },
    Json {
        doc: String,
    },
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisScored {
    pub member: Value,
    pub score: String,
}

/// `newKey`: a key that must not exist yet.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisNewKeyArgs {
    pub db: u32,
    pub key: Value,
    /// Seconds until it expires; none for no expiry.
    #[serde(default)]
    pub ttl: Option<Value>,
    pub value: RedisNewValue,
}

/// `copyKey`: a copy of `from` named `to` (refused when `to` exists).
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisCopyArgs {
    pub db: u32,
    pub from: Value,
    pub to: Value,
}

/// `flushDb`: every key of a database goes.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisDbArgs {
    pub db: u32,
}

/// What became of each edit, in order, and the commands they were.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisEditResult {
    pub results: Vec<RedisEditOutcome>,
    pub preview: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisEditOutcome {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisInfoSection {
    pub name: String,
    pub fields: Vec<(String, String)>,
}

/// `serverInfo`: the server and its `INFO`.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisServerInfo {
    pub product: String,
    pub version: String,
    pub mode: String,
    pub protocol: u8,
    pub databases: u32,
    pub read_only: bool,
    pub sections: Vec<RedisInfoSection>,
}

/// `slowLog`: one entry of `SLOWLOG GET`.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedisSlowEntry {
    pub id: Value,
    /// Unix time in seconds.
    pub started_at: Value,
    pub duration_us: Value,
    /// The command as redis-cli would type it (the server cuts long arguments).
    pub command: String,
    pub client: Option<String>,
    pub client_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RedisSlowLogArgs {
    pub count: Option<u32>,
}

fn args<T: serde::de::DeserializeOwned>(method: &str, v: serde_json::Value) -> Result<T, AppError> {
    serde_json::from_value(v).map_err(|e| AppError::InvalidInput(format!("Redis {method}: {e}")))
}

fn reply<T: Serialize>(v: &T) -> Result<serde_json::Value, AppError> {
    serde_json::to_value(v).map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn call(
    s: &RedisSession,
    method: &str,
    a: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    s.ensure_open()?;
    if s.read_only && call_kind(method) == CallKind::Write {
        return Err(read_only_error());
    }
    match method {
        "serverInfo" => reply(&server_info(s).await?),
        "keyPage" => reply(&key_page(s, &args(method, a)?).await?),
        "slowLog" => reply(&slow_log(s, &args(method, a)?).await?),
        "previewEdits" => {
            let a: RedisEditArgs = args(method, a)?;
            reply(
                &preview(&key_edits(&a, s.gates())?)
                    .into_iter()
                    .map(|(_, t)| t)
                    .collect::<Vec<_>>(),
            )
        }
        "editKey" => {
            let a: RedisEditArgs = args(method, a)?;
            let edits = key_edits(&a, s.gates())?;
            reply(&run_edits(s, a.key.db, edits).await?)
        }
        "newKey" => {
            let a: RedisNewKeyArgs = args(method, a)?;
            let ttl = a.ttl.as_ref().map(ttl_ms).transpose()?.flatten();
            let edit = cmd::create_key(&key_arg(&a.key)?, &new_key(&a.value)?, ttl)?.edit(0);
            reply(&run_edits(s, a.db, vec![edit]).await?)
        }
        "copyKey" => {
            let a: RedisCopyArgs = args(method, a)?;
            let c = cmd::copy_key(&key_arg(&a.from)?, &key_arg(&a.to)?, &s.gates())?;
            reply(&run_edits(s, a.db, vec![single(c)]).await?)
        }
        "flushDb" => {
            let a: RedisDbArgs = args(method, a)?;
            check_db(s, a.db)?;
            reply(&run_edits(s, a.db, vec![single(cmd::flush_db(&s.gates()))]).await?)
        }
        other => Err(unknown_call(Driver::Redis, other)),
    }
}

fn check_db(s: &RedisSession, db: u32) -> Result<(), AppError> {
    if db < s.databases {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("Redis has no database db{db}")))
    }
}

fn single(c: Command) -> KeyEdit {
    KeyEdit {
        change: 0,
        guard: Guard::None,
        commands: vec![c],
    }
}

async fn run_edits(
    s: &RedisSession,
    db: u32,
    edits: Vec<KeyEdit>,
) -> Result<RedisEditResult, AppError> {
    check_db(s, db)?;
    let outcomes = apply_edits(s, db, &edits).await;
    Ok(RedisEditResult {
        results: outcomes
            .into_iter()
            .map(|o| match o {
                Outcome::Applied => RedisEditOutcome {
                    ok: true,
                    error: None,
                },
                Outcome::Refused(msg) => RedisEditOutcome {
                    ok: false,
                    error: Some(msg),
                },
                Outcome::Lost(e) => RedisEditOutcome {
                    ok: false,
                    error: Some(format!(
                        "{e}: the edit may or may not have been applied; reload before trying again"
                    )),
                },
                Outcome::NotSent(e) => RedisEditOutcome {
                    ok: false,
                    error: Some(format!("not applied: {e}")),
                },
            })
            .collect(),
        preview: preview(&edits).into_iter().map(|(_, t)| t).collect(),
    })
}

/// The edits of one key, each behind a guard: the key still has its type, or the edit's own
/// checks (a rename, a position).
fn key_edits(a: &RedisEditArgs, gates: cmd::Gates) -> Result<Vec<KeyEdit>, AppError> {
    let key = key_arg(&a.key.key)?;
    let ty = a.key.ty.as_str();
    let typed = |commands: Vec<Command>| cmd::Guarded {
        guard: Guard::Type {
            key: key.clone(),
            ty: ty.to_string(),
        },
        commands,
    };
    let v = |value: &Value, what: &str| value_arg(value, what);
    let fits = |want: &str, op: &str| {
        if want == ty {
            Ok(())
        } else {
            Err(AppError::InvalidInput(format!(
                "{op} edits a {want}, not a {ty}"
            )))
        }
    };
    a.edits
        .iter()
        .enumerate()
        .map(|(i, edit)| {
            let g = match edit {
                RedisEdit::StringSet { value } => {
                    fits(cmd::TYPE_STRING, "a string value")?;
                    typed(vec![cmd::set_string(&key, &v(value, "value")?, &gates)])
                }
                RedisEdit::HashSet { field, value } => {
                    fits("hash", "a field")?;
                    typed(vec![cmd::hash_set(
                        &key,
                        &v(field, "field")?,
                        &v(value, "value")?,
                    )])
                }
                RedisEdit::HashAdd { field, value } => {
                    fits("hash", "a field")?;
                    typed(vec![cmd::hash_add(
                        &key,
                        &v(field, "field")?,
                        &v(value, "value")?,
                    )])
                }
                RedisEdit::HashDelete { field } => {
                    fits("hash", "a field")?;
                    typed(vec![cmd::hash_delete(&key, &v(field, "field")?)])
                }
                RedisEdit::HashRename { from, to, value } => {
                    fits("hash", "a field")?;
                    cmd::hash_rename(
                        &key,
                        &v(from, "field")?,
                        &v(to, "field")?,
                        &v(value, "value")?,
                    )?
                }
                RedisEdit::ListSet { index, old, value } => {
                    fits("list", "an element")?;
                    cmd::list_replace(&key, *index, &v(old, "element")?, &v(value, "element")?)
                }
                RedisEdit::ListPush { values, head } => {
                    fits("list", "an element")?;
                    if values.is_empty() {
                        return Err(AppError::InvalidInput("nothing to push".into()));
                    }
                    let values = values
                        .iter()
                        .map(|x| v(x, "element"))
                        .collect::<Result<Vec<_>, _>>()?;
                    typed(vec![cmd::list_push(&key, &values, *head)])
                }
                RedisEdit::ListInsert {
                    before,
                    pivot,
                    value,
                } => {
                    fits("list", "an element")?;
                    typed(vec![cmd::list_insert(
                        &key,
                        *before,
                        &v(pivot, "element")?,
                        &v(value, "element")?,
                    )])
                }
                RedisEdit::ListDelete { index, old } => {
                    fits("list", "an element")?;
                    // A marker no element can hold: removed by value right after.
                    let marker = format!("__tabler_deleted_{}", uuid::Uuid::new_v4());
                    cmd::list_delete_at(&key, *index, &v(old, "element")?, marker.as_bytes())
                }
                RedisEdit::SetAdd { member } => {
                    fits("set", "a member")?;
                    typed(vec![cmd::set_add(&key, &v(member, "member")?)])
                }
                RedisEdit::SetRemove { member } => {
                    fits("set", "a member")?;
                    typed(vec![cmd::set_remove(&key, &v(member, "member")?)])
                }
                RedisEdit::SetRename { from, to } => {
                    fits("set", "a member")?;
                    cmd::set_rename(&key, &v(from, "member")?, &v(to, "member")?)?
                }
                RedisEdit::ZsetAdd { member, score } => {
                    fits("zset", "a member")?;
                    typed(vec![cmd::zset_add(&key, score, &v(member, "member")?)?])
                }
                RedisEdit::ZsetScore { member, score } => {
                    fits("zset", "a member")?;
                    typed(vec![cmd::zset_score(&key, score, &v(member, "member")?)?])
                }
                RedisEdit::ZsetRemove { member } => {
                    fits("zset", "a member")?;
                    typed(vec![cmd::zset_remove(&key, &v(member, "member")?)])
                }
                RedisEdit::StreamAdd { id, fields } => {
                    fits("stream", "an entry")?;
                    typed(vec![cmd::stream_add(
                        &key,
                        id.as_deref(),
                        &field_pairs(fields)?,
                    )?])
                }
                RedisEdit::StreamDelete { id } => {
                    fits("stream", "an entry")?;
                    typed(vec![cmd::stream_delete(&key, id)?])
                }
                RedisEdit::JsonSet { path, doc } => {
                    fits(cmd::TYPE_JSON, "a JSON path")?;
                    typed(vec![cmd::json_set(&key, path, doc)?])
                }
                RedisEdit::JsonDelete { path } => {
                    fits(cmd::TYPE_JSON, "a JSON path")?;
                    typed(vec![cmd::json_delete(&key, path)])
                }
            };
            Ok(g.edit(i))
        })
        .collect()
}

/// Field / value pairs as argv bytes.
type Pairs = Vec<(Vec<u8>, Vec<u8>)>;

fn field_pairs(fields: &[RedisField]) -> Result<Pairs, AppError> {
    fields
        .iter()
        .map(|f| Ok((value_arg(&f.name, "field")?, value_arg(&f.value, "value")?)))
        .collect()
}

fn new_key(v: &RedisNewValue) -> Result<NewKey, AppError> {
    let all = |items: &[Value], what: &str| {
        items
            .iter()
            .map(|x| value_arg(x, what))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match v {
        RedisNewValue::String { value } => NewKey::String(value_arg(value, "value")?),
        RedisNewValue::Hash { fields } => NewKey::Hash(field_pairs(fields)?),
        RedisNewValue::List { values } => NewKey::List(all(values, "element")?),
        RedisNewValue::Set { members } => NewKey::Set(all(members, "member")?),
        RedisNewValue::Zset { members } => NewKey::ZSet(
            members
                .iter()
                .map(|m| Ok((m.score.clone(), value_arg(&m.member, "member")?)))
                .collect::<Result<Vec<_>, AppError>>()?,
        ),
        RedisNewValue::Stream { id, fields } => NewKey::Stream {
            id: id.clone(),
            fields: field_pairs(fields)?,
        },
        RedisNewValue::Json { doc } => NewKey::Json(doc.clone()),
    })
}

async fn server_info(s: &RedisSession) -> Result<RedisServerInfo, AppError> {
    let reply = s
        .read_one(s.default_db, TraceCategory::Structure, &cmd::info(None))
        .await?;
    let info = Info::from_reply(&reply);
    Ok(RedisServerInfo {
        product: s.server.product.clone(),
        version: s.server.version.clone(),
        mode: s.server.mode.clone(),
        protocol: s.protocol(),
        databases: s.databases,
        read_only: s.read_only,
        sections: info
            .sections
            .into_iter()
            .map(|(name, fields)| RedisInfoSection { name, fields })
            .collect(),
    })
}

async fn slow_log(s: &RedisSession, a: &RedisSlowLogArgs) -> Result<Vec<RedisSlowEntry>, AppError> {
    let count = a.count.unwrap_or(128).clamp(1, 10_000);
    let reply = s
        .read_one(s.default_db, TraceCategory::Structure, &cmd::slowlog(count))
        .await?;
    Ok(as_items(&reply)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            // [id, time, duration µs, [argv…], client addr (4.0), client name (4.0)]
            let f = as_items(entry)?;
            let argv: Vec<Vec<u8>> = f
                .get(3)
                .and_then(as_items)
                .unwrap_or_default()
                .iter()
                .filter_map(super::values::as_bytes)
                .collect();
            Some(RedisSlowEntry {
                id: f.first().map_or(Value::Null, int_cell),
                started_at: f.get(1).map_or(Value::Null, int_cell),
                duration_us: f.get(2).map_or(Value::Null, int_cell),
                command: cmd::argv_text(&argv),
                client: f.get(4).and_then(as_text).filter(|t| !t.is_empty()),
                client_name: f.get(5).and_then(as_text).filter(|t| !t.is_empty()),
            })
        })
        .collect())
}

/// A page of a key's elements: a hash or set by `HSCAN` / `SSCAN` cursor, a list or sorted set
/// by position, a stream from the newest entry back (the cursor is the last id read); a string
/// or a RedisJSON document as its value.
async fn key_page(s: &RedisSession, a: &RedisPageArgs) -> Result<RedisKeyPage, AppError> {
    let db = a.key.db;
    check_db(s, db)?;
    let key = key_arg(&a.key.key)?;
    let count = a.count.unwrap_or(PAGE_COUNT).clamp(1, MAX_PAGE_COUNT);
    let ty = KeyType::parse(&a.key.ty);
    let head = s
        .read_many(
            db,
            TraceCategory::Data,
            &[
                cmd::key_type(&key),
                cmd::pttl(&key),
                ty.size_command(&key).unwrap_or_else(|| cmd::exists(&key)),
            ],
        )
        .await?;
    let [found, pttl, size] = head.as_slice() else {
        return Err(AppError::Internal("Redis: replies went missing".into()));
    };
    let found = KeyType::from_reply(found);
    if !found.exists() {
        return Err(AppError::NotFound(
            "the key no longer exists (changed by someone else)".into(),
        ));
    }
    if found != ty {
        return Err(AppError::InvalidInput(cmd::CHANGED.into()));
    }
    let size_n = as_int(size);
    let mut page = RedisKeyPage {
        key: key_value(&key),
        ty: found.name().to_string(),
        ttl: ttl_cell(as_int(pttl).unwrap_or(-1)),
        size: if ty.size_command(&key).is_some() {
            int_cell(size)
        } else {
            Value::Null
        },
        value: None,
        elements: Vec::new(),
        next_cursor: None,
    };
    let at = |token: Option<&str>| -> Result<i64, AppError> {
        cursor(token)?
            .parse::<i64>()
            .map_err(|_| AppError::InvalidInput("the position is out of range".into()))
    };
    let read = |c: Command| async move { s.read_one(db, TraceCategory::Data, &c).await };
    let token = a.cursor.as_deref();
    match &found {
        KeyType::String => {
            let cap = s.limits.bytes.max(1);
            let r = read(cmd::get_range(&key, 0, cap - 1)).await?;
            page.value = super::values::as_bytes(&r)
                .map(|b| string_preview(&b, size_n.and_then(|n| u64::try_from(n).ok()), cap));
        }
        KeyType::Json => {
            let r = read(cmd::json_get(&key, None)).await?;
            page.value = Some(match as_text(&r) {
                Some(text) => Value::Json(text),
                None => cell(&r),
            });
        }
        KeyType::Hash | KeyType::Set => {
            let c = cursor(token)?;
            let scan = if found == KeyType::Hash {
                cmd::hash_scan(&key, &c, count, false)
            } else {
                cmd::set_scan(&key, &c, count)
            };
            let r = parse_scan(&read(scan).await?)?;
            page.next_cursor = next_token(&r.cursor);
            page.elements = if found == KeyType::Hash {
                r.items
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[f, v]| RedisElement {
                        name: element(f),
                        value: Some(element(v)),
                    })
                    .collect()
            } else {
                r.items
                    .iter()
                    .map(|m| RedisElement {
                        name: element(m),
                        value: None,
                    })
                    .collect()
            };
        }
        KeyType::List => {
            let start = at(token)?;
            // A client cursor can be any position: the end saturates instead of overflowing.
            let stop = start.saturating_add(i64::from(count) - 1);
            let r = read(cmd::list_range(&key, start, stop)).await?;
            let items = as_items(&r).unwrap_or_default();
            page.elements = items
                .iter()
                .enumerate()
                .map(|(i, v)| RedisElement {
                    name: Value::from_i64(start.saturating_add(i as i64)),
                    value: Some(element(v)),
                })
                .collect();
            page.next_cursor = more(start, items.len(), size_n);
        }
        KeyType::ZSet => {
            let start = at(token)?;
            // A client cursor can be any position: the end saturates instead of overflowing.
            let stop = start.saturating_add(i64::from(count) - 1);
            let r = read(cmd::zset_range(&key, start, stop)).await?;
            let pairs = as_scored(&r).unwrap_or_default();
            page.elements = pairs
                .iter()
                .map(|(m, score)| RedisElement {
                    name: element(m),
                    value: Some(number_cell(score)),
                })
                .collect();
            page.next_cursor = more(start, pairs.len(), size_n);
        }
        KeyType::Stream => {
            // From the newest entry back; the next page ends just before the last id read.
            let end = match token.map(str::trim).filter(|t| !t.is_empty()) {
                None => "+".to_string(),
                Some(id) => before(id)?,
            };
            let r = read(cmd::stream_range_rev(&key, &end, "-", count)).await?;
            let entries = as_items(&r).unwrap_or_default();
            let mut last = None;
            page.elements = entries
                .iter()
                .filter_map(|e| {
                    let [id, fields] = as_items(e)? else {
                        return None;
                    };
                    let id = as_text(id)?;
                    last = Some(id.clone());
                    let pairs: Vec<(Resp, Resp)> = as_pairs(fields)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Some(RedisElement {
                        name: Value::Text(id),
                        value: Some(Value::Json(json(&Resp::Map(pairs)))),
                    })
                })
                .collect();
            if entries.len() == count as usize {
                page.next_cursor = last.filter(|id| id != "0-0");
            }
        }
        KeyType::Other(_) | KeyType::None => {}
    }
    Ok(page)
}

/// A container element as a cell: text, or bytes (capped) when not UTF-8.
fn element(v: &Resp) -> Value {
    match super::values::as_bytes(v) {
        Some(b) => bytes_value(&b),
        None => cell(v),
    }
}

/// The next position after a page of `read` elements from `start`, if any is left.
fn more(start: i64, read: usize, size: Option<i64>) -> Option<String> {
    // None past the last position (a client cursor can be any).
    let next = start.checked_add(i64::try_from(read).ok()?)?;
    (read > 0 && size.is_none_or(|n| next < n)).then(|| next.to_string())
}

/// The stream id just before `id` (`XREVRANGE` takes its end inclusive; `(` needs 6.2).
fn before(id: &str) -> Result<String, AppError> {
    let bad = || AppError::InvalidInput(format!("\u{201c}{id}\u{201d} is not a stream entry id"));
    let (ms, seq) = match id.split_once('-') {
        Some((ms, seq)) => (
            ms.parse::<u64>().map_err(|_| bad())?,
            seq.parse::<u64>().map_err(|_| bad())?,
        ),
        None => (id.parse::<u64>().map_err(|_| bad())?, 0),
    };
    Ok(match (ms, seq) {
        (0, 0) => "0-0".to_string(),
        (ms, 0) => format!("{}-{}", ms - 1, u64::MAX),
        (ms, seq) => format!("{ms}-{}", seq - 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_writes_unless_known_reads() {
        for read in ["serverInfo", "keyPage", "slowLog", "previewEdits"] {
            assert_eq!(call_kind(read), CallKind::Read, "{read}");
        }
        for write in ["editKey", "newKey", "copyKey", "flushDb", "anything", ""] {
            assert_eq!(call_kind(write), CallKind::Write, "{write}");
        }
    }

    #[test]
    fn stream_ids_step_back() {
        assert_eq!(before("5-3").unwrap(), "5-2");
        assert_eq!(before("5-0").unwrap(), format!("4-{}", u64::MAX));
        assert_eq!(before("7").unwrap(), format!("6-{}", u64::MAX));
        assert_eq!(before("0-0").unwrap(), "0-0");
        assert!(before("x-1").is_err());
        assert_eq!(more(0, 10, Some(25)), Some("10".into()));
        assert_eq!(more(20, 5, Some(25)), None);
        assert_eq!(more(0, 0, None), None);
        // A cursor at the end of the range: no overflow, and no next page past it.
        assert_eq!(more(i64::MAX - 1, 1, None), Some(i64::MAX.to_string()));
        assert_eq!(more(i64::MAX, 1, None), None);
    }

    #[test]
    fn edits_check_the_key_type_and_arguments() {
        let args = |ty: &str, edits: serde_json::Value| -> RedisEditArgs {
            serde_json::from_value(serde_json::json!({
                "db": 0, "key": {"t": "text", "v": "k"}, "type": ty, "edits": edits,
            }))
            .unwrap()
        };
        let text = |s: &str| serde_json::json!({"t": "text", "v": s});
        let edits = key_edits(
            &args(
                "hash",
                serde_json::json!([
                    {"op": "hashSet", "field": text("f"), "value": text("v")},
                    {"op": "hashRename", "from": text("f"), "to": text("g"), "value": text("v")},
                ]),
            ),
            cmd::Gates::default(),
        )
        .unwrap();
        assert!(matches!(&edits[0].guard, Guard::Type { ty, .. } if ty == "hash"));
        assert!(matches!(&edits[1].guard, Guard::Checks { .. }));
        assert_eq!(edits[1].change, 1);
        let wrong = key_edits(
            &args(
                "set",
                serde_json::json!([{"op": "hashDelete", "field": text("f")}]),
            ),
            cmd::Gates::default(),
        )
        .unwrap_err();
        assert!(wrong.to_string().contains("not a set"), "{wrong}");
        // A NULL is no Redis value.
        let null = key_edits(
            &args(
                "list",
                serde_json::json!([{"op": "listPush", "values": [{"t": "null"}], "head": false}]),
            ),
            cmd::Gates::default(),
        );
        assert!(null.is_err());
        let bad = serde_json::from_value::<RedisEditArgs>(serde_json::json!({
            "db": 0, "key": text("k"), "type": "set", "edits": [{"op": "dropEverything"}],
        }));
        assert!(bad.is_err());
    }
}
