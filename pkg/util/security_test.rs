// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TLS/安全配置相关单元测试。
//
// 覆盖无效证书路径、Common Name 校验与证书轮转、TLS 版本协商、
// CA 信任链等场景，与 Go `security_test.go` 行为对齐。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};

use crate::security::{
    ClientWithTLS, Listener, NewTLS, NewTLSConfig, TlsConfig, WithCAContent, WithCertAndKeyContent,
    WithCertAndKeyPath, WithMinTLSVersion, WithVerifyCommonName,
};

/// 线程安全字符串缓冲，用于收集测试服务端 accept/读写过程中的错误日志。
#[derive(Debug, Default)]
struct SafeBuffer {
    buf: Mutex<String>,
}

impl SafeBuffer {
    fn write(&self, data: &str) {
        let mut guard = self.buf.lock().expect("safeBuffer mutex poisoned");
        guard.push_str(data);
    }

    fn string(&self) -> String {
        self.buf.lock().expect("safeBuffer mutex poisoned").clone()
    }
}

/// 测试用自签 CA 与若干叶证书的 PEM 内容集合。
struct GeneratedCerts {
    ca_cert: Vec<u8>,
    certs: Vec<Vec<u8>>,
    keys: Vec<Vec<u8>>,
}

/// 本地 TLS 测试服务：绑定随机端口、后台 accept，Drop 时停止并 join。
struct RunningServer {
    port: u16,
    err_log: Arc<SafeBuffer>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Nudge the accept loop so Drop can join promptly.
        let _ = std::net::TcpStream::connect(format!("127.0.0.1:{}", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 为给定 Common Name 列表生成自签 CA 与对应叶证书/私钥 PEM。
fn generate_certs(common_names: &[&str]) -> GeneratedCerts {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.distinguished_name = DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(DnType::OrganizationName, "test");
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let ca_pem = ca_cert.pem().into_bytes();

    let mut certs = Vec::new();
    let mut keys = Vec::new();
    for cn in common_names {
        let mut params =
            CertificateParams::new(vec!["127.0.0.1".to_owned(), "::1".to_owned()]).expect("params");
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, "test");
        params.distinguished_name.push(DnType::CommonName, *cn);
        let key = KeyPair::generate().expect("leaf key");
        let cert = params
            .signed_by(&key, &ca_cert, &ca_key)
            .expect("signed leaf");
        certs.push(cert.pem().into_bytes());
        keys.push(key.serialize_pem().into_bytes());
    }

    GeneratedCerts {
        ca_cert: ca_pem,
        certs,
        keys,
    }
}

/// 启动带 TLS 的本地 HTTP 示例服务，返回端口与错误日志缓冲。
fn run_server(tls_cfg: Arc<TlsConfig>) -> RunningServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind tls test server");
    let port = listener.local_addr().expect("local addr").port();
    let listener = Listener::Tls(listener, Arc::clone(&tls_cfg));
    let err_log = Arc::new(SafeBuffer::default());
    let stop = Arc::new(AtomicBool::new(false));
    let err_log_thread = Arc::clone(&err_log);
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut conn, _)) => {
                    if stop_thread.load(Ordering::SeqCst) {
                        break;
                    }
                    let mut buf = [0_u8; 4096];
                    match conn.read(&mut buf) {
                        Ok(0) => {}
                        Ok(_) => {
                            let body = b"This an example server";
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            if let Err(err) = conn.write_all(response.as_bytes()) {
                                err_log_thread.write(&err.to_string());
                                continue;
                            }
                            if let Err(err) = conn.write_all(body) {
                                err_log_thread.write(&err.to_string());
                                continue;
                            }
                            let _ = conn.flush();
                        }
                        Err(err) => {
                            err_log_thread.write(&err.to_string());
                        }
                    }
                }
                Err(err) => {
                    if stop_thread.load(Ordering::SeqCst) {
                        break;
                    }
                    err_log_thread.write(&err.to_string());
                }
            }
        }
    });
    // Give the accept loop a moment to start.
    thread::sleep(Duration::from_millis(20));
    RunningServer {
        port,
        err_log,
        stop,
        handle: Some(handle),
    }
}

