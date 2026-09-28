// Copyright 2026 AsterSQL.

// TLS 证书加载、校验、热更新、状态元数据与自动签发测试。
//
// 场景逐一对应 Go tls_test.go；网络/数据库边界使用共享 TLS/Server 实现，
// 文件系统边界使用独立临时目录，避免测试间证书互相覆盖。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::x509::extension::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
};
use openssl::x509::{X509, X509NameBuilder};
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};

use astersql_server::server::{Domain, Server, ServerConfig, ServerDriver, TlsConfig};
use astersql_server::stat::{SSL_SERVER_NOT_AFTER, SSL_SERVER_NOT_BEFORE, StatusValue};
use astersql_util::misc::{
    ClientAuthPolicy, CreateCertificates, LoadTLSCertificates, PublicKeyAlgorithm,
    SetRequireSecureTransport, SignatureAlgorithm,
};

/// 串行化会触碰全局 config、metrics 与 TLS 原子开关的测试。
pub(crate) fn global_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const GO_TLS_SCENARIOS: &[&str] = &[
    "TLSVerify",
    "TLSBasic",
    "ErrorNoRollback",
    "ReloadTLS",
    "StatusAPIWithTLS",
    "StatusAPIWithTLSCNCheck",
    "InvalidTLS",
    "TLSAuto",
];

const RUST_TLS_SCENARIOS: &[&str] = &[
    "TLSVerify",
    "TLSBasic",
    "ErrorNoRollback",
    "ReloadTLS",
    "StatusAPIWithTLS",
    "StatusAPIWithTLSCNCheck",
    "InvalidTLS",
    "TLSAuto",
];

struct Driver;
impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "tls"
    }
}

struct TestDomain {
    start_timestamp: i64,
}

impl Domain for TestDomain {
    fn server_id(&self) -> u64 {
        1
    }

    fn start_timestamp(&self) -> i64 {
        self.start_timestamp
    }
}

struct RunningServer(Arc<Server>);

