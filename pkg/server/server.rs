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

// MySQL 协议 Server：监听、连接登记、进程列表、KILL 与优雅排空。
//
// 管理客户端连接生命周期、capability 位、TLS 配置、Standby 钩子与 AutoID
// 服务；查询取消（KILL QUERY）与连接关闭分离，避免误杀客户端。
// 排空流程严格分三步：自然退出、取消当前 SQL、最后才关闭连接。

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use astersql_executor_mppcoordmanager::InstanceMPPCoordinatorManager;
use astersql_server_handler_extractorhandler::extractor::ExtractRuntime;
use astersql_server_handler_tikvhandler::{DxfRuntime, TikvRuntime};
use astersql_session_sessmgr::{
    InfoSchemaCoordinator, InternalSession, Manager as SessionManager, NormalCloseKiller,
    PerformanceSchemaAccountSummary, ProcessInfo as SessionProcessInfo,
};

use crate::conn::{
    ClientConn, ConnError, ConnectionDomain, ConnectionServer, PacketIo, SessionDriver,
    newClientConn,
};
use crate::runtime::TcpPacketIo;
#[cfg(unix)]
use crate::runtime::UnixPacketIo;
use crate::standby::{
    StandbyController, StandbyReadyServer, StandbyShutdownServer, noop_standby_controller,
};

/// 默认客户端 capability 位掩码（MySQL 协议能力标志）。
pub const DEFAULT_CAPABILITY: u32 = 0x01ff_ffff;
/// 正常关闭连接消息缓存容量上限。
const NORMAL_CLOSED_CONNECTIONS_CAPACITY: usize = 1_000;
const PROXY_V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\0\r\nQUIT\n";

/// 按资源组跟踪当前连接；关闭未登记连接不会把计数降为负数。
#[derive(Default)]
pub struct ResourceGroupConnectionCount {
    connections: Mutex<HashMap<u64, String>>,
}

impl ResourceGroupConnectionCount {
    pub fn open(&self, connection_id: u64, resource_group: &str) {
        self.connections
            .lock()
            .expect("resource-group connection lock poisoned")
            .insert(connection_id, resource_group.to_owned());
    }

    pub fn close(&self, connection_id: u64) {
        self.connections
            .lock()
            .expect("resource-group connection lock poisoned")
            .remove(&connection_id);
    }

    pub fn move_to(&self, connection_id: u64, resource_group: &str) -> bool {
        let mut connections = self
            .connections
            .lock()
            .expect("resource-group connection lock poisoned");
        let Some(group) = connections.get_mut(&connection_id) else {
            return false;
        };
        *group = resource_group.to_owned();
        true
    }

    pub fn count(&self, resource_group: &str) -> usize {
        self.connections
            .lock()
            .expect("resource-group connection lock poisoned")
            .values()
            .filter(|group| group.as_str() == resource_group)
            .count()
    }
}

/// Installs rustls' AWS-LC provider only when it is a validated FIPS build.
/// This is fail-closed: a binary that requests FIPS cannot silently continue
/// with the ordinary provider.
pub fn install_fips_crypto_provider() -> Result<(), String> {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    if !provider.fips() {
        return Err("the linked rustls AWS-LC provider is not FIPS validated".into());
    }
    provider
        .install_default()
        .map_err(|_| "a different rustls crypto provider is already installed".to_owned())
}

fn build_sql_tls_config(
    config: &ServerConfig,
) -> Result<Option<Arc<rustls::ServerConfig>>, String> {
    let mut certificate = config.sql_tls_certificate.clone();
    let mut key = config.sql_tls_key.clone();
    if certificate.is_none() && key.is_none() && config.sql_auto_tls {
        let directory = config
            .temp_storage_path
            .as_deref()
            .ok_or_else(|| "AutoTLS requires a temp storage path".to_owned())?;
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("create AutoTLS directory {directory}: {error}"))?;
        let certificate_path = std::path::Path::new(directory).join("cert.pem");
        let key_path = std::path::Path::new(directory).join("key.pem");
        if !certificate_path.is_file() || !key_path.is_file() {
            astersql_util::misc::CreateCertificates(
                &certificate_path,
                &key_path,
                config.rsa_key_size,
                astersql_util::misc::PublicKeyAlgorithm::Rsa,
                astersql_util::misc::SignatureAlgorithm::Unspecified,
            )
            .map_err(|error| format!("create AutoTLS certificate: {error}"))?;
        }
        certificate = Some(certificate_path.to_string_lossy().into_owned());
        key = Some(key_path.to_string_lossy().into_owned());
    }
    let certificate = certificate.as_deref().unwrap_or_default();
    let key = key.as_deref().unwrap_or_default();
    let ca = config.sql_tls_ca.as_deref().unwrap_or_default();
    if certificate.is_empty() && key.is_empty() && ca.is_empty() {
        return Ok(None);
    }
    if certificate.is_empty() || key.is_empty() {
        return Err("SQL TLS requires both a certificate and private key".into());
    }
    let mut options = vec![astersql_util::security::WithCertAndKeyPath(
        certificate.to_owned(),
        key.to_owned(),
    )];
    if !ca.is_empty() {
        options.push(astersql_util::security::WithCAPath(ca.to_owned()));
    }
    astersql_util::security::NewTLSConfig(options)
        .map_err(|error| format!("load SQL TLS configuration: {error}"))?
        .ok_or_else(|| "SQL TLS configuration is empty".to_owned())?
        .server_config()
        .map(Some)
        .map_err(|error| format!("build SQL TLS configuration: {error}"))
}

fn build_sql_tls_metadata(config: &ServerConfig) -> Result<Option<TlsConfig>, String> {
    let (certificate, key) = if let (Some(certificate), Some(key)) = (
        config.sql_tls_certificate.as_deref(),
        config.sql_tls_key.as_deref(),
    ) {
        (certificate.to_owned(), key.to_owned())
    } else if config.sql_auto_tls {
        let directory = config
            .temp_storage_path
            .as_deref()
            .ok_or_else(|| "AutoTLS requires a temp storage path".to_owned())?;
        (
            std::path::Path::new(directory)
                .join("cert.pem")
                .to_string_lossy()
                .into_owned(),
            std::path::Path::new(directory)
                .join("key.pem")
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        return Ok(None);
    };
    let ca = config.sql_tls_ca.as_deref().unwrap_or_default();
    let (loaded, _) = astersql_util::misc::LoadTLSCertificates(
        ca,
        &key,
        &certificate,
        false,
        config.rsa_key_size,
    )
    .map_err(|error| format!("load SQL TLS metadata: {error}"))?;
    let loaded = loaded.ok_or_else(|| "SQL TLS metadata is empty".to_owned())?;
    let certificate_dates = loaded.certificate().certificate;
    Ok(Some(TlsConfig {
        ca_path: (!ca.is_empty()).then(|| ca.to_owned()),
        certificate_path: Some(certificate),
        key_path: Some(key),
        not_before: Some(certificate_dates.not_before().to_string()),
        not_after: Some(certificate_dates.not_after().to_string()),
        ..TlsConfig::default()
    }))
}

fn parse_network(network: &str) -> Result<(IpAddr, u8), String> {
    let (address, prefix) = match network.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (network, None),
    };
    let address: IpAddr = address
        .trim()
        .parse()
        .map_err(|_| format!("invalid PROXY Protocol network {network:?}"))?;
    let bits = if address.is_ipv4() { 32 } else { 128 };
    let prefix = prefix
        .map(str::parse::<u8>)
        .transpose()
        .map_err(|_| format!("invalid PROXY Protocol network {network:?}"))?
        .unwrap_or(bits);
    if prefix > bits {
        return Err(format!("invalid PROXY Protocol network {network:?}"));
    }
    Ok((address, prefix))
}

fn validate_proxy_networks(networks: &str) -> Result<(), String> {
    for network in networks
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if network != "*" {
            parse_network(network)?;
        }
    }
    Ok(())
}

fn network_contains(network: IpAddr, prefix: u8, address: IpAddr) -> bool {
    match (network, address) {
        (IpAddr::V4(network), IpAddr::V4(address)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            u32::from(network) & mask == u32::from(address) & mask
        }
        (IpAddr::V6(network), IpAddr::V6(address)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            u128::from(network) & mask == u128::from(address) & mask
        }
        _ => false,
    }
}

fn proxy_peer_allowed(networks: &str, peer: IpAddr) -> Result<bool, String> {
    networks
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .try_fold(false, |matched, network| {
            if network == "*" {
                Ok(true)
            } else {
                let (address, prefix) = parse_network(network)?;
                Ok(matched || network_contains(address, prefix, peer))
            }
        })
}