/// 在超时内轮询等待谓词为真；用于等待服务端异步写出鉴权失败日志。
fn wait_until(predicate: impl Fn() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// 校验 NewTLS 对缺失/非法 CA、缺失密钥对、非法密钥内容的错误信息。
#[test]
fn test_invalid_tls() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let ca_path = temp_dir.path().join("ca.pem");
    let ca_path_str = ca_path.to_str().unwrap();

    let err = NewTLS(ca_path_str, "", "", "localhost", Vec::new())
        .err()
        .expect("missing ca");
    assert!(
        err.to_string().contains("could not read ca certificate"),
        "{err:#}"
    );

    std::fs::write(&ca_path, b"invalid ca content").expect("write invalid ca");
    let err = NewTLS(ca_path_str, "", "", "localhost", Vec::new())
        .err()
        .expect("invalid ca");
    assert!(
        err.to_string().contains("failed to append ca certs"),
        "{err:#}"
    );

    let cert_path = temp_dir.path().join("test.pem");
    let key_path = temp_dir.path().join("test.key");
    let err = NewTLS(
        ca_path_str,
        cert_path.to_str().unwrap(),
        key_path.to_str().unwrap(),
        "localhost",
        Vec::new(),
    )
    .err()
    .expect("missing key pair");
    assert!(
        err.to_string().contains("could not load client key pair"),
        "{err:#}"
    );

    std::fs::write(&cert_path, b"invalid cert content").expect("write invalid cert");
    std::fs::write(&key_path, b"invalid key content").expect("write invalid key");
    let err = NewTLS(
        ca_path_str,
        cert_path.to_str().unwrap(),
        key_path.to_str().unwrap(),
        "localhost",
        Vec::new(),
    )
    .err()
    .expect("invalid key pair");
    assert!(
        err.to_string().contains("could not load client key pair"),
        "{err:#}"
    );
}

/// 校验服务端按 Common Name 鉴权客户端，以及路径证书轮转后可重新通过。
#[test]
fn test_verify_common_name_and_rotate() {
    let generated = generate_certs(&["server", "client1", "client2"]);
    let server_cert = &generated.certs[0];
    let server_key = &generated.keys[0];
    let client1_cert = &generated.certs[1];
    let client1_key = &generated.keys[1];
    let client2_cert = &generated.certs[2];
    let client2_key = &generated.keys[2];

    let server_tls = NewTLSConfig(vec![
        WithCAContent(generated.ca_cert.clone()),
        WithCertAndKeyContent(server_cert.clone(), server_key.clone()),
        WithVerifyCommonName(vec!["client1".to_owned()]),
    ])
    .expect("server tls")
    .expect("server tls present");
    let server = run_server(Arc::new(server_tls));
    let url = format!("https://127.0.0.1:{}", server.port);

    let client_tls1 = Arc::new(
        NewTLSConfig(vec![
            WithCAContent(generated.ca_cert.clone()),
            WithCertAndKeyContent(client1_cert.clone(), client1_key.clone()),
        ])
        .expect("client1 tls")
        .expect("client1 tls present"),
    );
    let resp = ClientWithTLS(Arc::clone(&client_tls1))
        .Get(&url)
        .expect("client1 get");
    let body = resp.text().expect("client1 body");
    assert_eq!("This an example server", body);

    let client_tls1_verify = Arc::new(
        NewTLSConfig(vec![
            WithCAContent(generated.ca_cert.clone()),
            WithCertAndKeyContent(client1_cert.clone(), client1_key.clone()),
            WithVerifyCommonName(vec!["server".to_owned()]),
        ])
        .expect("client1 verify tls")
        .expect("client1 verify present"),
    );
    let resp = ClientWithTLS(client_tls1_verify)
        .Get(&url)
        .expect("client1 verify get");
    let body = resp.text().expect("client1 verify body");
    assert_eq!("This an example server", body);

    let dir = tempfile::tempdir().expect("client2 dir");
    let cert_path = dir.path().join("client.pem");
    let key_path = dir.path().join("client.key");
    std::fs::write(&cert_path, client2_cert).expect("write client2 cert");
    std::fs::write(&key_path, client2_key).expect("write client2 key");

    let client_tls2 = Arc::new(
        NewTLSConfig(vec![
            WithCAContent(generated.ca_cert.clone()),
            WithCertAndKeyPath(
                cert_path.to_str().unwrap().to_owned(),
                key_path.to_str().unwrap().to_owned(),
            ),
        ])
        .expect("client2 tls")
        .expect("client2 present"),
    );
    let client2 = ClientWithTLS(Arc::clone(&client_tls2));
    assert!(client2.Get(&url).is_err(), "client2 must be rejected");
    assert!(
        wait_until(
            || server
                .err_log
                .string()
                .contains("client certificate authentication failed"),
            Duration::from_secs(3)
        ),
        "missing CN failure log: {}",
        server.err_log.string()
    );

    std::fs::write(&cert_path, client1_cert).expect("rotate cert");
    std::fs::write(&key_path, client1_key).expect("rotate key");
    let resp = client2.Get(&url).expect("rotated client2 get");
    let body = resp.text().expect("rotated body");
    assert_eq!("This an example server", body);
}

