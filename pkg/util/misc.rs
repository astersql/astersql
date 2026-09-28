// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 杂项工具：重试/panic 恢复、语法错误包装、X.509/TLS、列元数据转 Proto、HTTP 与证书生成。
//
// 对应 Go `pkg/util/misc.go`。涵盖 RunWithRetry、WithRecovery、SQL 语法错误前缀、
// PKIX/SAN 解析、DistSQL 列信息序列化、TLS 证书加载与自动签发、集群内部 HTTP 客户端，
// 以及 INSERT/IMPORT INTO 的类型转换 Flags。

#![allow(non_snake_case, non_upper_case_globals)]

use anyhow::{Context as _, Error, anyhow};
use openssl::asn1::{Asn1Integer, Asn1Time};
use openssl::bn::BigNum;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::x509::extension::SubjectAlternativeName;
use openssl::x509::{X509, X509NameBuilder};
use std::any::Any;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Once, OnceLock, RwLock};
use std::thread;
use std::time::Duration;
use task_mysql::r#const::SQLMode;
use task_types_group::Flags;
use tokio_util::sync::CancellationToken;

/// 默认最大重试次数。
pub const DefaultMaxRetries: i32 = 30;
/// 重试基础间隔（毫秒）；实际睡眠为 `backoff * attempt`。
pub const RetryInterval: u64 = 500;

/// 在可重试错误时按指数式间隔重试，直到成功、不可重试或次数耗尽。
pub fn RunWithRetry<F>(retryCnt: i32, backoff: u64, mut f: F) -> Result<(), Error>
where
    F: FnMut() -> (bool, Option<Error>),
{
    let mut last_error = None;
    for attempt in 1..=retryCnt {
        let (retryable, error) = f();
        if error.is_none() || !retryable {
            return error.map_or(Ok(()), Err);
        }
        last_error = error;
        thread::sleep(Duration::from_millis(backoff * attempt as u64));
    }
    last_error.map_or(Ok(()), Err)
}

/// 用 catch_unwind 包裹执行；panic 时调用 recoverFn 并记录错误日志。
pub fn WithRecovery<Exec, RecoverFn>(exec: Exec, recoverFn: Option<RecoverFn>)
where
    Exec: FnOnce(),
    RecoverFn: FnOnce(Option<&(dyn Any + Send)>),
{
    match catch_unwind(AssertUnwindSafe(exec)) {
        Ok(()) => {
            if let Some(recover) = recoverFn {
                recover(None);
            }
        }
        Err(payload) => {
            if let Some(recover) = recoverFn {
                recover(Some(payload.as_ref()));
            }
            log::error!(
                "panic in the recoverable goroutine: {}",
                panic_text(payload.as_ref())
            );
        }
    }
}

/// 将 panic payload 转为可读字符串。
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

// Recover performs the post-catch half of Go's deferred recovery helper. Rust
// callers pass catch_unwind's payload explicitly because Rust has no recover().
/// 处理已捕获的 panic：打日志、可选回调；`quit` 为真时延迟退出进程。
pub fn Recover(
    payload: Option<Box<dyn Any + Send>>,
    metricsLabel: &str,
    funcInfo: &str,
    recoverFn: Option<fn()>,
    quit: bool,
) {
    let Some(payload) = payload else { return };
    let text = panic_text(payload.as_ref());
    log::error!(
        "panic in the recoverable goroutine; label={metricsLabel}; funcInfo={funcInfo}; panic={text}"
    );
    if let Some(recover) = recoverFn {
        recover();
    }
    if quit {
        thread::sleep(Duration::from_secs(15));
        std::process::exit(1);
    }
}

/// 查询 CancellationToken 是否已取消。
pub fn HasCancelled(ctx: &CancellationToken) -> bool {
    ctx.is_cancelled()
}

/// MySQL/TiDB 风格 SQL 语法错误消息前缀。
pub const SyntaxErrorPrefix: &str = "You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use";

/// 包装后的 SQL 语法错误；`warning` 表示作为警告而非硬错误。
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SqlSyntaxError {
    pub message: String,
    pub warning: bool,
}

/// 将任意错误包装为语法错误（非 warning）。
pub fn SyntaxError(err: Option<Error>) -> Option<Error> {
    syntax_issue(err, false)
}

/// 将任意错误包装为语法警告。
pub fn SyntaxWarn(err: Option<Error>) -> Option<Error> {
    syntax_issue(err, true)
}