fn read_proxy_v1(stream: &mut TcpStream) -> Result<SocketAddr, String> {
    let mut header = Vec::with_capacity(108);
    while header.len() < 108 {
        let mut byte = [0_u8; 1];
        stream
            .read_exact(&mut byte)
            .map_err(|error| error.to_string())?;
        header.push(byte[0]);
        if header.ends_with(b"\r\n") {
            break;
        }
    }
    if !header.ends_with(b"\r\n") {
        return Err("PROXY v1 header exceeds 108 bytes".into());
    }
    let header = std::str::from_utf8(&header).map_err(|_| "invalid PROXY v1 header encoding")?;
    let fields = header
        .trim_end()
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    if fields.len() != 6 || fields[0] != "PROXY" || !matches!(fields[1], "TCP4" | "TCP6") {
        return Err("invalid PROXY v1 header".into());
    }
    let address: IpAddr = fields[2]
        .parse()
        .map_err(|_| "invalid PROXY v1 source address")?;
    let port: u16 = fields[4]
        .parse()
        .map_err(|_| "invalid PROXY v1 source port")?;
    Ok(SocketAddr::new(address, port))
}

fn read_proxy_v2(stream: &mut TcpStream) -> Result<Option<SocketAddr>, String> {
    let mut header = [0_u8; 16];
    stream
        .read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    if &header[..12] != PROXY_V2_SIGNATURE || header[12] >> 4 != 2 {
        return Err("invalid PROXY v2 signature or version".into());
    }
    let length = u16::from_be_bytes([header[14], header[15]]) as usize;
    let mut address = vec![0_u8; length];
    stream
        .read_exact(&mut address)
        .map_err(|error| error.to_string())?;
    if header[12] & 0x0f == 0 {
        return Ok(None);
    }
    let source = match header[13] {
        0x11 if address.len() >= 12 => SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(
                address[0], address[1], address[2], address[3],
            )),
            u16::from_be_bytes([address[8], address[9]]),
        ),
        0x21 if address.len() >= 36 => {
            let bytes: [u8; 16] = address[..16]
                .try_into()
                .map_err(|_| "invalid IPv6 address")?;
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::from(bytes)),
                u16::from_be_bytes([address[32], address[33]]),
            )
        }
        _ => return Err("unsupported PROXY v2 address family or transport".into()),
    };
    Ok(Some(source))
}

pub(crate) fn proxy_source_addr(
    stream: &mut TcpStream,
    peer: SocketAddr,
    networks: &str,
    fallbackable: bool,
    timeout: Duration,
) -> Result<Option<SocketAddr>, String> {
    if networks.trim().is_empty() {
        return Ok(None);
    }
    if !proxy_peer_allowed(networks, peer.ip())? {
        return Err(format!("PROXY Protocol peer {} is not allowed", peer.ip()));
    }
    stream
        .set_read_timeout((!timeout.is_zero()).then_some(timeout))
        .map_err(|error| error.to_string())?;
    let mut prefix = [0_u8; 12];
    let peeked = stream
        .peek(&mut prefix)
        .map_err(|error| error.to_string())?;
    let result = if peeked > 0 && prefix[0] == b'P' {
        read_proxy_v1(stream).map(Some)
    } else if peeked > 0 && prefix[0] == b'\r' {
        read_proxy_v2(stream)
    } else if fallbackable {
        Ok(Some(peer))
    } else {
        Err("missing PROXY Protocol header".into())
    };
    let _ = stream.set_read_timeout(None);
    result
}

#[derive(Clone, Debug)]
/// HTTP/gRPC 状态服务配置（地址、保活、并发流与窗口）。
pub struct StatusConfig {
    pub report_status: bool,
    pub host: String,
    pub port: u16,
    pub grpc_keep_alive: Duration,
    pub grpc_keep_alive_timeout: Duration,
    pub grpc_concurrent_streams: u32,
    pub grpc_initial_window_size: u32,
    pub grpc_max_send_message_size: usize,
    /// Cluster/status TLS CA path.
    pub tls_ca: Option<String>,
    /// Cluster/status TLS server certificate path.
    pub tls_certificate: Option<String>,
    /// Cluster/status TLS server private key path.
    pub tls_key: Option<String>,
    /// Allowed client certificate Common Names; non-empty enables mTLS.
    pub tls_verify_common_names: Vec<String>,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            report_status: true,
            host: "127.0.0.1".into(),
            port: 10_080,
            grpc_keep_alive: Duration::from_secs(10),
            grpc_keep_alive_timeout: Duration::from_secs(3),
            grpc_concurrent_streams: 1_024,
            grpc_initial_window_size: 65_535,
            grpc_max_send_message_size: 16 << 20,
            tls_ca: None,
            tls_certificate: None,
            tls_key: None,
            tls_verify_common_names: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
/// MySQL 监听与连接上限、代理协议、ballast 及嵌套 StatusConfig。
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// None disables PostgreSQL; Some(0) requests an OS-assigned TCP port.
    pub postgres_port: Option<u16>,
    pub socket: Option<String>,
    pub max_connections: usize,
    pub proxy_protocol_enabled: bool,
    pub proxy_protocol_networks: String,
    pub proxy_protocol_fallbackable: bool,
    pub proxy_protocol_header_timeout: Duration,
    pub sql_tls_ca: Option<String>,
    pub sql_tls_certificate: Option<String>,
    pub sql_tls_key: Option<String>,
    pub sql_auto_tls: bool,
    pub rsa_key_size: i32,
    pub temp_storage_path: Option<String>,
    pub max_ballast_object_size: usize,
    pub status: StatusConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 4_000,
            postgres_port: None,
            socket: None,
            max_connections: 0,
            proxy_protocol_enabled: false,
            proxy_protocol_networks: String::new(),
            proxy_protocol_fallbackable: false,
            proxy_protocol_header_timeout: Duration::from_secs(5),
            sql_tls_ca: None,
            sql_tls_certificate: None,
            sql_tls_key: None,
            sql_auto_tls: false,
            rsa_key_size: 4_096,
            temp_storage_path: None,
            max_ballast_object_size: 0,
            status: StatusConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
/// TLS 证书路径、CN 白名单与有效期元数据。
pub struct TlsConfig {
    pub ca_path: Option<String>,
    pub certificate_path: Option<String>,
    pub key_path: Option<String>,
    pub verify_common_names: Vec<String>,
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub not_before_unix: Option<i64>,
    pub not_after_unix: Option<i64>,
}

impl TlsConfig {
    /// Verifies a peer certificate's Common Name with the configured CA and
    /// `cluster-verify-cn` allow-list.  The server's protocol TLS metadata is
    /// kept lightweight, while the certificate parser and verifier remain the
    /// shared implementation used by the rest of AsterSQL.
    pub fn verify_peer_common_name(&self, certificate_pem: &[u8]) -> Result<(), String> {
        let ca_path = self
            .ca_path
            .as_deref()
            .ok_or_else(|| "CA certificate is required for Common Name verification".to_owned())?;
        let config = astersql_util::security::ToTLSConfigWithVerify(
            ca_path,
            self.certificate_path.as_deref().unwrap_or_default(),
            self.key_path.as_deref().unwrap_or_default(),
            self.verify_common_names.clone(),
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "TLS configuration is not configured".to_owned())?;
        config
            .verify_common_name_pem(certificate_pem)
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug)]
/// SHOW PROCESSLIST 风格的连接过程信息。
pub struct ProcessInfo {
    pub connection_id: u64,
    pub user: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub command: String,
    pub sql: String,
    pub start_time: SystemTime,
    pub state: u16,
    pub internal: bool,
    pub attributes: HashMap<String, String>,
}

impl Default for ProcessInfo {
    fn default() -> Self {
        Self {
            connection_id: 0,
            user: String::new(),
            host: String::new(),
            port: String::new(),
            database: String::new(),
            command: String::new(),
            sql: String::new(),
            start_time: UNIX_EPOCH,
            state: 0,
            internal: false,
            attributes: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 连接上当前事务摘要（start_ts 为事务开始时间戳）。
pub struct TransactionInfo {
    pub connection_id: u64,
    pub start_ts: u64,
    pub current_sql_digest: String,
}

/// The connection contract intentionally contains the operations used by the
/// Go session-manager implementation. Query cancellation and connection close
/// remain separate so KILL QUERY does not accidentally terminate a client.
/// 受管连接契约：进程信息、KILL QUERY 与关闭分离。
pub trait ManagedConnection: Send + Sync {
    /// 连接 ID。
    fn id(&self) -> u64;
    /// 该连接协商的 capability 位。
    fn capability(&self) -> u32;
    /// 当前过程信息；无则 None。
    fn process_info(&self) -> Option<ProcessInfo>;
    /// 当前事务信息；无事务则 None。
    fn transaction_info(&self) -> Option<TransactionInfo>;
    /// 客户端连接属性（如 program_name）。
    fn connection_attributes(&self) -> HashMap<String, String>;
    /// 会话级状态变量快照。
    fn status_variables(&self) -> HashMap<String, String>;
    /// 累计某条 SQL 的 CPU 时间。
    fn update_cpu_time(&self, sql_id: u64, cpu_time: Duration);
    /// 仅取消当前查询，不关闭连接。
    fn kill_query(&self, max_execution_time: bool, runaway: bool);
    /// 关闭连接。
    fn close(&self);
    /// 是否为内部/系统会话（process_info.internal）。
    fn is_system_session(&self) -> bool {
        self.process_info().is_some_and(|p| p.internal)
    }
}

/// Server 驱动标识（如 "tidb"）。
pub trait ServerDriver: Send + Sync {
    fn name(&self) -> &str;
}

/// 域（Domain）：提供 server_id 与启动时间戳。
pub trait Domain: Send + Sync {
    fn server_id(&self) -> u64;
    fn start_timestamp(&self) -> i64;

    fn system_process_list(&self) -> HashMap<u64, Arc<SessionProcessInfo>> {
        HashMap::new()
    }

    fn kill_system_process(&self, _connection_id: u64) {}

    /// Extract status HTTP handler 所需的运行时，对应 Go `GetExtractHandle`。
    fn extract_runtime(&self) -> Option<Arc<dyn ExtractRuntime>> {
        None
    }

    /// Status HTTP handler 所需的 TiKV 运行时。
    ///
    /// 不具备 KV/PD/DDL 依赖的 server 仍可正常运行；相应的 status 路由会
    /// 返回明确的未配置错误，而不是伪造成功响应。
    fn tikv_runtime(&self) -> Option<Arc<dyn TikvRuntime>> {
        None
    }

    /// Current SQL schema for the status metadata routes.
    fn schema_snapshot(&self) -> Option<astersql_infoschema::SchemaRef> {
        None
    }

    /// Publish a TiFlash status report for a physical table ID.
    fn publish_tiflash_replica_report(
        &self,
        _table_id: i64,
        _region_count: u64,
        _flash_region_count: u64,
    ) -> Result<(), String> {
        Err("TiFlash replica reports are not configured".into())
    }

    /// DXF status HTTP handler 所需的运行时。
    fn dxf_runtime(&self) -> Option<Arc<dyn DxfRuntime>> {
        None
    }
}

/// 自动分配 ID 服务：关闭与 owner 判定。
pub trait AutoIdService: Send + Sync {
    fn close(&self);
    fn is_owner(&self) -> bool;
}

#[derive(Clone, Debug, Eq)]
/// 正常关闭缓存键：keyspace + connection_id。
struct NormalCloseConnectionKey {
    keyspace: String,
    connection_id: String,
}

impl PartialEq for NormalCloseConnectionKey {
    fn eq(&self, other: &Self) -> bool {
        self.keyspace == other.keyspace && self.connection_id == other.connection_id
    }
}

impl Hash for NormalCloseConnectionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.keyspace.hash(state);
        self.connection_id.hash(state);
    }
}

#[derive(Default)]
/// LRU 风格的正常关闭消息缓存（容量见常量）。
struct NormalCloseCache {
    values: HashMap<NormalCloseConnectionKey, String>,
    order: VecDeque<NormalCloseConnectionKey>,
}

impl NormalCloseCache {
    /// 插入或刷新条目；超出容量时淘汰最旧键。
    fn insert(&mut self, key: NormalCloseConnectionKey, value: String) {
        if self.values.contains_key(&key) {
            self.order.retain(|entry| entry != &key);
        }
        self.values.insert(key.clone(), value);
        self.order.push_back(key);
        // 超出容量时从队首淘汰。
        while self.order.len() > NORMAL_CLOSED_CONNECTIONS_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.values.remove(&expired);
            }
        }
    }
}

