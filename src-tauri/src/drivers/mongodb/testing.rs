//! An in-memory MongoDB for the tests. `FakeMongo` holds databases of collections and answers
//! the commands Tabler sends as a server would: the catalog (`hello`, `buildInfo`,
//! `listDatabases`, `listCollections`, `listIndexes`, `dbStats`), reads (`find` with cursors and
//! `getMore`, `count`, `distinct`, `aggregate` with the stages Tabler and the console use,
//! `explain`), writes (`insert`, `update`, `delete`, `findAndModify` with their `n`), indexes,
//! collections and views, and `$currentOp` / `killOp`. `serve` puts it behind the wire protocol
//! (OP_MSG on a localhost port), so the `mongodb` crate itself connects to it; a delay on a
//! command lets a test cancel it (`killOp`) or time it out (`maxTimeMS`).
//!
//! Queries support equality (also into arrays), `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$in`
//! `$nin` `$exists` `$regex` `$options` `$not` `$type` `$size` `$all` `$elemMatch`, `$and` `$or`
//! `$nor`, dotted paths and `$expr` with `$eq`, `$literal` and `$$ROOT`; batches stop at a byte
//! cap (16 MiB, or lower for a test) as a server's do; updates `$set` `$unset` `$inc` `$mul` `$currentDate` `$push`
//! `$rename` `$setOnInsert`, replacements and `$set` / `$unset` pipelines of literals. Anything
//! else is refused with the server's `BadValue` error, so a test never passes on a shortcut.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ::mongodb::bson::{Bson, DateTime, Document, doc, oid::ObjectId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::literal::int_of;
use super::values::type_name;

/// A server error: its code, code name and message.
#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub code: i32,
    pub name: &'static str,
    pub message: String,
}

impl Fail {
    fn new(code: i32, name: &'static str, message: impl Into<String>) -> Fail {
        Fail {
            code,
            name,
            message: message.into(),
        }
    }

    fn bad(message: impl Into<String>) -> Fail {
        Fail::new(2, "BadValue", message)
    }

    fn reply(&self) -> Document {
        doc! { "ok": 0.0, "errmsg": self.message.clone(), "code": self.code, "codeName": self.name }
    }
}

type R<T> = Result<T, Fail>;

#[derive(Debug, Clone, Default)]
struct Coll {
    docs: Vec<Document>,
    indexes: Vec<Document>,
    options: Document,
    view: bool,
}

impl Coll {
    fn new(options: Document) -> Coll {
        Coll {
            docs: Vec::new(),
            indexes: vec![doc! { "v": 2, "key": { "_id": 1 }, "name": "_id_" }],
            options,
            view: false,
        }
    }
}

struct Cursor {
    ns: String,
    rest: Vec<Document>,
    batch: usize,
    /// The session that opened it (`lsid`); only that session may read on.
    lsid: Option<Bson>,
}

struct Op {
    opid: i32,
    ns: String,
    command: Document,
    killed: bool,
}

#[derive(Default)]
struct State {
    dbs: BTreeMap<String, BTreeMap<String, Coll>>,
    cursors: HashMap<i64, Cursor>,
    next_cursor: i64,
    ops: Vec<Op>,
    next_op: i32,
    log: Vec<(String, Document)>,
    fail_next: HashMap<String, Fail>,
    delays: HashMap<String, Duration>,
    hello_extra: Document,
    connections: i32,
    /// The byte cap of a batch, when a test lowers it from the server's 16 MiB.
    batch_bytes: Option<usize>,
    /// The session (`lsid`) of the command being run.
    lsid: Option<Bson>,
}

/// The most document bytes a server puts in one batch (`BSONObjMaxUserSize`).
const BATCH_BYTES: usize = 16 * 1024 * 1024;

/// The fake server. Clones share the same data.
#[derive(Clone, Default)]
pub struct FakeMongo {
    state: Arc<Mutex<State>>,
}

impl FakeMongo {
    pub fn new() -> FakeMongo {
        FakeMongo::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A test that panicked while holding the lock leaves usable data behind.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds documents to a collection (created when missing), as stored, without checks.
    pub fn seed(&self, db: &str, coll: &str, docs: Vec<Document>) {
        let mut s = self.lock();
        let c = s
            .dbs
            .entry(db.into())
            .or_default()
            .entry(coll.into())
            .or_insert_with(|| Coll::new(Document::new()));
        c.docs.extend(docs);
    }

    /// The documents of a collection, in storage order.
    pub fn docs(&self, db: &str, coll: &str) -> Vec<Document> {
        self.lock()
            .dbs
            .get(db)
            .and_then(|d| d.get(coll))
            .map(|c| c.docs.clone())
            .unwrap_or_default()
    }

    /// Every command received, with its database, without the `$`-fields the driver adds.
    pub fn commands(&self) -> Vec<(String, Document)> {
        self.lock().log.clone()
    }

    /// The commands received with name `name`.
    pub fn commands_named(&self, name: &str) -> Vec<Document> {
        self.commands()
            .into_iter()
            .filter(|(_, c)| c.keys().next().map(String::as_str) == Some(name))
            .map(|(_, c)| c)
            .collect()
    }

    /// The next command named `name` fails with `code`.
    pub fn fail_next(&self, name: &str, code: i32, message: &str) {
        self.lock()
            .fail_next
            .insert(name.into(), Fail::new(code, "TestFailure", message));
    }

    /// Commands named `name` take `d` before they run (served ones only), as a running
    /// operation that `killOp` stops and `maxTimeMS` times out.
    pub fn delay(&self, name: &str, d: Duration) {
        self.lock().delays.insert(name.into(), d);
    }

    /// Fields added to the `hello` reply (`setName` for a replica set, `msg: "isdbgrid"`).
    pub fn hello_with(&self, extra: Document) {
        self.lock().hello_extra = extra;
    }

    /// Caps a batch at `bytes` of documents instead of the server's 16 MiB, so a test cuts a
    /// page without 16 MiB of data. As on a server, a batch always holds one document.
    pub fn batch_bytes(&self, bytes: usize) {
        self.lock().batch_bytes = Some(bytes);
    }

    /// Open cursors (a test checks that pages leave none behind).
    pub fn open_cursors(&self) -> usize {
        self.lock().cursors.len()
    }

    /// Runs one command on `db` and returns the reply (errors as `ok: 0` replies).
    pub fn run(&self, db: &str, command: &Document) -> Document {
        let mut s = self.lock();
        let clean: Document = command
            .iter()
            .filter(|(k, _)| !k.starts_with('$') && k.as_str() != "lsid")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        s.log.push((db.to_string(), clean.clone()));
        s.lsid = command.get("lsid").cloned();
        let name = clean.keys().next().cloned().unwrap_or_default();
        if let Some(f) = s.fail_next.remove(&name) {
            return f.reply();
        }
        match dispatch(&mut s, db, &clean) {
            Ok(mut reply) => {
                if !reply.contains_key("ok") {
                    reply.insert("ok", 1.0);
                }
                reply
            }
            Err(f) => f.reply(),
        }
    }

    /// Serves the wire protocol on a localhost port until the test's runtime ends.
    pub async fn serve(&self) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a localhost port");
        let addr = listener.local_addr().expect("local address");
        let fake = self.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let fake = fake.clone();
                tokio::spawn(async move {
                    let _ = fake.connection(stream).await;
                });
            }
        });
        addr
    }

    /// The URI of a served fake (one node, no TLS, quick failures).
    pub fn uri(addr: SocketAddr) -> String {
        format!(
            "mongodb://{addr}/?directConnection=true&serverSelectionTimeoutMS=3000&connectTimeoutMS=3000"
        )
    }

    async fn connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let connection_id = {
            let mut s = self.lock();
            s.connections += 1;
            s.connections
        };
        loop {
            let mut header = [0u8; 16];
            stream.read_exact(&mut header).await?;
            let len = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
            let request_id = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            let op_code = i32::from_le_bytes([header[12], header[13], header[14], header[15]]);
            let body_len = usize::try_from(len).unwrap_or(0).saturating_sub(16);
            if body_len > 48_000_000 {
                return Ok(());
            }
            let mut body = vec![0u8; body_len];
            stream.read_exact(&mut body).await?;
            if op_code != OP_MSG {
                // Only OP_MSG is spoken (the crate's 3.x never sends anything else).
                return Ok(());
            }
            let Ok((flags, command)) = decode_msg(&body) else {
                return Ok(());
            };
            let db = command.get_str("$db").unwrap_or("admin").to_string();
            let mut reply = match self.wait_delay(&db, &command).await {
                Some(f) => f.reply(),
                None => self.run(&db, &command),
            };
            if is_hello(&command) {
                reply.insert("connectionId", connection_id);
            }
            // moreToCome from the client: an unacknowledged write, no reply.
            if flags & 2 != 0 {
                continue;
            }
            stream.write_all(&encode_msg(request_id, &reply)).await?;
        }
    }

    /// The delay of a command, as an operation `$currentOp` lists until it ends, `killOp`
    /// stops it, or its `maxTimeMS` runs out.
    async fn wait_delay(&self, db: &str, command: &Document) -> Option<Fail> {
        let name = command.keys().next()?.clone();
        let delay = *self.lock().delays.get(&name)?;
        let opid = {
            let mut s = self.lock();
            s.next_op += 1;
            let opid = s.next_op;
            let coll = command.get_str(&name).unwrap_or_default();
            let command: Document = command
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && k.as_str() != "lsid")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            s.ops.push(Op {
                opid,
                ns: format!("{db}.{coll}"),
                command,
                killed: false,
            });
            opid
        };
        let max_time = command
            .get("maxTimeMS")
            .and_then(int_of)
            .filter(|ms| *ms > 0)
            .map(|ms| Duration::from_millis(ms as u64));
        let started = Instant::now();
        let outcome = loop {
            let killed = self.lock().ops.iter().any(|o| o.opid == opid && o.killed);
            if killed {
                break Some(Fail::new(11601, "Interrupted", "operation was interrupted"));
            }
            if max_time.is_some_and(|m| started.elapsed() >= m) {
                break Some(Fail::new(
                    50,
                    "MaxTimeMSExpired",
                    "operation exceeded time limit",
                ));
            }
            if started.elapsed() >= delay {
                break None;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        self.lock().ops.retain(|o| o.opid != opid);
        outcome
    }
}