/// 已是 SqlSyntaxError 则原样返回，否则加前缀包装。
fn syntax_issue(err: Option<Error>, warning: bool) -> Option<Error> {
    let err = err?;
    if err.downcast_ref::<SqlSyntaxError>().is_some() {
        return Some(err);
    }
    Some(Error::new(SqlSyntaxError {
        message: format!("{SyntaxErrorPrefix}: {err}"),
        warning,
    }))
}

/// X.509 DN 短名：国家。
pub const Country: &str = "C";
/// X.509 DN 短名：组织。
pub const Organization: &str = "O";
/// X.509 DN 短名：组织单位。
pub const OrganizationalUnit: &str = "OU";
/// X.509 DN 短名：地区。
pub const Locality: &str = "L";
/// X.509 DN 短名：邮箱。
pub const Email: &str = "emailAddress";
/// X.509 DN 短名：通用名。
pub const CommonName: &str = "CN";
/// X.509 DN 短名：省/州。
pub const Province: &str = "ST";

/// OID → DN 短名对照表。
pub static pkixAttributeTypeNames: &[(&str, &str)] = &[
    ("2.5.4.6", Country),
    ("2.5.4.10", Organization),
    ("2.5.4.11", OrganizationalUnit),
    ("2.5.4.3", CommonName),
    ("2.5.4.5", "SERIALNUMBER"),
    ("2.5.4.7", Locality),
    ("2.5.4.8", Province),
    ("2.5.4.9", "STREET"),
    ("2.5.4.17", "POSTALCODE"),
    ("1.2.840.113549.1.9.1", Email),
];

/// DN 短名 → OID 反向索引（懒加载）。
static pkixTypeNameAttributes: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

/// 获取短名到 OID 的映射。
fn pkix_type_name_attributes() -> &'static HashMap<&'static str, &'static str> {
    pkixTypeNameAttributes.get_or_init(|| {
        pkixAttributeTypeNames
            .iter()
            .map(|(oid, name)| (*name, *oid))
            .collect()
    })
}

/// PKIX 属性类型与值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PkixAttributeTypeAndValue {
    pub Type: Vec<u32>,
    pub Value: String,
}

/// PKIX 名称（属性列表）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PkixName {
    pub Names: Vec<PkixAttributeTypeAndValue>,
}

/// 按短名构造 mock PKIX 属性（测试用）。
pub fn MockPkixAttribute(name: &str, value: &str) -> PkixAttributeTypeAndValue {
    let oid = pkix_type_name_attributes()
        .get(name)
        .unwrap_or_else(|| panic!("unsupport mock type: {name}"));
    PkixAttributeTypeAndValue {
        Type: oid
            .split('.')
            .map(|part| part.parse::<u32>().expect("static OID is valid"))
            .collect(),
        Value: value.to_owned(),
    }
}

/// 将 PkixName 格式化为 OpenSSL oneline 风格（`/CN=.../O=...`）。
pub fn X509NameOnline(name: PkixName) -> String {
    let mut entries = Vec::with_capacity(name.Names.len());
    for attribute in name.Names {
        let oid = attribute
            .Type
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".");
        if let Some((_, short_name)) = pkixAttributeTypeNames
            .iter()
            .find(|(candidate, _)| *candidate == oid)
        {
            entries.push(format!("{short_name}={}", attribute.Value));
        }
    }
    if entries.is_empty() {
        String::new()
    } else {
        format!("/{}", entries.join("/"))
    }
}

/// Subject Alternative Name 条目类型字符串。
pub type SANType = String;
/// SAN 键：URI。
pub const URI: &str = "URI";
/// SAN 键：DNS。
pub const DNS: &str = "DNS";
/// SAN 键：IP。
pub const IP: &str = "IP";

/// 解析并校验 SAN 串（`DNS:a,IP:1.2.3.4` 形式）。
pub fn ParseAndCheckSAN(san: &str) -> Result<HashMap<SANType, Vec<String>>, Error> {
    let mut parsed = HashMap::<SANType, Vec<String>>::new();
    for entry in san.split(',') {
        let Some((key, value)) = entry.split_once(':') else {
            return Err(anyhow!("invalid SAN value {entry}"));
        };
        let key = key.trim().to_uppercase();
        if !matches!(key.as_str(), URI | DNS | IP) {
            return Err(anyhow!(
                "unsupported SAN key {key}, current only support [URI, DNS, IP]"
            ));
        }
        parsed.entry(key).or_default().push(value.trim().to_owned());
    }
    Ok(parsed)
}

