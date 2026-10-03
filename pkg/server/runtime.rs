// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::net::{Shutdown, SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};
use std::{
    io::{Read, Write},
    mem,
};

use rustls::{ServerConnection, StreamOwned};

use astersql_domain::Domain;
use astersql_session::runtime::{
    CONCRETE_NULL_VALUE, CanonicalSessionFactory, ConcretePreparedArgument, ConcreteProtocolState,
    ConcreteResultField, ConcreteSession, SplitSQLStatements,
};
use astersql_session_sessmgr::Manager as SessionManager;
use astersql_util_sqlkiller::sqlkiller::{QueryInterrupted, SQLKiller};

use crate::conn::{
    AUTH_NATIVE_PASSWORD, AuthRequest, CancellationToken, ColumnInfo, Command,
    CompressionAlgorithm, ConnError, ConnResult, ConnectionDomain, PacketIo, PreparedMetadata,
    QueryResult, ResponseLifecycle, SessionDriver, SessionProcessSnapshot, SessionState,
    TiDBContext, TlsState, Value,
};
use crate::server::{Domain as ServerDomain, ServerDriver};

const MYSQL_TYPE_VAR_STRING: u8 = 0xfd;
const DEFAULT_COLUMN_LENGTH: u32 = 1024;

fn packet_error(error: impl ToString) -> ConnError {
    ConnError::Io(error.to_string())
}

/// Finish the local write side even when a peer half-close makes `Both`
/// report `NotConnected` (notably on macOS). PacketIO retains a cloned read
/// descriptor, so relying on descriptor drop would delay the peer's EOF.
fn shutdown_tcp(stream: &TcpStream) -> std::io::Result<()> {
    match stream.shutdown(Shutdown::Both) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotConnected => {
            match stream.shutdown(Shutdown::Write) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotConnected => Ok(()),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// TCP transport for the existing MySQL packet codec.
pub struct TcpPacketIo {
    stream: TcpStream,
    packet: astersql_server_internal::PacketIO,
    tls_config: Option<Arc<rustls::ServerConfig>>,
    tls_stream: Option<Arc<Mutex<StreamOwned<ServerConnection, TcpStream>>>>,
    peer_addr: Option<SocketAddr>,
    proxy_protocol: Option<(String, bool, Duration)>,
    proxy_protocol_checked: bool,
    alive: bool,
    closed: bool,
}

impl TcpPacketIo {
    pub fn new(stream: TcpStream, max_allowed_packet: u64) -> ConnResult<Self> {
        Self::new_with_options(stream, max_allowed_packet, None, None, None)
    }

    pub fn new_with_options(
        stream: TcpStream,
        max_allowed_packet: u64,
        tls_config: Option<Arc<rustls::ServerConfig>>,
        peer_addr: Option<SocketAddr>,
        proxy_protocol: Option<(String, bool, Duration)>,
    ) -> ConnResult<Self> {
        let reader = stream.try_clone().map_err(packet_error)?;
        Ok(Self {
            stream,
            packet: astersql_server_internal::PacketIO::new_packet_io(
                Box::new(reader),
                max_allowed_packet,
            ),
            tls_config,
            tls_stream: None,
            peer_addr,
            proxy_protocol,
            proxy_protocol_checked: false,
            alive: true,
            closed: false,
        })
    }

    fn address_parts(address: SocketAddr) -> (String, String) {
        (address.ip().to_string(), address.port().to_string())
    }

    fn fail<T>(&mut self, error: impl ToString) -> ConnResult<T> {
        self.alive = false;
        Err(packet_error(error))
    }
}

impl PacketIo for TcpPacketIo {
    fn read_packet(&mut self) -> ConnResult<Vec<u8>> {
        if !self.proxy_protocol_checked {
            self.proxy_protocol_checked = true;
            if let Some((networks, fallbackable, timeout)) = &self.proxy_protocol {
                let peer = self.stream.peer_addr().map_err(packet_error)?;
                self.peer_addr = crate::server::proxy_source_addr(
                    &mut self.stream,
                    peer,
                    networks,
                    *fallbackable,
                    *timeout,
                )
                .map_err(packet_error)?;
            }
        }
        match self.packet.read_packet() {
            Ok(packet) => Ok(packet),
            Err(error) => self.fail(error),
        }
    }

    fn write_packet(&mut self, payload: &[u8]) -> ConnResult<()> {
        let mut packet = vec![0_u8; 4];
        packet.extend_from_slice(payload);
        self.packet.write_packet(&mut packet).map_err(packet_error)
    }

    fn flush(&mut self) -> ConnResult<()> {
        self.packet.flush().map_err(packet_error)?;
        let encoded = self.packet.take_written_data();
        let result = if let Some(stream) = &self.tls_stream {
            stream
                .lock()
                .map_err(|_| std::io::Error::other("TLS stream lock poisoned"))
                .and_then(|mut stream| stream.write_all(&encoded).and_then(|_| stream.flush()))
        } else {
            self.stream
                .write_all(&encoded)
                .and_then(|_| self.stream.flush())
        };
        if let Err(error) = result {
            return self.fail(error);
        }
        Ok(())
    }

    fn reset_sequence(&mut self) {
        self.packet.reset_sequence();
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> ConnResult<()> {
        let timeout = (!timeout.is_zero()).then_some(timeout);
        self.stream
            .set_read_timeout(timeout)
            .map_err(packet_error)?;
        self.packet.set_read_timeout(timeout.unwrap_or_default());
        Ok(())
    }

    fn set_compression(&mut self, algorithm: CompressionAlgorithm, zstd_level: i32) {
        let result = match algorithm {
            CompressionAlgorithm::None => Ok(()),
            CompressionAlgorithm::Zlib => self
                .packet
                .set_compression_algorithm(astersql_server_internal::COMPRESSION_ZLIB),
            CompressionAlgorithm::Zstd => {
                self.packet.set_zstd_level(zstd_level);
                self.packet
                    .set_compression_algorithm(astersql_server_internal::COMPRESSION_ZSTD)
            }
        };
        if result.is_err() {
            self.alive = false;
        }
    }

    fn upgrade_to_tls(&mut self) -> ConnResult<TlsState> {
        let config = self.tls_config.take().ok_or_else(|| {
            ConnError::Session("SQL TLS is not configured for this listener".to_owned())
        })?;
        let mut connection = ServerConnection::new(config).map_err(packet_error)?;
        let mut stream = self.stream.try_clone().map_err(packet_error)?;
        while connection.is_handshaking() {
            connection.complete_io(&mut stream).map_err(packet_error)?;
        }
        let version = connection
            .protocol_version()
            .map(u16::from)
            .unwrap_or_default();
        let cipher_suite = connection
            .negotiated_cipher_suite()
            .map(|suite| u16::from(suite.suite()))
            .unwrap_or_default();
        let shared = Arc::new(Mutex::new(StreamOwned::new(connection, stream)));
        self.packet
            .set_buffered_read_conn(Box::new(SharedTlsReader(Arc::clone(&shared))));
        self.tls_stream = Some(shared);
        Ok(TlsState {
            version,
            cipher_suite,
        })
    }

    fn peer_addr(&self) -> ConnResult<(String, String)> {
        self.peer_addr
            .map(Ok)
            .unwrap_or_else(|| self.stream.peer_addr())
            .map(Self::address_parts)
            .map_err(packet_error)
    }

    fn local_addr(&self) -> ConnResult<(String, String)> {
        self.stream
            .local_addr()
            .map(Self::address_parts)
            .map_err(packet_error)
    }

    fn connection_alive(&self) -> bool {
        self.alive
    }

    fn close_handle(&self) -> Option<crate::conn::PacketCloseHandle> {
        let stream = self.stream.try_clone().ok()?;
        Some(Arc::new(move || {
            let _ = shutdown_tcp(&stream);
        }))
    }

    fn close(&mut self) -> ConnResult<()> {
        self.alive = false;
        if mem::replace(&mut self.closed, true) {
            return Ok(());
        }
        shutdown_tcp(&self.stream).map_err(packet_error)
    }
}

struct SharedTlsReader(Arc<Mutex<StreamOwned<ServerConnection, TcpStream>>>);

impl Read for SharedTlsReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("TLS stream lock poisoned"))?
            .read(buffer)
    }
}