// ---------------------------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------------------------

const OP_MSG: i32 = 2013;

fn is_hello(command: &Document) -> bool {
    matches!(
        command.keys().next().map(String::as_str),
        Some("hello" | "isMaster" | "ismaster")
    )
}

/// The body of an OP_MSG: its flags and its command, with the document sequences (kind 1
/// sections, which the driver uses for `documents`, `updates`, `deletes`) as arrays.
pub fn decode_msg(body: &[u8]) -> Result<(u32, Document), String> {
    let flags = u32::from_le_bytes(
        body.get(0..4)
            .ok_or("short message")?
            .try_into()
            .map_err(|_| "short message")?,
    );
    let end = if flags & 1 != 0 {
        body.len().saturating_sub(4)
    } else {
        body.len()
    };
    let mut at = 4;
    let mut command: Option<Document> = None;
    let mut sequences: Vec<(String, Vec<Bson>)> = Vec::new();
    while at < end {
        let kind = body[at];
        at += 1;
        match kind {
            0 => {
                let len = doc_len(body, at)?;
                let d =
                    Document::from_reader(&mut &body[at..at + len]).map_err(|e| e.to_string())?;
                command = Some(d);
                at += len;
            }
            1 => {
                let size = doc_len(body, at)?;
                let section_end = at + size;
                let mut p = at + 4;
                let name_end = body[p..section_end]
                    .iter()
                    .position(|b| *b == 0)
                    .ok_or("unterminated identifier")?;
                let name = String::from_utf8_lossy(&body[p..p + name_end]).into_owned();
                p += name_end + 1;
                let mut docs = Vec::new();
                while p < section_end {
                    let len = doc_len(body, p)?;
                    let d =
                        Document::from_reader(&mut &body[p..p + len]).map_err(|e| e.to_string())?;
                    docs.push(Bson::Document(d));
                    p += len;
                }
                sequences.push((name, docs));
                at = section_end;
            }
            k => return Err(format!("unknown section kind {k}")),
        }
    }
    let mut command = command.ok_or("no body section")?;
    for (name, docs) in sequences {
        command.insert(name, docs);
    }
    Ok((flags, command))
}

fn doc_len(body: &[u8], at: usize) -> Result<usize, String> {
    let bytes: [u8; 4] = body
        .get(at..at + 4)
        .ok_or("short section")?
        .try_into()
        .map_err(|_| "short section")?;
    let len = usize::try_from(i32::from_le_bytes(bytes)).map_err(|_| "negative length")?;
    if len < 5 || at + len > body.len() {
        return Err("section past the message".into());
    }
    Ok(len)
}

/// A reply: OP_MSG with one body section.
pub fn encode_msg(response_to: i32, reply: &Document) -> Vec<u8> {
    let mut doc_bytes = Vec::new();
    reply
        .to_writer(&mut doc_bytes)
        .expect("a reply document serializes");
    let len = 16 + 4 + 1 + doc_bytes.len();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&(len as i32).to_le_bytes());
    out.extend_from_slice(&response_to.wrapping_add(1_000_000).to_le_bytes());
    out.extend_from_slice(&response_to.to_le_bytes());
    out.extend_from_slice(&OP_MSG.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0);
    out.extend_from_slice(&doc_bytes);
    out
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