/// MySQL protocol server state. All mutating operations use interior
/// synchronization because listener, status and shutdown workers share it.
/// MySQL 协议服务器共享状态（内置同步，供监听/状态/关闭协程共用）。
pub struct Server {
    config: Arc<ServerConfig>,
    driver: Arc<dyn ServerDriver>,
    domain: RwLock<Option<Arc<dyn Domain>>>,
    listener: Mutex<Option<TcpListener>>,
    postgres_listener: Mutex<Option<TcpListener>>,
    postgres_service: Mutex<Option<Arc<crate::pg_conn::PgService>>>,
    #[cfg(unix)]
    unix_listener: Mutex<Option<UnixListener>>,
    status_listener: Mutex<Option<TcpListener>>,
    listen_addr: RwLock<Option<SocketAddr>>,
    postgres_addr: RwLock<Option<SocketAddr>>,
    status_addr: RwLock<Option<SocketAddr>>,
    clients: RwLock<HashMap<u64, Arc<dyn ManagedConnection>>>,
    pending_clients: RwLock<HashMap<u64, Arc<ClientConn>>>,
    clients_changed: Condvar,
    clients_wait_lock: Mutex<()>,
    connection_driver: RwLock<Option<Arc<dyn SessionDriver>>>,
    connection_domain: RwLock<Option<Arc<dyn ConnectionDomain>>>,
    accept_worker: Mutex<Option<JoinHandle<()>>>,
    #[cfg(unix)]
    unix_accept_worker: Mutex<Option<JoinHandle<()>>>,
    status_worker: Mutex<Option<JoinHandle<()>>>,
    connection_workers: Mutex<Vec<JoinHandle<()>>>,
    normal_closed: Mutex<NormalCloseCache>,
    internal_sessions: Mutex<HashMap<usize, u64>>,
    capability: AtomicU32,
    health: AtomicBool,
    running: AtomicBool,
    shutdown_mode: AtomicBool,
    close_started: AtomicBool,
    force_shutdown: AtomicBool,
    need_request_manager_free: AtomicBool,
    accepted_connections: AtomicU64,
    performance_schema_account_totals: Mutex<HashMap<(Option<String>, Option<String>), u64>>,
    active_tokens: AtomicUsize,
    tls_config: RwLock<Option<Arc<TlsConfig>>>,
    session_tls_config: RwLock<Option<Arc<rustls::ServerConfig>>>,
    standby: Arc<dyn StandbyController>,
    auto_id_service: RwLock<Option<Arc<dyn AutoIdService>>>,
}

impl Server {
    /// 使用默认 NoopStandbyController 创建 Server。
    pub fn new(config: ServerConfig, driver: Arc<dyn ServerDriver>) -> Result<Arc<Self>, String> {
        Self::with_standby(config, driver, noop_standby_controller())
    }

    /// 指定 StandbyController 创建 Server，并触发 on_server_created。
    pub fn with_standby(
        config: ServerConfig,
        driver: Arc<dyn ServerDriver>,
        standby: Arc<dyn StandbyController>,
    ) -> Result<Arc<Self>, String> {
        validate_proxy_networks(&config.proxy_protocol_networks)?;
        let session_tls_config = build_sql_tls_config(&config)?;
        let tls_config = if session_tls_config.is_some() {
            build_sql_tls_metadata(&config)?
        } else {
            None
        };
        // 空 host 无法绑定监听，直接拒绝。
        if config.host.is_empty() {
            return Err("server host cannot be empty".into());
        }
        let server = Arc::new(Self {
            config: Arc::new(config),
            driver,
            domain: RwLock::new(None),
            listener: Mutex::new(None),
            postgres_listener: Mutex::new(None),
            postgres_service: Mutex::new(None),
            #[cfg(unix)]
            unix_listener: Mutex::new(None),
            status_listener: Mutex::new(None),
            listen_addr: RwLock::new(None),
            postgres_addr: RwLock::new(None),
            status_addr: RwLock::new(None),
            clients: RwLock::new(HashMap::new()),
            pending_clients: RwLock::new(HashMap::new()),
            clients_changed: Condvar::new(),
            clients_wait_lock: Mutex::new(()),
            connection_driver: RwLock::new(None),
            connection_domain: RwLock::new(None),
            accept_worker: Mutex::new(None),
            #[cfg(unix)]
            unix_accept_worker: Mutex::new(None),
            status_worker: Mutex::new(None),
            connection_workers: Mutex::new(Vec::new()),
            normal_closed: Mutex::new(NormalCloseCache::default()),
            internal_sessions: Mutex::new(HashMap::new()),
            capability: AtomicU32::new(DEFAULT_CAPABILITY),
            health: AtomicBool::new(false),
            running: AtomicBool::new(false),
            shutdown_mode: AtomicBool::new(false),
            close_started: AtomicBool::new(false),
            force_shutdown: AtomicBool::new(false),
            need_request_manager_free: AtomicBool::new(false),
            accepted_connections: AtomicU64::new(0),
            performance_schema_account_totals: Mutex::new(HashMap::new()),
            active_tokens: AtomicUsize::new(0),
            tls_config: RwLock::new(tls_config.map(Arc::new)),
            session_tls_config: RwLock::new(session_tls_config),
            standby,
            auto_id_service: RwLock::new(None),
        });
        server.standby.on_server_created(server.as_ref());
        Ok(server)
    }