/// 校验 X509_NAME oneline 中每个属性短名是否受支持。
pub fn CheckSupportX509NameOneline(oneline: &str) -> Result<(), Error> {
    for entry in oneline.split('/') {
        if entry.is_empty() {
            continue;
        }
        let fields = entry.split('=').collect::<Vec<_>>();
        if fields.len() != 2 {
            return Err(anyhow!("invalid X509_NAME input: {oneline}"));
        }
        if !pkix_type_name_attributes().contains_key(fields[0]) {
            return Err(anyhow!(
                "Unsupport check '{}' in current version TiDB",
                fields[0]
            ));
        }
    }
    Ok(())
}

/// DistSQL/TiKV 侧列元信息的 Proto 友好结构。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProtoColumnInfo {
    pub ColumnId: i64,
    pub Collation: i32,
    pub ColumnLen: i32,
    pub Decimal: i32,
    pub Flag: i32,
    pub Elems: Vec<String>,
    pub Tp: i32,
    pub PkHandle: bool,
}

/// 列元数据抽象，供 ColumnsToProto 读取。
pub trait ColumnMetadata {
    fn id(&self) -> i64;
    fn collation_id(&self) -> i32;
    fn column_len(&self) -> i32;
    fn decimal(&self) -> i32;
    fn flags(&self) -> i32;
    fn elements(&self) -> Vec<String>;
    fn field_type(&self) -> i32;
    fn array_element_type(&self) -> i32;
    fn is_array(&self) -> bool;
    fn is_virtual_generated(&self) -> bool;
    fn is_primary_key(&self) -> bool;
}

/// 批量将列元数据转为 ProtoColumnInfo，并标记主键 handle。
pub fn ColumnsToProto<C: ColumnMetadata>(
    columns: &[C],
    pkIsHandle: bool,
    forIndex: bool,
    isTiFlashStore: bool,
) -> Vec<ProtoColumnInfo> {
    columns
        .iter()
        .map(|column| {
            let mut proto = ColumnToProto(column, forIndex, isTiFlashStore);
            proto.PkHandle = (pkIsHandle && column.is_primary_key()) || column.id() == -1;
            proto
        })
        .collect()
}

/// 单列转换；索引路径用数组元素类型，TiFlash 虚拟列打特殊 flag。
pub fn ColumnToProto<C: ColumnMetadata>(
    column: &C,
    forIndex: bool,
    isTiFlashStore: bool,
) -> ProtoColumnInfo {
    let mut proto = ProtoColumnInfo {
        ColumnId: column.id(),
        Collation: column.collation_id(),
        ColumnLen: column.column_len(),
        Decimal: column.decimal(),
        Flag: column.flags(),
        Elems: column.elements(),
        Tp: column.field_type(),
        PkHandle: false,
    };
    if isTiFlashStore && column.is_virtual_generated() {
        proto.Flag |= 1 << 23;
    }
    if forIndex {
        proto.Tp = column.array_element_type();
        if column.is_array() {
            proto.Collation = 63;
        }
    }
    proto
}

/// 预热 PKIX 反向索引。
pub fn init() {
    let _ = pkix_type_name_attributes();
}

/// 按 schema/序列名查找 SequenceTable 的函数指针类型。
pub type GetSequenceByNameFn =
    fn(is: &dyn Any, schema: &str, sequence: &str) -> Result<Box<dyn SequenceTable>, Error>;
/// 全局序列查找钩子（由 infoschema 等注入）。
pub static GetSequenceByName: OnceLock<GetSequenceByNameFn> = OnceLock::new();

/// SEQUENCE 对象接口：取 ID、NEXTVAL、SETVAL。
pub trait SequenceTable {
    fn GetSequenceID(&self) -> i64;
    fn GetSequenceNextVal(&self, ctx: &dyn Any, dbName: &str, seqName: &str) -> Result<i64, Error>;
    fn SetSequenceVal(
        &self,
        ctx: &dyn Any,
        newVal: i64,
        dbName: &str,
        seqName: &str,
    ) -> Result<(i64, bool), Error>;
}

/// TLS 客户端证书认证策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientAuthPolicy {
    NoClientCert,
    RequestClientCert,
    VerifyClientCertIfGiven,
    RequireAndVerifyClientCert,
}

/// 允许的最低 TLS 版本。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

/// 已加载的服务器证书与私钥。
#[derive(Clone)]
pub struct LoadedCertificate {
    pub certificate: X509,
    pub private_key: PKey<Private>,
}