impl RunningServer {
    fn start(config: ServerConfig) -> Self {
        let server = Server::new(config, Arc::new(Driver)).expect("create TLS test server");
        server
            .run(Arc::new(TestDomain {
                start_timestamp: unix_now(),
            }))
            .expect("start TLS test server");
        Self(server)
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.0.close();
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "astersql-server-tls-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create TLS test temp directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn generated_pair(dir: &Path, stem: &str) -> (PathBuf, PathBuf) {
    let cert = dir.join(format!("{stem}-cert.pem"));
    let key = dir.join(format!("{stem}-key.pem"));
    CreateCertificates(
        &cert,
        &key,
        2048,
        PublicKeyAlgorithm::Rsa,
        SignatureAlgorithm::Unspecified,
    )
    .expect("generate TLS certificate");
    (cert, key)
}

struct GeneratedCertificate {
    certificate: X509,
    private_key: PKey<Private>,
    certificate_path: PathBuf,
    key_path: PathBuf,
}

fn generate_cert(
    serial: u32,
    common_name: &str,
    parent: Option<&GeneratedCertificate>,
    directory: &Path,
    stem: &str,
    not_before_unix: i64,
    not_after_unix: i64,
) -> GeneratedCertificate {
    let private_key = PKey::from_rsa(Rsa::generate(2048).expect("generate RSA key")).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", common_name).unwrap();
    let name = name.build();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    let serial = BigNum::from_u32(serial).unwrap().to_asn1_integer().unwrap();
    builder.set_serial_number(&serial).unwrap();
    builder.set_subject_name(&name).unwrap();
    builder
        .set_issuer_name(parent.map_or(name.as_ref(), |ca| ca.certificate.subject_name()))
        .unwrap();
    builder.set_pubkey(&private_key).unwrap();
    builder
        .set_not_before(&Asn1Time::from_unix(not_before_unix).unwrap())
        .unwrap();
    builder
        .set_not_after(&Asn1Time::from_unix(not_after_unix).unwrap())
        .unwrap();
    if parent.is_none() {
        builder
            .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        builder
            .append_extension(
                KeyUsage::new()
                    .key_cert_sign()
                    .digital_signature()
                    .build()
                    .unwrap(),
            )
            .unwrap();
    } else {
        builder
            .append_extension(
                KeyUsage::new()
                    .key_encipherment()
                    .digital_signature()
                    .build()
                    .unwrap(),
            )
            .unwrap();
        builder
            .append_extension(
                ExtendedKeyUsage::new()
                    .server_auth()
                    .client_auth()
                    .build()
                    .unwrap(),
            )
            .unwrap();
    }
    let subject_alt_name = SubjectAlternativeName::new()
        .dns(common_name)
        .build(&builder.x509v3_context(parent.map(|ca| ca.certificate.as_ref()), None))
        .unwrap();
    builder.append_extension(subject_alt_name).unwrap();
    let signer = parent.map_or(&private_key, |ca| &ca.private_key);
    builder.sign(signer, MessageDigest::sha256()).unwrap();
    let certificate = builder.build();
    let certificate_path = directory.join(format!("{stem}-cert.pem"));
    let key_path = directory.join(format!("{stem}-key.pem"));
    std::fs::write(&certificate_path, certificate.to_pem().unwrap()).unwrap();
    std::fs::write(
        &key_path,
        private_key.rsa().unwrap().private_key_to_pem().unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    GeneratedCertificate {
        certificate,
        private_key,
        certificate_path,
        key_path,
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn tls_client_config(
    ca: &Path,
    client: Option<&GeneratedCertificate>,
) -> Arc<rustls::ClientConfig> {
    let mut options = vec![astersql_util::security::WithCAPath(
        path_text(ca).to_owned(),
    )];
    if let Some(client) = client {
        options.push(astersql_util::security::WithCertAndKeyPath(
            path_text(&client.certificate_path).to_owned(),
            path_text(&client.key_path).to_owned(),
        ));
    }
    astersql_util::security::NewTLSConfig(options)
        .unwrap()
        .unwrap()
        .client_config()
        .unwrap()
}

fn https_get(
    address: std::net::SocketAddr,
    server_name: &str,
    config: Arc<rustls::ClientConfig>,
) -> Result<Vec<u8>, String> {
    let socket = TcpStream::connect(address).map_err(|error| error.to_string())?;
    let server_name = ServerName::try_from(server_name.to_owned()).map_err(|e| e.to_string())?;
    let connection = ClientConnection::new(config, server_name).map_err(|e| e.to_string())?;
    let mut stream = StreamOwned::new(connection, socket);
    stream
        .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => response.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(response)
}

fn tls_handshake(
    server_config: Arc<rustls::ServerConfig>,
    client_config: Arc<rustls::ClientConfig>,
) -> Result<(), String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    let server = std::thread::spawn(move || -> Result<(), String> {
        let (mut socket, _) = listener.accept().map_err(|error| error.to_string())?;
        let mut connection =
            rustls::ServerConnection::new(server_config).map_err(|error| error.to_string())?;
        while connection.is_handshaking() {
            connection
                .complete_io(&mut socket)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    let mut socket = TcpStream::connect(address).map_err(|error| error.to_string())?;
    let mut connection = ClientConnection::new(
        client_config,
        ServerName::try_from("localhost")
            .map_err(|error| error.to_string())?
            .to_owned(),
    )
    .map_err(|error| error.to_string())?;
    let client_result = (|| {
        while connection.is_handshaking() {
            connection
                .complete_io(&mut socket)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })();
    let server_result = server
        .join()
        .map_err(|_| "TLS server thread panicked".to_owned())?;
    client_result.and(server_result)
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("TLS test path must be UTF-8")
}

#[test]
fn canonical_go_tls_scenarios_are_all_covered() {
    assert_eq!(RUST_TLS_SCENARIOS, GO_TLS_SCENARIOS);
}

/// TestTLSVerify：CA 存在时安全传输要求客户端证书，并按 CN 白名单验签。
#[test]
fn test_tls_verify() {
    let _lock = global_test_lock();
    let temp = TempDir::new("verify");
    let now = unix_now();
    let ca = generate_cert(
        1,
        "AsterSQL CA",
        None,
        temp.path(),
        "ca",
        now - 60,
        now + 3600,
    );
    let server_certificate = generate_cert(
        2,
        "localhost",
        Some(&ca),
        temp.path(),
        "server",
        now - 60,
        now + 3600,
    );
    let client = generate_cert(
        3,
        "SQL Client Certificate",
        Some(&ca),
        temp.path(),
        "client",
        now - 60,
        now + 3600,
    );
    let other_ca = generate_cert(
        4,
        "Other CA",
        None,
        temp.path(),
        "other-ca",
        now - 60,
        now + 3600,
    );
    let untrusted_client = generate_cert(
        5,
        "untrusted-client",
        Some(&other_ca),
        temp.path(),
        "untrusted-client",
        now - 60,
        now + 3600,
    );

    SetRequireSecureTransport(false);
    let (loaded, auto_reload) = LoadTLSCertificates(
        path_text(&ca.certificate_path),
        path_text(&server_certificate.key_path),
        path_text(&server_certificate.certificate_path),
        false,
        2048,
    )
    .expect("load CA-backed TLS config");
    assert!(!auto_reload);
    assert_eq!(
        loaded.expect("TLS config").ClientAuth,
        ClientAuthPolicy::VerifyClientCertIfGiven
    );

    let server = RunningServer::start(ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        sql_tls_ca: Some(path_text(&ca.certificate_path).into()),
        sql_tls_certificate: Some(path_text(&server_certificate.certificate_path).into()),
        sql_tls_key: Some(path_text(&server_certificate.key_path).into()),
        status: astersql_server::server::StatusConfig {
            report_status: false,
            ..Default::default()
        },
        ..ServerConfig::default()
    });
    let server_tls = server.0.sql_tls_config().unwrap();
    SetRequireSecureTransport(true);
    let connection_config = astersql_server::conn::ConnectionServer::config(server.0.as_ref());
    assert!(connection_config.require_secure_transport);
    tls_handshake(
        Arc::clone(&server_tls),
        tls_client_config(&ca.certificate_path, None),
    )
    .expect("TLS without a client certificate remains allowed");
    tls_handshake(
        Arc::clone(&server_tls),
        tls_client_config(&ca.certificate_path, Some(&client)),
    )
    .expect("CA-signed client certificate");
    assert!(
        tls_handshake(
            server_tls,
            tls_client_config(&ca.certificate_path, Some(&untrusted_client)),
        )
        .is_err(),
        "presented client certificates must be CA-verified"
    );
    SetRequireSecureTransport(false);

    let config = TlsConfig {
        ca_path: Some(path_text(&ca.certificate_path).to_owned()),
        verify_common_names: vec!["SQL Client Certificate".into()],
        ..TlsConfig::default()
    };
    assert!(
        config
            .verify_peer_common_name(&untrusted_client.certificate.to_pem().unwrap())
            .is_err()
    );
    assert!(
        config
            .verify_peer_common_name(&client.certificate.to_pem().unwrap())
            .is_ok()
    );
}

/// TestTLSBasic：加载无 CA 的证书时不校验客户端，并向 status 暴露证书有效期。
#[test]
fn test_tls_basic() {
    let _lock = global_test_lock();
    let temp = TempDir::new("basic");
    let (cert, key) = generated_pair(temp.path(), "server");
    let (loaded, auto_reload) =
        LoadTLSCertificates("", path_text(&key), path_text(&cert), false, 1024).unwrap();
    let loaded = loaded.expect("TLS config");
    assert!(!auto_reload);
    assert_eq!(loaded.ClientAuth, ClientAuthPolicy::NoClientCert);
    let certificate = loaded.certificate().certificate;
    let not_before = certificate.not_before().to_string();
    let not_after = certificate.not_after().to_string();

    let server = RunningServer::start(ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        sql_tls_certificate: Some(path_text(&cert).into()),
        sql_tls_key: Some(path_text(&key).into()),
        status: astersql_server::server::StatusConfig {
            report_status: false,
            ..Default::default()
        },
        ..ServerConfig::default()
    });
    tls_handshake(
        server.0.sql_tls_config().unwrap(),
        tls_client_config(&cert, None),
    )
    .expect("TLS connection without configured server CA");
    let stats = server.0.statistics();
    assert!(!not_before.is_empty() && !not_after.is_empty());
    assert_eq!(
        stats[SSL_SERVER_NOT_BEFORE],
        StatusValue::String(not_before)
    );
    assert_eq!(stats[SSL_SERVER_NOT_AFTER], StatusValue::String(not_after));
}

/// TestErrorNoRollback：磁盘热加载失败保留旧证书，显式 no-rollback 才清空 TLS。
#[test]
fn test_error_no_rollback() {
    let _lock = global_test_lock();
    let temp = TempDir::new("no-rollback");
    let now = unix_now();
    let ca = generate_cert(
        1,
        "AsterSQL CA",
        None,
        temp.path(),
        "ca",
        now - 60,
        now + 3600,
    );
    let generated = generate_cert(
        2,
        "localhost",
        Some(&ca),
        temp.path(),
        "server",
        now - 60,
        now + 3600,
    );
    let cert = generated.certificate_path.clone();
    let key = generated.key_path.clone();
    let (loaded, _) = LoadTLSCertificates(
        path_text(&ca.certificate_path),
        path_text(&key),
        path_text(&cert),
        false,
        2048,
    )
    .unwrap();
    let loaded = loaded.unwrap();
    let original = loaded.certificate().certificate.to_der().unwrap();
    let server = RunningServer::start(ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        sql_tls_ca: Some(path_text(&ca.certificate_path).into()),
        sql_tls_certificate: Some(path_text(&cert).into()),
        sql_tls_key: Some(path_text(&key).into()),
        status: astersql_server::server::StatusConfig {
            report_status: false,
            ..Default::default()
        },
        ..ServerConfig::default()
    });
    let server_tls = server.0.sql_tls_config().unwrap();
    let client_config = tls_client_config(&ca.certificate_path, None);
    assert!(server.0.sql_tls_enabled());
    tls_handshake(Arc::clone(&server_tls), Arc::clone(&client_config))
        .expect("initial TLS handshake");
    std::fs::remove_file(&key).unwrap();
    let retained = loaded.reload_certificate().unwrap();
    assert_eq!(retained.certificate.to_der().unwrap(), original);
    assert!(server.0.reload_tls(false).is_err());
    assert!(
        server.0.sql_tls_enabled(),
        "failed reload must retain old TLS"
    );
    tls_handshake(server_tls, Arc::clone(&client_config))
        .expect("failed reload must retain the usable certificate");
    SetRequireSecureTransport(true);
    assert!(server.0.reload_tls(true).is_err());
    assert!(
        server.0.sql_tls_enabled(),
        "secure transport must not allow TLS to be disabled"
    );
    SetRequireSecureTransport(false);
    server.0.reload_tls(true).unwrap();
    assert!(
        !server.0.sql_tls_enabled(),
        "NO ROLLBACK ON ERROR must disable TLS"
    );
    let connection_config = astersql_server::conn::ConnectionServer::config(server.0.as_ref());
    assert_eq!(connection_config.capability & (1 << 11), 0);
}

/// TestReloadTLS：有效文件替换会更新证书；Server 快照整份替换且旧快照独立。
#[test]
fn test_reload_tls() {
    let _lock = global_test_lock();
    let temp = TempDir::new("reload");
    let now = unix_now();
    let ca = generate_cert(
        1,
        "AsterSQL CA",
        None,
        temp.path(),
        "ca",
        now - 60,
        now + 7200,
    );
    let active = generate_cert(
        2,
        "localhost",
        Some(&ca),
        temp.path(),
        "active",
        now - 60,
        now + 1800,
    );
    let next = generate_cert(
        3,
        "localhost",
        Some(&ca),
        temp.path(),
        "next",
        now - 60,
        now + 3600,
    );
    let expired = generate_cert(
        4,
        "localhost",
        Some(&ca),
        temp.path(),
        "expired",
        now - 7200,
        now - 3600,
    );
    let cert = active.certificate_path.clone();
    let key = active.key_path.clone();
    let (loaded, _) = LoadTLSCertificates(
        path_text(&ca.certificate_path),
        path_text(&key),
        path_text(&cert),
        false,
        2048,
    )
    .unwrap();
    let loaded = loaded.unwrap();
    let old_der = loaded.certificate().certificate.to_der().unwrap();
    let server = RunningServer::start(ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        sql_tls_ca: Some(path_text(&ca.certificate_path).into()),
        sql_tls_certificate: Some(path_text(&cert).into()),
        sql_tls_key: Some(path_text(&key).into()),
        status: astersql_server::server::StatusConfig {
            report_status: false,
            ..Default::default()
        },
        ..ServerConfig::default()
    });
    let client_config = tls_client_config(&ca.certificate_path, None);
    tls_handshake(
        server.0.sql_tls_config().unwrap(),
        Arc::clone(&client_config),
    )
    .expect("initial TLS certificate");

    std::fs::copy(&next.certificate_path, &cert).unwrap();
    std::fs::copy(&next.key_path, &key).unwrap();
    let new_der = loaded
        .reload_certificate()
        .unwrap()
        .certificate
        .to_der()
        .unwrap();
    assert_ne!(new_der, old_der);
    server
        .0
        .reload_tls(false)
        .expect("reload replacement keypair");
    assert!(server.0.sql_tls_enabled());
    tls_handshake(
        server.0.sql_tls_config().unwrap(),
        Arc::clone(&client_config),
    )
    .expect("reloaded TLS certificate");

    std::fs::copy(&expired.certificate_path, &cert).unwrap();
    std::fs::copy(&expired.key_path, &key).unwrap();
    server
        .0
        .reload_tls(false)
        .expect("expired certificate is syntactically reloadable");
    assert!(
        tls_handshake(server.0.sql_tls_config().unwrap(), client_config).is_err(),
        "clients must reject the expired reloaded certificate"
    );

    // 首次写入完整 TLS 配置（CA/证书/密钥、CN 校验与有效期窗口）。
    let first = TlsConfig {
        ca_path: Some("ca-1".into()),
        certificate_path: Some("cert-1".into()),
        key_path: Some("key-1".into()),
        verify_common_names: vec!["client-a".into()],
        not_before: None,
        not_after: None,
        not_before_unix: Some(1),
        not_after_unix: Some(9),
    };
    server.0.update_tls_config(Some(first));
    let snapshot = server.0.tls_config().unwrap();
    assert_eq!(snapshot.certificate_path.as_deref(), Some("cert-1"));
    assert_eq!(snapshot.verify_common_names, vec!["client-a"]);
    // 再次更新：未指定字段回落到 default，旧 snapshot 应仍保留 cert-1。
    server.0.update_tls_config(Some(TlsConfig {
        certificate_path: Some("cert-2".into()),
        ..TlsConfig::default()
    }));
    assert_eq!(
        server.0.tls_config().unwrap().certificate_path.as_deref(),
        Some("cert-2")
    );
    assert_eq!(snapshot.certificate_path.as_deref(), Some("cert-1"));
    // None：禁用 TLS，配置变为空。
    server.0.update_tls_config(None);
    assert!(server.0.tls_config().is_none());
}

/// TestStatusAPIWithTLS：status 监听配置与 cluster TLS 元数据可同时启用。
#[test]
fn test_status_api_with_tls() {
    let _lock = global_test_lock();
    let temp = TempDir::new("status-tls");
    let now = unix_now();
    let ca = generate_cert(
        1,
        "AsterSQL CA",
        None,
        temp.path(),
        "ca",
        now - 60,
        now + 3600,
    );
    let server_certificate = generate_cert(
        2,
        "localhost",
        Some(&ca),
        temp.path(),
        "server",
        now - 60,
        now + 3600,
    );
    let config = ServerConfig {
        port: 0,
        status: astersql_server::server::StatusConfig {
            host: "127.0.0.1".into(),
            port: 0,
            report_status: true,
            tls_ca: Some(path_text(&ca.certificate_path).into()),
            tls_certificate: Some(path_text(&server_certificate.certificate_path).into()),
            tls_key: Some(path_text(&server_certificate.key_path).into()),
            ..Default::default()
        },
        ..ServerConfig::default()
    };
    let server = RunningServer::start(config);
    let address = server.0.status_listener_addr().unwrap();
    let response = https_get(
        address,
        "localhost",
        tls_client_config(&ca.certificate_path, None),
    )
    .expect("HTTPS status request");
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));

    let mut plain = TcpStream::connect(address).unwrap();
    plain
        .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = Vec::new();
    let _ = plain.read_to_end(&mut response);
    assert!(
        !response.starts_with(b"HTTP/1.1 200 OK\r\n"),
        "plain HTTP must not succeed on the HTTPS status listener"
    );
}

/// TestStatusAPIWithTLSCNCheck：未列入 cluster-verify-cn 的客户端拒绝，命中的通过。
#[test]
fn test_status_api_with_tls_cn_check() {
    let _lock = global_test_lock();
    let temp = TempDir::new("status-cn");
    let now = unix_now();
    let ca = generate_cert(
        1,
        "AsterSQL CA",
        None,
        temp.path(),
        "ca",
        now - 60,
        now + 3600,
    );
    let server_certificate = generate_cert(
        2,
        "localhost",
        Some(&ca),
        temp.path(),
        "server",
        now - 60,
        now + 3600,
    );
    let client_one = generate_cert(
        3,
        "tidb-client-1",
        Some(&ca),
        temp.path(),
        "client-one",
        now - 60,
        now + 3600,
    );
    let client_two = generate_cert(
        4,
        "tidb-client-2",
        Some(&ca),
        temp.path(),
        "client-two",
        now - 60,
        now + 3600,
    );
    let server = RunningServer::start(ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        status: astersql_server::server::StatusConfig {
            host: "127.0.0.1".into(),
            port: 0,
            report_status: true,
            tls_ca: Some(path_text(&ca.certificate_path).into()),
            tls_certificate: Some(path_text(&server_certificate.certificate_path).into()),
            tls_key: Some(path_text(&server_certificate.key_path).into()),
            tls_verify_common_names: vec!["tidb-client-2".into()],
            ..Default::default()
        },
        ..ServerConfig::default()
    });
    let address = server.0.status_listener_addr().unwrap();
    let rejected = https_get(
        address,
        "localhost",
        tls_client_config(&ca.certificate_path, Some(&client_one)),
    );
    assert!(rejected.is_err(), "unlisted client CN must be rejected");
    let accepted = https_get(
        address,
        "localhost",
        tls_client_config(&ca.certificate_path, Some(&client_two)),
    )
    .expect("listed client CN must be accepted");
    assert!(accepted.starts_with(b"HTTP/1.1 200 OK\r\n"));

