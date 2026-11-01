//! The real `Wire`: a scylla `Session`, opened from a `Dial`.
//!
//! - The contact point is resolved here and given to the driver as an address, so TLS can check
//!   the profile's host name: the driver names a node by its IP address (`ServerName::IpAddress`),
//!   which `tls_client::rustls_config_through` maps back to the host for that address.
//! - Single node (always through an SSH tunnel): a host filter keeps only the contact point and an
//!   address translator sends any peer the driver learns of back to it, so nothing dials the
//!   cluster's private addresses.
//! - A Command password runs again for each connection the driver opens once it has expired.
//! - Values are bound through a prepared statement, whose markers give the types to encode them
//!   as (`encode`); a statement without values is sent as it is.

use std::net::SocketAddr;
use std::sync::Arc;

use ::scylla::authentication::{AuthError, AuthenticatorProvider, AuthenticatorSession};
use ::scylla::client::execution_profile::ExecutionProfile;
use ::scylla::client::session::Session as ScyllaSession;
use ::scylla::client::session_builder::SessionBuilder;
use ::scylla::cluster::NodeAddr;
use ::scylla::cluster::metadata::Peer;
use ::scylla::errors::TranslationError;
use ::scylla::frame::Compression;
use ::scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
use ::scylla::policies::host_filter::HostFilter;
use ::scylla::policies::load_balancing::DefaultPolicy;
use ::scylla::response::PagingStateResponse;
use ::scylla::response::query_result::QueryResult;
use ::scylla::statement::prepared::PreparedStatement;
use ::scylla::statement::unprepared::Statement;
use ::scylla::value::{CqlValue, Row};
use async_trait::async_trait;
use uuid::Uuid;

use super::encode::encode;
use super::errors;
use super::params::CassandraCompression;
use super::values::{result_column, timestamp_text};
use super::wire::{Connector, Credentials, Dial, Page, Password, Reply, Request, RunOptions, Wire};
use crate::error::AppError;

pub struct ScyllaConnector;

#[async_trait]
impl Connector for ScyllaConnector {
    async fn connect(&self, dial: &Dial) -> Result<Arc<dyn Wire>, AppError> {
        let addr = resolve(&dial.host, dial.port).await?;
        let mut profile = ExecutionProfile::builder()
            .consistency(dial.options.consistency)
            .serial_consistency(Some(dial.options.serial))
            .request_timeout(Some(dial.options.timeout));
        if let Some(dc) = &dial.local_datacenter {
            profile = profile.load_balancing_policy(
                DefaultPolicy::builder()
                    .prefer_datacenter(dc.clone())
                    .build(),
            );
        }
        let mut builder = SessionBuilder::new()
            .known_node_addr(addr)
            .connection_timeout(dial.connect_timeout)
            .keepalive_interval(dial.keepalive)
            // The catalog reads `system_schema` itself, when the sidebar asks.
            .fetch_schema_metadata(false)
            .default_execution_profile_handle(profile.build().into_handle())
            .compression(match dial.compression {
                CassandraCompression::None => None,
                CassandraCompression::Lz4 => Some(Compression::Lz4),
                CassandraCompression::Snappy => Some(Compression::Snappy),
            });
        if let Some(tls) = &dial.tls {
            let identity = crate::tls_files::load_client_certificate(
                tls.key_path.as_deref(),
                tls.cert_path.as_deref(),
            )?;
            let config = crate::tls_client::rustls_config_through(
                tls.mode,
                tls.ca_path.as_deref(),
                identity,
                &[],
                None,
                Some((addr.ip(), crate::tls_client::server_name(&tls.server_name)?)),
            )?;
            builder = builder.tls_context(Some(Arc::new(config)));
        }
        if let Some(credentials) = &dial.credentials {
            builder = builder.authenticator_provider(Arc::new(PasswordAuth(credentials.clone())));
        }
        if let Some(keyspace) = &dial.keyspace {
            builder = builder.use_keyspace(keyspace.clone(), true);
        }
        if dial.single_node {
            builder = builder
                .host_filter(Arc::new(ContactPointOnly))
                .address_translator(Arc::new(ToContactPoint(addr)));
        }
        // The driver's own timeout covers each connection; this one the whole start.
        let limit = dial.connect_timeout * 2 + std::time::Duration::from_secs(5);
        let session = tokio::time::timeout(limit, builder.build())
            .await
            .map_err(|_| AppError::Connect(format!("Cassandra: no answer from {addr}")))?
            .map_err(errors::new_session)?;
        Ok(Arc::new(ScyllaWire { session }))
    }
}