/// 服务端 TLS 配置：CA、客户端认证策略、最低版本、密码套件与可热加载证书。
pub struct TlsConfig {
    pub ClientCAs: Vec<X509>,
    pub ClientAuth: ClientAuthPolicy,
    pub MinVersion: TlsVersion,
    pub CipherSuites: Vec<&'static str>,
    certificate: Arc<RwLock<LoadedCertificate>>,
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl TlsConfig {
    /// 读取当前已加载证书（克隆）。
    pub fn certificate(&self) -> LoadedCertificate {
        self.certificate
            .read()
            .expect("certificate lock poisoned")
            .clone()
    }

    /// 从磁盘重新加载证书；失败则保留旧证书并告警。
    pub fn reload_certificate(&self) -> Result<LoadedCertificate, Error> {
        match load_key_pair(&self.cert_path, &self.key_path) {
            Ok(certificate) => {
                *self.certificate.write().expect("certificate lock poisoned") = certificate.clone();
                Ok(certificate)
            }
            Err(error) => {
                log::warn!("could not load server certificate, using the old one: {error}");
                Ok(self.certificate())
            }
        }
    }
}

/// 是否要求安全传输（影响客户端认证策略）。
static RequireSecureTransport: AtomicBool = AtomicBool::new(false);
/// 最低 TLS 版本（12 或 13）。
static MinimumTlsVersion: AtomicU8 = AtomicU8::new(12);

/// 设置是否要求安全传输。
pub fn SetRequireSecureTransport(required: bool) {
    RequireSecureTransport.store(required, Ordering::Release);
}

/// Returns the process-wide `require_secure_transport` switch.
pub fn RequireSecureTransportEnabled() -> bool {
    RequireSecureTransport.load(Ordering::Acquire)
}

/// 设置最低 TLS 版本。
pub fn SetMinimumTLSVersion(version: TlsVersion) {
    MinimumTlsVersion.store(
        match version {
            TlsVersion::Tls12 => 12,
            TlsVersion::Tls13 => 13,
        },
        Ordering::Release,
    );
}

/// 加载或自动生成 TLS 证书，返回配置与是否启用自动重载。
pub fn LoadTLSCertificates(
    ca: &str,
    key: &str,
    cert: &str,
    autoTLS: bool,
    rsaKeySize: i32,
) -> Result<(Option<TlsConfig>, bool), Error> {
    let mut cert_path = PathBuf::from(cert);
    let mut key_path = PathBuf::from(key);
    let mut auto_reload = false;
    if cert.is_empty() || key.is_empty() {
        if !autoTLS {
            log::warn!("Automatic TLS Certificate creation is disabled");
            return Ok((None, false));
        }
        // 未提供证书路径时，在临时目录自动签发。
        auto_reload = true;
        let configured = task_config::get_global_config().temp_storage_path.clone();
        let temp_path = if configured.is_empty() {
            std::env::temp_dir()
        } else {
            PathBuf::from(configured)
        };
        cert_path = temp_path.join("cert.pem");
        key_path = temp_path.join("key.pem");
        CreateCertificates(
            &cert_path,
            &key_path,
            rsaKeySize,
            PublicKeyAlgorithm::Rsa,
            SignatureAlgorithm::Unspecified,
        )?;
    }

    let certificate = load_key_pair(&cert_path, &key_path)?;
    let require_tls = RequireSecureTransport.load(Ordering::Acquire);
    let mut client_auth = if require_tls {
        ClientAuthPolicy::RequestClientCert
    } else {
        ClientAuthPolicy::NoClientCert
    };
    let client_cas = if ca.is_empty() {
        Vec::new()
    } else {
        let bytes = fs::read(ca).with_context(|| format!("read CA file {ca}"))?;
        let certificates = X509::stack_from_pem(&bytes).unwrap_or_default();
        if !certificates.is_empty() {
            client_auth = if require_tls {
                ClientAuthPolicy::RequireAndVerifyClientCert
            } else {
                ClientAuthPolicy::VerifyClientCertIfGiven
            };
        }
        certificates
    };
    let min_version = if MinimumTlsVersion.load(Ordering::Acquire) >= 13 {
        TlsVersion::Tls13
    } else {
        TlsVersion::Tls12
    };
    let cipher_suites = vec![
        "TLS_AES_128_GCM_SHA256",
        "TLS_AES_256_GCM_SHA384",
        "TLS_CHACHA20_POLY1305_SHA256",
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
        "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    ];

    Ok((
        Some(TlsConfig {
            ClientCAs: client_cas,
            ClientAuth: client_auth,
            MinVersion: min_version,
            CipherSuites: cipher_suites,
            certificate: Arc::new(RwLock::new(certificate)),
            cert_path,
            key_path,
        }),
        auto_reload,
    ))
}

/// 从 PEM 文件加载证书与私钥并校验公钥匹配。
fn load_key_pair(cert: &Path, key: &Path) -> Result<LoadedCertificate, Error> {
    let certificate = X509::from_pem(
        &fs::read(cert).with_context(|| format!("read certificate {}", cert.display()))?,
    )?;
    let private_key = PKey::private_key_from_pem(
        &fs::read(key).with_context(|| format!("read private key {}", key.display()))?,
    )?;
    if !certificate.public_key()?.public_eq(&private_key) {
        return Err(anyhow!("certificate and private key do not match"));
    }
    Ok(LoadedCertificate {
        certificate,
        private_key,
    })
}

static internalClientInit: Once = Once::new();
static internalHTTPClient: OnceLock<reqwest::blocking::Client> = OnceLock::new();
static internalHTTPSchema: OnceLock<String> = OnceLock::new();

/// 懒初始化并返回集群内部 HTTP 客户端。
pub fn InternalHTTPClient() -> &'static reqwest::blocking::Client {
    internalClientInit.call_once(initInternalClient);
    internalHTTPClient
        .get()
        .expect("internal client initialized")
}

/// 返回内部 HTTP schema（`http` 或 `https`）。
pub fn InternalHTTPSchema() -> &'static str {
    internalClientInit.call_once(initInternalClient);
    internalHTTPSchema
        .get()
        .expect("internal schema initialized")
}

