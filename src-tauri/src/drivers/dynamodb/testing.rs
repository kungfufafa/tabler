//! An in-memory DynamoDB for the tests: tables with key schemas, items kept in key order, the
//! JSON 1.0 actions the driver sends (ListTables pages, DescribeTable, DescribeTimeToLive,
//! DescribeEndpoints, Scan and Query with `Limit`, `ExclusiveStartKey`, filters and `Select:
//! COUNT` (a Query refuses key conditions DynamoDB refuses and filters naming a key), GetItem,
//! PutItem, UpdateItem and DeleteItem with conditions, TransactWriteItems with
//! `CancellationReasons` and idempotency tokens, `SELECT * FROM t` through ExecuteStatement, whose
//! writes it acknowledges without applying), the expression subset the builders write, and the
//! failures a client must survive: scripted errors (throttling, server errors), SigV4 checked
//! against a known secret, a server clock that refuses stale signatures, expired session tokens,
//! slow replies (`set_delay`). It is a `Transport` itself, and is
//! served over a socket by `testkit::fake_http` (`FakeDynamo::route`), which the session's own
//! transport (`HttpTransport` over the shared `HttpClient`) reaches in the wire tests.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{Map, Value as J, json};

use super::attr::{Attr, AttrType, Item, item_from_json, map_json};
use super::transport::{HttpRequest, HttpResponse, HttpTransport, Transport, header};
use crate::drivers::common::aws::credentials::Credentials;
use crate::drivers::common::aws::sigv4::{amz_date, sign_parts};
use crate::drivers::common::http::{HttpClient, HttpEndpoint, HttpOptions, RetryPolicy};
use crate::error::AppError;
use crate::sqlgen::native::dynamo::{KeySchema, same_attr};
use crate::testkit::fake_http;

/// An error reply: status and body.
type Fail = (u16, J);

fn fail(code: &str, message: &str) -> Fail {
    (
        400,
        json!({"__type": format!("com.amazonaws.dynamodb.v20120810#{code}"), "message": message}),
    )
}

fn auth_fail(code: &str, message: &str) -> Fail {
    (
        400,
        json!({"__type": format!("com.amazon.coral.service#{code}"), "message": message}),
    )
}

fn invalid(message: &str) -> Fail {
    fail("ValidationException", message)
}

fn condition_failed() -> Fail {
    fail(
        "ConditionalCheckFailedException",
        "The conditional request failed",
    )
}

pub struct FakeTable {
    pub keys: KeySchema,
    pub items: Vec<Item>,
    pub ttl: Option<String>,
    pub extra: Map<String, J>,
}

impl FakeTable {
    fn key_of(&self, item: &Item) -> Option<Item> {
        self.keys.key_of(item)
    }

    fn position(&self, key: &Item) -> Option<usize> {
        self.items
            .iter()
            .position(|i| self.key_of(i).as_ref() == Some(key))
    }