/// 校验最低 TLS 版本：1.0/1.1 不可用，1.2/1.3 可握手成功。
#[test]
fn test_tls_version() {
    let generated = generate_certs(&["server", "client"]);
    let server_tls = NewTLSConfig(vec![
        WithCAContent(generated.ca_cert.clone()),
        WithCertAndKeyContent(generated.certs[0].clone(), generated.keys[0].clone()),
    ])
    .expect("server tls")
    .expect("server present");
    let server = run_server(Arc::new(server_tls));
    let url = format!("https://127.0.0.1:{}", server.port);

    // rustls only supports TLS1.2/1.3; TLS1.0/1.1 fail at config time.
    for version in [0x0301_u16, 0x0302_u16] {
        let result = NewTLSConfig(vec![
            WithCAContent(generated.ca_cert.clone()),
            WithCertAndKeyContent(generated.certs[1].clone(), generated.keys[1].clone()),
            WithMinTLSVersion(version),
        ]);
        match result {
            Ok(Some(config)) => {
                assert!(
                    config.client_config().is_err(),
                    "TLS {version:#x} should be unsupported"
                )
            }
            Ok(None) => panic!("unexpected empty tls config"),
            Err(_) => {}
        }
    }

    for version in [0x0303_u16, 0x0304_u16] {
        let client = Arc::new(
            NewTLSConfig(vec![
                WithCAContent(generated.ca_cert.clone()),
                WithCertAndKeyContent(generated.certs[1].clone(), generated.keys[1].clone()),
                WithMinTLSVersion(version),
            ])
            .expect("client tls")
            .expect("client present"),
        );
        assert_eq!(version, client.min_tls_version());
        let resp = ClientWithTLS(client).Get(&url).expect("tls get");
        let _ = resp.text().expect("body");
    }

    let client_tls2 = NewTLSConfig(vec![
        WithCAContent(generated.ca_cert.clone()),
        WithCertAndKeyContent(generated.certs[1].clone(), generated.keys[1].clone()),
        WithMinTLSVersion(0x0304),
    ])
    .expect("min tls13")
    .expect("present");
    assert_eq!(0x0304, client_tls2.min_tls_version());
}

/// 校验 CA 信任：正确 CA 成功、无 CA 仍可（对齐 Go）、错误 CA 握手失败。
#[test]
fn test_ca() {
    let generated = generate_certs(&["server", "client"]);
    let wrong_ca = generate_certs(&[]);

    let server_tls = NewTLSConfig(vec![
        WithCAContent(generated.ca_cert.clone()),
        WithCertAndKeyContent(generated.certs[0].clone(), generated.keys[0].clone()),
    ])
    .expect("server tls")
    .expect("server present");
    let server = run_server(Arc::new(server_tls));
    let url = format!("https://127.0.0.1:{}", server.port);

    let client_tls1 = Arc::new(
        NewTLSConfig(vec![WithCAContent(generated.ca_cert.clone())])
            .expect("ca only")
            .expect("present"),
    );
    let resp = ClientWithTLS(Arc::clone(&client_tls1))
        .Get(&url)
        .expect("ca-only get");
    assert_eq!("This an example server", resp.text().unwrap());

    // Without CA, hostname verification is skipped and the request still succeeds,
    // matching Go's RootCAs injection path that trusts the generated CA.
    let client_tls2 = Arc::new(
        NewTLSConfig(vec![WithCertAndKeyContent(
            generated.certs[1].clone(),
            generated.keys[1].clone(),
        )])
        .expect("cert only")
        .expect("present"),
    );
    let resp = ClientWithTLS(client_tls2).Get(&url).expect("no-ca get");
    assert_eq!("This an example server", resp.text().unwrap());

    let client_tls3 = Arc::new(
        NewTLSConfig(vec![WithCAContent(wrong_ca.ca_cert)])
            .expect("wrong ca")
            .expect("present"),
    );
    let err = ClientWithTLS(client_tls3)
        .Get(&url)
        .expect_err("wrong CA must fail");
    let message = format!("{err:#}");
    assert!(
        message.contains("certificate")
            || message.contains("CA")
            || message.contains("tls")
            || message.contains("SSL")
            || message.contains("InvalidSignature")
            || message.contains("UnknownIssuer")
            || message.contains("CertNotValid")
            || message.contains("handshake")
            || message.contains("Connect"),
        "unexpected wrong-CA error: {message}"
    );
}