fn dispatch(s: &mut State, db: &str, c: &Document) -> R<Document> {
    let name = c.keys().next().map(String::as_str).unwrap_or_default();
    match name {
        "hello" | "isMaster" | "ismaster" => {
            let mut d = doc! {
                "helloOk": true,
                "isWritablePrimary": true,
                "ismaster": true,
                "maxBsonObjectSize": 16_777_216,
                "maxMessageSizeBytes": 48_000_000,
                "maxWriteBatchSize": 100_000,
                "localTime": DateTime::now(),
                "minWireVersion": 0,
                "maxWireVersion": 21,
                "readOnly": false,
                // Sessions: the driver sends an `lsid`, and a cursor reads on only in its own.
                "logicalSessionTimeoutMinutes": 30,
            };
            d.extend(s.hello_extra.clone());
            Ok(d)
        }
        "buildInfo" | "buildinfo" => {
            Ok(doc! { "version": "8.0.0-fake", "versionArray": [8, 0, 0, 0] })
        }
        "ping" | "endSessions" => Ok(Document::new()),
        "connectionStatus" => {
            Ok(doc! { "authInfo": { "authenticatedUsers": [], "authenticatedUserRoles": [] } })
        }
        "serverStatus" => Ok(doc! { "host": "fake", "version": "8.0.0-fake", "uptime": 1.0 }),
        "usersInfo" => Ok(doc! { "users": [] }),
        "rolesInfo" => Ok(doc! { "roles": [] }),
        "listDatabases" => {
            let name_only = c.get_bool("nameOnly").unwrap_or(false);
            let dbs: Vec<Bson> = s
                .dbs
                .iter()
                .map(|(name, colls)| {
                    if name_only {
                        Bson::Document(doc! { "name": name })
                    } else {
                        let size: i64 = colls.values().map(|c| data_size(&c.docs)).sum();
                        Bson::Document(
                            doc! { "name": name, "sizeOnDisk": size, "empty": colls.is_empty() },
                        )
                    }
                })
                .collect();
            Ok(doc! { "databases": dbs })
        }
        "dbStats" => {
            let colls = s.dbs.get(db).cloned().unwrap_or_default();
            let objects: i64 = colls.values().map(|c| c.docs.len() as i64).sum();
            let size: i64 = colls.values().map(|c| data_size(&c.docs)).sum();
            Ok(
                doc! { "db": db, "collections": colls.len() as i64, "objects": objects, "dataSize": size },
            )
        }
        "listCollections" => {
            let filter = c.get_document("filter").cloned().unwrap_or_default();
            let name_only = c.get_bool("nameOnly").unwrap_or(false);
            let mut entries = Vec::new();
            for (name, coll) in s.dbs.get(db).into_iter().flatten() {
                let kind = if coll.view { "view" } else { "collection" };
                let entry = if name_only {
                    doc! { "name": name, "type": kind }
                } else {
                    doc! {
                        "name": name,
                        "type": kind,
                        "options": coll.options.clone(),
                        "info": { "readOnly": coll.view },
                    }
                };
                if matches(&entry, &filter)? {
                    entries.push(entry);
                }
            }
            Ok(cursor_reply(
                s,
                &format!("{db}.$cmd.listCollections"),
                entries,
                c,
            ))
        }
        "listIndexes" => {
            let coll = target(c, name)?;
            let indexes = get_coll(s, db, &coll)?.indexes.clone();
            Ok(cursor_reply(s, &format!("{db}.{coll}"), indexes, c))
        }
        "create" => {
            let coll = target(c, name)?;
            if s.dbs.get(db).is_some_and(|d| d.contains_key(&coll)) {
                return Err(Fail::new(
                    48,
                    "NamespaceExists",
                    format!("Collection {db}.{coll} already exists."),
                ));
            }
            let mut options: Document = c
                .iter()
                .skip(1)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            options.remove("writeConcern");
            let view = options.contains_key("viewOn");
            let mut new = Coll::new(options);
            if view {
                new.indexes.clear();
                new.view = true;
            }
            s.dbs.entry(db.into()).or_default().insert(coll, new);
            Ok(Document::new())
        }
        "drop" => {
            let coll = target(c, name)?;
            match s.dbs.get_mut(db).and_then(|d| d.remove(&coll)) {
                Some(_) => Ok(doc! { "ns": format!("{db}.{coll}"), "nIndexesWas": 1 }),
                None => Err(Fail::new(26, "NamespaceNotFound", "ns not found")),
            }
        }
        "dropDatabase" => {
            s.dbs.remove(db);
            Ok(Document::new())
        }
        "renameCollection" => {
            let from = c
                .get_str("renameCollection")
                .map_err(|_| Fail::bad("renameCollection takes a namespace"))?;
            let to = c
                .get_str("to")
                .map_err(|_| Fail::bad("to takes a namespace"))?;
            let (fdb, fcoll) = split_ns(from)?;
            let (tdb, tcoll) = split_ns(to)?;
            let drop_target = c.get_bool("dropTarget").unwrap_or(false);
            if s.dbs.get(tdb).is_some_and(|d| d.contains_key(tcoll)) && !drop_target {
                return Err(Fail::new(48, "NamespaceExists", "target namespace exists"));
            }
            let moved = s
                .dbs
                .get_mut(fdb)
                .and_then(|d| d.remove(fcoll))
                .ok_or_else(|| {
                    Fail::new(26, "NamespaceNotFound", "source namespace does not exist")
                })?;
            s.dbs
                .entry(tdb.into())
                .or_default()
                .insert(tcoll.into(), moved);
            Ok(Document::new())
        }
        "createIndexes" => {
            let coll = target(c, name)?;
            let specs = c
                .get_array("indexes")
                .map_err(|_| Fail::bad("indexes takes an array"))?
                .clone();
            let entry = s
                .dbs
                .entry(db.into())
                .or_default()
                .entry(coll)
                .or_insert_with(|| Coll::new(Document::new()));
            let before = entry.indexes.len() as i32;
            for spec in specs {
                let Bson::Document(spec) = spec else {
                    return Err(Fail::bad("index specs are documents"));
                };
                let name = spec
                    .get_str("name")
                    .map_err(|_| Fail::bad("an index needs a name"))?
                    .to_string();
                if !entry
                    .indexes
                    .iter()
                    .any(|i| i.get_str("name") == Ok(name.as_str()))
                {
                    let mut stored = doc! { "v": 2 };
                    stored.extend(spec);
                    entry.indexes.push(stored);
                }
            }
            Ok(doc! { "numIndexesBefore": before, "numIndexesAfter": entry.indexes.len() as i32 })
        }
        "dropIndexes" => {
            let coll = target(c, name)?;
            let entry = get_coll_mut(s, db, &coll)?;
            match c.get("index") {
                Some(Bson::String(n)) if n == "*" => {
                    entry.indexes.retain(|i| i.get_str("name") == Ok("_id_"))
                }
                Some(Bson::String(n)) => {
                    let before = entry.indexes.len();
                    entry
                        .indexes
                        .retain(|i| i.get_str("name") != Ok(n.as_str()) || n == "_id_");
                    if before == entry.indexes.len() {
                        return Err(Fail::new(
                            27,
                            "IndexNotFound",
                            format!("index not found with name [{n}]"),
                        ));
                    }
                }
                Some(Bson::Document(keys)) => {
                    entry.indexes.retain(|i| i.get_document("key") != Ok(keys))
                }
                _ => return Err(Fail::bad("index takes a name or a key pattern")),
            }
            Ok(Document::new())
        }
        "collMod" => {
            let coll = target(c, name)?;
            let entry = get_coll_mut(s, db, &coll)?;
            for (k, v) in c.iter().skip(1) {
                match k.as_str() {
                    "validator" | "validationLevel" | "validationAction" => {
                        entry.options.insert(k.clone(), v.clone());
                    }
                    "index" => {
                        let Bson::Document(spec) = v else {
                            return Err(Fail::bad("index takes a document"));
                        };
                        let hidden = spec.get_bool("hidden").unwrap_or(false);
                        let idx = entry
                            .indexes
                            .iter_mut()
                            .find(|i| {
                                spec.get_str("name")
                                    .is_ok_and(|n| i.get_str("name") == Ok(n))
                                    || spec
                                        .get_document("keyPattern")
                                        .is_ok_and(|k| i.get_document("key") == Ok(k))
                            })
                            .ok_or_else(|| Fail::new(27, "IndexNotFound", "cannot find index"))?;
                        idx.insert("hidden", hidden);
                    }
                    _ => {}
                }
            }
            Ok(Document::new())
        }
        "find" => {
            let coll = target(c, name)?;
            let mut docs = source(s, db, &coll)?;
            let filter = c.get_document("filter").cloned().unwrap_or_default();
            docs = filter_docs(docs, &filter)?;
            if let Ok(sort) = c.get_document("sort") {
                sort_docs(&mut docs, sort);
            }
            let skip = c.get("skip").and_then(int_of).unwrap_or(0).max(0) as usize;
            let limit = c.get("limit").and_then(int_of).unwrap_or(0).unsigned_abs() as usize;
            docs = docs.into_iter().skip(skip).collect();
            if limit > 0 {
                docs.truncate(limit);
            }
            if let Ok(p) = c.get_document("projection") {
                docs = docs.iter().map(|d| project(d, p)).collect::<R<_>>()?;
            }
            Ok(cursor_reply(s, &format!("{db}.{coll}"), docs, c))
        }
        "getMore" => {
            let id = c
                .get("getMore")
                .and_then(int_of)
                .ok_or_else(|| Fail::bad("getMore takes a cursor id"))?;
            let cap = s.batch_bytes.unwrap_or(BATCH_BYTES);
            let lsid = s.lsid.clone();
            let cursor = s.cursors.get_mut(&id).ok_or_else(|| {
                Fail::new(43, "CursorNotFound", format!("cursor id {id} not found"))
            })?;
            if cursor.lsid != lsid {
                return Err(Fail::new(
                    50737,
                    "Location50737",
                    format!(
                        "Cannot run getMore on cursor {id}, which was created in another session"
                    ),
                ));
            }
            let batch = c
                .get("batchSize")
                .and_then(int_of)
                .filter(|n| *n > 0)
                .map_or(cursor.batch, |n| n as usize);
            let take = batch_len(&cursor.rest, batch, cap);
            let next: Vec<Bson> = cursor.rest.drain(..take).map(Bson::Document).collect();
            let ns = cursor.ns.clone();
            let done = cursor.rest.is_empty();
            if done {
                s.cursors.remove(&id);
            }
            Ok(doc! { "cursor": { "id": if done { 0 } else { id }, "ns": ns, "nextBatch": next } })
        }
        "killCursors" => {
            let ids = c.get_array("cursors").cloned().unwrap_or_default();
            let mut killed = Vec::new();
            for id in ids.iter().filter_map(int_of) {
                if s.cursors.remove(&id).is_some() {
                    killed.push(Bson::Int64(id));
                }
            }
            Ok(doc! { "cursorsKilled": killed })
        }
        "count" => {
            let coll = target(c, name)?;
            let docs = source(s, db, &coll)?;
            let query = c.get_document("query").cloned().unwrap_or_default();
            let mut n = filter_docs(docs, &query)?.len() as i64;
            let skip = c.get("skip").and_then(int_of).unwrap_or(0).max(0);
            n = (n - skip).max(0);
            if let Some(limit) = c.get("limit").and_then(int_of).filter(|l| *l != 0) {
                n = n.min(limit.abs());
            }
            Ok(doc! { "n": n })
        }
        "distinct" => {
            let coll = target(c, name)?;
            let key = c
                .get_str("key")
                .map_err(|_| Fail::bad("distinct takes a key"))?;
            let query = c.get_document("query").cloned().unwrap_or_default();
            let mut values: Vec<Bson> = Vec::new();
            for d in filter_docs(source(s, db, &coll)?, &query)? {
                for v in lookup(&d, key) {
                    let items = match v {
                        Bson::Array(a) => a,
                        v => vec![v],
                    };
                    for i in items {
                        if !values.iter().any(|x| equal(x, &i)) {
                            values.push(i);
                        }
                    }
                }
            }
            values.sort_by(compare);
            Ok(doc! { "values": values })
        }
        "aggregate" => {
            let pipeline = c
                .get_array("pipeline")
                .map_err(|_| Fail::bad("pipeline takes an array"))?
                .clone();
            let (ns, docs) = match c.get("aggregate") {
                Some(Bson::String(coll)) => {
                    let first_stage = pipeline
                        .first()
                        .and_then(|b| b.as_document())
                        .and_then(|d| d.keys().next().cloned());
                    let docs = if first_stage.as_deref() == Some("$collStats") {
                        Vec::new()
                    } else {
                        source(s, db, coll)?
                    };
                    (format!("{db}.{coll}"), docs)
                }
                _ => (format!("{db}.$cmd.aggregate"), Vec::new()),
            };
            let target_coll = c.get_str("aggregate").ok().map(str::to_string);
            let out = run_pipeline(s, db, target_coll.as_deref(), docs, &pipeline)?;
            if c.get_bool("explain") == Ok(true) {
                return Ok(doc! { "stages": pipeline, "explainVersion": "1" });
            }
            Ok(cursor_reply(s, &ns, out, c))
        }
        "explain" => {
            let inner = c
                .get_document("explain")
                .map_err(|_| Fail::bad("explain takes a command"))?;
            let ns = format!(
                "{db}.{}",
                inner.values().next().and_then(|v| v.as_str()).unwrap_or("")
            );
            Ok(doc! {
                "queryPlanner": { "namespace": ns, "winningPlan": { "stage": "COLLSCAN" } },
                "command": inner.clone(),
                "explainVersion": "1",
            })
        }
        "insert" => {
            let coll = target(c, name)?;
            let ordered = c.get_bool("ordered").unwrap_or(true);
            let docs = c
                .get_array("documents")
                .map_err(|_| Fail::bad("documents takes an array"))?
                .clone();
            let entry = writable(s, db, &coll)?;
            let mut n = 0;
            let mut errors = Vec::new();
            for (i, d) in docs.into_iter().enumerate() {
                let Bson::Document(d) = d else {
                    return Err(Fail::bad("documents are documents"));
                };
                let (d, _) = crate::sqlgen::native::mongo::with_id(d);
                match check_unique(entry, &d, None) {
                    Ok(()) => {
                        entry.docs.push(d);
                        n += 1;
                    }
                    Err(f) => {
                        errors.push(write_error(i, &f));
                        if ordered {
                            break;
                        }
                    }
                }
            }
            Ok(with_write_errors(doc! { "n": n }, errors))
        }
        "update" => {
            let coll = target(c, name)?;
            let ordered = c.get_bool("ordered").unwrap_or(true);
            let statements = c
                .get_array("updates")
                .map_err(|_| Fail::bad("updates takes an array"))?
                .clone();
            let entry = writable(s, db, &coll)?;
            let (mut n, mut modified) = (0_i64, 0_i64);
            let mut upserted = Vec::new();
            let mut errors = Vec::new();
            for (i, st) in statements.into_iter().enumerate() {
                let Bson::Document(st) = st else {
                    return Err(Fail::bad("updates are documents"));
                };
                match update_one_statement(entry, &st) {
                    Ok((matched, changed, up)) => {
                        n += matched;
                        modified += changed;
                        if let Some(id) = up {
                            n += 1;
                            upserted.push(Bson::Document(doc! { "index": i as i32, "_id": id }));
                        }
                    }
                    Err(f) => {
                        errors.push(write_error(i, &f));
                        if ordered {
                            break;
                        }
                    }
                }
            }
            let mut reply = doc! { "n": n, "nModified": modified };
            if !upserted.is_empty() {
                reply.insert("upserted", upserted);
            }
            Ok(with_write_errors(reply, errors))
        }
        "delete" => {
            let coll = target(c, name)?;
            let statements = c
                .get_array("deletes")
                .map_err(|_| Fail::bad("deletes takes an array"))?
                .clone();
            let entry = writable(s, db, &coll)?;
            let mut n = 0_i64;
            for st in statements {
                let Bson::Document(st) = st else {
                    return Err(Fail::bad("deletes are documents"));
                };
                let q = st
                    .get_document("q")
                    .map_err(|_| Fail::bad("q takes a filter"))?;
                let limit = st.get("limit").and_then(int_of).unwrap_or(0);
                let mut removed = 0;
                let mut keep = Vec::with_capacity(entry.docs.len());
                for d in std::mem::take(&mut entry.docs) {
                    if (limit == 0 || removed < limit) && matches(&d, q)? {
                        removed += 1;
                    } else {
                        keep.push(d);
                    }
                }
                entry.docs = keep;
                n += removed;
            }
            Ok(doc! { "n": n })
        }
        "findAndModify" => {
            let coll = target(c, name)?;
            let entry = writable(s, db, &coll)?;
            let q = c.get_document("query").cloned().unwrap_or_default();
            let mut positions: Vec<usize> = Vec::new();
            for (i, d) in entry.docs.iter().enumerate() {
                if matches(d, &q)? {
                    positions.push(i);
                }
            }
            if let Ok(sort) = c.get_document("sort") {
                positions.sort_by(|a, b| sort_cmp(&entry.docs[*a], &entry.docs[*b], sort));
            }
            let fields = c.get_document("fields").ok();
            let shown = |d: &Document| -> R<Bson> {
                Ok(Bson::Document(match fields {
                    Some(p) => project(d, p)?,
                    None => d.clone(),
                }))
            };
            if c.get_bool("remove") == Ok(true) {
                return Ok(match positions.first() {
                    Some(&i) => {
                        let d = entry.docs.remove(i);
                        doc! { "lastErrorObject": { "n": 1 }, "value": shown(&d)? }
                    }
                    None => doc! { "lastErrorObject": { "n": 0 }, "value": Bson::Null },
                });
            }
            let u = c
                .get("update")
                .cloned()
                .ok_or_else(|| Fail::bad("findAndModify needs update or remove"))?;
            let new = c.get_bool("new").unwrap_or(false);
            match positions.first() {
                Some(&i) => {
                    let before = entry.docs[i].clone();
                    let mut after = before.clone();
                    apply_update(&mut after, &u, false)?;
                    check_unique(entry, &after, Some(i))?;
                    entry.docs[i] = after.clone();
                    let value = if new { shown(&after)? } else { shown(&before)? };
                    Ok(
                        doc! { "lastErrorObject": { "n": 1, "updatedExisting": true }, "value": value },
                    )
                }
                None if c.get_bool("upsert") == Ok(true) => {
                    let d = upsert_document(&q, &u)?;
                    check_unique(entry, &d, None)?;
                    entry.docs.push(d.clone());
                    let value = if new { shown(&d)? } else { Bson::Null };
                    Ok(
                        doc! { "lastErrorObject": { "n": 1, "updatedExisting": false, "upserted": d.get("_id").cloned().unwrap_or(Bson::Null) }, "value": value },
                    )
                }
                None => Ok(
                    doc! { "lastErrorObject": { "n": 0, "updatedExisting": false }, "value": Bson::Null },
                ),
            }
        }
        "killOp" => {
            let op = c
                .get("op")
                .and_then(int_of)
                .ok_or_else(|| Fail::bad("killOp takes an op"))?;
            for o in s.ops.iter_mut().filter(|o| i64::from(o.opid) == op) {
                o.killed = true;
            }
            Ok(doc! { "info": "attempting to kill op" })
        }
        "currentOp" => Ok(doc! { "inprog": current_ops(s) }),
        _ => Err(Fail::new(
            59,
            "CommandNotFound",
            format!("no such command: '{name}'"),
        )),
    }
}