    fn sort(&mut self) {
        let names: Vec<String> = self.keys.names().into_iter().map(str::to_string).collect();
        self.items.sort_by(|a, b| {
            for n in &names {
                let o = order(a.get(n), b.get(n));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    /// A key with exactly the key attributes, of their types.
    fn check_key(&self, key: &Item) -> Result<(), Fail> {
        let names = self.keys.names();
        if key.len() != names.len() {
            return Err(invalid(
                "The provided key element does not match the schema",
            ));
        }
        for n in names {
            match (key.get(n), self.keys.type_of(n)) {
                (Some(a), Some(t)) if a.ty() == t => {}
                _ => {
                    return Err(invalid(
                        "The provided key element does not match the schema",
                    ));
                }
            }
        }
        Ok(())
    }

    fn description(&self, name: &str) -> J {
        let mut defs = vec![
            json!({"AttributeName": self.keys.hash.0, "AttributeType": self.keys.hash.1.as_str()}),
        ];
        let mut schema = vec![json!({"AttributeName": self.keys.hash.0, "KeyType": "HASH"})];
        if let Some((r, t)) = &self.keys.range {
            defs.push(json!({"AttributeName": r, "AttributeType": t.as_str()}));
            schema.push(json!({"AttributeName": r, "KeyType": "RANGE"}));
        }
        let mut t = Map::new();
        t.insert("TableName".into(), json!(name));
        t.insert("TableStatus".into(), json!("ACTIVE"));
        t.insert("AttributeDefinitions".into(), J::Array(defs));
        t.insert("KeySchema".into(), J::Array(schema));
        t.insert("ItemCount".into(), json!(self.items.len()));
        t.insert("TableSizeBytes".into(), json!(self.items.len() * 100));
        t.insert(
            "BillingModeSummary".into(),
            json!({"BillingMode": "PAY_PER_REQUEST"}),
        );
        t.extend(self.extra.clone());
        json!({ "Table": t })
    }
}

fn order(a: Option<&Attr>, b: Option<&Attr>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(Attr::N(x)), Some(Attr::N(y))) => {
            let (x, y) = (
                x.parse::<f64>().unwrap_or(0.0),
                y.parse::<f64>().unwrap_or(0.0),
            );
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
        (Some(Attr::S(x)), Some(Attr::S(y))) => x.cmp(y),
        (Some(Attr::B(x)), Some(Attr::B(y))) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

#[derive(Default)]
struct State {
    tables: BTreeMap<String, FakeTable>,
    /// The next request of an action gets this reply instead.
    faults: Vec<(String, Fail)>,
    requests: Vec<(String, J)>,
    /// Signatures are checked with this key id and secret.
    signer: Option<(String, String)>,
    /// The server's clock: signatures more than five minutes off are refused.
    clock: Option<DateTime<Utc>>,
    expired_tokens: HashSet<String>,
    /// `ClientRequestToken`s of the transactions applied.
    tokens: HashSet<String>,
    /// Table names per ListTables page, items per ExecuteStatement page.
    page: usize,
    /// How long an in-process request takes (a slow server, for cancels and time outs).
    delay: Option<std::time::Duration>,
}

pub struct FakeDynamo {
    state: Mutex<State>,
}

impl FakeDynamo {
    pub fn new() -> Arc<FakeDynamo> {
        Arc::new(FakeDynamo {
            state: Mutex::new(State {
                page: 100,
                ..State::default()
            }),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn create_table(
        &self,
        name: &str,
        hash: (&str, AttrType),
        range: Option<(&str, AttrType)>,
    ) {
        self.state().tables.insert(
            name.to_string(),
            FakeTable {
                keys: KeySchema {
                    hash: (hash.0.to_string(), hash.1),
                    range: range.map(|(n, t)| (n.to_string(), t)),
                },
                items: Vec::new(),
                ttl: None,
                extra: Map::new(),
            },
        );
    }

    /// Adds fields to a table's description (indexes, a stream…).
    pub fn describe_with(&self, table: &str, field: &str, value: J) {
        if let Some(t) = self.state().tables.get_mut(table) {
            t.extra.insert(field.to_string(), value);
        }
    }

    pub fn set_ttl(&self, table: &str, attribute: &str) {
        if let Some(t) = self.state().tables.get_mut(table) {
            t.ttl = Some(attribute.to_string());
        }
    }

    /// Stores an item given as DynamoDB JSON (replacing one with its key).
    pub fn put(&self, table: &str, item: J) {
        let item = item_from_json(&item).expect("an item");
        let mut s = self.state();
        let t = s.tables.get_mut(table).expect("a table");
        let key = t.key_of(&item).expect("an item with its key");
        match t.position(&key) {
            Some(i) => t.items[i] = item,
            None => t.items.push(item),
        }
        t.sort();
    }

    pub fn items(&self, table: &str) -> Vec<Item> {
        self.state()
            .tables
            .get(table)
            .map(|t| t.items.clone())
            .unwrap_or_default()
    }

    /// The next `action` request fails with `code`.
    pub fn fail_next(&self, action: &str, status: u16, code: &str, message: &str) {
        let body = json!({"__type": format!("com.amazonaws.dynamodb.v20120810#{code}"), "message": message});
        self.state()
            .faults
            .push((action.to_string(), (status, body)));
    }

    /// Every request so far: action and body.
    pub fn requests(&self) -> Vec<(String, J)> {
        self.state().requests.clone()
    }

    pub fn actions(&self) -> Vec<String> {
        self.requests().into_iter().map(|(a, _)| a).collect()
    }

    /// Refuses requests not signed with this key.
    pub fn check_signatures(&self, key_id: &str, secret: &str) {
        self.state().signer = Some((key_id.to_string(), secret.to_string()));
    }

    pub fn set_clock(&self, now: DateTime<Utc>) {
        self.state().clock = Some(now);
    }

    /// Requests with this session token get `ExpiredTokenException`.
    pub fn expire_token(&self, token: &str) {
        self.state().expired_tokens.insert(token.to_string());
    }

    /// Table names per ListTables page and items per ExecuteStatement page.
    pub fn set_page(&self, n: usize) {
        self.state().page = n.max(1);
    }

    /// Every in-process request from now on takes `delay` before its reply.
    pub fn set_delay(&self, delay: std::time::Duration) {
        self.state().delay = Some(delay);
    }

    /// A route of `testkit::fake_http` serving this DynamoDB on every path.
    pub fn route(self: &Arc<Self>) -> fake_http::Route {
        let me = self.clone();
        fake_http::Route::new("POST", "/*").handler(move |req: &fake_http::Request| {
            let host = req.header("host").unwrap_or("localhost").to_string();
            let request = HttpRequest {
                url: format!("http://{host}{}", req.path),
                headers: req
                    .headers
                    .iter()
                    .filter(|(k, _)| k != "host")
                    .cloned()
                    .collect(),
                body: req.body.clone(),
            };
            let resp = me.reply(&request, &host);
            let mut reply = fake_http::Reply::new(resp.status).body(resp.body);
            for (k, v) in &resp.headers {
                reply = reply.header(k, v);
            }
            reply
        })
    }

    /// The reply to a request that reached `host`.
    pub fn reply(&self, req: &HttpRequest, host: &str) -> HttpResponse {
        let action = req.action().unwrap_or_default().to_string();
        let body: J = serde_json::from_slice(&req.body).unwrap_or(J::Null);
        let mut s = self.state();
        s.requests.push((action.clone(), body.clone()));
        if let Err((status, body, date)) = s.check_auth(req, host) {
            let resp = HttpResponse::json(status, &body);
            return match date {
                Some(d) => resp.with_header("Date", &d),
                None => resp,
            };
        }
        if let Some(i) = s.faults.iter().position(|(a, _)| *a == action) {
            let (_, (status, body)) = s.faults.remove(i);
            return HttpResponse::json(status, &body);
        }
        match s.handle(&action, &body) {
            Ok(reply) => HttpResponse::json(200, &reply),
            Err((status, body)) => HttpResponse::json(status, &body),
        }
    }
}

#[async_trait]
impl Transport for FakeDynamo {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let url = reqwest::Url::parse(&req.url).map_err(|e| AppError::Connect(e.to_string()))?;
        let host = match (url.host_str(), url.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            (None, _) => return Err(AppError::Connect("no host".into())),
        };
        let delay = self.state().delay;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Ok(self.reply(&req, &host))
    }
}

/// `Date` of an HTTP reply.
fn http_date(t: DateTime<Utc>) -> String {
    t.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

impl State {
    fn check_auth(&self, req: &HttpRequest, host: &str) -> Result<(), (u16, J, Option<String>)> {
        let refuse = |f: Fail| (f.0, f.1, None);
        if req
            .header(header::SECURITY_TOKEN)
            .is_some_and(|t| self.expired_tokens.contains(t))
        {
            return Err(refuse(auth_fail(
                "ExpiredTokenException",
                "The security token included in the request is expired",
            )));
        }
        let Some((key_id, secret)) = &self.signer else {
            return Ok(());
        };
        let Some(auth) = req.header(header::AUTHORIZATION) else {
            return Err(refuse(auth_fail(
                "MissingAuthenticationTokenException",
                "Request is missing Authentication Token",
            )));
        };
        let Some((credential, signed, signature)) = parse_authorization(auth) else {
            return Err(refuse(auth_fail(
                "IncompleteSignatureException",
                "bad Authorization",
            )));
        };
        let parts: Vec<&str> = credential.split('/').collect();
        let [id, _, region, service, "aws4_request"] = parts.as_slice() else {
            return Err(refuse(auth_fail(
                "IncompleteSignatureException",
                "bad Credential",
            )));
        };
        if id != key_id {
            return Err(refuse(auth_fail(
                "UnrecognizedClientException",
                "The security token included in the request is invalid.",
            )));
        }
        let Some(at) = req
            .header(header::AMZ_DATE)
            .and_then(|d| NaiveDateTime::parse_from_str(d, "%Y%m%dT%H%M%SZ").ok())
            .map(|d| d.and_utc())
        else {
            return Err(refuse(auth_fail(
                "IncompleteSignatureException",
                "no X-Amz-Date",
            )));
        };
        if let Some(now) = self.clock
            && (at - now).num_seconds().abs() > 300
        {
            return Err((
                400,
                auth_fail(
                    "InvalidSignatureException",
                    &format!(
                        "Signature expired: {} is now earlier than {} ({} - 5 min.)",
                        amz_date(at),
                        amz_date(now - chrono::Duration::minutes(5)),
                        amz_date(now)
                    ),
                )
                .1,
                Some(http_date(now)),
            ));
        }
        if !signed.iter().any(|h| h == "host") {
            return Err(refuse(auth_fail(
                "InvalidSignatureException",
                "host must be signed",
            )));
        }
        let headers: Vec<(String, String)> = signed
            .iter()
            .map(|n| {
                let v = if n == "host" {
                    host.to_string()
                } else {
                    req.header(n).unwrap_or_default().to_string()
                };
                (n.clone(), v)
            })
            .collect();
        let Ok(creds) = Credentials::static_keys(key_id, secret, None) else {
            return Err(refuse(invalid("no signing key")));
        };
        let again = sign_parts(
            "POST", "/", "", &headers, &req.body, &creds, at, region, service,
        );
        if again.signature != signature {
            return Err(refuse(auth_fail(
                "InvalidSignatureException",
                "The request signature we calculated does not match the signature you provided.",
            )));
        }
        Ok(())
    }

    fn table(&self, body: &J) -> Result<(&String, &FakeTable), Fail> {
        let name = body
            .get("TableName")
            .and_then(J::as_str)
            .unwrap_or_default();
        self.tables.get_key_value(name).ok_or_else(|| {
            fail(
                "ResourceNotFoundException",
                &format!("Requested resource not found: Table: {name} not found"),
            )
        })
    }

    fn handle(&mut self, action: &str, body: &J) -> Result<J, Fail> {
        match action {
            "ListTables" => {
                let start = body.get("ExclusiveStartTableName").and_then(J::as_str);
                let limit = body
                    .get("Limit")
                    .and_then(J::as_u64)
                    .map_or(100, |l| l as usize)
                    .min(self.page);
                let names: Vec<&String> = self
                    .tables
                    .keys()
                    .filter(|n| start.is_none_or(|s| n.as_str() > s))
                    .collect();
                let page: Vec<&String> = names.iter().take(limit).copied().collect();
                let mut reply = json!({ "TableNames": page });
                if names.len() > limit
                    && let Some(last) = page.last()
                {
                    reply["LastEvaluatedTableName"] = json!(last);
                }
                Ok(reply)
            }
            "DescribeTable" => {
                let (name, t) = self.table(body)?;
                Ok(t.description(name))
            }
            "DescribeTimeToLive" => {
                let (_, t) = self.table(body)?;
                Ok(match &t.ttl {
                    Some(a) => {
                        json!({"TimeToLiveDescription": {"TimeToLiveStatus": "ENABLED", "AttributeName": a}})
                    }
                    None => json!({"TimeToLiveDescription": {"TimeToLiveStatus": "DISABLED"}}),
                })
            }
            "DescribeEndpoints" => Ok(
                json!({"Endpoints": [{"Address": "dynamodb.fake", "CachePeriodInMinutes": 1440}]}),
            ),
            "Scan" => self.read(body, false),
            "Query" => self.read(body, true),
            "GetItem" => {
                let (_, t) = self.table(body)?;
                let key = attrs(body.get("Key"))?;
                t.check_key(&key)?;
                let Some(i) = t.position(&key) else {
                    return Ok(json!({}));
                };
                let mut item = t.items[i].clone();
                if let Some(p) = body.get("ProjectionExpression").and_then(J::as_str) {
                    let names = body
                        .get("ExpressionAttributeNames")
                        .cloned()
                        .unwrap_or(json!({}));
                    let wanted: Vec<String> = p
                        .split(',')
                        .map(|n| resolve(n.trim(), &names))
                        .collect::<Result<_, _>>()?;
                    item.retain(|k, _| wanted.contains(k));
                }
                Ok(json!({ "Item": map_json(&item) }))
            }
            "PutItem" | "UpdateItem" | "DeleteItem" => {
                let kind = match action {
                    "PutItem" => "Put",
                    "UpdateItem" => "Update",
                    _ => "Delete",
                };
                self.write(kind, body, true)?;
                Ok(json!({}))
            }
            "TransactWriteItems" => self.transact(body),
            "ExecuteStatement" => self.statement(body),
            _ => Err((
                400,
                json!({"__type": "com.amazon.coral.service#UnknownOperationException"}),
            )),
        }
    }

    fn read(&self, body: &J, query: bool) -> Result<J, Fail> {
        let (name, t) = self.table(body)?;
        if body.get("IndexName").is_some() {
            return Err(invalid("The fake has no indexes"));
        }
        let ctx = Ctx::new(body)?;
        let mut items: Vec<&Item> = t.items.iter().collect();
        if query {
            let kc = body
                .get("KeyConditionExpression")
                .and_then(J::as_str)
                .ok_or_else(|| invalid("Either the KeyConditions or KeyConditionExpression parameter must be specified"))?;
            check_key_condition(kc, &ctx.names, &t.keys)?;
            for tok in tokens(kc)? {
                if let Tok::Val(v) = tok
                    && matches!(ctx.values.get(&v), Some(Attr::S(s)) if s.is_empty())
                {
                    return Err(invalid(
                        "One or more parameter values are not valid. The AttributeValue for a key attribute cannot contain an empty string value.",
                    ));
                }
            }
            if let Some(f) = body.get("FilterExpression").and_then(J::as_str) {
                for n in named(f, &ctx.names)? {
                    if t.keys.type_of(&n).is_some() {
                        return Err(invalid(&format!(
                            "Filter Expression can only contain non-primary key attributes: Primary key attribute: {n}"
                        )));
                    }
                }
            }
            let mut kept = Vec::new();
            for i in items {
                if ctx.eval(kc, Some(i))? {
                    kept.push(i);
                }
            }
            items = kept;
            if body.get("ScanIndexForward") == Some(&json!(false)) {
                items.reverse();
            }
        }
        let start = match body.get("ExclusiveStartKey") {
            Some(k) => {
                let key = attrs(Some(k))?;
                let at = items
                    .iter()
                    .position(|i| t.key_of(i).as_ref() == Some(&key))
                    .ok_or_else(|| invalid("The provided starting key is invalid"))?;
                at + 1
            }
            None => 0,
        };
        let rest = &items[start.min(items.len())..];
        let limit = body
            .get("Limit")
            .and_then(J::as_u64)
            .map_or(rest.len(), |l| l as usize);
        let evaluated = &rest[..limit.min(rest.len())];
        let mut matched = Vec::new();
        if let Some(f) = body.get("FilterExpression").and_then(J::as_str) {
            for i in evaluated {
                if ctx.eval(f, Some(i))? {
                    matched.push(*i);
                }
            }
        } else {
            matched = evaluated.to_vec();
        }
        let mut reply = json!({ "Count": matched.len(), "ScannedCount": evaluated.len() });
        if body.get("Select").and_then(J::as_str) != Some("COUNT") {
            reply["Items"] = J::Array(matched.iter().map(|i| map_json(i)).collect());
        }
        if evaluated.len() < rest.len()
            && let Some(last) = evaluated.last()
            && let Some(k) = t.key_of(last)
        {
            reply["LastEvaluatedKey"] = map_json(&k);
        }
        if body.get("ReturnConsumedCapacity").and_then(J::as_str) == Some("TOTAL") {
            reply["ConsumedCapacity"] =
                json!({"TableName": name, "CapacityUnits": evaluated.len() as f64 * 0.5});
        }
        Ok(reply)
    }

    /// Checks (`check`) and applies one write of `kind` (`Put`, `Update`, `Delete`).
    fn write(&mut self, kind: &str, body: &J, apply: bool) -> Result<(), Fail> {
        let ctx = Ctx::new(body)?;
        let name = self.table(body)?.0.clone();
        let t = self
            .tables
            .get_mut(&name)
            .ok_or_else(|| invalid("no table"))?;
        let (key, new_item) = if kind == "Put" {
            let item = attrs(body.get("Item"))?;
            let key = t
                .key_of(&item)
                .ok_or_else(|| invalid("One of the required keys was not given a value"))?;
            (key, Some(item))
        } else {
            (attrs(body.get("Key"))?, None)
        };
        t.check_key(&key)?;
        let at = t.position(&key);
        let existing = at.map(|i| &t.items[i]);
        if let Some(c) = body.get("ConditionExpression").and_then(J::as_str)
            && !ctx.eval(c, existing)?
        {
            return Err(condition_failed());
        }
        if !apply {
            return Ok(());
        }
        match kind {
            "Put" => {
                let item = new_item.unwrap_or_default();
                match at {
                    Some(i) => t.items[i] = item,
                    None => t.items.push(item),
                }
            }
            "Update" => {
                let mut item = existing.cloned().unwrap_or_else(|| key.clone());
                if let Some(u) = body.get("UpdateExpression").and_then(J::as_str) {
                    ctx.update(u, &mut item)?;
                }
                match at {
                    Some(i) => t.items[i] = item,
                    None => t.items.push(item),
                }
            }
            _ => {
                if let Some(i) = at {
                    t.items.remove(i);
                }
            }
        }
        t.sort();
        Ok(())
    }

    fn transact(&mut self, body: &J) -> Result<J, Fail> {
        let token = body
            .get("ClientRequestToken")
            .and_then(J::as_str)
            .map(str::to_string);
        if token.as_ref().is_some_and(|t| self.tokens.contains(t)) {
            return Ok(json!({}));
        }
        let entries = body
            .get("TransactItems")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default();
        if entries.is_empty() || entries.len() > 100 {
            return Err(invalid("TransactItems takes 1 to 100 items"));
        }
        let mut writes = Vec::new();
        let mut keys = Vec::new();
        for e in &entries {
            let Some((kind, inner)) = e.as_object().and_then(|o| o.iter().next()) else {
                return Err(invalid("an empty TransactItem"));
            };
            let key = match inner.get("Key") {
                Some(k) => k.clone(),
                None => {
                    let (_, t) = self.table(inner)?;
                    map_json(&t.key_of(&attrs(inner.get("Item"))?).unwrap_or_default())
                }
            };
            let id = (inner.get("TableName").cloned(), key);
            if keys.contains(&id) {
                return Err(invalid(
                    "Transaction request cannot include multiple operations on one item",
                ));
            }
            keys.push(id);
            writes.push((kind.clone(), inner.clone()));
        }
        let mut reasons = Vec::new();
        let mut cancelled = false;
        for (kind, inner) in &writes {
            match self.write(kind, inner, false) {
                Ok(()) => reasons.push(json!({"Code": "None"})),
                Err((_, b))
                    if b["__type"]
                        .as_str()
                        .is_some_and(|t| t.ends_with("ConditionalCheckFailedException")) =>
                {
                    cancelled = true;
                    reasons.push(json!({"Code": "ConditionalCheckFailed", "Message": "The conditional request failed"}));
                }
                Err(e) => return Err(e),
            }
        }
        if cancelled {
            let codes: Vec<&str> = reasons.iter().filter_map(|r| r["Code"].as_str()).collect();
            return Err((
                400,
                json!({
                    "__type": "com.amazonaws.dynamodb.v20120810#TransactionCanceledException",
                    "Message": format!("Transaction cancelled, please refer cancellation reasons for specific reasons [{}]", codes.join(", ")),
                    "CancellationReasons": reasons,
                }),
            ));
        }
        for (kind, inner) in &writes {
            self.write(kind, inner, true)?;
        }
        if let Some(t) = token {
            self.tokens.insert(t);
        }
        Ok(json!({}))
    }

    /// `SELECT * FROM t` (the name quoted or not), paged by `NextToken`.
    fn statement(&self, body: &J) -> Result<J, Fail> {
        let text = body
            .get("Statement")
            .and_then(J::as_str)
            .unwrap_or_default()
            .trim();
        let upper = text.to_ascii_uppercase();
        // PartiQL writes are acknowledged, not applied: the fake applies the API's edits.
        if ["INSERT ", "UPDATE ", "DELETE "]
            .iter()
            .any(|w| upper.starts_with(w))
        {
            return Ok(json!({}));
        }
        let Some(rest) = upper.strip_prefix("SELECT * FROM ") else {
            return Err(invalid("Statement wasn't well formed, can't be processed"));
        };
        let name = text[text.len() - rest.len()..].trim().trim_matches('"');
        let (_, t) = self.table(&json!({ "TableName": name }))?;
        let from: usize = body
            .get("NextToken")
            .and_then(J::as_str)
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let page: Vec<J> = t
            .items
            .iter()
            .skip(from)
            .take(self.page)
            .map(map_json)
            .collect();
        let mut reply = json!({ "Items": page });
        if from + self.page < t.items.len() {
            reply["NextToken"] = json!((from + self.page).to_string());
        }
        Ok(reply)
    }
}

fn attrs(v: Option<&J>) -> Result<Item, Fail> {
    item_from_json(v.unwrap_or(&J::Null)).map_err(|e| invalid(&e.to_string()))
}

fn resolve(token: &str, names: &J) -> Result<String, Fail> {
    match token.strip_prefix('#') {
        Some(_) => names
            .get(token)
            .and_then(J::as_str)
            .map(str::to_string)
            .ok_or_else(|| invalid(&format!("An expression attribute name used in the document path is not defined; attribute name: {token}"))),
        None => Ok(token.to_string()),
    }
}

// --- the expression subset the builders write -------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Val(String),
    Word(String),
    Open,
    Close,
    Comma,
    Op(String),
}

fn tokens(text: &str) -> Result<Vec<Tok>, Fail> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            out.push(Tok::Open);
            i += 1;
        } else if c == ')' {
            out.push(Tok::Close);
            i += 1;
        } else if c == ',' {
            out.push(Tok::Comma);
            i += 1;
        } else if "=<>".contains(c) {
            let mut op = c.to_string();
            if let Some(n) = chars.get(i + 1)
                && ((c == '<' && (*n == '=' || *n == '>')) || (c == '>' && *n == '='))
            {
                op.push(*n);
                i += 1;
            }
            out.push(Tok::Op(op));
            i += 1;
        } else if c == '#' || c == ':' || c.is_ascii_alphanumeric() || c == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            out.push(match c {
                '#' => Tok::Name(word),
                ':' => Tok::Val(word),
                _ => Tok::Word(word),
            });
        } else {
            return Err(invalid(&format!("Invalid expression: unexpected {c}")));
        }
    }
    Ok(out)
}

/// The attributes an expression names: its `#` placeholders, and words that are not keywords
/// or functions.
fn named(expr: &str, names: &J) -> Result<Vec<String>, Fail> {
    let toks = tokens(expr)?;
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Name(n) => out.push(resolve(n, names)?),
            Tok::Word(w)
                if toks.get(i + 1) != Some(&Tok::Open)
                    && !["AND", "OR", "NOT", "BETWEEN", "IN"]
                        .iter()
                        .any(|k| w.eq_ignore_ascii_case(k)) =>
            {
                out.push(w.clone());
            }
            _ => {}
        }
    }
    Ok(out)
}

/// DynamoDB's rule for a Query's `KeyConditionExpression`: an equality on the partition key,
/// then at most one condition on the sort key (`=`, `<`, `<=`, `>`, `>=`, `BETWEEN`,
/// `begins_with`) after AND; nothing else.
fn check_key_condition(kc: &str, names: &J, keys: &KeySchema) -> Result<(), Fail> {
    let toks = tokens(kc)?;
    let bad = || invalid("Query key condition not supported");
    let is_key = |t: &Tok, key: &str| -> Result<bool, Fail> {
        Ok(match t {
            Tok::Name(n) => resolve(n, names)? == key,
            Tok::Word(w) => w == key,
            _ => false,
        })
    };
    let (hash, rest) = match toks.as_slice() {
        [h, Tok::Op(op), Tok::Val(_), rest @ ..] if op == "=" => (h, rest),
        _ => return Err(bad()),
    };
    if !is_key(hash, &keys.hash.0)? {
        return Err(bad());
    }
    let range = match (rest, &keys.range) {
        ([], _) => return Ok(()),
        ([Tok::Word(and), range @ ..], Some((name, _))) if and.eq_ignore_ascii_case("AND") => {
            (range, name)
        }
        _ => return Err(bad()),
    };
    let ok = match range.0 {
        [a, Tok::Op(op), Tok::Val(_)] => op != "<>" && is_key(a, range.1)?,
        [
            a,
            Tok::Word(between),
            Tok::Val(_),
            Tok::Word(and),
            Tok::Val(_),
        ] => {
            between.eq_ignore_ascii_case("BETWEEN")
                && and.eq_ignore_ascii_case("AND")
                && is_key(a, range.1)?
        }
        [
            Tok::Word(f),
            Tok::Open,
            a,
            Tok::Comma,
            Tok::Val(_),
            Tok::Close,
        ] => f == "begins_with" && is_key(a, range.1)?,
        _ => false,
    };
    if ok { Ok(()) } else { Err(bad()) }
}

/// The placeholders of a request.
struct Ctx {
    names: J,
    values: BTreeMap<String, Attr>,
}

impl Ctx {
    fn new(body: &J) -> Result<Ctx, Fail> {
        Ok(Ctx {
            names: body
                .get("ExpressionAttributeNames")
                .cloned()
                .unwrap_or(json!({})),
            values: match body.get("ExpressionAttributeValues") {
                Some(v) => attrs(Some(v))?,
                None => BTreeMap::new(),
            },
        })
    }

