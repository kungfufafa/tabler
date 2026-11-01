//! A scripted node for the session's tests: a `Wire` that answers each request with its handler
//! and records what it was sent, a `Connector` that hands it out, and `KvNode`, a keyspace whose
//! table it pages and edits as a server would.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ::scylla::response::{PagingState, PagingStateResponse};
use ::scylla::value::CqlValue;
use async_trait::async_trait;
use uuid::Uuid;

use super::catalog::{self, CellRow};
use super::testing::{b, events_fixture, i, map, t};
use super::types::native_kind;
use super::wire::{Connector, Dial, Page, Reply, Request, RunOptions, Wire};
use crate::error::AppError;
use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// One request the fake received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub cql: String,
    pub params: Vec<Value>,
    /// The paging state's bytes and the page size, for a paged request.
    pub page: Option<(Vec<u8>, i32)>,
    pub options: RunOptions,
}

type Handler = dyn Fn(&Seen) -> Result<Reply, AppError> + Send + Sync;

pub struct FakeWire {
    handler: Box<Handler>,
    pub seen: Mutex<Vec<Seen>>,
    pub uses: Mutex<Vec<(String, bool)>>,
    pub agreements: AtomicUsize,
    /// A request whose text contains this never answers (cancel and time-out tests).
    pub hang_on: Mutex<Option<String>>,
}

impl FakeWire {
    pub fn new(
        handler: impl Fn(&Seen) -> Result<Reply, AppError> + Send + Sync + 'static,
    ) -> Arc<FakeWire> {
        Arc::new(FakeWire {
            handler: Box::new(handler),
            seen: Mutex::new(Vec::new()),
            uses: Mutex::new(Vec::new()),
            agreements: AtomicUsize::new(0),
            hang_on: Mutex::new(None),
        })
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The texts sent, in order.
    pub fn texts(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.cql).collect()
    }

    pub fn clear(&self) {
        self.seen.lock().unwrap().clear();
    }

    pub fn hang_on(&self, text: &str) {
        *self.hang_on.lock().unwrap() = Some(text.to_string());
    }
}

#[async_trait]
impl Wire for FakeWire {
    async fn run(&self, req: Request<'_>) -> Result<Reply, AppError> {
        let seen = Seen {
            cql: req.cql.to_string(),
            params: req.params.to_vec(),
            page: match &req.page {
                Page::Unpaged => None,
                Page::Single { state, size } => Some((
                    state
                        .as_bytes_slice()
                        .map(|b| b.to_vec())
                        .unwrap_or_default(),
                    *size,
                )),
            },
            options: req.options,
        };
        self.seen.lock().unwrap().push(seen.clone());
        let hang = self.hang_on.lock().unwrap().clone();
        if hang.is_some_and(|h| req.cql.contains(&h)) {
            std::future::pending::<()>().await;
        }
        (self.handler)(&seen)
    }

    async fn use_keyspace(&self, keyspace: &str, case_sensitive: bool) -> Result<(), AppError> {
        if keyspace == "missing" {
            return Err(AppError::db("Keyspace 'missing' does not exist"));
        }
        self.uses
            .lock()
            .unwrap()
            .push((keyspace.to_string(), case_sensitive));
        Ok(())
    }