fn data_size(docs: &[Document]) -> i64 {
    docs.iter().map(|d| doc_size(d) as i64).sum()
}

/// A document's size as BSON.
fn doc_size(d: &Document) -> usize {
    let mut v = Vec::new();
    d.to_writer(&mut v).map(|_| v.len()).unwrap_or(0)
}

fn current_ops(s: &State) -> Vec<Bson> {
    s.ops
        .iter()
        .map(|o| Bson::Document(doc! { "opid": o.opid, "active": true, "op": "command", "ns": o.ns.clone(), "command": o.command.clone() }))
        .collect()
}

fn target(c: &Document, name: &str) -> R<String> {
    match c.get(name) {
        Some(Bson::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => Err(Fail::new(
            73,
            "InvalidNamespace",
            format!("{name} takes a collection name"),
        )),
    }
}

fn split_ns(ns: &str) -> R<(&str, &str)> {
    ns.split_once('.')
        .ok_or_else(|| Fail::new(73, "InvalidNamespace", format!("invalid namespace {ns}")))
}

fn get_coll<'a>(s: &'a State, db: &str, coll: &str) -> R<&'a Coll> {
    s.dbs.get(db).and_then(|d| d.get(coll)).ok_or_else(|| {
        Fail::new(
            26,
            "NamespaceNotFound",
            format!("ns does not exist: {db}.{coll}"),
        )
    })
}

fn get_coll_mut<'a>(s: &'a mut State, db: &str, coll: &str) -> R<&'a mut Coll> {
    s.dbs
        .get_mut(db)
        .and_then(|d| d.get_mut(coll))
        .ok_or_else(|| {
            Fail::new(
                26,
                "NamespaceNotFound",
                format!("ns does not exist: {db}.{coll}"),
            )
        })
}

/// A collection a write may change: created on first use, never a view.
fn writable<'a>(s: &'a mut State, db: &str, coll: &str) -> R<&'a mut Coll> {
    let entry = s
        .dbs
        .entry(db.into())
        .or_default()
        .entry(coll.into())
        .or_insert_with(|| Coll::new(Document::new()));
    if entry.view {
        return Err(Fail::new(
            166,
            "CommandNotSupportedOnView",
            format!("Namespace {db}.{coll} is a view, not a collection"),
        ));
    }
    Ok(entry)
}

/// The documents a read sees: a collection's, or a view's pipeline over its source. A missing
/// collection reads as empty, as on a server.
fn source(s: &mut State, db: &str, coll: &str) -> R<Vec<Document>> {
    let Some(c) = s.dbs.get(db).and_then(|d| d.get(coll)) else {
        return Ok(Vec::new());
    };
    if !c.view {
        return Ok(c.docs.clone());
    }
    let on = c.options.get_str("viewOn").unwrap_or_default().to_string();
    let pipeline = c.options.get_array("pipeline").cloned().unwrap_or_default();
    let docs = source(s, db, &on)?;
    run_pipeline(s, db, Some(&on), docs, &pipeline)
}

/// A cursor reply: the first batch (`batchSize` documents, all of them without one, and never
/// more than the byte cap), and an open cursor for the rest. `singleBatch` closes the cursor
/// after the first batch, whatever it left out, as a server does.
fn cursor_reply(s: &mut State, ns: &str, mut docs: Vec<Document>, c: &Document) -> Document {
    let batch = c
        .get("batchSize")
        .or_else(|| {
            c.get_document("cursor")
                .ok()
                .and_then(|cur| cur.get("batchSize"))
        })
        .and_then(int_of)
        .filter(|n| *n > 0)
        .map_or(usize::MAX, |n| n as usize);
    let single = c.get_bool("singleBatch").unwrap_or(false);
    let first_len = batch_len(&docs, batch, s.batch_bytes.unwrap_or(BATCH_BYTES));
    let rest = docs.split_off(first_len);
    let id = if rest.is_empty() || single {
        0
    } else {
        s.next_cursor += 1;
        let id = 1000 + s.next_cursor;
        s.cursors.insert(
            id,
            Cursor {
                ns: ns.to_string(),
                rest,
                batch,
                lsid: s.lsid.clone(),
            },
        );
        id
    };
    let first: Vec<Bson> = docs.into_iter().map(Bson::Document).collect();
    doc! { "cursor": { "id": id, "ns": ns, "firstBatch": first } }
}

/// How many of `docs` the next batch holds: at most `count`, and no more than `cap` bytes of
/// documents, but always one (a server's `FindCommon::haveSpaceForNext`).
fn batch_len(docs: &[Document], count: usize, cap: usize) -> usize {
    let mut bytes = 0;
    for (i, d) in docs.iter().take(count).enumerate() {
        let size = doc_size(d);
        if i > 0 && bytes + size > cap {
            return i;
        }
        bytes += size;
    }
    docs.len().min(count)
}

fn write_error(index: usize, f: &Fail) -> Bson {
    Bson::Document(
        doc! { "index": index as i32, "code": f.code, "codeName": f.name, "errmsg": f.message.clone() },
    )
}

fn with_write_errors(mut reply: Document, errors: Vec<Bson>) -> Document {
    if !errors.is_empty() {
        reply.insert("writeErrors", errors);
    }
    reply
}

/// `_id` and unique indexes; `skip` is the document being replaced.
fn check_unique(c: &Coll, d: &Document, skip: Option<usize>) -> R<()> {
    for index in &c.indexes {
        let unique = index.get_str("name") == Ok("_id_") || index.get_bool("unique") == Ok(true);
        let Ok(keys) = index.get_document("key") else {
            continue;
        };
        if !unique {
            continue;
        }
        let key_of = |doc: &Document| -> Vec<Bson> {
            keys.keys()
                .map(|k| lookup(doc, k).into_iter().next().unwrap_or(Bson::Null))
                .collect()
        };
        let mine = key_of(d);
        let dup = c
            .docs
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != skip)
            .any(|(_, other)| {
                let theirs = key_of(other);
                mine.len() == theirs.len() && mine.iter().zip(&theirs).all(|(a, b)| equal(a, b))
            });
        if dup {
            let name = index.get_str("name").unwrap_or_default();
            return Err(Fail::new(
                11000,
                "DuplicateKey",
                format!("E11000 duplicate key error collection index: {name}"),
            ));
        }
    }
    Ok(())
}