    let metadata = TlsConfig {
        ca_path: Some(path_text(&ca.certificate_path).into()),
        verify_common_names: vec!["tidb-client-2".into()],
        ..TlsConfig::default()
    };
    assert!(
        metadata
            .verify_peer_common_name(&client_one.certificate.to_pem().unwrap())
            .is_err()
    );
    assert!(
        metadata
            .verify_peer_common_name(&client_two.certificate.to_pem().unwrap())
            .is_ok()
    );
}

/// TestInvalidTLS：不存在的 CA、证书或私钥必须在启动前返回错误。
#[test]
fn test_invalid_tls() {
    assert!(LoadTLSCertificates("", "wrong-key", "wrong-cert", false, 1024).is_err());
    assert!(
        Server::new(
            ServerConfig {
                sql_tls_ca: Some("wrong-ca".into()),
                sql_tls_certificate: Some("wrong-cert".into()),
                sql_tls_key: Some("wrong-key".into()),
                ..ServerConfig::default()
            },
            Arc::new(Driver),
        )
        .is_err()
    );
    let temp = TempDir::new("invalid");
    let (cert, key) = generated_pair(temp.path(), "server");
    assert!(
        LoadTLSCertificates(
            path_text(&temp.path().join("wrong-ca")),
            path_text(&key),
            path_text(&cert),
            false,
            1024,
        )
        .is_err()
    );
}

/// TestTLSAuto：未配置证书且 AutoTLS 开启时自动签发并启用真实 Server TLS。
#[test]
fn test_tls_auto() {
    let _lock = global_test_lock();
    let temp = TempDir::new("auto");
    let server = RunningServer::start(ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        sql_auto_tls: true,
        rsa_key_size: 2_048,
        temp_storage_path: Some(path_text(temp.path()).into()),
        status: astersql_server::server::StatusConfig {
            report_status: false,
            ..Default::default()
        },
        ..ServerConfig::default()
    });
    assert!(server.0.sql_tls_enabled());
    assert!(temp.path().join("cert.pem").is_file());
    assert!(temp.path().join("key.pem").is_file());
    tls_handshake(
        server.0.sql_tls_config().unwrap(),
        tls_client_config(&temp.path().join("cert.pem"), None),
    )
    .expect("TLS connection using automatically created certificate");
}