    fn eval(&self, expr: &str, item: Option<&Item>) -> Result<bool, Fail> {
        let empty = Item::new();
        let mut p = Parser {
            toks: tokens(expr)?,
            at: 0,
            ctx: self,
            item: item.unwrap_or(&empty),
        };
        let v = p.or()?;
        if p.at != p.toks.len() {
            return Err(invalid("Invalid ConditionExpression: unexpected tokens"));
        }
        Ok(v)
    }

    /// `SET #a = :v, … REMOVE #b, …`.
    fn update(&self, expr: &str, item: &mut Item) -> Result<(), Fail> {
        let toks = tokens(expr)?;
        let mut i = 0;
        let mut clause = String::new();
        while i < toks.len() {
            match &toks[i] {
                Tok::Word(w)
                    if w.eq_ignore_ascii_case("SET") || w.eq_ignore_ascii_case("REMOVE") =>
                {
                    clause = w.to_ascii_uppercase();
                    i += 1;
                }
                Tok::Comma => i += 1,
                Tok::Name(n) if clause == "SET" => {
                    let (Some(Tok::Op(op)), Some(Tok::Val(v))) = (toks.get(i + 1), toks.get(i + 2))
                    else {
                        return Err(invalid("Invalid UpdateExpression"));
                    };
                    if op != "=" {
                        return Err(invalid("Invalid UpdateExpression"));
                    }
                    let name = resolve(n, &self.names)?;
                    let value = self
                        .values
                        .get(v)
                        .cloned()
                        .ok_or_else(|| invalid("undefined value"))?;
                    item.insert(name, value);
                    i += 3;
                }
                Tok::Name(n) if clause == "REMOVE" => {
                    item.remove(&resolve(n, &self.names)?);
                    i += 1;
                }
                _ => {
                    return Err(invalid(
                        "Invalid UpdateExpression: the fake takes SET and REMOVE only",
                    ));
                }
            }
        }
        Ok(())
    }
}

struct Parser<'a> {
    toks: Vec<Tok>,
    at: usize,
    ctx: &'a Ctx,
    item: &'a Item,
}