/// 按集群 SSL 配置构建内部 HTTP 客户端与 schema。
fn initInternalClient() {
    let config = task_config::get_global_config();
    let security = &config.security;
    let tls_enabled = !security.cluster_ssl_ca.is_empty()
        || !security.cluster_ssl_cert.is_empty()
        || !security.cluster_ssl_key.is_empty();
    let mut builder = reqwest::blocking::Client::builder().timeout(Duration::from_secs(5 * 60));
    if tls_enabled {
        if !security.cluster_ssl_ca.is_empty() {
            let bytes =
                fs::read(&security.cluster_ssl_ca).expect("could not load cluster CA certificate");
            let ca = reqwest::Certificate::from_pem(&bytes)
                .expect("could not parse cluster CA certificate");
            builder = builder.add_root_certificate(ca);
        }
        if !security.cluster_ssl_cert.is_empty() && !security.cluster_ssl_key.is_empty() {
            let mut identity =
                fs::read(&security.cluster_ssl_cert).expect("could not load cluster certificate");
            identity.extend(
                fs::read(&security.cluster_ssl_key).expect("could not load cluster private key"),
            );
            builder = builder.identity(
                reqwest::Identity::from_pem(&identity)
                    .expect("could not parse cluster certificate identity"),
            );
        }
    }
    internalHTTPSchema
        .set(if tls_enabled { "https" } else { "http" }.to_owned())
        .expect("internal schema set once");
    internalHTTPClient
        .set(
            builder
                .build()
                .expect("could not create internal HTTP client"),
        )
        .unwrap_or_else(|_| panic!("internal client set once"));
}

/// 拼接地址与路径；无 scheme 时补上 InternalHTTPSchema。
pub fn ComposeURL(address: &str, path: &str) -> String {
    if address.starts_with("http://") || address.starts_with("https://") {
        format!("{address}{path}")
    } else {
        format!("{}://{address}{path}", InternalHTTPSchema())
    }
}

/// 取本机第一个全局单播 IP；失败返回空串。
pub fn GetLocalIP() -> String {
    get_if_addrs::get_if_addrs()
        .ok()
        .and_then(|interfaces| {
            interfaces
                .into_iter()
                .map(|interface| interface.ip())
                .find(is_global_unicast)
        })
        .map(|address| address.to_string())
        .unwrap_or_default()
}

/// 是否为非环回、非未指定、非组播的单播地址。
pub(crate) fn is_global_unicast(address: &IpAddr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && match address {
            IpAddr::V4(address) => !address.is_link_local(),
            IpAddr::V6(address) => !address.is_unicast_link_local(),
        }
}

/// 自签证书公钥算法。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicKeyAlgorithm {
    Rsa,
    Ecdsa,
    Ed25519,
}