/// The contact point's address (the tunnel's local end through SSH).
async fn resolve(host: &str, port: u16) -> Result<SocketAddr, AppError> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    tokio::net::lookup_host((bare, port))
        .await
        .map_err(|e| AppError::Connect(format!("Cassandra: cannot resolve {host}: {e}")))?
        .next()
        .ok_or_else(|| AppError::Connect(format!("Cassandra: cannot resolve {host}")))
}

/// Keeps only the contact point: every peer the driver discovers is translatable.
struct ContactPointOnly;

impl HostFilter for ContactPointOnly {
    fn accept(&self, peer: &Peer) -> bool {
        matches!(peer.address, NodeAddr::Untranslatable(_))
    }
}

/// Sends any peer to the contact point, should the driver dial one before the filter.
struct ToContactPoint(SocketAddr);

#[async_trait]
impl AddressTranslator for ToContactPoint {
    async fn translate_address(
        &self,
        _peer: &UntranslatedPeer,
    ) -> Result<SocketAddr, TranslationError> {
        Ok(self.0)
    }
}

/// PasswordAuthenticator (`\0user\0password`), with the password read for each connection.
struct PasswordAuth(Credentials);

struct NoChallenge;

#[async_trait]
impl AuthenticatorSession for NoChallenge {
    async fn evaluate_challenge(
        &mut self,
        _token: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, AuthError> {
        Err("Cassandra: the server asked for more than a password".into())
    }

    async fn success(&mut self, _token: Option<&[u8]>) -> Result<(), AuthError> {
        Ok(())
    }
}

#[async_trait]
impl AuthenticatorProvider for PasswordAuth {
    async fn start_authentication_session(
        &self,
        _authenticator_name: &str,
    ) -> Result<(Option<Vec<u8>>, Box<dyn AuthenticatorSession>), AuthError> {
        let password = match &self.0.password {
            Password::Fixed(p) => p.clone(),
            Password::Command(command) => command
                .password()
                .await
                .map_err(|e| format!("the password command failed: {e}"))?,
        };
        let mut token = Vec::with_capacity(self.0.user.len() + password.len() + 2);
        token.push(0);
        token.extend_from_slice(self.0.user.as_bytes());
        token.push(0);
        token.extend_from_slice(password.as_bytes());
        Ok((Some(token), Box::new(NoChallenge)))
    }
}

struct ScyllaWire {
    session: ScyllaSession,
}

fn statement(cql: &str, options: &RunOptions) -> Statement {
    let mut st = Statement::new(cql);
    st.set_consistency(options.consistency);
    st.set_serial_consistency(Some(options.serial));
    st.set_request_timeout(Some(options.timeout));
    st.set_tracing(options.tracing);
    st
}

/// The values of `req` for the markers of `prepared`.
fn bound(
    prepared: &PreparedStatement,
    req: &Request<'_>,
) -> Result<Vec<Option<CqlValue>>, AppError> {
    let specs = prepared.get_variable_col_specs();
    if specs.len() != req.params.len() {
        return Err(AppError::Internal(format!(
            "Cassandra: the statement has {} markers for {} values",
            specs.len(),
            req.params.len()
        )));
    }
    specs
        .iter()
        .zip(req.params)
        .map(|(spec, v)| {
            encode(v, spec.typ()).map_err(|e| match e {
                AppError::InvalidInput(m) => {
                    AppError::InvalidInput(format!("\u{201c}{}\u{201d} is {m}", spec.name()))
                }
                other => other,
            })
        })
        .collect()
}

fn reply(result: QueryResult, paging: PagingStateResponse) -> Result<Reply, AppError> {
    let tracing_id = result.tracing_id();
    let warnings: Vec<String> = result.warnings().map(str::to_string).collect();
    if !result.is_rows() {
        return Ok(Reply {
            tracing_id,
            warnings,
            ..Reply::done()
        });
    }
    let rows = result
        .into_rows_result()
        .map_err(|e| AppError::db(format!("Cassandra: cannot read the result: {e}")))?;
    let columns = rows.column_specs().iter().map(result_column).collect();
    let cells = rows
        .rows::<Row>()
        .map_err(|e| AppError::db(format!("Cassandra: cannot read the rows: {e}")))?
        .map(|r| r.map(|row| row.columns))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::db(format!("Cassandra: cannot read a row: {e}")))?;
    Ok(Reply {
        is_rows: true,
        columns,
        rows: cells,
        paging,
        tracing_id,
        warnings,
    })
}