/// Unix-domain transport for the MySQL packet codec.
#[cfg(unix)]
pub struct UnixPacketIo {
    stream: UnixStream,
    packet: astersql_server_internal::PacketIO,
    alive: bool,
    closed: bool,
}

#[cfg(unix)]
impl UnixPacketIo {
    pub fn new(stream: UnixStream, max_allowed_packet: u64) -> ConnResult<Self> {
        let reader = stream.try_clone().map_err(packet_error)?;
        Ok(Self {
            stream,
            packet: astersql_server_internal::PacketIO::new_packet_io(
                Box::new(reader),
                max_allowed_packet,
            ),
            alive: true,
            closed: false,
        })
    }

    fn fail<T>(&mut self, error: impl ToString) -> ConnResult<T> {
        self.alive = false;
        Err(packet_error(error))
    }
}

#[cfg(unix)]
impl PacketIo for UnixPacketIo {
    fn read_packet(&mut self) -> ConnResult<Vec<u8>> {
        match self.packet.read_packet() {
            Ok(packet) => Ok(packet),
            Err(error) => self.fail(error),
        }
    }

    fn write_packet(&mut self, payload: &[u8]) -> ConnResult<()> {
        let mut packet = vec![0_u8; 4];
        packet.extend_from_slice(payload);
        self.packet.write_packet(&mut packet).map_err(packet_error)
    }

    fn flush(&mut self) -> ConnResult<()> {
        self.packet.flush().map_err(packet_error)?;
        let encoded = self.packet.take_written_data();
        if let Err(error) = self
            .stream
            .write_all(&encoded)
            .and_then(|_| self.stream.flush())
        {
            return self.fail(error);
        }
        Ok(())
    }