/// 证书签名摘要算法。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureAlgorithm {
    Unspecified,
    Sha256,
    Sha384,
    Sha512,
}

/// 生成自签服务器证书与私钥文件（含主机名 SAN）。
pub fn CreateCertificates(
    certpath: impl AsRef<Path>,
    keypath: impl AsRef<Path>,
    rsaKeySize: i32,
    pubKeyAlgo: PublicKeyAlgorithm,
    signAlgo: SignatureAlgorithm,
) -> Result<(), Error> {
    let private_key = match pubKeyAlgo {
        PublicKeyAlgorithm::Rsa => PKey::from_rsa(Rsa::generate(
            u32::try_from(rsaKeySize).context("invalid RSA key size")?,
        )?)?,
        PublicKeyAlgorithm::Ecdsa => {
            let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
            PKey::from_ec_key(EcKey::generate(&group)?)?
        }
        PublicKeyAlgorithm::Ed25519 => PKey::generate_ed25519()?,
    };

    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_text("CN", "TiDB_Server_Auto_Generated_Server_Certificate")?;
    let name = name.build();
    let mut builder = X509::builder()?;
    builder.set_version(2)?;
    let serial_number = BigNum::from_u32(1)?;
    let serial = Asn1Integer::from_bn(&serial_number)?;
    builder.set_serial_number(&serial)?;
    builder.set_subject_name(&name)?;
    builder.set_issuer_name(&name)?;
    builder.set_pubkey(&private_key)?;
    let not_before = Asn1Time::days_from_now(0)?;
    let not_after = Asn1Time::days_from_now(90)?;
    builder.set_not_before(&not_before)?;
    builder.set_not_after(&not_after)?;
    let hostname = hostname::get()?.to_string_lossy().into_owned();
    let san = SubjectAlternativeName::new()
        .dns(&hostname)
        .build(&builder.x509v3_context(None, None))?;
    builder.append_extension(san)?;
    let digest = match pubKeyAlgo {
        PublicKeyAlgorithm::Ed25519 => MessageDigest::null(),
        _ => match signAlgo {
            SignatureAlgorithm::Unspecified | SignatureAlgorithm::Sha256 => MessageDigest::sha256(),
            SignatureAlgorithm::Sha384 => MessageDigest::sha384(),
            SignatureAlgorithm::Sha512 => MessageDigest::sha512(),
        },
    };
    builder.sign(&private_key, digest)?;

    fs::write(certpath.as_ref(), builder.build().to_pem()?)?;
    let mut key_file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(keypath.as_ref())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        key_file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    key_file.write_all(&private_key.private_key_to_pem_pkcs8()?)?;
    key_file.flush()?;
    log::info!(
        "TLS Certificates created; cert={}; key={}; validity=2160h; rsaKeySize={rsaKeySize}",
        certpath.as_ref().display(),
        keypath.as_ref().display(),
    );
    Ok(())
}

/// RSA + 默认摘要的 CreateCertificates 便捷封装。
fn createTLSCertificates(certpath: &Path, keypath: &Path, rsaKeySize: i32) -> Result<(), Error> {
    CreateCertificates(
        certpath,
        keypath,
        rsaKeySize,
        PublicKeyAlgorithm::Rsa,
        SignatureAlgorithm::Unspecified,
    )
}

/// 按 SQL Mode 与 ignoreErr 调整 INSERT 路径的类型转换 Flags。
pub fn GetTypeFlagsForInsert(baseFlags: Flags, sqlMode: SQLMode, ignoreErr: bool) -> Flags {
    let strictSQLMode = sqlMode.HasStrictMode();
    baseFlags
        .WithTruncateAsWarning(!strictSQLMode || ignoreErr)
        .WithIgnoreInvalidDateErr(sqlMode.HasAllowInvalidDatesMode())
        .WithIgnoreZeroInDate(
            !sqlMode.HasNoZeroInDateMode()
                || !sqlMode.HasNoZeroDateMode()
                || !strictSQLMode
                || ignoreErr
                || sqlMode.HasAllowInvalidDatesMode(),
        )
        .WithAllowNegativeToUnsigned(false)
}

/// IMPORT INTO 使用与 INSERT（非 ignore）相同的类型 Flags。
pub fn GetTypeFlagsForImportInto(baseFlags: Flags, sqlMode: SQLMode) -> Flags {
    GetTypeFlagsForInsert(baseFlags, sqlMode, false)
}