/// One statement of an `update`: (matched, modified, the upserted `_id`).
fn update_one_statement(c: &mut Coll, st: &Document) -> R<(i64, i64, Option<Bson>)> {
    let q = st
        .get_document("q")
        .map_err(|_| Fail::bad("q takes a filter"))?;
    let u = st.get("u").ok_or_else(|| Fail::bad("u is required"))?;
    let multi = st.get_bool("multi").unwrap_or(false);
    let upsert = st.get_bool("upsert").unwrap_or(false);
    if multi && matches!(u, Bson::Document(d) if !d.keys().any(|k| k.starts_with('$'))) {
        return Err(Fail::new(
            9,
            "FailedToParse",
            "multi update is not supported for replacement-style update",
        ));
    }
    let mut matched = 0;
    let mut modified = 0;
    for i in 0..c.docs.len() {
        if !matches(&c.docs[i], q)? {
            continue;
        }
        matched += 1;
        let mut after = c.docs[i].clone();
        apply_update(&mut after, u, false)?;
        if after != c.docs[i] {
            check_unique(c, &after, Some(i))?;
            c.docs[i] = after;
            modified += 1;
        }
        if !multi {
            break;
        }
    }
    if matched == 0 && upsert {
        let d = upsert_document(q, u)?;
        check_unique(c, &d, None)?;
        let id = d.get("_id").cloned();
        c.docs.push(d);
        return Ok((0, 0, id));
    }
    Ok((matched, modified, None))
}

/// The document an upsert inserts: the query's equalities, then the update.
fn upsert_document(q: &Document, u: &Bson) -> R<Document> {
    let mut d = Document::new();
    if let Bson::Document(u) = u
        && !u.keys().any(|k| k.starts_with('$'))
    {
        d.extend(u.clone());
        if let Some(id) = q.get("_id") {
            d.insert("_id", id.clone());
        }
        return Ok(crate::sqlgen::native::mongo::with_id(d).0);
    }
    for (k, v) in q {
        let is_op =
            matches!(v, Bson::Document(o) if o.keys().next().is_some_and(|k| k.starts_with('$')));
        if !k.starts_with('$') && !is_op {
            set_path(&mut d, k, v.clone())?;
        }
    }
    apply_update(&mut d, u, true)?;
    Ok(crate::sqlgen::native::mongo::with_id(d).0)
}

fn apply_update(d: &mut Document, u: &Bson, inserting: bool) -> R<()> {
    let id = d.get("_id").cloned();
    match u {
        Bson::Document(u) if u.keys().all(|k| k.starts_with('$')) && !u.is_empty() => {
            for (op, fields) in u {
                let Bson::Document(fields) = fields else {
                    return Err(Fail::new(
                        9,
                        "FailedToParse",
                        format!(
                            "Modifiers operate on fields but we found type {} instead",
                            type_name(fields)
                        ),
                    ));
                };
                for (path, v) in fields {
                    match op.as_str() {
                        "$set" => set_path(d, path, v.clone())?,
                        "$setOnInsert" if inserting => set_path(d, path, v.clone())?,
                        "$setOnInsert" => {}
                        "$unset" => remove_path(d, path),
                        "$inc" | "$mul" => {
                            let current = lookup(d, path).into_iter().next();
                            let base = current.unwrap_or(Bson::Int32(0));
                            let result = arith(&base, v, op == "$mul")?;
                            set_path(d, path, result)?;
                        }
                        "$currentDate" => {
                            let ts = matches!(v, Bson::Document(t) if t.get_str("$type") == Ok("timestamp"));
                            let now = if ts {
                                Bson::Timestamp(::mongodb::bson::Timestamp {
                                    time: (DateTime::now().timestamp_millis() / 1000) as u32,
                                    increment: 1,
                                })
                            } else {
                                Bson::DateTime(DateTime::now())
                            };
                            set_path(d, path, now)?;
                        }
                        "$push" => {
                            let mut items = match lookup(d, path).into_iter().next() {
                                Some(Bson::Array(a)) => a,
                                None => Vec::new(),
                                Some(_) => return Err(Fail::bad("$push needs an array")),
                            };
                            match v {
                                Bson::Document(e) if e.contains_key("$each") => {
                                    items.extend(e.get_array("$each").cloned().unwrap_or_default());
                                }
                                v => items.push(v.clone()),
                            }
                            set_path(d, path, Bson::Array(items))?;
                        }
                        "$rename" => {
                            let Bson::String(to) = v else {
                                return Err(Fail::bad("$rename takes a field name"));
                            };
                            if let Some(value) = lookup(d, path).into_iter().next() {
                                remove_path(d, path);
                                set_path(d, to, value)?;
                            }
                        }
                        other => {
                            return Err(Fail::bad(format!(
                                "the fake server does not know {other}"
                            )));
                        }
                    }
                }
            }
        }
        Bson::Document(replacement) => {
            if replacement.keys().any(|k| k.starts_with('$')) {
                return Err(Fail::bad("a replacement takes no operators"));
            }
            let mut next = Document::new();
            if let Some(id) = &id {
                next.insert("_id", id.clone());
            }
            for (k, v) in replacement {
                next.insert(k.clone(), v.clone());
            }
            *d = next;
        }
        Bson::Array(stages) => {
            for stage in stages {
                let Bson::Document(stage) = stage else {
                    return Err(Fail::bad("stages are documents"));
                };
                match stage.iter().next() {
                    Some((op, Bson::Document(fields))) if op == "$set" || op == "$addFields" => {
                        for (path, v) in fields {
                            if matches!(v, Bson::String(s) if s.starts_with('$'))
                                || matches!(v, Bson::Document(_))
                            {
                                return Err(Fail::bad(
                                    "the fake server takes literal values in update pipelines",
                                ));
                            }
                            set_path(d, path, v.clone())?;
                        }
                    }
                    Some((op, fields)) if op == "$unset" => {
                        let names = match fields {
                            Bson::String(f) => vec![f.clone()],
                            Bson::Array(a) => a
                                .iter()
                                .filter_map(|b| b.as_str().map(str::to_string))
                                .collect(),
                            _ => return Err(Fail::bad("$unset takes names")),
                        };
                        for n in names {
                            remove_path(d, &n);
                        }
                    }
                    _ => {
                        return Err(Fail::bad(
                            "the fake server takes $set and $unset stages in update pipelines",
                        ));
                    }
                }
            }
        }
        _ => return Err(Fail::bad("u takes a document or a pipeline")),
    }
    if d.get("_id") != id.as_ref() && id.is_some() {
        return Err(Fail::new(
            66,
            "ImmutableField",
            "Performing an update on the path '_id' would modify the immutable field '_id'",
        ));
    }
    Ok(())
}

fn arith(a: &Bson, b: &Bson, mul: bool) -> R<Bson> {
    let bad = || {
        Fail::new(
            14,
            "TypeMismatch",
            "Cannot apply $inc to a value of non-numeric type",
        )
    };
    Ok(match (a, b) {
        (Bson::Int32(x), Bson::Int32(y)) => {
            let r = if mul {
                x.checked_mul(*y)
            } else {
                x.checked_add(*y)
            };
            match r {
                Some(r) => Bson::Int32(r),
                None => Bson::Int64(if mul {
                    i64::from(*x) * i64::from(*y)
                } else {
                    i64::from(*x) + i64::from(*y)
                }),
            }
        }
        (Bson::Int32(_) | Bson::Int64(_), Bson::Int32(_) | Bson::Int64(_)) => {
            let (x, y) = (int_of(a).ok_or_else(bad)?, int_of(b).ok_or_else(bad)?);
            Bson::Int64(if mul {
                x.wrapping_mul(y)
            } else {
                x.wrapping_add(y)
            })
        }
        _ => {
            let (x, y) = (number(a).ok_or_else(bad)?, number(b).ok_or_else(bad)?);
            Bson::Double(if mul { x * y } else { x + y })
        }
    })
}

fn set_path(d: &mut Document, path: &str, v: Bson) -> R<()> {
    match path.split_once('.') {
        None => {
            d.insert(path, v);
            Ok(())
        }
        Some((head, rest)) => {
            if !d.contains_key(head) {
                d.insert(head, Document::new());
            }
            match d.get_mut(head) {
                Some(Bson::Document(inner)) => set_path(inner, rest, v),
                _ => Err(Fail::new(
                    28,
                    "PathNotViable",
                    format!("Cannot create field '{rest}' in element {head}"),
                )),
            }
        }
    }
}