    /// 测试辅助：配置非法则 panic。
    pub fn new_test(config: ServerConfig, driver: Arc<dyn ServerDriver>) -> Arc<Self> {
        Self::new(config, driver).expect("test server configuration must be valid")
    }

    /// 返回共享配置。
    pub fn config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.config)
    }

    /// 返回驱动。
    pub fn driver(&self) -> Arc<dyn ServerDriver> {
        Arc::clone(&self.driver)
    }

    /// 绑定 Domain（提供 server_id / 启动时间）。
    pub fn set_domain(&self, domain: Arc<dyn Domain>) {
        *self.domain.write().expect("domain lock poisoned") = Some(domain);
    }

    /// 当前 Domain（若已设置）。
    pub fn domain(&self) -> Option<Arc<dyn Domain>> {
        self.domain.read().expect("domain lock poisoned").clone()
    }

    /// 返回 standby 控制器提供的 status 路由，供 status server 挂载。
    pub(crate) fn standby_handler(
        self: &Arc<Self>,
    ) -> Option<(String, crate::http_status::Router)> {
        self.standby
            .handler(Arc::clone(self) as Arc<dyn StandbyShutdownServer>)
    }

    /// 安装生产 MySQL 连接所用的 canonical SessionDriver 与 ConnectionDomain。
    pub fn set_connection_runtime(
        self: &Arc<Self>,
        driver: Arc<dyn SessionDriver>,
        domain: Arc<dyn ConnectionDomain>,
    ) -> Result<(), String> {
        if self.running.load(Ordering::Acquire) {
            return Err("connection runtime cannot change after server start".into());
        }
        let manager: Arc<dyn SessionManager> = self.clone();
        driver.set_session_manager(Arc::downgrade(&manager));
        *self
            .connection_driver
            .write()
            .expect("connection driver lock poisoned") = Some(driver);
        *self
            .connection_domain
            .write()
            .expect("connection domain lock poisoned") = Some(domain);
        Ok(())
    }

    /// MySQL 监听实际地址。
    /// Address of the independent PostgreSQL listener, if enabled and bound.
    pub fn postgres_listener_addr(&self) -> Option<SocketAddr> {
        *self
            .postgres_addr
            .read()
            .expect("PostgreSQL address lock poisoned")
    }

    pub fn listener_addr(&self) -> Option<SocketAddr> {
        *self
            .listen_addr
            .read()
            .expect("listen address lock poisoned")
    }

    /// 状态服务监听实际地址。
    pub fn status_listener_addr(&self) -> Option<SocketAddr> {
        *self
            .status_addr
            .read()
            .expect("status address lock poisoned")
    }

    /// 对外报告的状态服务地址字符串；未开启 report_status 则 None。
    pub fn status_server_addr(&self) -> Option<String> {
        if !self.config.status.report_status {
            return None;
        }
        Some(
            self.status_listener_addr()
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| {
                    format!("{}:{}", self.config.status.host, self.config.status.port)
                }),
        )
    }

    /// 对全局 capability 做按位异或翻转。
    pub fn xor_capability(&self, capability: u32) {
        self.capability.fetch_xor(capability, Ordering::AcqRel);
    }

    /// 按位或加入 capability 标志。
    pub fn add_capability(&self, capability: u32) {
        self.capability.fetch_or(capability, Ordering::AcqRel);
    }

    /// 当前全局 capability。
    pub fn capability(&self) -> u32 {
        self.capability.load(Ordering::Acquire)
    }

    /// 记录连接正常关闭原因（按 keyspace + id）。
    pub fn set_normal_closed_connection(
        &self,
        keyspace: impl Into<String>,
        connection_id: impl Into<String>,
        message: impl Into<String>,
    ) {
        let key = NormalCloseConnectionKey {
            keyspace: keyspace.into(),
            connection_id: connection_id.into(),
        };
        self.normal_closed
            .lock()
            .expect("normal close cache lock poisoned")
            .insert(key, message.into());
    }

    /// 查询正常关闭缓存中的消息。
    pub fn normal_closed_connection(&self, keyspace: &str, connection_id: &str) -> Option<String> {
        let key = NormalCloseConnectionKey {
            keyspace: keyspace.into(),
            connection_id: connection_id.into(),
        };
        self.normal_closed
            .lock()
            .expect("normal close cache lock poisoned")
            .values
            .get(&key)
            .cloned()
    }

    /// 当前已登记连接数。
    pub fn connection_count(&self) -> usize {
        self.clients.read().expect("clients lock poisoned").len()
    }

    /// 累计成功登记过的连接次数。
    pub fn accepted_connection_count(&self) -> u64 {
        self.accepted_connections.load(Ordering::Acquire)
    }

    /// 在 max_connections 限制下获取连接令牌。
    fn acquire_token(&self) -> Result<(), String> {
        // 0 表示不限制。
        if self.config.max_connections == 0 {
            return Ok(());
        }
        self.active_tokens
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.config.max_connections).then_some(active + 1)
            })
            .map(|_| ())
            .map_err(|_| "too many connections".into())
    }

    /// 释放连接令牌。
    fn release_token(&self) {
        if self.config.max_connections != 0 {
            let _ =
                self.active_tokens
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                        active.checked_sub(1)
                    });
        }
    }

    /// 登记连接：关机中拒绝；占令牌；重复 ID 回滚令牌。
    pub fn register_connection(
        &self,
        connection: Arc<dyn ManagedConnection>,
    ) -> Result<(), String> {
        // 关机模式不再接受新连接。
        if self.shutdown_mode.load(Ordering::Acquire) {
            return Err("server is shutting down".into());
        }
        self.acquire_token()?;
        let id = connection.id();
        let mut clients = self.clients.write().expect("clients lock poisoned");
        // 重复登记：归还令牌并报错。
        if clients.contains_key(&id) {
            self.release_token();
            return Err(format!("connection {id} is already registered"));
        }
        clients.insert(id, Arc::clone(&connection));
        if !connection.is_system_session()
            && let Some(process) = connection.process_info()
        {
            let key = (
                (!process.user.is_empty()).then_some(process.user),
                (!process.host.is_empty()).then_some(process.host),
            );
            *self
                .performance_schema_account_totals
                .lock()
                .expect("performance_schema account totals lock poisoned")
                .entry(key)
                .or_default() += 1;
        }
        drop(clients);
        self.accepted_connections.fetch_add(1, Ordering::Relaxed);
        self.standby.on_connection_active();
        self.clients_changed.notify_all();
        Ok(())
    }

    /// 注销连接并释放令牌；返回是否确实移除。
    pub fn unregister_connection(&self, connection_id: u64) -> bool {
        let removed = self
            .clients
            .write()
            .expect("clients lock poisoned")
            .remove(&connection_id)
            .is_some();
        if removed {
            self.release_token();
            self.clients_changed.notify_all();
        }
        removed
    }

    /// 初始化监听、激活 Domain，并启动 status/MySQL accept worker。
    pub fn run(self: &Arc<Self>, domain: Arc<dyn Domain>) -> Result<(), String> {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("server is already running".into());
        }
        if let Err(error) = self.init_tidb_listener() {
            self.running.store(false, Ordering::Release);
            return Err(error);
        }
        if let Err(error) = self.init_postgres_listener() {
            self.running.store(false, Ordering::Release);
            self.close_listeners();
            return Err(error);
        }
        // MPP executors advertise through the same SQL listener as TiDB.  Keep
        // the coordinator manager in sync with the OS-assigned address (which
        // is especially important when tests bind port 0).
        if let Some(address) = self.listener_addr() {
            InstanceMPPCoordinatorManager.init_server_address(true, address.to_string());
        }
        self.set_domain(domain);
        // 热备场景下阻塞至激活；Noop 则立即返回。
        self.standby.wait_for_activate();
        let result = self.standby.prepare_for_activation(self.as_ref());
        self.standby.end_standby(result.clone());
        if let Err(error) = result {
            self.running.store(false, Ordering::Release);
            self.close_listeners();
            return Err(error);
        }
        self.health.store(true, Ordering::Release);
        if let Err(error) = self
            .start_status_http()
            .and_then(|_| self.start_mysql_accept())
            .and_then(|_| self.start_postgres_accept())
        {
            self.health.store(false, Ordering::Release);
            self.running.store(false, Ordering::Release);
            self.close_listeners();
            self.join_listener_workers();
            return Err(error);
        }
        Ok(())
    }

    fn start_postgres_accept(&self) -> Result<(), String> {
        let Some(listener) = self
            .postgres_listener
            .lock()
            .unwrap()
            .as_ref()
            .map(TcpListener::try_clone)
            .transpose()
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        let Some(driver) = self.connection_driver.read().unwrap().clone() else {
            return Ok(());
        };
        let Some(domain) = self.connection_domain.read().unwrap().clone() else {
            return Ok(());
        };
        let service = crate::pg_conn::PgService::start(
            listener,
            driver,
            domain,
            astersql_util::misc::RequireSecureTransportEnabled(),
        )
        .map_err(|e| e.to_string())?;
        *self.postgres_service.lock().unwrap() = Some(service);
        Ok(())
    }

    /// 启动 MySQL accept worker；未安装连接运行时时保留旧 Server 管理能力。
    fn start_mysql_accept(self: &Arc<Self>) -> Result<(), String> {
        let runtime_ready = self
            .connection_driver
            .read()
            .expect("connection driver lock poisoned")
            .is_some()
            && self
                .connection_domain
                .read()
                .expect("connection domain lock poisoned")
                .is_some();
        if !runtime_ready {
            return Ok(());
        }
        let listener = self
            .listener
            .lock()
            .expect("listener lock poisoned")
            .as_ref()
            .ok_or_else(|| "MySQL listener is not initialized".to_owned())?
            .try_clone()
            .map_err(|error| format!("clone MySQL listener: {error}"))?;
        let server = Arc::clone(self);
        let worker = thread::Builder::new()
            .name("astersql-mysql-accept".into())
            .spawn(move || server.serve_mysql_loop(listener))
            .map_err(|error| format!("start MySQL accept worker: {error}"))?;
        *self
            .accept_worker
            .lock()
            .expect("accept worker lock poisoned") = Some(worker);
        #[cfg(unix)]
        if let Some(listener) = self
            .unix_listener
            .lock()
            .expect("Unix listener lock poisoned")
            .as_ref()
        {
            let listener = listener
                .try_clone()
                .map_err(|error| format!("clone MySQL Unix listener: {error}"))?;
            let server = Arc::clone(self);
            let worker = thread::Builder::new()
                .name("astersql-mysql-unix-accept".into())
                .spawn(move || server.serve_mysql_unix_loop(listener))
                .map_err(|error| format!("start MySQL Unix accept worker: {error}"))?;
            *self
                .unix_accept_worker
                .lock()
                .expect("Unix accept worker lock poisoned") = Some(worker);
        }
        Ok(())
    }

    /// 非阻塞 accept 循环；单连接错误只终止对应 worker。
    fn serve_mysql_loop(self: Arc<Self>, listener: TcpListener) {
        while !self.is_shutdown() {
            match listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let tls_config = self
                        .session_tls_config
                        .read()
                        .expect("session TLS config lock poisoned")
                        .clone();
                    let packet = match TcpPacketIo::new_with_options(
                        stream,
                        64 << 20,
                        tls_config,
                        None,
                        self.config.proxy_protocol_enabled.then(|| {
                            (
                                self.config.proxy_protocol_networks.clone(),
                                self.config.proxy_protocol_fallbackable,
                                self.config.proxy_protocol_header_timeout,
                            )
                        }),
                    ) {
                        Ok(packet) => packet,
                        Err(_) => continue,
                    };
                    self.serve_mysql_connection(Box::new(packet), false);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }

    #[cfg(unix)]
    fn serve_mysql_unix_loop(self: Arc<Self>, listener: UnixListener) {
        while !self.is_shutdown() {
            match listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let packet = match UnixPacketIo::new(stream, 64 << 20) {
                        Ok(packet) => packet,
                        Err(_) => continue,
                    };
                    self.serve_mysql_connection(Box::new(packet), true);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }

    fn serve_mysql_connection(self: &Arc<Self>, packet: Box<dyn PacketIo>, unix_socket: bool) {
        let connection_server: Arc<dyn ConnectionServer> = self.clone();
        let connection = newClientConn(
            connection_server,
            packet,
            authentication_salt(self.accepted_connection_count()),
            unix_socket,
        );
        let connection_id = connection.connection_id();
        self.pending_clients
            .write()
            .expect("pending clients lock poisoned")
            .insert(connection_id, Arc::clone(&connection));
        let server = Arc::clone(self);
        let worker = thread::Builder::new()
            .name(format!("astersql-mysql-{connection_id}"))
            .spawn(move || {
                if connection.handshake().is_err() {
                    let _ = connection.Close();
                    server
                        .pending_clients
                        .write()
                        .expect("pending clients lock poisoned")
                        .remove(&connection_id);
                } else {
                    server
                        .pending_clients
                        .write()
                        .expect("pending clients lock poisoned")
                        .remove(&connection_id);
                    let _ = connection.Run();
                }
            });
        match worker {
            Ok(worker) => self
                .connection_workers
                .lock()
                .expect("connection workers lock poisoned")
                .push(worker),
            Err(_) => {
                if let Some(connection) = self
                    .pending_clients
                    .write()
                    .expect("pending clients lock poisoned")
                    .remove(&connection_id)
                {
                    let _ = connection.Close();
                }
            }
        }
    }

    /// Bind the independent PostgreSQL listener before installing its service.
    fn init_postgres_listener(&self) -> Result<(), String> {
        let Some(port) = self.config.postgres_port else {
            return Ok(());
        };
        let mut slot = self
            .postgres_listener
            .lock()
            .expect("PostgreSQL listener lock poisoned");
        if slot.is_some() {
            return Ok(());
        }
        let listener = TcpListener::bind((self.config.host.as_str(), port))
            .map_err(|error| format!("listen PostgreSQL {}:{port}: {error}", self.config.host))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("set PostgreSQL listener nonblocking: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("read PostgreSQL listener address: {error}"))?;
        *self
            .postgres_addr
            .write()
            .expect("PostgreSQL address lock poisoned") = Some(address);
        *slot = Some(listener);
        Ok(())
    }

    /// 绑定 MySQL TCP 监听（非阻塞）；已存在则跳过。
    pub fn init_tidb_listener(&self) -> Result<(), String> {
        if self
            .listener
            .lock()
            .expect("listener lock poisoned")
            .is_some()
        {
            return Ok(());
        }
        let listener =
            TcpListener::bind((self.config.host.as_str(), self.config.port)).map_err(|error| {
                format!("listen {}:{}: {error}", self.config.host, self.config.port)
            })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("set listener nonblocking: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("read listener address: {error}"))?;
        #[cfg(unix)]
        let unix_listener = self
            .config
            .socket
            .as_deref()
            .map(|path| {
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path)
                        .map_err(|error| format!("remove stale socket {path}: {error}"))?,
                    Ok(_) => return Err(format!("socket path {path} exists and is not a socket")),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("inspect socket path {path}: {error}")),
                }
                let unix_listener = UnixListener::bind(path)
                    .map_err(|error| format!("listen Unix socket {path}: {error}"))?;
                if let Err(error) = unix_listener.set_nonblocking(true) {
                    let _ = std::fs::remove_file(path);
                    return Err(format!("set Unix listener nonblocking: {error}"));
                }
                Ok(unix_listener)
            })
            .transpose()?;
        #[cfg(not(unix))]
        if self.config.socket.is_some() {
            return Err("Unix socket listener is unsupported on this platform".into());
        }
        *self
            .listen_addr
            .write()
            .expect("listen address lock poisoned") = Some(address);
        *self.listener.lock().expect("listener lock poisoned") = Some(listener);
        #[cfg(unix)]
        {
            *self
                .unix_listener
                .lock()
                .expect("Unix listener lock poisoned") = unix_listener;
        }
        Ok(())
    }

    /// 设置状态服务监听器并记录地址。
    pub(crate) fn set_status_listener(&self, listener: TcpListener) -> Result<(), String> {
        let address = listener
            .local_addr()
            .map_err(|error| format!("read status listener address: {error}"))?;
        *self
            .status_addr
            .write()
            .expect("status address lock poisoned") = Some(address);
        *self
            .status_listener
            .lock()
            .expect("status listener lock poisoned") = Some(listener);
        Ok(())
    }

    /// 保存 status worker，以便关闭时等待其退出。
    pub(crate) fn set_status_worker(&self, worker: JoinHandle<()>) {
        *self
            .status_worker
            .lock()
            .expect("status worker lock poisoned") = Some(worker);
    }

    /// 进入关机模式并标记不健康。
    pub fn enter_shutdown_mode(&self) {
        self.shutdown_mode.store(true, Ordering::Release);
        self.health.store(false, Ordering::Release);
        self.close_listeners();
    }

    /// Server 是否已进入 shutdown。
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_mode.load(Ordering::Acquire)
    }

    /// 关闭 SQL/状态监听并清空地址。
    pub fn close_listeners(&self) {
        if let Some(service) = self.postgres_service.lock().unwrap().take() {
            service.close();
        }
        self.postgres_listener
            .lock()
            .expect("PostgreSQL listener lock poisoned")
            .take();
        *self
            .postgres_addr
            .write()
            .expect("PostgreSQL address lock poisoned") = None;
        self.listener.lock().expect("listener lock poisoned").take();
        #[cfg(unix)]
        self.unix_listener
            .lock()
            .expect("Unix listener lock poisoned")
            .take();
        self.status_listener
            .lock()
            .expect("status listener lock poisoned")
            .take();
        *self
            .listen_addr
            .write()
            .expect("listen address lock poisoned") = None;
        *self
            .status_addr
            .write()
            .expect("status address lock poisoned") = None;
        #[cfg(unix)]
        if let Some(path) = self.config.socket.as_deref() {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {}
            }
        }
    }

    /// 完整关闭：关机钩子、关监听、杀连接、关 AutoID。
    pub fn close(&self) {
        if self.close_started.swap(true, Ordering::AcqRel) {
            return;
        }
        self.enter_shutdown_mode();
        self.standby.on_server_shutdown(self);
        self.close_listeners();
        self.join_listener_workers();
        let pending: Vec<_> = self
            .pending_clients
            .read()
            .expect("pending clients lock poisoned")
            .values()
            .cloned()
            .collect();
        for connection in pending {
            let _ = connection.Close();
        }
        self.kill_all_connections();
        let workers = std::mem::take(
            &mut *self
                .connection_workers
                .lock()
                .expect("worker lock poisoned"),
        );
        for worker in workers {
            let _ = worker.join();
        }
        self.pending_clients
            .write()
            .expect("pending clients lock poisoned")
            .clear();
        self.auto_id_service_close();
        self.running.store(false, Ordering::Release);
    }

    /// 等待 MySQL/status 监听 worker 收敛。
    fn join_listener_workers(&self) {
        if let Some(worker) = self
            .accept_worker
            .lock()
            .expect("accept worker lock poisoned")
            .take()
        {
            let _ = worker.join();
        }
        #[cfg(unix)]
        if let Some(worker) = self
            .unix_accept_worker
            .lock()
            .expect("Unix accept worker lock poisoned")
            .take()
        {
            let _ = worker.join();
        }
        if let Some(worker) = self
            .status_worker
            .lock()
            .expect("status worker lock poisoned")
            .take()
        {
            let _ = worker.join();
        }
    }

    /// 全部连接的过程信息（含内部会话）。
    pub fn show_process_list(&self) -> HashMap<u64, ProcessInfo> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .iter()
            .filter_map(|(id, connection)| connection.process_info().map(|info| (*id, info)))
            .collect()
    }

    /// 仅用户连接的过程信息（排除 internal）。
    pub fn user_process_list(&self) -> HashMap<u64, ProcessInfo> {
        self.show_process_list()
            .into_iter()
            .filter(|(_, info)| !info.internal)
            .collect()
    }

    /// 各连接 capability 映射。
    pub fn client_capability_list(&self) -> HashMap<u64, u32> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .iter()
            .map(|(id, connection)| (*id, connection.capability()))
            .collect()
    }

    /// 所有活跃事务摘要。
    pub fn transaction_list(&self) -> Vec<TransactionInfo> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .values()
            .filter_map(|connection| connection.transaction_info())
            .collect()
    }

    /// 按连接 ID 取过程信息。
    pub fn process_info(&self, connection_id: u64) -> Option<ProcessInfo> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .get(&connection_id)
            .and_then(|connection| connection.process_info())
    }

    /// 指定用户各连接的属性映射。
    pub fn connection_attributes(&self, user: &str) -> HashMap<u64, HashMap<String, String>> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .iter()
            .filter_map(|(id, connection)| {
                connection
                    .process_info()
                    .filter(|info| info.user == user)
                    .map(|_| (*id, connection.connection_attributes()))
            })
            .collect()
    }

    /// 各连接会话状态变量。
    pub fn status_variables(&self) -> HashMap<u64, HashMap<String, String>> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .iter()
            .map(|(id, connection)| (*id, connection.status_variables()))
            .collect()
    }

    /// 更新指定连接上某 SQL 的 CPU 时间。
    pub fn update_process_cpu_time(&self, connection_id: u64, sql_id: u64, cpu_time: Duration) {
        if let Some(connection) = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .get(&connection_id)
        {
            connection.update_cpu_time(sql_id, cpu_time);
        }
    }

    /// KILL：query_only 时只杀查询，否则可记录正常关闭消息并关连接。
    pub fn kill(
        &self,
        connection_id: u64,
        query_only: bool,
        max_execution_time: bool,
        runaway: bool,
        normal_close_message: Option<&str>,
    ) -> bool {
        let connection = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .get(&connection_id)
            .cloned();
        let Some(connection) = connection else {
            return false;
        };
        // KILL QUERY：不关闭连接。
        if query_only {
            connection.kill_query(max_execution_time, runaway);
        } else {
            if let Some(message) = normal_close_message {
                self.set_normal_closed_connection("", connection_id.to_string(), message);
            }
            connection.close();
            // Go's kill path always interrupts the active statement after
            // marking/closing the connection. Closing the transport alone can
            // leave execution running behind a blocked or detached client.
            connection.kill_query(max_execution_time, runaway);
        }
        true
    }

    /// 关闭所有系统/内部会话连接。
    pub fn kill_system_processes(&self) {
        let targets: Vec<_> = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .values()
            .filter(|connection| connection.is_system_session())
            .cloned()
            .collect();
        for connection in targets {
            connection.close();
        }
    }

    /// 关闭全部已登记连接。
    pub fn kill_all_connections(&self) {
        let targets: Vec<_> = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .values()
            .cloned()
            .collect();
        for connection in targets {
            connection.close();
        }
    }

    /// Wait for clients to leave naturally, cancel their current statements,
    /// then close the remaining connections after the second grace period.
    /// 优雅排空：先等自然离开，再取消语句，最后强制关闭。
    pub fn drain_clients(&self, drain_wait: Duration, cancel_wait: Duration) {
        self.enter_shutdown_mode();
        if self.wait_zero_connections_timeout(drain_wait) {
            return;
        }
        let clients: Vec<_> = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .values()
            .cloned()
            .collect();
        for connection in &clients {
            connection.kill_query(false, false);
        }
        if self.wait_zero_connections_timeout(cancel_wait) {
            return;
        }
        for connection in clients {
            connection.close();
        }
    }

    /// 在超时内等待连接数为 0；成功返回 true。
    pub fn wait_zero_connections_timeout(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .clients_wait_lock
            .lock()
            .expect("clients wait lock poisoned");
        loop {
            if self.connection_count() == 0 {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next_guard, result) = self
                .clients_changed
                .wait_timeout(guard, deadline - now)
                .expect("clients wait lock poisoned");
            guard = next_guard;
            if result.timed_out() && self.connection_count() != 0 {
                return false;
            }
        }
    }

    /// Domain 的 server_id；无 Domain 则 0。
    pub fn server_id(&self) -> u64 {
        self.domain().map_or(0, |domain| domain.server_id())
    }

    /// 登记内部会话及其开始时间戳。
    pub fn store_internal_session(&self, identity: usize, start_ts: u64) {
        self.internal_sessions
            .lock()
            .expect("internal sessions lock poisoned")
            .insert(identity, start_ts);
    }

    /// 是否已登记该内部会话。
    pub fn contains_internal_session(&self, identity: usize) -> bool {
        self.internal_sessions
            .lock()
            .expect("internal sessions lock poisoned")
            .contains_key(&identity)
    }

    /// 内部会话数量。
    pub fn internal_session_count(&self) -> usize {
        self.internal_sessions
            .lock()
            .expect("internal sessions lock poisoned")
            .len()
    }

    /// 删除内部会话登记。
    pub fn delete_internal_session(&self, identity: usize) {
        self.internal_sessions
            .lock()
            .expect("internal sessions lock poisoned")
            .remove(&identity);
    }

    /// 全部内部会话的 start_ts 列表。
    pub fn internal_session_start_timestamps(&self) -> Vec<u64> {
        self.internal_sessions
            .lock()
            .expect("internal sessions lock poisoned")
            .values()
            .copied()
            .collect()
    }

    /// 关闭不在保留集合中的连接。
    pub fn kill_connections_not_in(&self, retained: &HashSet<u64>) {
        let targets: Vec<_> = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .iter()
            .filter(|(id, _)| !retained.contains(id))
            .map(|(_, connection)| Arc::clone(connection))
            .collect();
        for connection in targets {
            connection.close();
        }
    }

    /// 更新或清空 TLS 配置。
    pub fn update_tls_config(&self, config: Option<TlsConfig>) {
        *self.tls_config.write().expect("TLS config lock poisoned") = config.map(Arc::new);
    }

    /// Whether new SQL connections currently advertise and accept TLS.
    pub fn sql_tls_enabled(&self) -> bool {
        self.session_tls_config
            .read()
            .expect("session TLS config lock poisoned")
            .is_some()
    }

    /// Returns the TLS configuration currently used for new SQL connections.
    pub fn sql_tls_config(&self) -> Option<Arc<rustls::ServerConfig>> {
        self.session_tls_config
            .read()
            .expect("session TLS config lock poisoned")
            .clone()
    }

    /// Reload SQL TLS material from the configured paths.
    ///
    /// A normal failure preserves the last usable configuration. `NO ROLLBACK
    /// ON ERROR` clears TLS only when secure transport is not mandatory, which
    /// matches TiDB's lock-out prevention rule.
    pub fn reload_tls(&self, no_rollback_on_error: bool) -> Result<(), String> {
        match build_sql_tls_config(&self.config) {
            Ok(config) => {
                let metadata = if config.is_some() {
                    build_sql_tls_metadata(&self.config)?
                } else {
                    None
                };
                *self
                    .session_tls_config
                    .write()
                    .expect("session TLS config lock poisoned") = config;
                self.update_tls_config(metadata);
                Ok(())
            }
            Err(_)
                if no_rollback_on_error
                    && !astersql_util::misc::RequireSecureTransportEnabled() =>
            {
                *self
                    .session_tls_config
                    .write()
                    .expect("session TLS config lock poisoned") = None;
                self.update_tls_config(None);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// 当前 TLS 配置。
    pub fn tls_config(&self) -> Option<Arc<TlsConfig>> {
        self.tls_config
            .read()
            .expect("TLS config lock poisoned")
            .clone()
    }

    /// 健康标志（run 成功后为 true，关机为 false）。
    pub fn health(&self) -> bool {
        self.health.load(Ordering::Acquire)
    }

    /// 标记强制关闭。
    pub fn set_force_shutdown(&self) {
        self.force_shutdown.store(true, Ordering::Release);
    }

    /// 是否强制关闭。
    pub fn force_shutdown(&self) -> bool {
        self.force_shutdown.load(Ordering::Acquire)
    }

    /// 标记需要请求 manager 释放。
    pub fn set_need_request_manager_free(&self) {
        self.need_request_manager_free
            .store(true, Ordering::Release);
    }

    /// 是否需要请求 manager 释放。
    pub fn need_request_manager_free(&self) -> bool {
        self.need_request_manager_free.load(Ordering::Acquire)
    }

    /// 设置 AutoID 服务。
    pub fn set_auto_id_service(&self, service: Arc<dyn AutoIdService>) {
        *self.auto_id_service.write().expect("auto id lock poisoned") = Some(service);
    }

    /// 取出并关闭 AutoID 服务。
    pub fn auto_id_service_close(&self) {
        if let Some(service) = self
            .auto_id_service
            .write()
            .expect("auto id lock poisoned")
            .take()
        {
            service.close();
        }
    }

    /// 本实例是否为 AutoID owner。
    pub fn is_auto_id_owner(&self) -> bool {
        self.auto_id_service
            .read()
            .expect("auto id lock poisoned")
            .as_ref()
            .is_some_and(|service| service.is_owner())
    }

    /// 阻塞直到连接数为 0（条件变量唤醒）。
    pub fn wait_zero_connections(&self) {
        let mut guard = self
            .clients_wait_lock
            .lock()
            .expect("clients wait lock poisoned");
        while self.connection_count() != 0 {
            guard = self
                .clients_changed
                .wait(guard)
                .expect("clients wait lock poisoned");
        }
    }

    /// 轮询等待连接数为 0。
    pub fn wait_zero_connections_polling(&self, interval: Duration) {
        while self.connection_count() != 0 {
            thread::sleep(interval);
        }
    }
}

/// 生成每个连接独立的 20 字节握手 salt，避免固定认证挑战值。
fn authentication_salt(sequence: u64) -> Vec<u8> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut state = timestamp ^ sequence.rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15;
    (0..20)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let byte = state as u8;
            if byte == 0 { 1 } else { byte }
        })
        .collect()
}