#[async_trait]
impl Wire for ScyllaWire {
    async fn run(&self, req: Request<'_>) -> Result<Reply, AppError> {
        let s = &self.session;
        if req.params.is_empty() {
            let mut st = statement(req.cql, &req.options);
            let (result, paging) = match req.page {
                Page::Unpaged => (
                    s.query_unpaged(st, ()).await.map_err(errors::execution)?,
                    PagingStateResponse::NoMorePages,
                ),
                Page::Single { state, size } => {
                    st.set_page_size(size);
                    s.query_single_page(st, (), state)
                        .await
                        .map_err(errors::execution)?
                }
            };
            return reply(result, paging);
        }
        let mut prepared = s
            .prepare(statement(req.cql, &req.options))
            .await
            .map_err(errors::prepare)?;
        prepared.set_consistency(req.options.consistency);
        prepared.set_serial_consistency(Some(req.options.serial));
        prepared.set_request_timeout(Some(req.options.timeout));
        prepared.set_tracing(req.options.tracing);
        let values = bound(&prepared, &req)?;
        let (result, paging) = match req.page {
            Page::Unpaged => (
                s.execute_unpaged(&prepared, values)
                    .await
                    .map_err(errors::execution)?,
                PagingStateResponse::NoMorePages,
            ),
            Page::Single { state, size } => {
                prepared.set_page_size(size);
                s.execute_single_page(&prepared, values, state)
                    .await
                    .map_err(errors::execution)?
            }
        };
        reply(result, paging)
    }

    async fn use_keyspace(&self, keyspace: &str, case_sensitive: bool) -> Result<(), AppError> {
        self.session
            .use_keyspace(keyspace, case_sensitive)
            .await
            .map_err(errors::use_keyspace)
    }

    async fn await_schema_agreement(&self) -> Result<(), AppError> {
        self.session
            .await_schema_agreement()
            .await
            .map(|_| ())
            .map_err(|e| {
                AppError::db(format!(
                    "Cassandra: the nodes did not agree on the schema: {e}"
                ))
            })
    }

    async fn tracing(&self, id: Uuid) -> Result<Vec<String>, AppError> {
        let info = self
            .session
            .get_tracing_info(&id)
            .await
            .map_err(|e| AppError::db(format!("Cassandra: cannot read the trace {id}: {e}")))?;
        let mut lines = vec![format!(
            "Tracing session {id}: {}{}{}",
            info.request.as_deref().unwrap_or("request"),
            info.coordinator
                .map(|c| format!(", coordinator {c}"))
                .unwrap_or_default(),
            info.duration
                .map(|d| format!(", {d} \u{b5}s"))
                .unwrap_or_default(),
        )];
        if let Some(at) = info.started_at {
            lines.push(format!("started at {}", timestamp_text(at.0)));
        }
        for event in &info.events {
            lines.push(format!(
                "{} | {} | {} \u{b5}s",
                event.activity.as_deref().unwrap_or(""),
                event.source.map(|s| s.to_string()).unwrap_or_default(),
                event.source_elapsed.unwrap_or_default()
            ));
        }
        Ok(lines)
    }
}