fn remove_path(d: &mut Document, path: &str) {
    match path.split_once('.') {
        None => {
            d.remove(path);
        }
        Some((head, rest)) => {
            if let Some(Bson::Document(inner)) = d.get_mut(head) {
                remove_path(inner, rest);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------------------------

fn run_pipeline(
    s: &mut State,
    db: &str,
    coll: Option<&str>,
    mut docs: Vec<Document>,
    pipeline: &[Bson],
) -> R<Vec<Document>> {
    for stage in pipeline {
        let Bson::Document(stage) = stage else {
            return Err(Fail::bad("a stage is a document"));
        };
        let Some((name, arg)) = stage.iter().next() else {
            return Err(Fail::bad("an empty stage"));
        };
        docs = match (name.as_str(), arg) {
            ("$match", Bson::Document(f)) => filter_docs(docs, f)?,
            ("$sort", Bson::Document(sort)) => {
                sort_docs(&mut docs, sort);
                docs
            }
            ("$skip", n) => docs
                .into_iter()
                .skip(int_of(n).unwrap_or(0).max(0) as usize)
                .collect(),
            ("$limit", n) => {
                docs.truncate(int_of(n).unwrap_or(0).max(0) as usize);
                docs
            }
            // A server samples at random; the first documents keep the tests repeatable.
            ("$sample", Bson::Document(o)) => {
                docs.truncate(o.get("size").and_then(int_of).unwrap_or(0).max(0) as usize);
                docs
            }
            ("$project", Bson::Document(p)) => {
                docs.iter().map(|d| project(d, p)).collect::<R<_>>()?
            }
            ("$count", Bson::String(field)) => {
                let n = docs.len() as i32;
                if n == 0 {
                    Vec::new()
                } else {
                    let mut d = Document::new();
                    d.insert(field.clone(), n);
                    vec![d]
                }
            }
            ("$group", Bson::Document(g)) => group(docs, g)?,
            ("$set" | "$addFields", Bson::Document(fields)) => {
                for d in docs.iter_mut() {
                    for (k, v) in fields {
                        if matches!(v, Bson::String(s) if s.starts_with('$'))
                            || matches!(v, Bson::Document(_))
                        {
                            return Err(Fail::bad("the fake server takes literal values in $set"));
                        }
                        set_path(d, k, v.clone())?;
                    }
                }
                docs
            }
            ("$collStats", _) => {
                let c = get_coll(s, db, coll.unwrap_or_default())?;
                let count = c.docs.len() as i64;
                let size = data_size(&c.docs);
                vec![doc! {
                    "ns": format!("{db}.{}", coll.unwrap_or_default()),
                    "storageStats": { "count": count, "size": size, "storageSize": size, "totalIndexSize": 4096 },
                    "count": count,
                }]
            }
            ("$currentOp", _) => current_ops(s)
                .into_iter()
                .filter_map(|b| match b {
                    Bson::Document(d) => Some(d),
                    _ => None,
                })
                .collect(),
            ("$documents", Bson::Array(items)) => items
                .iter()
                .filter_map(|b| b.as_document().cloned())
                .collect(),
            ("$out", Bson::String(target)) => {
                let entry = writable(s, db, target)?;
                entry.docs = docs;
                Vec::new()
            }
            _ => {
                return Err(Fail::bad(format!(
                    "the fake server does not run the stage {name}"
                )));
            }
        };
    }
    Ok(docs)
}

fn group(docs: Vec<Document>, g: &Document) -> R<Vec<Document>> {
    let id_expr = g.get("_id").cloned().unwrap_or(Bson::Null);
    let mut groups: Vec<(Bson, Document)> = Vec::new();
    for d in &docs {
        let key = eval(d, &id_expr);
        let pos = match groups.iter().position(|(k, _)| equal(k, &key)) {
            Some(p) => p,
            None => {
                groups.push((key.clone(), doc! { "_id": key }));
                groups.len() - 1
            }
        };
        let acc = &mut groups[pos].1;
        for (field, spec) in g.iter().filter(|(k, _)| k.as_str() != "_id") {
            let Bson::Document(spec) = spec else {
                return Err(Fail::bad("an accumulator is a document"));
            };
            match spec.iter().next() {
                Some((op, arg)) if op == "$sum" => {
                    let add = eval(d, arg);
                    let current = acc.get(field).cloned().unwrap_or(Bson::Int32(0));
                    let next = if number(&add).is_some() {
                        arith(&current, &add, false)?
                    } else {
                        current
                    };
                    acc.insert(field.clone(), next);
                }
                _ => return Err(Fail::bad("the fake server knows $sum only")),
            }
        }
    }
    Ok(groups.into_iter().map(|(_, d)| d).collect())
}

/// `"$field"` paths and literals.
fn eval(d: &Document, expr: &Bson) -> Bson {
    match expr {
        Bson::String(s) if s.starts_with('$') => {
            lookup(d, &s[1..]).into_iter().next().unwrap_or(Bson::Null)
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------------------------

fn filter_docs(docs: Vec<Document>, filter: &Document) -> R<Vec<Document>> {
    let mut out = Vec::new();
    for d in docs {
        if matches(&d, filter)? {
            out.push(d);
        }
    }
    Ok(out)
}

/// The values at a dotted path; through arrays of documents too.
pub fn lookup(d: &Document, path: &str) -> Vec<Bson> {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (path, None),
    };
    let Some(v) = d.get(head) else {
        return Vec::new();
    };
    match rest {
        None => vec![v.clone()],
        Some(rest) => match v {
            Bson::Document(inner) => lookup(inner, rest),
            Bson::Array(items) => {
                if let Ok(i) = rest.split('.').next().unwrap_or_default().parse::<usize>() {
                    let after = rest.split_once('.').map(|(_, r)| r);
                    return match (items.get(i), after) {
                        (Some(item), None) => vec![item.clone()],
                        (Some(Bson::Document(inner)), Some(r)) => lookup(inner, r),
                        _ => Vec::new(),
                    };
                }
                items
                    .iter()
                    .filter_map(|i| i.as_document())
                    .flat_map(|inner| lookup(inner, rest))
                    .collect()
            }
            _ => Vec::new(),
        },
    }
}

pub fn matches(d: &Document, filter: &Document) -> R<bool> {
    for (k, v) in filter {
        let ok = match k.as_str() {
            "$and" | "$or" | "$nor" => {
                let Bson::Array(parts) = v else {
                    return Err(Fail::bad(format!("{k} takes an array")));
                };
                let mut results = Vec::new();
                for p in parts {
                    let Bson::Document(p) = p else {
                        return Err(Fail::bad(format!("{k} takes documents")));
                    };
                    results.push(matches(d, p)?);
                }
                match k.as_str() {
                    "$and" => results.iter().all(|r| *r),
                    "$or" => results.iter().any(|r| *r),
                    _ => !results.iter().any(|r| *r),
                }
            }
            "$expr" => truthy(&expr_value(d, v)?),
            k if k.starts_with('$') => {
                return Err(Fail::bad(format!(
                    "the fake server does not know the operator {k}"
                )));
            }
            path => match_field(&lookup(d, path), v)?,
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

/// An aggregation expression over a document, enough for `$expr` in a query: `$$ROOT`,
/// `"$field"` paths, `$literal`, `$eq`, object and array literals and constants.
fn expr_value(d: &Document, e: &Bson) -> R<Bson> {
    Ok(match e {
        Bson::String(s) if s == "$$ROOT" => Bson::Document(d.clone()),
        Bson::String(s) if s.starts_with("$$") => {
            return Err(Fail::bad(format!(
                "the fake server does not know the variable {s}"
            )));
        }
        Bson::String(s) if s.starts_with('$') => {
            lookup(d, &s[1..]).into_iter().next().unwrap_or(Bson::Null)
        }
        Bson::Document(o) if o.keys().any(|k| k.starts_with('$')) => {
            match (o.len(), o.iter().next()) {
                (1, Some((op, arg))) if op == "$literal" => arg.clone(),
                (1, Some((op, Bson::Array(args)))) if op == "$eq" && args.len() == 2 => {
                    Bson::Boolean(equal(&expr_value(d, &args[0])?, &expr_value(d, &args[1])?))
                }
                _ => {
                    return Err(Fail::bad(format!(
                        "the fake server does not know the expression {o}"
                    )));
                }
            }
        }
        Bson::Document(o) => {
            let mut out = Document::new();
            for (k, v) in o {
                out.insert(k.clone(), expr_value(d, v)?);
            }
            Bson::Document(out)
        }
        Bson::Array(items) => {
            Bson::Array(items.iter().map(|i| expr_value(d, i)).collect::<R<_>>()?)
        }
        other => other.clone(),
    })
}

/// How an expression's value reads as a condition.
fn truthy(b: &Bson) -> bool {
    match b {
        Bson::Boolean(v) => *v,
        Bson::Null | Bson::Undefined => false,
        Bson::Int32(n) => *n != 0,
        Bson::Int64(n) => *n != 0,
        Bson::Double(f) => *f != 0.0,
        _ => true,
    }
}

fn is_operator_doc(v: &Bson) -> Option<&Document> {
    match v {
        Bson::Document(o) if o.keys().next().is_some_and(|k| k.starts_with('$')) => Some(o),
        _ => None,
    }
}

fn match_field(values: &[Bson], cond: &Bson) -> R<bool> {
    if let Some(ops) = is_operator_doc(cond) {
        for (op, arg) in ops {
            if !match_op(values, op, arg, ops)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    Ok(eq_match(values, cond))
}

/// Equality as a query means it: the value, an element of an array value, or null for a
/// missing field.
fn eq_match(values: &[Bson], q: &Bson) -> bool {
    if values.is_empty() {
        return matches!(q, Bson::Null);
    }
    values.iter().any(|v| {
        if let Bson::RegularExpression(re) = q {
            return regex_match(v, &re.pattern, &re.options);
        }
        equal(v, q) || matches!(v, Bson::Array(items) if items.iter().any(|i| equal(i, q)))
    })
}

fn regex_match(v: &Bson, pattern: &str, options: &str) -> bool {
    let check = |s: &str| {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(options.contains('i'))
            .multi_line(options.contains('m'))
            .dot_matches_new_line(options.contains('s'))
            .ignore_whitespace(options.contains('x'))
            .build()
            .is_ok_and(|re| re.is_match(s))
    };
    match v {
        Bson::String(s) | Bson::Symbol(s) => check(s),
        Bson::Array(items) => items.iter().any(|i| regex_match(i, pattern, options)),
        _ => false,
    }
}

/// Each value, and each element of an array value.
fn flat(values: &[Bson]) -> Vec<Bson> {
    let mut out = Vec::new();
    for v in values {
        if let Bson::Array(items) = v {
            out.extend(items.iter().cloned());
        }
        out.push(v.clone());
    }
    out
}

fn match_op(values: &[Bson], op: &str, arg: &Bson, all: &Document) -> R<bool> {
    Ok(match op {
        "$eq" => eq_match(values, arg),
        "$ne" => !eq_match(values, arg),
        "$gt" | "$gte" | "$lt" | "$lte" => {
            let candidates = if values.is_empty() {
                vec![Bson::Null]
            } else {
                flat(values)
            };
            candidates.iter().any(|v| {
                if rank(v) != rank(arg) {
                    return false;
                }
                let o = compare(v, arg);
                match op {
                    "$gt" => o == Ordering::Greater,
                    "$gte" => o != Ordering::Less,
                    "$lt" => o == Ordering::Less,
                    _ => o != Ordering::Greater,
                }
            })
        }
        "$in" | "$nin" => {
            let Bson::Array(items) = arg else {
                return Err(Fail::bad(format!("{op} needs an array")));
            };
            let hit = items.iter().any(|i| eq_match(values, i));
            if op == "$in" { hit } else { !hit }
        }
        "$exists" => {
            let want = match arg {
                Bson::Boolean(b) => *b,
                other => int_of(other).is_none_or(|n| n != 0),
            };
            values.is_empty() != want
        }
        "$regex" => {
            let (pattern, mut options) = match arg {
                Bson::String(p) => (p.clone(), String::new()),
                Bson::RegularExpression(r) => (r.pattern.clone(), r.options.clone()),
                _ => return Err(Fail::bad("$regex has to be a string")),
            };
            if let Ok(o) = all.get_str("$options") {
                options.push_str(o);
            }
            values.iter().any(|v| regex_match(v, &pattern, &options))
        }
        "$options" => true,
        "$not" => match arg {
            Bson::RegularExpression(r) => !values
                .iter()
                .any(|v| regex_match(v, &r.pattern, &r.options)),
            Bson::Document(_) => !match_field(values, arg)?,
            _ => return Err(Fail::bad("$not needs a regex or a document")),
        },
        "$type" => {
            let wanted: Vec<String> = match arg {
                Bson::Array(a) => a.iter().map(type_arg).collect(),
                other => vec![type_arg(other)],
            };
            flat(values).iter().any(|v| {
                let t = type_name(v);
                wanted
                    .iter()
                    .any(|w| w == t || (w == "number" && number(v).is_some()))
            })
        }
        "$size" => {
            let n = int_of(arg).ok_or_else(|| Fail::bad("$size needs a number"))?;
            values
                .iter()
                .any(|v| matches!(v, Bson::Array(a) if a.len() as i64 == n))
        }
        "$all" => {
            let Bson::Array(items) = arg else {
                return Err(Fail::bad("$all needs an array"));
            };
            !items.is_empty() && items.iter().all(|i| eq_match(values, i))
        }
        "$elemMatch" => {
            let Bson::Document(f) = arg else {
                return Err(Fail::bad("$elemMatch needs an Object"));
            };
            let mut hit = false;
            for v in values {
                if let Bson::Array(items) = v {
                    for i in items {
                        let ok = match i {
                            Bson::Document(inner) if is_operator_doc(arg).is_none() => {
                                matches(inner, f)?
                            }
                            other => match_field(std::slice::from_ref(other), arg)?,
                        };
                        hit |= ok;
                    }
                }
            }
            hit
        }
        other => {
            return Err(Fail::bad(format!(
                "the fake server does not know the operator {other}"
            )));
        }
    })
}

fn type_arg(b: &Bson) -> String {
    match b {
        Bson::String(s) => s.clone(),
        n => match int_of(n) {
            Some(1) => "double".into(),
            Some(2) => "string".into(),
            Some(3) => "object".into(),
            Some(4) => "array".into(),
            Some(5) => "binData".into(),
            Some(7) => "objectId".into(),
            Some(8) => "bool".into(),
            Some(9) => "date".into(),
            Some(10) => "null".into(),
            Some(16) => "int".into(),
            Some(18) => "long".into(),
            Some(19) => "decimal".into(),
            _ => "unknown".into(),
        },
    }
}

fn number(b: &Bson) -> Option<f64> {
    match b {
        Bson::Int32(n) => Some(f64::from(*n)),
        Bson::Int64(n) => Some(*n as f64),
        Bson::Double(f) => Some(*f),
        Bson::Decimal128(d) => d.to_string().parse().ok(),
        _ => None,
    }
}

/// The BSON comparison order of types.
fn rank(b: &Bson) -> u8 {
    match b {
        Bson::MinKey => 1,
        Bson::Null | Bson::Undefined => 2,
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => 3,
        Bson::String(_) | Bson::Symbol(_) => 4,
        Bson::Document(_) => 5,
        Bson::Array(_) => 6,
        Bson::Binary(_) => 7,
        Bson::ObjectId(_) => 8,
        Bson::Boolean(_) => 9,
        Bson::DateTime(_) => 10,
        Bson::Timestamp(_) => 11,
        Bson::RegularExpression(_) => 12,
        Bson::DbPointer(_) => 13,
        Bson::JavaScriptCode(_) => 14,
        Bson::JavaScriptCodeWithScope(_) => 15,
        Bson::MaxKey => 16,
    }
}

pub fn compare(a: &Bson, b: &Bson) -> Ordering {
    let (ra, rb) = (rank(a), rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        _ if ra == 3 => {
            let (x, y) = (number(a).unwrap_or(f64::NAN), number(b).unwrap_or(f64::NAN));
            x.partial_cmp(&y)
                .unwrap_or_else(|| x.is_nan().cmp(&y.is_nan()).reverse())
        }
        (Bson::String(x) | Bson::Symbol(x), Bson::String(y) | Bson::Symbol(y)) => x.cmp(y),
        (Bson::Document(x), Bson::Document(y)) => {
            for ((kx, vx), (ky, vy)) in x.iter().zip(y.iter()) {
                let o = compare(vx, vy).then_with(|| kx.cmp(ky));
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Bson::Array(x), Bson::Array(y)) => {
            for (vx, vy) in x.iter().zip(y.iter()) {
                let o = compare(vx, vy);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Bson::Binary(x), Bson::Binary(y)) => x
            .bytes
            .len()
            .cmp(&y.bytes.len())
            .then(u8::from(x.subtype).cmp(&u8::from(y.subtype)))
            .then(x.bytes.cmp(&y.bytes)),
        (Bson::ObjectId(x), Bson::ObjectId(y)) => x.bytes().cmp(&y.bytes()),
        (Bson::Boolean(x), Bson::Boolean(y)) => x.cmp(y),
        (Bson::DateTime(x), Bson::DateTime(y)) => x.timestamp_millis().cmp(&y.timestamp_millis()),
        (Bson::Timestamp(x), Bson::Timestamp(y)) => {
            (x.time, x.increment).cmp(&(y.time, y.increment))
        }
        (Bson::RegularExpression(x), Bson::RegularExpression(y)) => {
            (&x.pattern, &x.options).cmp(&(&y.pattern, &y.options))
        }
        (Bson::JavaScriptCode(x), Bson::JavaScriptCode(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// Equal as a query compares: numbers across types, documents field by field in order.
pub fn equal(a: &Bson, b: &Bson) -> bool {
    rank(a) == rank(b) && compare(a, b) == Ordering::Equal
}

fn sort_cmp(a: &Document, b: &Document, sort: &Document) -> Ordering {
    for (k, dir) in sort {
        let va = lookup(a, k).into_iter().next().unwrap_or(Bson::Null);
        let vb = lookup(b, k).into_iter().next().unwrap_or(Bson::Null);
        let mut o = compare(&va, &vb);
        if int_of(dir).is_some_and(|n| n < 0) {
            o = o.reverse();
        }
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

fn sort_docs(docs: &mut [Document], sort: &Document) {
    docs.sort_by(|a, b| sort_cmp(a, b, sort));
}

/// Top-level inclusion or exclusion projections.
fn project(d: &Document, p: &Document) -> R<Document> {
    let include = p
        .iter()
        .filter(|(k, _)| k.as_str() != "_id")
        .any(|(_, v)| int_of(v).is_none_or(|n| n != 0) && !matches!(v, Bson::Boolean(false)));
    let id_shown = p
        .get("_id")
        .is_none_or(|v| int_of(v).is_none_or(|n| n != 0) && !matches!(v, Bson::Boolean(false)));
    let mut out = Document::new();
    for (k, v) in d {
        let named = p.get(k);
        let keep = if k == "_id" {
            id_shown
        } else if include {
            named.is_some_and(|v| {
                int_of(v).is_none_or(|n| n != 0) && !matches!(v, Bson::Boolean(false))
            })
        } else {
            named.is_none()
        };
        if keep {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

/// A new ObjectId, for seeds.
pub fn oid() -> ObjectId {
    ObjectId::new()
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::Regex;

    use super::*;

    fn fake() -> FakeMongo {
        let f = FakeMongo::new();
        f.seed(
            "shop",
            "orders",
            vec![
                doc! { "_id": 1, "item": "apple", "qty": 5, "tags": ["red", "fruit"], "info": { "origin": "NZ" } },
                doc! { "_id": 2, "item": "banana", "qty": 12_i64, "tags": ["yellow"] },
                doc! { "_id": 3, "item": "Cherry", "qty": 7.5, "note": null },
            ],
        );
        f
    }

    fn ids(reply: &Document) -> Vec<i64> {
        crate::drivers::mongodb::catalog::cursor_batch(reply)
            .0
            .iter()
            .filter_map(|d| d.get("_id").and_then(int_of))
            .collect()
    }

    #[test]
    fn queries_follow_server_semantics() {
        let f = fake();
        let find =
            |filter: Document| ids(&f.run("shop", &doc! { "find": "orders", "filter": filter }));
        assert_eq!(find(doc! { "qty": { "$gt": 6 } }), [2, 3]);
        assert_eq!(find(doc! { "qty": 12 }), [2]);
        assert_eq!(find(doc! { "tags": "red" }), [1]);
        assert_eq!(find(doc! { "note": null }), [1, 2, 3]);
        assert_eq!(find(doc! { "note": { "$exists": true } }), [3]);
        assert_eq!(find(doc! { "note": { "$ne": null } }), Vec::<i64>::new());
        assert_eq!(find(doc! { "info.origin": "NZ" }), [1]);
        assert_eq!(
            find(doc! { "item": Regex { pattern: "^c".into(), options: "i".into() } }),
            [3]
        );
        assert_eq!(
            find(doc! { "item": { "$regex": "an", "$options": "" } }),
            [2]
        );
        assert_eq!(
            find(doc! { "$or": [{ "_id": 1 }, { "qty": { "$lt": 6 } }] }),
            [1]
        );
        assert_eq!(find(doc! { "_id": { "$in": [1, 3] } }), [1, 3]);
        assert_eq!(find(doc! { "qty": { "$type": "long" } }), [2]);
        assert_eq!(find(doc! { "tags": { "$size": 2 } }), [1]);
        assert_eq!(find(doc! { "item": { "$not": { "$regex": "a" } } }), [3]);
        let err = f.run(
            "shop",
            &doc! { "find": "orders", "filter": { "$where": "1" } },
        );
        assert_eq!(err.get_i32("code"), Ok(2));
    }

    #[test]
    fn cursors_page_and_close() {
        let f = fake();
        let first = f.run(
            "shop",
            &doc! { "find": "orders", "filter": {}, "sort": { "_id": -1 }, "batchSize": 2 },
        );
        let (batch, id) = crate::drivers::mongodb::catalog::cursor_batch(&first);
        assert_eq!(batch.len(), 2);
        assert_ne!(id, 0);
        assert_eq!(f.open_cursors(), 1);
        let next = f.run("shop", &doc! { "getMore": id, "collection": "orders" });
        assert_eq!(ids(&next), [1]);
        assert_eq!(f.open_cursors(), 0);
        let single = f.run("shop", &doc! { "find": "orders", "filter": {}, "limit": 2, "batchSize": 2, "singleBatch": true });
        assert_eq!(crate::drivers::mongodb::catalog::cursor_batch(&single).1, 0);
    }

    #[test]
    fn batches_stop_at_the_byte_cap() {
        let f = FakeMongo::new();
        let big = "x".repeat(400);
        f.seed(
            "shop",
            "logs",
            (1..=5)
                .map(|i| doc! { "_id": i, "text": big.clone() })
                .collect(),
        );
        f.batch_bytes(1000);
        let page = doc! { "find": "logs", "filter": {}, "limit": 4_i64, "batchSize": 4_i64 };
        let first = f.run("shop", &page);
        let (batch, id) = crate::drivers::mongodb::catalog::cursor_batch(&first);
        assert_eq!(batch.len(), 2);
        assert_ne!(id, 0);
        // The cursor keeps the limit: the rest of the page, then closed.
        let next = f.run("shop", &doc! { "getMore": id, "collection": "logs" });
        assert_eq!(ids(&next), [3, 4]);
        assert_eq!(crate::drivers::mongodb::catalog::cursor_batch(&next).1, 0);
        // `singleBatch` closes the cursor on a cut batch, as a server does: a short page.
        let mut single = page.clone();
        single.insert("singleBatch", true);
        let cut = f.run("shop", &single);
        assert_eq!(
            crate::drivers::mongodb::catalog::cursor_batch(&cut),
            (crate::drivers::mongodb::catalog::cursor_batch(&first).0, 0)
        );
        // One document over the cap still makes a batch.
        f.batch_bytes(10);
        assert_eq!(ids(&f.run("shop", &single)), [1]);
        assert_eq!(f.open_cursors(), 0);
    }

    #[test]
    fn expr_compares_whole_documents() {
        let f = FakeMongo::new();
        let stored = doc! { "_id": 1, "a.b": 1, "$x": { "$gt": 5 }, "re": Regex { pattern: "^a".into(), options: "i".into() } };
        f.seed("shop", "odd", vec![stored.clone()]);
        let find = |q: Document| ids(&f.run("shop", &doc! { "find": "odd", "filter": q }));
        let same = doc! { "$expr": { "$eq": ["$$ROOT", { "$literal": stored.clone() }] } };
        assert_eq!(find(same), [1]);
        let mut changed = stored.clone();
        changed.insert("extra", true);
        assert!(find(doc! { "$expr": { "$eq": ["$$ROOT", { "$literal": changed }] } }).is_empty());
        assert_eq!(find(doc! { "$expr": { "$eq": ["$_id", 1] } }), [1]);
        let err = f.run(
            "shop",
            &doc! { "find": "odd", "filter": { "$expr": { "$gt": ["$_id", 0] } } },
        );
        assert_eq!(err.get_i32("code"), Ok(2));
    }

    #[test]
    fn writes_report_n() {
        let f = fake();
        let r = f.run("shop", &doc! { "insert": "orders", "documents": [{ "_id": 4 }, { "_id": 1 }, { "_id": 5 }], "ordered": true });
        assert_eq!(r.get_i32("n"), Ok(1));
        assert_eq!(r.get_array("writeErrors").unwrap().len(), 1);
        let r = f.run("shop", &doc! { "update": "orders", "updates": [{ "q": { "_id": 1 }, "u": { "$inc": { "qty": 1 }, "$unset": { "info": "" } }, "multi": false }] });
        assert_eq!((r.get_i64("n"), r.get_i64("nModified")), (Ok(1), Ok(1)));
        let apple = &f.docs("shop", "orders")[0];
        assert_eq!(apple.get("qty"), Some(&Bson::Int32(6)));
        assert!(!apple.contains_key("info"));
        let r = f.run("shop", &doc! { "update": "orders", "updates": [{ "q": { "_id": 99 }, "u": { "$set": { "a": 1 } }, "multi": false }] });
        assert_eq!(r.get_i64("n"), Ok(0));
        let r = f.run("shop", &doc! { "update": "orders", "updates": [{ "q": { "_id": 1 }, "u": { "$set": { "_id": 9 } } }] });
        assert_eq!(
            r.get_array("writeErrors").unwrap()[0]
                .as_document()
                .unwrap()
                .get_i32("code"),
            Ok(66)
        );
        let r = f.run(
            "shop",
            &doc! { "delete": "orders", "deletes": [{ "q": { "qty": { "$gt": 0 } }, "limit": 1 }] },
        );
        assert_eq!(r.get_i64("n"), Ok(1));
        assert_eq!(f.docs("shop", "orders").len(), 3);
        let r = f.run("shop", &doc! { "findAndModify": "orders", "query": { "_id": 2 }, "update": { "$set": { "qty": 1 } }, "new": true });
        assert_eq!(r.get_document("value").unwrap().get_i32("qty"), Ok(1));
    }

    #[test]
    fn aggregates_counts_and_catalog() {
        let f = fake();
        let r = f.run("shop", &doc! { "aggregate": "orders", "pipeline": [{ "$match": { "qty": { "$gt": 6 } } }, { "$group": { "_id": 1, "n": { "$sum": 1 } } }], "cursor": {} });
        assert_eq!(crate::drivers::mongodb::catalog::count_of(&r), Some(2));
        let r = f.run("shop", &doc! { "count": "orders" });
        assert_eq!(r.get_i64("n"), Ok(3));
        let r = f.run("shop", &doc! { "distinct": "orders", "key": "tags" });
        assert_eq!(r.get_array("values").unwrap().len(), 3);
        f.run("shop", &doc! { "create": "big", "viewOn": "orders", "pipeline": [{ "$match": { "qty": { "$gt": 6 } } }] });
        let r = f.run("shop", &doc! { "find": "big", "filter": {} });
        assert_eq!(ids(&r), [2, 3]);
        let r = f.run("shop", &doc! { "insert": "big", "documents": [{}] });
        assert_eq!(r.get_i32("code"), Ok(166));
        let r = f.run("shop", &doc! { "listCollections": 1, "nameOnly": false });
        assert_eq!(
            crate::drivers::mongodb::catalog::cursor_batch(&r).0.len(),
            2
        );
        let r = f.run("admin", &doc! { "listDatabases": 1 });
        assert_eq!(
            crate::drivers::mongodb::catalog::databases(&r, None).len(),
            1
        );
        let r = f.run("shop", &doc! { "nope": 1 });
        assert_eq!(r.get_i32("code"), Ok(59));
        f.fail_next("count", 13, "not authorized");
        assert_eq!(
            f.run("shop", &doc! { "count": "orders" }).get_i32("code"),
            Ok(13)
        );
        assert_eq!(
            f.run("shop", &doc! { "count": "orders" }).get_f64("ok"),
            Ok(1.0)
        );
    }

    #[test]
    fn wire_messages_round_trip_with_document_sequences() {
        // A body section and a `documents` sequence, as the driver sends an insert.
        let body = doc! { "insert": "c", "$db": "shop" };
        let docs = [doc! { "_id": 1 }, doc! { "_id": 2 }];
        let mut msg = 0u32.to_le_bytes().to_vec();
        msg.push(0);
        body.to_writer(&mut msg).unwrap();
        let mut seq = Vec::new();
        seq.extend_from_slice(b"documents\0");
        for d in &docs {
            d.to_writer(&mut seq).unwrap();
        }
        msg.push(1);
        msg.extend_from_slice(&((seq.len() + 4) as i32).to_le_bytes());
        msg.extend_from_slice(&seq);
        let (flags, command) = decode_msg(&msg).unwrap();
        assert_eq!(flags, 0);
        assert_eq!(command.get_array("documents").unwrap().len(), 2);
        assert!(decode_msg(&[0, 0]).is_err());
        assert!(decode_msg(&[0, 0, 0, 0, 0, 200, 0, 0, 0]).is_err());
        let reply = encode_msg(7, &doc! { "ok": 1 });
        assert_eq!(i32::from_le_bytes(reply[8..12].try_into().unwrap()), 7);
        assert_eq!(
            i32::from_le_bytes(reply[0..4].try_into().unwrap()) as usize,
            reply.len()
        );
    }
}