    fn reset_sequence(&mut self) {
        self.packet.reset_sequence();
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> ConnResult<()> {
        let timeout = (!timeout.is_zero()).then_some(timeout);
        self.stream
            .set_read_timeout(timeout)
            .map_err(packet_error)?;
        self.packet.set_read_timeout(timeout.unwrap_or_default());
        Ok(())
    }

    fn set_compression(&mut self, algorithm: CompressionAlgorithm, zstd_level: i32) {
        let result = match algorithm {
            CompressionAlgorithm::None => Ok(()),
            CompressionAlgorithm::Zlib => self
                .packet
                .set_compression_algorithm(astersql_server_internal::COMPRESSION_ZLIB),
            CompressionAlgorithm::Zstd => {
                self.packet.set_zstd_level(zstd_level);
                self.packet
                    .set_compression_algorithm(astersql_server_internal::COMPRESSION_ZSTD)
            }
        };
        if result.is_err() {
            self.alive = false;
        }
    }

    fn upgrade_to_tls(&mut self) -> ConnResult<TlsState> {
        Err(ConnError::Session(
            "SQL TLS is not configured for this listener".to_owned(),
        ))
    }

    fn peer_addr(&self) -> ConnResult<(String, String)> {
        Ok(("localhost".to_owned(), String::new()))
    }

    fn remote_addr(&self) -> ConnResult<String> {
        self.stream
            .peer_addr()
            .map(|address| {
                address
                    .as_pathname()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .map_err(packet_error)
    }

    fn local_addr(&self) -> ConnResult<(String, String)> {
        Ok(("localhost".to_owned(), String::new()))
    }

    fn connection_alive(&self) -> bool {
        self.alive
    }

    fn close_handle(&self) -> Option<crate::conn::PacketCloseHandle> {
        let stream = self.stream.try_clone().ok()?;
        Some(Arc::new(move || {
            let _ = stream.shutdown(Shutdown::Both);
        }))
    }

    fn close(&mut self) -> ConnResult<()> {
        self.alive = false;
        if mem::replace(&mut self.closed, true) {
            return Ok(());
        }
        match self.stream.shutdown(Shutdown::Both) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotConnected => Ok(()),
            Err(error) => Err(packet_error(error)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapAuthMode {
    InsecureRootOnly,
    SecureUnsupported,
}

pub struct CanonicalConnectionDomain {
    domain: Arc<Domain>,
}

/// canonical Domain 在 Server 生命周期接口上的只读适配。
pub struct CanonicalServerDomain {
    domain: Arc<Domain>,
    start_timestamp: i64,
    extract_runtime: Arc<crate::extract_runtime::CanonicalExtractRuntime>,
}

impl CanonicalServerDomain {
    pub fn new(domain: Arc<Domain>) -> Self {
        let start_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self {
            extract_runtime: Arc::new(crate::extract_runtime::CanonicalExtractRuntime::new(
                Arc::clone(&domain),
            )),
            domain,
            start_timestamp,
        }
    }

    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }
}

impl ServerDomain for CanonicalServerDomain {
    fn dxf_history_available(&self) -> bool {
        self.domain.storage().with_storage(astersql_kv::IsSystemKS)
    }

    fn list_dxf_history(
        &self,
        page_size: i32,
        page_token: i64,
        keyspace: &str,
    ) -> Option<Result<serde_json::Value, String>> {
        Some((|| {
            let session = ConcreteSession::new(self.domain.clone());
            let manager = session
                .ImportTaskManager()
                .map_err(|error| error.to_string())?;
            let page = manager
                .ListHistoryTasks((), page_size, page_token, keyspace.to_owned())
                .map_err(|error| error.to_string())?;
            let timestamp = |value: SystemTime| {
                if value == std::time::UNIX_EPOCH {
                    "0001-01-01T00:00:00Z".to_owned()
                } else {
                    chrono::DateTime::<chrono::Local>::from(value)
                        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
                }
            };
            let items: Vec<_> = page.Items.into_iter().map(|item| {
                let base = item.TaskBase;
                serde_json::json!({
                    "ID": base.ID, "Key": base.Key, "Type": base.Type,
                    "State": base.State, "Step": base.Step, "Priority": base.Priority,
                    "RequiredSlots": base.RequiredSlots, "CreateTime": timestamp(base.CreateTime),
                    "TargetScope": base.TargetScope, "MaxNodeCount": base.MaxNodeCount,
                    "ExtraParams": base.ExtraParams, "Keyspace": base.Keyspace,
                    "ErrorCode": item.ErrorCode, "ErrorCategory": item.ErrorCategory,
                    "StartTime": timestamp(item.StartTime), "StateUpdateTime": timestamp(item.StateUpdateTime),
                    "EndTime": timestamp(item.EndTime),
                })
            }).collect();
            Ok(serde_json::json!({"Items": items, "HasMore": page.HasMore,
                "NextPageToken": page.NextPageToken, "ApproxTotalCount": page.ApproxTotalCount}))
        })())
    }

    fn extract_runtime(
        &self,
    ) -> Option<Arc<dyn astersql_server_handler_extractorhandler::extractor::ExtractRuntime>> {
        Some(self.extract_runtime.clone())
    }

    fn schema_snapshot(&self) -> Option<astersql_infoschema::SchemaRef> {
        Some(self.domain.info_schema())
    }

    fn publish_tiflash_replica_report(
        &self,
        table_id: i64,
        region_count: u64,
        flash_region_count: u64,
    ) -> Result<(), String> {
        if region_count == 0 || flash_region_count > region_count {
            return Err("invalid TiFlash region counts".into());
        }
        let schema = self.domain.info_schema();
        let table = schema
            .TableByID(table_id)
            .ok_or_else(|| format!("table {table_id} not found"))?;
        let database = schema
            .AllSchemas()
            .into_iter()
            .find(|database| {
                schema
                    .SchemaTableInfos(&database.name)
                    .is_ok_and(|tables| tables.iter().any(|candidate| candidate.id == table_id))
            })
            .ok_or_else(|| format!("schema for table {table_id} not found"))?;
        self.domain
            .publish_tiflash_replica_progress(
                &database.name.lower,
                &table.Meta().name.lower,
                table_id,
                flash_region_count as f64 / region_count as f64,
            )
            .map_err(|error| error.to_string())
    }

    fn server_id(&self) -> u64 {
        self.domain.server_id()
    }

    fn start_timestamp(&self) -> i64 {
        self.start_timestamp
    }

    fn system_process_list(
        &self,
    ) -> std::collections::HashMap<u64, Arc<astersql_session_sessmgr::ProcessInfo>> {
        self.domain
            .sys_processes()
            .process_list()
            .into_iter()
            .map(|(id, process)| {
                let command = astersql_parser_mysql::r#const::Command2Str
                    .iter()
                    .find_map(|&(value, name)| (name == process.command).then_some(value))
                    .unwrap_or(astersql_parser_mysql::r#const::ComSleep);
                (
                    id,
                    Arc::new(astersql_session_sessmgr::ProcessInfo {
                        Time: SystemTime::now(),
                        User: process.user,
                        DB: process.database,
                        Info: process.sql,
                        ID: process.id,
                        Command: command,
                        ..Default::default()
                    }),
                )
            })
            .collect()
    }

    fn kill_system_process(&self, connection_id: u64) {
        let _ = self.domain.sys_processes().kill(connection_id);
    }
}

/// canonical tidb-server 的 ServerDriver 标识。
pub struct CanonicalServerDriver;

impl ServerDriver for CanonicalServerDriver {
    fn name(&self) -> &str {
        "tidb"
    }
}

impl CanonicalConnectionDomain {
    pub fn new(domain: Arc<Domain>) -> Self {
        Self { domain }
    }

    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }
}

impl ConnectionDomain for CanonicalConnectionDomain {
    fn next_connection_id(&self) -> u64 {
        self.domain.next_connection_id()
    }

    fn release_connection_id(&self, connection_id: u64) {
        self.domain.release_connection_id(connection_id);
    }
}

pub struct ConcreteSessionDriver {
    domain: Arc<Domain>,
    auth_mode: BootstrapAuthMode,
    session_manager: RwLock<Option<Weak<dyn SessionManager>>>,
}

impl ConcreteSessionDriver {
    pub fn new(factory: Arc<CanonicalSessionFactory>, auth_mode: BootstrapAuthMode) -> Self {
        Self {
            domain: Arc::clone(factory.domain()),
            auth_mode,
            session_manager: RwLock::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(domain: Arc<Domain>, auth_mode: BootstrapAuthMode) -> Self {
        Self {
            domain,
            auth_mode,
            session_manager: RwLock::new(None),
        }
    }

    /// 使用已初始化的 canonical Domain 装配驱动，供嵌入式入口与集成测试复用。
    pub fn from_initialized_domain(domain: Arc<Domain>, auth_mode: BootstrapAuthMode) -> Self {
        Self {
            domain,
            auth_mode,
            session_manager: RwLock::new(None),
        }
    }

    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }
}

impl SessionDriver for ConcreteSessionDriver {
    fn open_ctx(
        &self,
        connection_id: u64,
        capability: u32,
        collation: u8,
        database: &str,
        _tls_state: Option<&TlsState>,
    ) -> ConnResult<Arc<dyn TiDBContext>> {
        let (request_tx, request_rx) = mpsc::channel();
        let request_tx = Arc::new(request_tx);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let domain = Arc::clone(&self.domain);
        let database = database.to_owned();
        let session_manager = self
            .session_manager
            .read()
            .map_err(|_| ConnError::Poisoned("session manager"))?
            .clone();
        let result_sender = Arc::downgrade(&request_tx);
        let worker = thread::Builder::new()
            .name(format!("mysql-session-{connection_id}"))
            .spawn(move || {
                run_session_worker(
                    domain,
                    connection_id,
                    capability,
                    collation,
                    database,
                    session_manager,
                    init_tx,
                    request_rx,
                    result_sender,
                );
            })
            .map_err(packet_error)?;
        let (cancellation, transaction_mdl) = match init_rx.recv().map_err(packet_error)? {
            Ok(cancellation) => cancellation,
            Err(error) => {
                let _ = worker.join();
                return Err(error);
            }
        };
        Ok(Arc::new(ConcreteTiDBContext {
            domain: Arc::clone(&self.domain),
            requests: Mutex::new(Some(request_tx)),
            worker: Mutex::new(Some(worker)),
            auth_mode: self.auth_mode,
            state: Mutex::new(SessionState {
                status: 0x0002,
                resource_group: "default".to_owned(),
                ..SessionState::default()
            }),
            connection_status: AtomicI32::new(0),
            compression: Mutex::new((CompressionAlgorithm::None, 0)),
            process_info: Mutex::new(SessionProcessSnapshot {
                sql: String::new(),
                command: astersql_parser_mysql::r#const::ComSleep,
                start_time: SystemTime::now(),
            }),
            last_statement: Mutex::new(String::new()),
            cancellation,
            transaction_mdl,
            cancel_requested: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }))
    }

    fn set_session_manager(&self, manager: Weak<dyn SessionManager>) {
        if let Some(manager) = manager.upgrade() {
            let coordinator: Arc<dyn astersql_session_sessmgr::InfoSchemaCoordinator> = manager;
            self.domain
                .set_schema_coordinator(Arc::downgrade(&coordinator));
        }
        *self
            .session_manager
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(manager);
    }
}

struct ConcreteTiDBContext {
    domain: Arc<Domain>,
    requests: Mutex<Option<Arc<mpsc::Sender<SessionRequest>>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    auth_mode: BootstrapAuthMode,
    state: Mutex<SessionState>,
    connection_status: AtomicI32,
    compression: Mutex<(CompressionAlgorithm, i32)>,
    process_info: Mutex<SessionProcessSnapshot>,
    last_statement: Mutex<String>,
    cancellation: Arc<SQLKiller>,
    transaction_mdl: Arc<astersql_session_sessmgr::TransactionMDL>,
    cancel_requested: AtomicBool,
    closed: AtomicBool,
}

pub(crate) enum SessionRequest {
    MaxAllowedPacket {
        response: mpsc::SyncSender<u64>,
    },
    UserIdentity {
        response: mpsc::SyncSender<String>,
    },
    SetAuthenticatedUser {
        username: String,
        has_process_privilege: bool,
        response: mpsc::SyncSender<ConnResult<()>>,
    },
    Execute {
        statements: Vec<String>,
        response: mpsc::SyncSender<ConnResult<Vec<QueryResult>>>,
    },
    FieldList {
        table: String,
        wildcard: String,
        response: mpsc::SyncSender<ConnResult<Vec<ColumnInfo>>>,
    },
    Prepare {
        sql: String,
        response: mpsc::SyncSender<ConnResult<PreparedMetadata>>,
    },
    ExecutePrepared {
        statement_id: u32,
        arguments: Vec<crate::conn_stmt::BinaryParam>,
        response: mpsc::SyncSender<ConnResult<QueryResult>>,
    },
    FinishProtocolResponse {
        write_duration: Duration,
        response: mpsc::SyncSender<()>,
    },
    ClosePrepared {
        statement_id: u32,
        response: mpsc::SyncSender<ConnResult<()>>,
    },
    Reset {
        response: mpsc::SyncSender<ConnResult<ConcreteProtocolState>>,
    },
    #[cfg(test)]
    WriteDuration {
        response: mpsc::SyncSender<Duration>,
    },
    #[cfg(test)]
    SetResultFault {
        operation: &'static str,
        at: usize,
        delay: Duration,
        fail: bool,
        response: mpsc::SyncSender<()>,
    },
    #[cfg(test)]
    ResultEvents {
        response: mpsc::SyncSender<Vec<String>>,
    },
    ExecuteLocalInfile {
        sql: String,
        data: Vec<u8>,
        response: mpsc::SyncSender<ConnResult<Vec<QueryResult>>>,
    },
    ExecuteStreaming {
        statements: Vec<String>,
        response: mpsc::SyncSender<ConnResult<Vec<QueryResult>>>,
    },
    ExecutePreparedStreaming {
        statement_id: u32,
        arguments: Vec<crate::conn_stmt::BinaryParam>,
        response: mpsc::SyncSender<ConnResult<QueryResult>>,
    },
    ResultOperation {
        id: u64,
        operation: super::protocol_result::Operation,
        response: mpsc::SyncSender<ConnResult<super::protocol_result::OperationResult>>,
    },
    Shutdown,
}

fn run_session_worker(
    domain: Arc<Domain>,
    connection_id: u64,
    capability: u32,
    collation: u8,
    database: String,
    session_manager: Option<Weak<dyn SessionManager>>,
    init: mpsc::SyncSender<
        ConnResult<(
            Arc<SQLKiller>,
            Arc<astersql_session_sessmgr::TransactionMDL>,
        )>,
    >,
    requests: mpsc::Receiver<SessionRequest>,
    result_sender: Weak<mpsc::Sender<SessionRequest>>,
) {
    let setup = (|| {
        let mut session = ConcreteSession::new(domain);
        if let Some(manager) = session_manager {
            session.SetSessionManager(manager);
        }
        session
            .configure_connection(connection_id, capability, collation)
            .map_err(|error| ConnError::Session(error.to_string()))?;
        if !database.is_empty() {
            session
                .execute(&format!("USE `{}`", database.replace('`', "``")))
                .map_err(|error| ConnError::Session(error.to_string()))?;
        }
        Ok::<_, ConnError>(session)
    })();
    let mut session = match setup {
        Ok(session) => session,
        Err(error) => {
            let _ = init.send(Err(error));
            return;
        }
    };
    let cancellation = session.cancellation_handle();
    if init
        .send(Ok((cancellation.clone(), session.transaction_mdl())))
        .is_err()
    {
        return;
    }
    let mut results = super::protocol_result::WorkerResults::new(result_sender);
    while let Ok(request) = requests.recv() {
        match request {
            SessionRequest::MaxAllowedPacket { response } => {
                let max = session.WithSessionVars(|vars| {
                    vars.GetSystemVar("max_allowed_packet")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(astersql_sessionctx_vardef::DefMaxAllowedPacket)
                });
                let _ = response.send(max);
            }
            SessionRequest::UserIdentity { response } => {
                let _ = response.send(session.authenticated_user_string());
            }
            #[cfg(test)]
            SessionRequest::WriteDuration { response } => {
                let _ = response.send(session.LastWriteSQLRespDurationForTest());
            }
            #[cfg(test)]
            SessionRequest::SetResultFault {
                operation,
                at,
                delay,
                fail,
                response,
            } => {
                results.set_fault(operation, at, delay, fail);
                let _ = response.send(());
            }
            #[cfg(test)]
            SessionRequest::ResultEvents { response } => {
                let _ = response.send(results.events());
            }
            SessionRequest::ExecuteLocalInfile {
                sql,
                data,
                response,
            } => {
                session.BeginProtocolResponse();
                let execution = session
                    .execute_with_load_data_reader(&sql, std::io::Cursor::new(data))
                    .map(|_| {
                        vec![QueryResult {
                            state: map_protocol_state(session.protocol_state()),
                            ..Default::default()
                        }]
                    })
                    .map_err(|error| ConnError::Session(error.to_string()));
                cancellation.Reset();
                let _ = response.send(execution);
            }
            SessionRequest::ExecuteStreaming {
                statements,
                response,
            } => {
                let _ = response.send(execute_on_session(
                    &session,
                    &cancellation,
                    u16::from(collation),
                    statements,
                    Some(&mut results),
                ));
            }
            SessionRequest::ExecutePreparedStreaming {
                statement_id,
                arguments,
                response,
            } => {
                let _ = response.send(execute_prepared_on_session(
                    &session,
                    &cancellation,
                    u16::from(collation),
                    statement_id,
                    &arguments,
                    Some(&mut results),
                ));
            }
            SessionRequest::ResultOperation {
                id,
                operation,
                response,
            } => {
                let _ = response.send(results.operate(id, operation));
            }
            SessionRequest::SetAuthenticatedUser {
                username,
                has_process_privilege,
                response,
            } => {
                session.SetAuthenticatedUser(username, has_process_privilege);
                let _ = response.send(Ok(()));
            }
            SessionRequest::Execute {
                statements,
                response,
            } => {
                let result = execute_on_session(
                    &session,
                    &cancellation,
                    u16::from(collation),
                    statements,
                    None,
                );
                let _ = response.send(result);
            }
            SessionRequest::FieldList {
                table,
                wildcard,
                response,
            } => {
                let _ = response.send(field_list_on_session(&session, &table, &wildcard));
            }
            SessionRequest::Prepare { sql, response } => {
                let result = session
                    .prepare_protocol_statement(&sql)
                    .and_then(|(statement_id, parameter_count, fields)| {
                        Ok(PreparedMetadata {
                            native_types: fields
                                .iter()
                                .map(|field| {
                                    let tp = &field.column.FieldType;
                                    crate::conn::NativeType {
                                        code: tp.GetType(),
                                        flags: tp.GetFlag(),
                                        length: tp.GetFlen(),
                                        decimal: tp.GetDecimal(),
                                    }
                                })
                                .collect(),
                            statement_id: u32::try_from(statement_id).map_err(|_| {
                                astersql_session::SessionError::new(
                                    "prepared statement id exceeds protocol width",
                                )
                            })?,
                            parameter_count,
                            columns: fields.iter().map(protocol_result_column).collect(),
                        })
                    })
                    .map_err(|error| ConnError::Session(error.to_string()));
                let _ = response.send(result);
            }
            SessionRequest::ExecutePrepared {
                statement_id,
                arguments,
                response,
            } => {
                let result = execute_prepared_on_session(
                    &session,
                    &cancellation,
                    u16::from(collation),
                    statement_id,
                    &arguments,
                    None,
                );
                let _ = response.send(result);
            }
            SessionRequest::FinishProtocolResponse {
                write_duration,
                response,
            } => {
                session.FinishProtocolResponse(write_duration);
                let _ = response.send(());
            }
            SessionRequest::ClosePrepared {
                statement_id,
                response,
            } => {
                let result = session
                    .close_protocol_statement(u64::from(statement_id))
                    .map_err(|error| ConnError::Session(error.to_string()));
                let _ = response.send(result);
            }
            SessionRequest::Reset { response } => {
                let result = session
                    .reset_connection()
                    .map(|()| session.protocol_state())
                    .map_err(|error| ConnError::Session(error.to_string()));
                let _ = response.send(result);
            }
            SessionRequest::Shutdown => break,
        }
    }
}

fn render_default_value(value: &astersql_server_internal_column::DefaultValue) -> Option<Vec<u8>> {
    use astersql_server_internal_column::DefaultValue;

    match value {
        DefaultValue::Bool(value) => Some(if *value { b"1".to_vec() } else { b"0".to_vec() }),
        DefaultValue::Int(value) => Some(value.to_string().into_bytes()),
        DefaultValue::Uint(value) => Some(value.to_string().into_bytes()),
        DefaultValue::Float(value) => Some(value.to_string().into_bytes()),
        DefaultValue::String(value)
            if value == b"CURRENT_TIMESTAMP" || value == b"CURRENT_DATE" =>
        {
            None
        }
        DefaultValue::String(value) => Some(value.clone()),
    }
}

fn protocol_column(field: &astersql_planner_core_resolve::ResultField) -> ColumnInfo {
    let info = astersql_server_internal_column::ConvertColumnInfo(field);
    ColumnInfo {
        schema: info.Schema,
        table: info.Table,
        org_table: info.OrgTable,
        name: info.Name,
        org_name: info.OrgName,
        charset: info.Charset,
        column_length: info.ColumnLength,
        column_type: astersql_server_internal_column::dumpType(info.Type),
        flags: astersql_server_internal_column::DumpFlag(info.Type, info.Flag),
        decimals: info.Decimal,
        default_value: info.DefaultValue.as_ref().and_then(render_default_value),
    }
}

fn protocol_result_column(field: &ConcreteResultField) -> ColumnInfo {
    let resolved = astersql_planner_core_resolve::ResultField {
        column: Some(Rc::new(field.column.clone())),
        column_as_name: field.column_as_name.clone(),
        empty_org_name: false,
        table: None,
        table_as_name: field.table_as_name.clone(),
        db_name: field.db_name.clone(),
    };
    let mut column = protocol_column(&resolved);
    column.org_table = field.table_name.O.clone();
    column
}

fn field_list_on_session(
    session: &ConcreteSession,
    table: &str,
    _wildcard: &str,
) -> ConnResult<Vec<ColumnInfo>> {
    session
        .field_list(table)
        .map(|fields| fields.iter().map(protocol_column).collect())
        .map_err(|error| ConnError::Session(error.to_string()))
}

fn execute_on_session(
    session: &ConcreteSession,
    cancellation: &SQLKiller,
    collation: u16,
    statements: Vec<String>,
    mut streaming: Option<&mut super::protocol_result::WorkerResults>,
) -> ConnResult<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());
    for statement in statements {
        session.BeginProtocolResponse();
        let execution = session
            .execute(&statement)
            .map_err(|error| ConnError::Session(error.to_string()));
        cancellation.Reset();
        let record_sets = execution?;
        let state = map_protocol_state(session.protocol_state());
        if record_sets.is_empty() {
            results.push(QueryResult {
                state,
                ..QueryResult::default()
            });
        } else {
            for record_set in record_sets {
                let mut result = if let Some(streaming) = streaming.as_deref_mut() {
                    {
                        let (initial, maximum) = protocol_chunk_sizes(session);
                        streaming.register(
                            record_set,
                            state.clone(),
                            collation,
                            initial,
                            maximum,
                        )?
                    }
                } else {
                    result_from_record_set(record_set, state.clone(), collation)?
                };
                // Preserve engine metadata separately from existing MySQL wire columns.
                // Metadata unavailable from the native resolver remains explicit.
                if !result.columns.is_empty()
                    && let Ok(fields) = session.describe_result_fields(&statement)
                {
                    result.native_types = fields
                        .iter()
                        .map(|field| {
                            let tp = &field.column.FieldType;
                            crate::conn::NativeType {
                                code: tp.GetType(),
                                flags: tp.GetFlag(),
                                length: tp.GetFlen(),
                                decimal: tp.GetDecimal(),
                            }
                        })
                        .collect();
                }
                results.push(result);
            }
        }
    }
    Ok(results)
}

fn prepared_argument(
    parameter: &crate::conn_stmt::BinaryParam,
) -> ConnResult<ConcretePreparedArgument> {
    if parameter.is_null || parameter.tp == 0x06 {
        return Ok(ConcretePreparedArgument::Null);
    }
    let fixed = |length: usize| {
        parameter
            .value
            .get(..length)
            .ok_or(ConnError::MalformedPacket("prepared parameter value"))
    };
    Ok(match parameter.tp {
        0x01 => {
            let value = fixed(1)?[0];
            if parameter.unsigned {
                ConcretePreparedArgument::Unsigned(u64::from(value))
            } else {
                ConcretePreparedArgument::Signed(i64::from(value as i8))
            }
        }
        0x02 | 0x0d => {
            let bytes: [u8; 2] = fixed(2)?.try_into().expect("two-byte parameter");
            if parameter.unsigned {
                ConcretePreparedArgument::Unsigned(u64::from(u16::from_le_bytes(bytes)))
            } else {
                ConcretePreparedArgument::Signed(i64::from(i16::from_le_bytes(bytes)))
            }
        }
        0x03 | 0x09 => {
            let bytes: [u8; 4] = fixed(4)?.try_into().expect("four-byte parameter");
            if parameter.unsigned {
                ConcretePreparedArgument::Unsigned(u64::from(u32::from_le_bytes(bytes)))
            } else {
                ConcretePreparedArgument::Signed(i64::from(i32::from_le_bytes(bytes)))
            }
        }
        0x08 => {
            let bytes: [u8; 8] = fixed(8)?.try_into().expect("eight-byte parameter");
            if parameter.unsigned {
                ConcretePreparedArgument::Unsigned(u64::from_le_bytes(bytes))
            } else {
                ConcretePreparedArgument::Signed(i64::from_le_bytes(bytes))
            }
        }
        0x04 => {
            let bytes: [u8; 4] = fixed(4)?.try_into().expect("float parameter");
            ConcretePreparedArgument::Float(f64::from(f32::from_le_bytes(bytes)))
        }
        0x05 => {
            let bytes: [u8; 8] = fixed(8)?.try_into().expect("double parameter");
            ConcretePreparedArgument::Float(f64::from_le_bytes(bytes))
        }
        0xf6 => ConcretePreparedArgument::Decimal(
            String::from_utf8(parameter.value.clone())
                .map_err(|_| ConnError::MalformedPacket("DECIMAL parameter is not UTF-8"))?,
        ),
        0x0f | 0xfd | 0xfe => ConcretePreparedArgument::Text(
            String::from_utf8(parameter.value.clone())
                .map_err(|_| ConnError::MalformedPacket("text parameter is not UTF-8"))?,
        ),
        0x07 | 0x0a | 0x0b | 0x0c => ConcretePreparedArgument::Temporal(decode_binary_temporal(
            parameter.tp,
            &parameter.value,
        )?),
        0x00 | 0x10 | 0xf7..=0xfc | 0xff => {
            ConcretePreparedArgument::Bytes(parameter.value.clone())
        }
        tp => {
            return Err(ConnError::Session(format!(
                "prepared parameter type {tp} is not supported"
            )));
        }
    })
}

fn decode_binary_temporal(tp: u8, value: &[u8]) -> ConnResult<String> {
    if tp == 0x0b {
        if value.is_empty() {
            return Ok("00:00:00".to_owned());
        }
        if !matches!(value.len(), 8 | 12) {
            return Err(ConnError::MalformedPacket("TIME parameter length"));
        }
        let negative = value[0] != 0;
        let days = u32::from_le_bytes(value[1..5].try_into().expect("TIME days"));
        let hours = days * 24 + u32::from(value[5]);
        let mut rendered = format!(
            "{}{hours:02}:{:02}:{:02}",
            if negative { "-" } else { "" },
            value[6],
            value[7]
        );
        if value.len() == 12 {
            let micros = u32::from_le_bytes(value[8..12].try_into().expect("TIME micros"));
            rendered.push_str(&format!(".{micros:06}"));
        }
        return Ok(rendered);
    }
    if value.is_empty() {
        return Ok("0000-00-00".to_owned());
    }
    if !matches!(value.len(), 4 | 7 | 11) {
        return Err(ConnError::MalformedPacket("date/time parameter length"));
    }
    let year = u16::from_le_bytes(value[0..2].try_into().expect("date year"));
    let date = format!("{year:04}-{:02}-{:02}", value[2], value[3]);
    if tp == 0x0a || value.len() == 4 {
        return Ok(date);
    }
    let mut rendered = format!("{date} {:02}:{:02}:{:02}", value[4], value[5], value[6]);
    if value.len() == 11 {
        let micros = u32::from_le_bytes(value[7..11].try_into().expect("datetime micros"));
        rendered.push_str(&format!(".{micros:06}"));
    }
    Ok(rendered)
}

fn execute_prepared_on_session(
    session: &ConcreteSession,
    cancellation: &SQLKiller,
    collation: u16,
    statement_id: u32,
    arguments: &[crate::conn_stmt::BinaryParam],
    streaming: Option<&mut super::protocol_result::WorkerResults>,
) -> ConnResult<QueryResult> {
    let arguments = arguments
        .iter()
        .map(prepared_argument)
        .collect::<ConnResult<Vec<_>>>()?;
    session.BeginProtocolResponse();
    let execution = session
        .execute_protocol_statement(u64::from(statement_id), &arguments)
        .map_err(|error| ConnError::Session(error.to_string()));
    cancellation.Reset();
    let mut record_sets = execution?;
    if record_sets.len() > 1 {
        return Err(ConnError::Session(
            "prepared SQL returned multiple result sets".to_owned(),
        ));
    }
    let state = map_protocol_state(session.protocol_state());
    match record_sets.pop() {
        Some(record_set) => match streaming {
            Some(streaming) => {
                let (initial, maximum) = protocol_chunk_sizes(session);
                streaming.register(record_set, state, collation, initial, maximum)
            }
            None => result_from_record_set(record_set, state, collation),
        },
        None => Ok(QueryResult {
            state,
            ..QueryResult::default()
        }),
    }
}

fn map_protocol_state(state: ConcreteProtocolState) -> SessionState {
    SessionState {
        affected_rows: state.affected_rows,
        last_insert_id: state.last_insert_id,
        warning_count: state.warning_count,
        status: state.status,
        resource_group: "default".to_owned(),
        ..SessionState::default()
    }
}

fn result_from_record_set(
    mut record_set: astersql_session::runtime::ConcreteRecordSet,
    state: SessionState,
    collation: u16,
) -> ConnResult<QueryResult> {
    let mut result = result_metadata(&record_set, state, collation);
    let mut rows = Vec::new();
    while let Some(row) = record_set
        .next_row()
        .map_err(|error| ConnError::Session(error.to_string()))?
    {
        rows.push(row.into_iter().map(protocol_value).collect());
    }
    record_set
        .close()
        .map_err(|error| ConnError::Session(error.to_string()))?;
    result.rows = rows;
    Ok(result)
}

fn protocol_chunk_sizes(session: &ConcreteSession) -> (usize, usize) {
    session.WithSessionVars(|vars| {
        let initial = vars
            .GetSystemVar("tidb_init_chunk_size")
            .and_then(|value| value.parse().ok())
            .unwrap_or(32);
        let maximum = vars
            .GetSystemVar("tidb_max_chunk_size")
            .and_then(|value| value.parse().ok())
            .unwrap_or(1024);
        (initial, maximum)
    })
}

pub(crate) fn result_metadata(
    record_set: &astersql_session::runtime::ConcreteRecordSet,
    state: SessionState,
    collation: u16,
) -> QueryResult {
    let native_types = record_set
        .result_fields()
        .iter()
        .map(|field| {
            field.as_ref().map(|field| {
                let tp = &field.column.FieldType;
                crate::conn::NativeType {
                    code: tp.GetType(),
                    flags: tp.GetFlag(),
                    length: tp.GetFlen(),
                    decimal: tp.GetDecimal(),
                }
            })
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    let columns = record_set
        .columns()
        .iter()
        .zip(record_set.result_fields())
        .map(|(name, field)| {
            field
                .as_ref()
                .map(protocol_result_column)
                .unwrap_or_else(|| ColumnInfo {
                    schema: String::new(),
                    table: String::new(),
                    org_table: String::new(),
                    name: name.clone(),
                    org_name: String::new(),
                    charset: collation,
                    column_length: DEFAULT_COLUMN_LENGTH,
                    column_type: MYSQL_TYPE_VAR_STRING,
                    flags: 0,
                    decimals: 0,
                    default_value: None,
                })
        })
        .collect();
    QueryResult {
        native_types,
        columns,
        state,
        ..QueryResult::default()
    }
}

pub(crate) fn protocol_value(value: String) -> Value {
    const PREFIX: &str = "__astersql_binary_hex__:";
    // Canonical record sets use the internal sentinel for newer
    // paths, while legacy relational rows still render SQL NULL
    // as `<nil>`. Normalize both at the protocol boundary so the
    // text and binary encoders emit MySQL NULL, never literal text.
    if value == CONCRETE_NULL_VALUE || value == "<nil>" {
        return Value::Null;
    }
    let Some(hex) = value.strip_prefix(PREFIX) else {
        return Value::Text(value);
    };
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Value::Text(value);
    }
    let bytes = hex
        .as_bytes()
        .chunks(2)
        .map(|chunk| {
            let high = (chunk[0] as char).to_digit(16).unwrap_or_default();
            let low = (chunk[1] as char).to_digit(16).unwrap_or_default();
            ((high << 4) | low) as u8
        })
        .collect();
    Value::Bytes(bytes)
}

impl ConcreteTiDBContext {
    fn send_request(&self, request: SessionRequest) -> ConnResult<()> {
        self.requests
            .lock()
            .map_err(|_| ConnError::Poisoned("session requests"))?
            .as_ref()
            .ok_or_else(|| ConnError::Session("session is closed".to_owned()))?
            .send(request)
            .map_err(packet_error)
    }

    fn ensure_not_cancelled(&self, cancel: &CancellationToken) -> ConnResult<()> {
        if cancel.is_cancelled() || self.cancel_requested.swap(false, Ordering::AcqRel) {
            return Err(ConnError::Session(
                "query execution was interrupted".to_owned(),
            ));
        }
        Ok(())
    }

    fn response_lifecycle(&self) -> ConnResult<Arc<ResponseLifecycle>> {
        let sender = self
            .requests
            .lock()
            .map_err(|_| ConnError::Poisoned("session requests"))?
            .as_ref()
            .cloned()
            .ok_or_else(|| ConnError::Session("session is closed".to_owned()))?;
        Ok(ResponseLifecycle::new(move |write_duration| {
            let (response_tx, response_rx) = mpsc::sync_channel(1);
            if sender
                .send(SessionRequest::FinishProtocolResponse {
                    write_duration,
                    response: response_tx,
                })
                .is_ok()
            {
                let _ = response_rx.recv();
            }
        }))
    }
}

impl TiDBContext for ConcreteTiDBContext {
    fn max_allowed_packet(&self) -> ConnResult<u64> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::MaxAllowedPacket { response: tx })?;
        rx.recv().map_err(packet_error)
    }
    fn user_identity(&self) -> ConnResult<String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::UserIdentity { response: tx })?;
        rx.recv().map_err(packet_error)
    }

    fn schema_snapshot(&self) -> Option<astersql_infoschema::SchemaRef> {
        Some(self.domain.info_schema())
    }

    fn transaction_mdl(&self) -> Option<Arc<astersql_session_sessmgr::TransactionMDL>> {
        Some(self.transaction_mdl.clone())
    }

    fn state(&self) -> SessionState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    fn set_connection_status(&self, status: i32) {
        self.connection_status.store(status, Ordering::Release);
    }

    fn wait_timeout(&self) -> Duration {
        Duration::from_secs(28_800)
    }

    fn in_transaction(&self) -> bool {
        self.state().status & 0x0001 != 0
    }

    fn auth_plugin_for_user(&self, _user: &str, _host: &str) -> ConnResult<String> {
        Ok(AUTH_NATIVE_PASSWORD.to_owned())
    }

    fn authenticate(&self, request: &AuthRequest) -> ConnResult<()> {
        if self.auth_mode == BootstrapAuthMode::SecureUnsupported {
            return Err(ConnError::Session(
                "secure bootstrap authentication is not implemented".to_owned(),
            ));
        }
        let allowed = self.auth_mode == BootstrapAuthMode::InsecureRootOnly
            && request.identity.username == "root"
            && request.auth_data.is_empty()
            && request.identity.plugin == AUTH_NATIVE_PASSWORD;
        if allowed {
            let (response_tx, response_rx) = mpsc::sync_channel(1);
            self.send_request(SessionRequest::SetAuthenticatedUser {
                username: request.identity.username.clone(),
                // InsecureRootOnly admits only root, matching PROCESS privilege.
                has_process_privilege: true,
                response: response_tx,
            })?;
            response_rx.recv().map_err(packet_error)?
        } else {
            Err(ConnError::AccessDenied {
                user: request.identity.username.clone(),
                host: request.identity.hostname.clone(),
            })
        }
    }

    fn set_compression(&self, algorithm: CompressionAlgorithm, zstd_level: i32) {
        if let Ok(mut compression) = self.compression.lock() {
            *compression = (algorithm, zstd_level);
        }
    }

    fn set_process_info(&self, sql: &str, command: u8) {
        if let Ok(mut process_info) = self.process_info.lock() {
            *process_info = SessionProcessSnapshot {
                sql: sql.to_owned(),
                command,
                start_time: SystemTime::now(),
            };
        }
    }

    fn clear_process_info(&self) {
        if let Ok(mut process_info) = self.process_info.lock() {
            *process_info = SessionProcessSnapshot {
                sql: String::new(),
                command: astersql_parser_mysql::r#const::ComSleep,
                start_time: SystemTime::now(),
            };
        }
    }

    fn is_connection_admin(&self) -> bool {
        true
    }

    fn execute_init_connect(&self, sql: &str) -> ConnResult<()> {
        if sql.trim().is_empty() {
            return Ok(());
        }
        self.execute_query(sql, true, &CancellationToken::new())
            .map(|_| ())
    }

    fn use_db(&self, db: &str, cancel: &CancellationToken) -> ConnResult<()> {
        self.ensure_not_cancelled(cancel)?;
        let sql = format!("USE `{}`", db.replace('`', "``"));
        self.execute_query(&sql, false, cancel).map(|_| ())
    }

    fn execute_query(
        &self,
        sql: &str,
        allow_multi_statements: bool,
        cancel: &CancellationToken,
    ) -> ConnResult<Vec<QueryResult>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnError::Session("session is closed".to_owned()));
        }
        self.ensure_not_cancelled(cancel)?;
        let statements =
            SplitSQLStatements(sql).map_err(|error| ConnError::Session(error.to_string()))?;
        if !allow_multi_statements && statements.len() > 1 {
            return Err(ConnError::Session(
                "multi-statement execution is disabled".to_owned(),
            ));
        }
        if let Some(statement) = statements.last() {
            *self
                .last_statement
                .lock()
                .map_err(|_| ConnError::Poisoned("last statement"))? = statement.clone();
        }
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::Execute {
            statements,
            response: response_tx,
        })?;
        let mut results = response_rx.recv().map_err(packet_error)??;
        for result in &mut results {
            result.response_lifecycle = Some(self.response_lifecycle()?);
        }
        if let Some(result) = results.last() {
            *self
                .state
                .lock()
                .map_err(|_| ConnError::Poisoned("session state"))? = result.state.clone();
        }
        Ok(results)
    }

    fn local_infile_path(&self, sql: &str) -> ConnResult<Option<String>> {
        // Avoid a second parse for ordinary queries. This is only a fast rejection;
        // the parser below determines the statement and file location, including comments.
        if !sql
            .as_bytes()
            .windows(4)
            .any(|word| word.eq_ignore_ascii_case(b"load"))
        {
            return Ok(None);
        }
        let Ok(statement) = astersql_parser::Parser::default().ParseOneStmt(sql, "", "") else {
            return Ok(None);
        };
        Ok(statement
            .as_any()
            .downcast_ref::<astersql_parser_ast::LoadDataStmt>()
            .filter(|load| load.FileLocRef == astersql_parser_ast::FileLocRef::Client)
            .map(|load| load.Path.clone()))
    }
    fn execute_local_infile(
        &self,
        sql: &str,
        data: Vec<u8>,
        cancel: &CancellationToken,
    ) -> ConnResult<Vec<QueryResult>> {
        self.ensure_not_cancelled(cancel)?;
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::ExecuteLocalInfile {
            sql: sql.into(),
            data,
            response: tx,
        })?;
        let mut results = rx.recv().map_err(packet_error)??;
        for result in &mut results {
            result.response_lifecycle = Some(self.response_lifecycle()?);
        }
        if let Some(result) = results.last() {
            *self
                .state
                .lock()
                .map_err(|_| ConnError::Poisoned("session state"))? = result.state.clone();
        }
        Ok(results)
    }
    fn execute_query_streaming(
        &self,
        sql: &str,
        allow_multi_statements: bool,
        cancel: &CancellationToken,
    ) -> ConnResult<Vec<QueryResult>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnError::Session("session is closed".to_owned()));
        }
        self.ensure_not_cancelled(cancel)?;
        let statements =
            SplitSQLStatements(sql).map_err(|error| ConnError::Session(error.to_string()))?;
        if !allow_multi_statements && statements.len() > 1 {
            return Err(ConnError::Session(
                "multi-statement execution is disabled".to_owned(),
            ));
        }
        if let Some(statement) = statements.last() {
            *self
                .last_statement
                .lock()
                .map_err(|_| ConnError::Poisoned("last statement"))? = statement.clone();
        }
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::ExecuteStreaming {
            statements,
            response: response_tx,
        })?;
        let mut results = response_rx.recv().map_err(packet_error)??;
        for result in &mut results {
            result.response_lifecycle = Some(self.response_lifecycle()?);
        }
        if let Some(result) = results.last() {
            *self
                .state
                .lock()
                .map_err(|_| ConnError::Poisoned("session state"))? = result.state.clone();
        }
        Ok(results)
    }

    fn field_list(&self, table: &str, wildcard: &str) -> ConnResult<Vec<ColumnInfo>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnError::Session("session is closed".to_owned()));
        }
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::FieldList {
            table: table.to_owned(),
            wildcard: wildcard.to_owned(),
            response: response_tx,
        })?;
        response_rx.recv().map_err(packet_error)?
    }

    fn prepare_statement(
        &self,
        sql: &str,
        cancel: &CancellationToken,
    ) -> ConnResult<PreparedMetadata> {
        self.ensure_not_cancelled(cancel)?;
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::Prepare {
            sql: sql.to_owned(),
            response: response_tx,
        })?;
        response_rx.recv().map_err(packet_error)?
    }

    fn execute_prepared_statement(
        &self,
        statement_id: u32,
        arguments: &[crate::conn_stmt::BinaryParam],
        cancel: &CancellationToken,
    ) -> ConnResult<QueryResult> {
        self.ensure_not_cancelled(cancel)?;
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::ExecutePrepared {
            statement_id,
            arguments: arguments.to_vec(),
            response: response_tx,
        })?;
        let mut result = response_rx.recv().map_err(packet_error)??;
        result.response_lifecycle = Some(self.response_lifecycle()?);
        *self
            .state
            .lock()
            .map_err(|_| ConnError::Poisoned("session state"))? = result.state.clone();
        Ok(result)
    }

    fn execute_prepared_streaming(
        &self,
        statement_id: u32,
        arguments: &[crate::conn_stmt::BinaryParam],
        cancel: &CancellationToken,
    ) -> ConnResult<QueryResult> {
        self.ensure_not_cancelled(cancel)?;
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::ExecutePreparedStreaming {
            statement_id,
            arguments: arguments.to_vec(),
            response: response_tx,
        })?;
        let mut result = response_rx.recv().map_err(packet_error)??;
        result.response_lifecycle = Some(self.response_lifecycle()?);
        *self
            .state
            .lock()
            .map_err(|_| ConnError::Poisoned("session state"))? = result.state.clone();
        Ok(result)
    }

    fn close_prepared_statement(&self, statement_id: u32) -> ConnResult<()> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::ClosePrepared {
            statement_id,
            response: response_tx,
        })?;
        response_rx.recv().map_err(packet_error)?
    }

    fn execute_command(
        &self,
        command: Command,
        _payload: &[u8],
        _cancel: &CancellationToken,
    ) -> ConnResult<Option<QueryResult>> {
        Err(ConnError::UnsupportedCommand(command as u8))
    }

    #[cfg(test)]
    fn result_fault_for_test(
        &self,
        operation: &'static str,
        at: usize,
        delay: Duration,
        fail: bool,
    ) {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::SetResultFault {
            operation,
            at,
            delay,
            fail,
            response: tx,
        })
        .unwrap();
        rx.recv().unwrap();
    }
    #[cfg(test)]
    fn result_events_for_test(&self) -> Vec<String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::ResultEvents { response: tx })
            .unwrap();
        rx.recv().unwrap()
    }

    #[cfg(test)]
    fn protocol_write_duration_for_test(&self) -> Duration {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::WriteDuration { response: tx })
            .unwrap();
        rx.recv().unwrap()
    }

    fn finish_protocol_response(&self, write_duration: Duration) {
        if let Ok(Some(sender)) = self.requests.lock().map(|requests| requests.clone()) {
            let (response_tx, response_rx) = mpsc::sync_channel(1);
            if sender
                .send(SessionRequest::FinishProtocolResponse {
                    write_duration,
                    response: response_tx,
                })
                .is_ok()
            {
                let _ = response_rx.recv();
            }
        }
    }

    fn change_user(&self, _payload: &[u8], _cancel: &CancellationToken) -> ConnResult<()> {
        Err(ConnError::UnsupportedCommand(Command::ChangeUser as u8))
    }

    fn reset_connection(&self, cancel: &CancellationToken) -> ConnResult<()> {
        self.ensure_not_cancelled(cancel)?;
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.send_request(SessionRequest::Reset {
            response: response_tx,
        })?;
        let protocol_state = response_rx.recv().map_err(packet_error)??;
        *self
            .state
            .lock()
            .map_err(|_| ConnError::Poisoned("session state"))? =
            map_protocol_state(protocol_state);
        self.cancellation.Reset();
        self.cancel_requested.store(false, Ordering::Release);
        self.last_statement
            .lock()
            .map_err(|_| ConnError::Poisoned("last statement"))?
            .clear();
        Ok(())
    }

    fn finish_query_cancellation(&self) {
        self.cancellation.Reset();
        self.cancel_requested.store(false, Ordering::Release);
    }

    fn cancel(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        self.cancellation.SendKillSignal(QueryInterrupted);
    }

    fn close(&self) -> ConnResult<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.cancel();
        if let Some(sender) = self
            .requests
            .lock()
            .map_err(|_| ConnError::Poisoned("session requests"))?
            .take()
        {
            let _ = sender.send(SessionRequest::Shutdown);
        }
        if let Some(worker) = self
            .worker
            .lock()
            .map_err(|_| ConnError::Poisoned("session worker"))?
            .take()
        {
            worker
                .join()
                .map_err(|_| ConnError::Session("session worker panicked".to_owned()))?;
        }
        Ok(())
    }

    fn last_statement(&self) -> String {
        self.last_statement
            .lock()
            .map(|statement| statement.clone())
            .unwrap_or_default()
    }

    fn process_snapshot(&self) -> SessionProcessSnapshot {
        self.process_info
            .lock()
            .map(|process| process.clone())
            .unwrap_or_else(|error| error.into_inner().clone())
    }
}