impl ManagedConnection for ClientConn {
    fn id(&self) -> u64 {
        self.connection_id()
    }

    fn capability(&self) -> u32 {
        self.negotiated_capability()
    }

    fn process_info(&self) -> Option<ProcessInfo> {
        let (user, database, attributes) = self.identity_snapshot();
        let context = self.getCtx().ok().flatten();
        let process = context
            .as_ref()
            .map(|context| context.process_snapshot())
            .unwrap_or(crate::conn::SessionProcessSnapshot {
                sql: String::new(),
                command: astersql_parser_mysql::r#const::ComSleep,
                start_time: SystemTime::now(),
            });
        let state = context.map_or(0, |context| context.state().status);
        let command = astersql_parser_mysql::r#const::Command2Str
            .iter()
            .find_map(|&(value, name)| (value == process.command).then_some(name))
            .unwrap_or("")
            .to_owned();
        let (host, port) = self.peer_address_snapshot();
        Some(ProcessInfo {
            connection_id: self.connection_id(),
            user,
            host,
            port,
            database,
            command,
            sql: process.sql,
            start_time: process.start_time,
            state,
            internal: false,
            attributes: attributes.into_iter().collect(),
        })
    }

    fn transaction_info(&self) -> Option<TransactionInfo> {
        None
    }