impl Parser<'_> {
    fn peek_word(&self, w: &str) -> bool {
        matches!(self.toks.get(self.at), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn expect(&mut self, t: Tok) -> Result<(), Fail> {
        if self.next() == Some(t) {
            Ok(())
        } else {
            Err(invalid("Invalid expression: syntax error"))
        }
    }

    fn or(&mut self) -> Result<bool, Fail> {
        let mut v = self.and()?;
        while self.peek_word("OR") {
            self.at += 1;
            let r = self.and()?;
            v = v || r;
        }
        Ok(v)
    }

    fn and(&mut self) -> Result<bool, Fail> {
        let mut v = self.not()?;
        while self.peek_word("AND") {
            self.at += 1;
            let r = self.not()?;
            v = v && r;
        }
        Ok(v)
    }

    fn not(&mut self) -> Result<bool, Fail> {
        if self.peek_word("NOT") {
            self.at += 1;
            return Ok(!self.not()?);
        }
        self.primary()
    }

    fn operand(&mut self) -> Result<Option<Attr>, Fail> {
        match self.next() {
            Some(Tok::Name(n)) => {
                let name = resolve(&n, &self.ctx.names)?;
                Ok(self.item.get(&name).cloned())
            }
            Some(Tok::Val(v)) => self
                .ctx
                .values
                .get(&v)
                .cloned()
                .map(Some)
                .ok_or_else(|| invalid(&format!("An expression attribute value used in expression is not defined; attribute value: {v}"))),
            _ => Err(invalid("Invalid expression: an operand is missing")),
        }
    }

    fn primary(&mut self) -> Result<bool, Fail> {
        if self.toks.get(self.at) == Some(&Tok::Open) {
            self.at += 1;
            let v = self.or()?;
            self.expect(Tok::Close)?;
            return Ok(v);
        }
        if let Some(Tok::Word(f)) = self.toks.get(self.at).cloned()
            && self.toks.get(self.at + 1) == Some(&Tok::Open)
        {
            self.at += 2;
            let a = self.operand()?;
            let b = if self.toks.get(self.at) == Some(&Tok::Comma) {
                self.at += 1;
                self.operand()?
            } else {
                None
            };
            self.expect(Tok::Close)?;
            return match (f.as_str(), a, b) {
                ("attribute_exists", a, None) => Ok(a.is_some()),
                ("attribute_not_exists", a, None) => Ok(a.is_none()),
                ("attribute_type", a, Some(Attr::S(t))) => {
                    Ok(a.is_some_and(|a| a.ty().as_str() == t))
                }
                ("begins_with", Some(Attr::S(s)), Some(Attr::S(p))) => Ok(s.starts_with(&p)),
                ("begins_with", _, _) => Ok(false),
                ("contains", Some(Attr::S(s)), Some(Attr::S(p))) => Ok(s.contains(&p)),
                ("contains", Some(Attr::Ss(s)), Some(Attr::S(p))) => Ok(s.contains(&p)),
                ("contains", Some(Attr::L(l)), Some(v)) => Ok(l.iter().any(|x| same_attr(x, &v))),
                ("contains", _, _) => Ok(false),
                _ => Err(invalid(&format!("Invalid function name; function: {f}"))),
            };
        }
        let a = self.operand()?;
        match self.next() {
            Some(Tok::Op(op)) => {
                let b = self.operand()?;
                Ok(compare(a.as_ref(), &op, b.as_ref()))
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("BETWEEN") => {
                let lo = self.operand()?;
                if !self.peek_word("AND") {
                    return Err(invalid("Invalid expression: BETWEEN needs AND"));
                }
                self.at += 1;
                let hi = self.operand()?;
                Ok(
                    compare(a.as_ref(), ">=", lo.as_ref())
                        && compare(a.as_ref(), "<=", hi.as_ref()),
                )
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("IN") => {
                self.expect(Tok::Open)?;
                let mut any = false;
                loop {
                    let v = self.operand()?;
                    any |= compare(a.as_ref(), "=", v.as_ref());
                    match self.next() {
                        Some(Tok::Comma) => continue,
                        Some(Tok::Close) => break,
                        _ => return Err(invalid("Invalid expression: IN list")),
                    }
                }
                Ok(any)
            }
            _ => Err(invalid("Invalid expression: a comparison is missing")),
        }
    }
}

fn compare(a: Option<&Attr>, op: &str, b: Option<&Attr>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return false;
    };
    match op {
        "=" => same_attr(a, b),
        "<>" => !same_attr(a, b),
        _ => {
            if a.ty() != b.ty() || !matches!(a.ty(), AttrType::S | AttrType::N | AttrType::B) {
                return false;
            }
            let o = order(Some(a), Some(b));
            match op {
                "<" => o.is_lt(),
                "<=" => o.is_le(),
                ">" => o.is_gt(),
                ">=" => o.is_ge(),
                _ => false,
            }
        }
    }
}

// --- the wire ---------------------------------------------------------------------------------

/// The session's own transport to `endpoint` (a `testkit::fake_http` server), without retries.
pub fn http_transport(endpoint: &str) -> std::sync::Arc<HttpTransport> {
    crate::testkit::install_crypto();
    let options = HttpOptions {
        retry: RetryPolicy::NONE,
        ..HttpOptions::default()
    };
    let endpoint = HttpEndpoint::new(endpoint, None).expect("an endpoint");
    let http = HttpClient::new(
        endpoint,
        options,
        crate::drivers::common::http::Credential::None,
    )
    .expect("an HTTP client");
    std::sync::Arc::new(HttpTransport::new(http))
}

/// The parts of an `Authorization` header: credential, signed header names, signature.
pub fn parse_authorization(header: &str) -> Option<(String, Vec<String>, String)> {
    let rest = header.strip_prefix("AWS4-HMAC-SHA256")?.trim();
    let mut credential = None;
    let mut signed = None;
    let mut signature = None;
    for part in rest.split(',') {
        let (k, v) = part.trim().split_once('=')?;
        match k {
            "Credential" => credential = Some(v.to_string()),
            "SignedHeaders" => signed = Some(v.split(';').map(str::to_string).collect()),
            "Signature" => signature = Some(v.to_string()),
            _ => {}
        }
    }
    Some((credential?, signed?, signature?))
}