    async fn await_schema_agreement(&self) -> Result<(), AppError> {
        self.agreements.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn tracing(&self, id: Uuid) -> Result<Vec<String>, AppError> {
        Ok(vec![format!("Tracing session {id}: QUERY")])
    }
}

pub struct FakeConnector {
    pub wire: Arc<FakeWire>,
    pub dials: Mutex<Vec<Dial>>,
}

impl FakeConnector {
    pub fn new(wire: Arc<FakeWire>) -> FakeConnector {
        FakeConnector {
            wire,
            dials: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Connector for FakeConnector {
    async fn connect(&self, dial: &Dial) -> Result<Arc<dyn Wire>, AppError> {
        self.dials.lock().unwrap().push(dial.clone());
        if dial
            .credentials
            .as_ref()
            .is_some_and(|c| c.user == "intruder")
        {
            return Err(AppError::Auth(
                "Provided username intruder and/or password are incorrect".into(),
            ));
        }
        Ok(self.wire.clone())
    }
}

/// A result column of a CQL type.
pub fn column(name: &str, ty: &str) -> ResultColumn {
    ResultColumn {
        name: name.into(),
        data_type: ty.into(),
        kind: native_kind(ty),
        nullable: None,
        table: None,
        origin: None,
    }
}

/// A reply with rows.
pub fn rows(columns: &[(&str, &str)], rows: Vec<CellRow>) -> Reply {
    Reply {
        is_rows: true,
        columns: columns.iter().map(|(n, t)| column(n, t)).collect(),
        rows,
        paging: PagingStateResponse::NoMorePages,
        tracing_id: None,
        warnings: Vec::new(),
    }
}

/// A lightweight transaction's answer.
pub fn applied(done: bool) -> Reply {
    rows(&[("[applied]", "boolean")], vec![vec![b(done)]])
}

/// A node with the keyspace `shop`: the table `kv` (`PRIMARY KEY ((id), seq)`, a `val` text),
/// whose rows it pages and edits as the server would, `events` (`events_fixture`), the view
/// `kv_by_val` and the counter table `hits`. It runs Cassandra 4.1 (no `system.versions`, no
/// server DESCRIBE) unless `scylla`.
pub struct KvNode {
    pub rows: Mutex<Vec<(i32, i32, Option<String>)>>,
    pub scylla: bool,
}

const KV_SELECT: &str = "SELECT \"id\", \"seq\", \"val\" FROM \"shop\".\"kv\"";
const KV_FETCH: &str = "SELECT \"val\" FROM \"shop\".\"kv\" WHERE \"id\" = ? AND \"seq\" = ?";
const KV_UPDATE: &str =
    "UPDATE \"shop\".\"kv\" SET \"val\" = ? WHERE \"id\" = ? AND \"seq\" = ? IF EXISTS";
const KV_INSERT: &str =
    "INSERT INTO \"shop\".\"kv\" (\"id\", \"seq\", \"val\") VALUES (?, ?, ?) IF NOT EXISTS";
const KV_DELETE: &str = "DELETE FROM \"shop\".\"kv\" WHERE \"id\" = ? AND \"seq\" = ? IF EXISTS";

fn kv_columns() -> [(&'static str, &'static str); 3] {
    [("id", "int"), ("seq", "int"), ("val", "text")]
}

impl KvNode {
    /// `n` rows: ids 1..=n, `seq` 0, `val` `v<id>`.
    pub fn new(n: i32) -> Arc<KvNode> {
        KvNode::with(n, false)
    }

    pub fn with(n: i32, scylla: bool) -> Arc<KvNode> {
        Arc::new(KvNode {
            rows: Mutex::new((1..=n).map(|i| (i, 0, Some(format!("v{i}")))).collect()),
            scylla,
        })
    }

    pub fn data(&self) -> Vec<(i32, i32, Option<String>)> {
        self.rows.lock().unwrap().clone()
    }

    fn key(params: &[Value]) -> Option<(i32, i32)> {
        let int = |v: &Value| match v {
            Value::Int(n) => i32::try_from(*n).ok(),
            _ => None,
        };
        Some((int(params.first()?)?, int(params.get(1)?)?))
    }

    fn kv_rows(&self) -> Vec<CellRow> {
        self.data()
            .into_iter()
            .map(|(id, seq, val)| {
                vec![i(i64::from(id)), i(i64::from(seq)), val.and_then(|v| t(&v))]
            })
            .collect()
    }

    fn catalog(&self, cql: &str, p: &[Value]) -> Option<Result<Reply, AppError>> {
        let is = |name: &str| {
            p.first() == Some(&Value::Text("shop".into()))
                && p.get(1) == Some(&Value::Text(name.into()))
        };
        let col =
            |name, kind, position, order, ty| vec![t(name), t(kind), i(position), t(order), t(ty)];
        let one = |name: &str, rows_of: Vec<CellRow>| Some(Ok(rows(&[(name, "text")], rows_of)));
        match cql {
            catalog::LOCAL => one(
                "release_version",
                vec![vec![
                    t("4.1.5"),
                    t("3.4.6"),
                    t("5"),
                    t("Test Cluster"),
                    t("dc1"),
                    t("rack1"),
                    t("Murmur3"),
                ]],
            ),
            catalog::SCYLLA_VERSION if self.scylla => {
                one("version", vec![vec![t("5.4.0-0.20231108")]])
            }
            catalog::SCYLLA_VERSION => Some(Err(AppError::db("unconfigured table versions"))),
            catalog::PING => one("key", vec![vec![t("local")]]),
            catalog::KEYSPACES => {
                let ks = |n: &str| vec![t(n), b(true), map(&[("class", "SimpleStrategy")])];
                one(
                    "keyspace_name",
                    vec![ks("system"), ks("shop"), ks("system_auth"), ks("Other")],
                )
            }
            catalog::TABLES => one(
                "table_name",
                vec![
                    vec![t("kv"), t("")],
                    vec![t("events"), t("user events")],
                    vec![t("hits"), t("")],
                ],
            ),
            catalog::VIEWS => one("view_name", vec![vec![t("kv_by_val"), t("kv"), t("")]]),
            catalog::COLUMNS => one(
                "column_name",
                if is("kv") {
                    vec![
                        col("val", "regular", -1, "none", "text"),
                        col("seq", "clustering", 0, "asc", "int"),
                        col("id", "partition_key", 0, "none", "int"),
                    ]
                } else if is("kv_by_val") {
                    vec![
                        col("val", "partition_key", 0, "none", "text"),
                        col("id", "clustering", 0, "asc", "int"),
                        col("seq", "clustering", 1, "asc", "int"),
                    ]
                } else if is("hits") {
                    vec![
                        col("page", "partition_key", 0, "none", "text"),
                        col("n", "regular", -1, "none", "counter"),
                    ]
                } else if is("events") {
                    events_fixture().columns
                } else {
                    Vec::new()
                },
            ),
            catalog::VIEW => one(
                "base_table_name",
                if is("kv_by_val") {
                    vec![vec![
                        t("kv"),
                        t("val IS NOT NULL AND id IS NOT NULL AND seq IS NOT NULL"),
                        b(true),
                        t(""),
                    ]]
                } else {
                    Vec::new()
                },
            ),
            catalog::TABLE_OPTIONS => one(
                "comment",
                if is("events") {
                    events_fixture().options
                } else {
                    Vec::new()
                },
            ),
            catalog::INDEXES => one(
                "index_name",
                if is("events") {
                    events_fixture().indexes
                } else {
                    Vec::new()
                },
            ),
            catalog::TRIGGERS => one("trigger_name", Vec::new()),
            catalog::SIZE_ESTIMATES => Some(Ok(rows(
                &[("partitions_count", "bigint")],
                if is("events") {
                    events_fixture().size_estimates
                } else {
                    vec![vec![i(1000)]]
                },
            ))),
            catalog::KEYSPACE_COLUMNS => one(
                "table_name",
                vec![vec![t("kv"), t("id")], vec![t("kv"), t("val")]],
            ),
            _ => None,
        }
    }

    pub fn answer(&self, s: &Seen) -> Result<Reply, AppError> {
        let cql = s.cql.as_str();
        let p = &s.params;
        if let Some(reply) = self.catalog(cql, p) {
            return reply;
        }
        if cql.starts_with("DESCRIBE") {
            return Err(AppError::db(
                "line 1:0 no viable alternative at input 'DESCRIBE'",
            ));
        }
        if cql.starts_with(KV_SELECT) || cql.trim_end_matches(';') == "SELECT * FROM kv" {
            let mut all = self.kv_rows();
            if cql.contains("WHERE \"val\" = ?") {
                let want = p.first().and_then(Value::as_text);
                all.retain(|r| match r.get(2) {
                    Some(Some(CqlValue::Text(v))) => Some(v) == want.as_ref(),
                    _ => false,
                });
            }
            let Some((state, size)) = &s.page else {
                return Ok(rows(&kv_columns(), all));
            };
            let start = state
                .get(..4)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map_or(0, |b| u32::from_be_bytes(b) as usize);
            let end = (start + usize::try_from(*size).unwrap_or(0)).min(all.len());
            let mut reply = rows(
                &kv_columns(),
                all.get(start..end)
                    .map(<[CellRow]>::to_vec)
                    .unwrap_or_default(),
            );
            if end < all.len() {
                reply.paging = PagingStateResponse::HasMorePages {
                    state: PagingState::new_from_raw_bytes(&(end as u32).to_be_bytes()[..]),
                };
            }
            return Ok(reply);
        }
        if cql.starts_with("SELECT count(*) FROM \"shop\".\"kv\"") {
            let n = self.data().len() as i64;
            return Ok(rows(
                &[("count", "bigint")],
                vec![vec![Some(CqlValue::BigInt(n))]],
            ));
        }
        let key = |params: &[Value]| Self::key(params);
        match cql {
            KV_FETCH => {
                let key = key(p);
                let found: Vec<CellRow> = self
                    .data()
                    .into_iter()
                    .filter(|(id, seq, _)| Some((*id, *seq)) == key)
                    .map(|(_, _, v)| vec![v.and_then(|v| t(&v))])
                    .collect();
                Ok(rows(&[("val", "text")], found))
            }
            KV_UPDATE => {
                let key = key(p.get(1..).unwrap_or_default());
                let mut data = self.rows.lock().unwrap();
                Ok(
                    match data
                        .iter_mut()
                        .find(|(id, seq, _)| Some((*id, *seq)) == key)
                    {
                        Some(row) => {
                            row.2 = p.first().and_then(Value::as_text);
                            applied(true)
                        }
                        None => applied(false),
                    },
                )
            }
            KV_INSERT => {
                let key = key(p);
                let mut data = self.rows.lock().unwrap();
                if data.iter().any(|(id, seq, _)| Some((*id, *seq)) == key) {
                    return Ok(applied(false));
                }
                if let Some((id, seq)) = key {
                    data.push((id, seq, p.get(2).and_then(Value::as_text)));
                }
                Ok(applied(true))
            }
            KV_DELETE => {
                let key = key(p);
                let mut data = self.rows.lock().unwrap();
                let before = data.len();
                data.retain(|(id, seq, _)| Some((*id, *seq)) != key);
                Ok(applied(data.len() < before))
            }
            _ if cql.starts_with("CREATE") || cql.starts_with("DROP") => Ok(Reply::done()),
            _ => Err(AppError::db(format!(
                "line 1:0 no viable alternative ({cql})"
            ))),
        }
    }
}

/// A fake node answering as `node`.
pub fn kv_wire(node: Arc<KvNode>) -> Arc<FakeWire> {
    FakeWire::new(move |s| node.answer(s))
}