    fn connection_attributes(&self) -> HashMap<String, String> {
        self.identity_snapshot().2.into_iter().collect()
    }

    fn status_variables(&self) -> HashMap<String, String> {
        self.compressionStats()
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect()
    }

    fn update_cpu_time(&self, _sql_id: u64, _cpu_time: Duration) {}

    fn kill_query(&self, _max_execution_time: bool, _runaway: bool) {
        self.cancelDispatch();
    }

    fn close(&self) {
        let _ = ClientConn::Close(self);
    }
}

fn to_session_process_info(process: ProcessInfo) -> SessionProcessInfo {
    let command = astersql_parser_mysql::r#const::Command2Str
        .iter()
        .find_map(|&(value, name)| (name == process.command).then_some(value))
        .unwrap_or(astersql_parser_mysql::r#const::ComSleep);
    SessionProcessInfo {
        Time: process.start_time,
        User: process.user,
        Host: process.host,
        DB: process.database,
        Info: process.sql,
        Port: process.port,
        ID: process.connection_id,
        State: process.state,
        Command: command,
        ..SessionProcessInfo::default()
    }
}

fn internal_session_identity(session: &InternalSession) -> usize {
    Arc::as_ptr(session) as *const () as usize
}

impl InfoSchemaCoordinator for Server {
    fn StoreInternalSession(&self, session: InternalSession) {
        self.store_internal_session(internal_session_identity(&session), 0);
    }

    fn DeleteInternalSession(&self, session: &InternalSession) {
        self.delete_internal_session(internal_session_identity(session));
    }

    fn ContainsInternalSession(&self, session: &InternalSession) -> bool {
        self.contains_internal_session(internal_session_identity(session))
    }

    fn InternalSessionCount(&self) -> isize {
        self.internal_session_count() as isize
    }

    fn CheckOldRunningTxn(
        &self,
        _jobs: &mut HashMap<i64, Arc<astersql_session_sessmgr::mdldef::JobMDL>>,
    ) {
        // Canonical ManagedConnection does not expose MDL yet, so there are no
        // session-side JobMDL snapshots to merge.
    }

    fn KillNonFlashbackClusterConn(&self) {
        let targets = self
            .clients
            .read()
            .expect("clients lock poisoned")
            .values()
            .filter(|connection| !connection.is_system_session())
            .cloned()
            .collect::<Vec<_>>();
        for connection in targets {
            connection.close();
        }
    }
}

impl NormalCloseKiller for Server {
    fn KillWithNormalCloseMsg(
        &self,
        connectionID: u64,
        query: bool,
        maxExecutionTime: bool,
        runaway: bool,
        normalCloseMsg: &str,
    ) {
        Server::kill(
            self,
            connectionID,
            query,
            maxExecutionTime,
            runaway,
            Some(normalCloseMsg),
        );
    }
}

impl SessionManager for Server {
    fn ShowProcessList(&self) -> HashMap<u64, Arc<SessionProcessInfo>> {
        let mut processes = self
            .show_process_list()
            .into_iter()
            .map(|(id, process)| (id, Arc::new(to_session_process_info(process))))
            .collect::<HashMap<_, _>>();
        if let Some(domain) = self.domain() {
            processes.extend(domain.system_process_list());
        }
        processes
    }

    fn ShowTxnList(&self) -> Vec<Arc<astersql_session_sessmgr::txninfo::TxnInfo>> {
        self.transaction_list()
            .into_iter()
            .map(|transaction| {
                let process = self.process_info(transaction.connection_id);
                Arc::new(astersql_session_sessmgr::txninfo::TxnInfo {
                    StartTS: transaction.start_ts,
                    CurrentSQLDigest: transaction.current_sql_digest,
                    ProcessInfo: process.map(|process| {
                        astersql_session_sessmgr::txninfo::ProcessInfo {
                            ConnectionID: process.connection_id,
                            Username: process.user,
                            CurrentDB: process.database,
                            ..Default::default()
                        }
                    }),
                    ..Default::default()
                })
            })
            .collect()
    }

    fn GetProcessInfo(&self, id: u64) -> Option<Arc<SessionProcessInfo>> {
        let process = self
            .process_info(id)
            .map(to_session_process_info)
            .map(Arc::new);
        process.or_else(|| {
            self.domain()
                .and_then(|domain| domain.system_process_list().remove(&id))
        })
    }

    fn Kill(&self, connectionID: u64, query: bool, maxExecutionTime: bool, runaway: bool) {
        if !Server::kill(self, connectionID, query, maxExecutionTime, runaway, None)
            && let Some(domain) = self.domain()
        {
            domain.kill_system_process(connectionID);
        }
    }

    fn KillAllConnections(&self) {
        self.kill_all_connections();
    }

    fn UpdateTLSConfig(&self, config: Option<Arc<rustls::ServerConfig>>) {
        *self
            .session_tls_config
            .write()
            .expect("session TLS config lock poisoned") = config;
    }

    fn ServerID(&self) -> u64 {
        self.server_id()
    }

    fn GetInternalSessionStartTSList(&self) -> Vec<u64> {
        self.internal_session_start_timestamps()
    }

    fn GetConAttrs(
        &self,
        user: &astersql_session_sessmgr::auth::UserIdentity,
    ) -> HashMap<u64, HashMap<String, String>> {
        self.clients
            .read()
            .expect("clients lock poisoned")
            .iter()
            .filter_map(|(id, connection)| {
                connection
                    .process_info()
                    .filter(|process| {
                        process.user == user.username && process.host == user.hostname
                    })
                    .map(|_| (*id, connection.connection_attributes()))
            })
            .collect()
    }

    fn GetStatusVars(&self) -> HashMap<u64, HashMap<String, String>> {
        self.status_variables()
    }

    fn GetPerformanceSchemaAccountSummaries(&self) -> Vec<PerformanceSchemaAccountSummary> {
        let clients = self.clients.read().expect("clients lock poisoned");
        let mut summaries = self
            .performance_schema_account_totals
            .lock()
            .expect("performance_schema account totals lock poisoned")
            .iter()
            .map(|(key, total)| {
                (
                    key.clone(),
                    PerformanceSchemaAccountSummary {
                        user: key.0.clone(),
                        host: key.1.clone(),
                        current_connections: 0,
                        total_connections: *total,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        for connection in clients
            .values()
            .filter(|connection| !connection.is_system_session())
        {
            let Some(process) = connection.process_info() else {
                continue;
            };
            let key = (
                (!process.user.is_empty()).then_some(process.user),
                (!process.host.is_empty()).then_some(process.host),
            );
            let summary =
                summaries
                    .entry(key.clone())
                    .or_insert_with(|| PerformanceSchemaAccountSummary {
                        user: key.0,
                        host: key.1,
                        current_connections: 0,
                        total_connections: 0,
                    });
            summary.current_connections += 1;
        }
        let mut summaries = summaries.into_values().collect::<Vec<_>>();
        summaries.sort_by(|left, right| (&left.user, &left.host).cmp(&(&right.user, &right.host)));
        summaries
    }

    fn as_normal_close_killer(&self) -> Option<&dyn NormalCloseKiller> {
        Some(self)
    }
}

impl ConnectionServer for Server {
    fn register_connection(&self, connection: Arc<ClientConn>) -> Result<(), ConnError> {
        let connection: Arc<dyn ManagedConnection> = connection;
        Server::register_connection(self, connection).map_err(ConnError::Session)
    }

    fn unregister_connection(&self, connection_id: u64) {
        Server::unregister_connection(self, connection_id);
    }

    fn connection_count(&self) -> usize {
        Server::connection_count(self)
    }

    fn config(&self) -> crate::conn::ServerConfig {
        let tls_enabled = self
            .session_tls_config
            .read()
            .expect("session TLS config lock poisoned")
            .is_some();
        crate::conn::ServerConfig {
            capability: if tls_enabled {
                self.capability() | (1 << 11)
            } else {
                self.capability() & !(1 << 11)
            },
            default_collation: 45,
            server_version: "8.0.11-TiDB-AsterSQL".into(),
            default_auth_plugin: crate::conn::AUTH_NATIVE_PASSWORD.into(),
            require_secure_transport: astersql_util::misc::RequireSecureTransportEnabled(),
            init_connect: String::new(),
            max_connections: self.config.max_connections,
        }
    }

    fn driver(&self) -> Arc<dyn SessionDriver> {
        self.connection_driver
            .read()
            .expect("connection driver lock poisoned")
            .as_ref()
            .expect("connection runtime is not configured")
            .clone()
    }

    fn domain(&self) -> Arc<dyn ConnectionDomain> {
        self.connection_domain
            .read()
            .expect("connection domain lock poisoned")
            .as_ref()
            .expect("connection runtime is not configured")
            .clone()
    }

    fn is_shutdown(&self) -> bool {
        Server::is_shutdown(self)
    }

    fn is_healthy(&self) -> bool {
        self.health()
    }

    fn connection_active(&self, _connection_id: u64) {}

    fn connection_closed(&self, connection_id: u64, reason: &str) {
        self.set_normal_closed_connection("", connection_id.to_string(), reason);
    }
}

/// 将 Server 适配为 Standby 激活所需接口。
impl StandbyReadyServer for Server {
    fn init_tidb_listener(&self) -> Result<(), String> {
        Server::init_tidb_listener(self)
    }
}

/// 将 Server 适配为 Standby 关闭协调接口。
impl StandbyShutdownServer for Server {
    fn health(&self) -> bool {
        Server::health(self)
    }

    fn normal_closed_connection(&self, keyspace: &str, connection_id: &str) -> Option<String> {
        Server::normal_closed_connection(self, keyspace, connection_id)
    }

    fn auto_id_service_close(&self) {
        Server::auto_id_service_close(self);
    }

    fn force_shutdown(&self) -> bool {
        Server::force_shutdown(self)
    }

    fn need_request_manager_free(&self) -> bool {
        Server::need_request_manager_free(self)
    }

    fn is_auto_id_owner(&self) -> bool {
        Server::is_auto_id_owner(self)
    }

    fn set_force_shutdown(&self) {
        Server::set_force_shutdown(self);
    }

    fn set_need_request_manager_free(&self) {
        Server::set_need_request_manager_free(self);
    }

    fn wait_zero_connections(&self) {
        Server::wait_zero_connections(self);
    }

    fn wait_zero_connections_timeout(&self, timeout: Duration) -> bool {
        Server::wait_zero_connections_timeout(self, timeout)
    }
}

/// Drop 时标记不健康并释放监听器。
impl Drop for Server {
    fn drop(&mut self) {
        self.health.store(false, Ordering::Release);
        if let Some(service) = self.postgres_service.get_mut().unwrap().take() {
            service.close();
        }
        self.postgres_listener
            .get_mut()
            .expect("PostgreSQL listener lock poisoned")
            .take();
        self.listener
            .get_mut()
            .expect("listener lock poisoned")
            .take();
        self.status_listener
            .get_mut()
            .expect("status listener lock poisoned")
            .take();
    }
}
