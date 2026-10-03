// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::advertisedstatus::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::watch;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

// Only the remote HTTP/time boundary is replaced. Tests use real TCP,
// reqwest status/body parsing, TLS handshakes and lifecycle cancellation.
struct Endpoint {
    address: SocketAddr,
    requests: mpsc::Receiver<String>,
    closed: mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Endpoint {
    fn bind(host: &str, response: Vec<u8>, wait_for_close: bool) -> Self {
        let listener = TcpListener::bind((host, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, requests) = mpsc::channel();
        let (closed_tx, closed) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            tx.send(String::from_utf8(request).unwrap()).unwrap();
            let _ = stream.write_all(&response);
            if wait_for_close {
                match stream.read(&mut byte) {
                    Ok(0) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    result => panic!("request connection was not closed: {result:?}"),
                }
            }
            closed_tx.send(()).unwrap();
        });
        Self {
            address,
            requests,
            closed,
            worker: Some(worker),
        }
    }
    fn response(status: &str, body: &[u8]) -> Self {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        Self::bind("127.0.0.1", response, false)
    }
    fn url(&self) -> String {
        format!("http://{}/info", self.address)
    }
    fn options(&self) -> Options {
        Options {
            report_status: true,
            status_address: Some(self.address),
            advertise_address: self.address.ip().to_string(),
            local_id: "local-id".into(),
            tls: TlsOptions::default(),
        }
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
fn check(endpoint: &str) -> CheckResult {
    let (_tx, rx) = watch::channel(false);
    runtime().block_on(async {
        let client = new_http_client(reqwest::Client::builder()).unwrap();
        check_endpoint(&client, endpoint, "local-id", rx).await
    })
}

#[test]
fn endpoint_url_uses_listener_port_for_ipv4_and_ipv6() {
    for host in ["127.0.0.1", "::1"] {
        // Match Go's optional IPv6 skip only when loopback cannot be bound.
        if host == "::1" && TcpListener::bind((host, 0)).is_err() {
            continue;
        }
        let endpoint = Endpoint::bind(
            host,
            b"HTTP/1.1 200 OK\r\nContent-Length: 21\r\n\r\n{\"ddl_id\":\"local-id\"}".to_vec(),
            true,
        );
        let (tx, rx) = mpsc::channel();
        let handle = start_with_reporter(endpoint.options(), move |_, _, result| {
            tx.send(result.clone()).unwrap();
        })
        .unwrap();
        let request = endpoint
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(request.starts_with("GET /info HTTP/1.1\r\n"));
        assert!(
            request
                .to_lowercase()
                .contains(&format!("host: {}\r\n", endpoint.address))
        );
        endpoint
            .closed
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        drop(handle);
        assert!(rx.try_recv().is_err(), "match must not warn");
    }
}

#[test]
fn start_prerequisites_do_not_request_or_report() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = Options {
        report_status: true,
        status_address: Some(listener.local_addr().unwrap()),
        advertise_address: "127.0.0.1".into(),
        local_id: "local-id".into(),
        tls: TlsOptions::default(),
    };
    for index in 0..4 {
        let mut options = base.clone();
        match index {
            0 => options.report_status = false,
            1 => options.status_address = None,
            2 => options.advertise_address.clear(),
            _ => options.local_id.clear(),
        }
        assert!(
            start_with_reporter(options, |_, _, _| panic!("missing prerequisites warned"))
                .is_none()
        );
    }
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn endpoint_responses_match_go_identity_and_body_limits() {
    let oversized = format!("{{\"ddl_id\":\"{}\"}}", "x".repeat(BODY_LIMIT));
    for (status, body, reason, id) in [
        ("200 OK", "{\"ddl_id\":\"local-id\"}", "", "local-id"),
        (
            "201 Created",
            "{\"ddl_id\":\"remote-id\"}",
            "identity-mismatch",
            "remote-id",
        ),
        ("200 OK", "{\"is_owner\":false}", "missing-identity", ""),
        ("200 OK", "null", "missing-identity", ""),
        ("200 OK", "{\"ddl_id\":null}", "missing-identity", ""),
        ("200 OK", "{\"ddl_id\":", "invalid-response", ""),
        ("200 OK", "{\"ddl_id\":12}", "invalid-response", ""),
        (
            "200 OK",
            "{\"DDL_ID\":\"local-id\",\"labels\":42}",
            "",
            "local-id",
        ),
        ("200 OK", oversized.as_str(), "invalid-response", ""),
        (
            "500 Internal Server Error",
            "{\"ddl_id\":\"local-id\"}",
            "unexpected-status",
            "",
        ),
        ("204 No Content", "", "invalid-response", ""),
    ] {
        let endpoint = Endpoint::response(status, body.as_bytes());
        let result = check(&endpoint.url());
        assert_eq!(
            (
                result.reason,
                result.remote_id.as_str(),
                result.status.as_str()
            ),
            (reason, id, status),
            "{body:.100}"
        );
        if reason == "invalid-response" || reason == "missing-identity" {
            assert!(result.error.is_some());
        }
    }
    let mut exact = b"{\"ddl_id\":\"local-id\"}".to_vec();
    exact.resize(BODY_LIMIT, b' ');
    let endpoint = Endpoint::response("200 OK", &exact);
    assert_eq!(check(&endpoint.url()).reason, "");
}

#[test]
fn endpoint_redirect_does_not_follow_a_matching_target() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://{}/info\r\nContent-Length: 0\r\n\r\n",
        target.local_addr().unwrap()
    );
    let endpoint = Endpoint::bind("127.0.0.1", response.into_bytes(), false);
    let result = check(&endpoint.url());
    assert_eq!(result.reason, "unexpected-status");
    assert_eq!(result.status, "302 Found");
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn endpoint_request_failures_include_connection_url_timeout_and_body_read() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    for url in [format!("http://{address}/info"), "://bad".into()] {
        let result = check(&url);
        assert_eq!(result.reason, "request-failed");
        assert!(result.error.is_some());
    }
    let endpoint = Endpoint::bind(
        "127.0.0.1",
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"ddl_id\":\"local-id\"}".to_vec(),
        false,
    );
    let result = check(&endpoint.url());
    assert_eq!(result.reason, "request-failed");
    assert_eq!(result.status, "200 OK");
    assert!(result.error.is_some());
    let endpoint = Endpoint::bind(
        "127.0.0.1",
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".to_vec(),
        true,
    );
    let (_tx, rx) = watch::channel(false);
    let result = runtime().block_on(async {
        // An explicit test deadline replaces only the external time boundary.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        check_endpoint(&client, &endpoint.url(), "local-id", rx).await
    });
    assert_eq!(result.reason, "request-failed");
    assert!(result.error.unwrap().contains("timed out"));
}

#[test]
fn endpoint_cancellation_closes_the_request_and_suppresses_warning() {
    let endpoint = Endpoint::bind("127.0.0.1", Vec::new(), true);
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    let handle = start_with_reporter(endpoint.options(), move |_, _, result| {
        tx.send(result.clone()).unwrap();
    })
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    endpoint
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let cancelled = Instant::now();
    drop(handle);
    assert!(cancelled.elapsed() < Duration::from_secs(1));
    assert!(rx.try_recv().is_err());
    let (tx, rx) = watch::channel(true);
    assert_eq!(
        runtime()
            .block_on(async {
                let client = new_http_client(reqwest::Client::builder()).unwrap();
                check_endpoint(&client, "http://unused.invalid/info", "local-id", rx).await
            })
            .reason,
        "request-failed"
    );
    drop(tx);
}

#[test]
fn completed_failure_reports_once_with_structured_diagnostics() {
    let endpoint = Endpoint::bind(
        "127.0.0.1",
        b"HTTP/1.1 200 OK\r\nContent-Length: 22\r\n\r\n{\"ddl_id\":\"remote-id\"}".to_vec(),
        true,
    );
    let (tx, rx) = mpsc::channel();
    let handle = start_with_reporter(endpoint.options(), move |url, id, result| {
        tx.send((url.to_owned(), id.to_owned(), result.clone()))
            .unwrap();
    })
    .unwrap();
    let (url, id, result) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(url, endpoint.url());
    assert_eq!(id, "local-id");
    assert_eq!(result.reason, "identity-mismatch");
    assert_eq!(result.remote_id, "remote-id");
    let fields = warning_fields(&url, &id, &result);
    assert_eq!(fields.len(), 6);
    assert!(fields.iter().any(|f| f.key() == "remote-tidb-id"));
    assert!(warning_action(result.reason).contains("route directly"));
    drop(handle);
    assert!(rx.try_recv().is_err());
}

#[test]
fn http_client_bypasses_explicit_proxy_without_mutating_base_builder() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let endpoint = Endpoint::response("200 OK", b"{\"ddl_id\":\"local-id\"}");
    let builder = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", proxy.local_addr().unwrap())).unwrap());
    let (_tx, rx) = watch::channel(false);
    let result = runtime().block_on(async {
        let client = new_http_client(builder).unwrap();
        check_endpoint(&client, &endpoint.url(), "local-id", rx).await
    });
    assert_eq!(result.reason, "");
    assert_eq!(
        proxy.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

fn tls_material() -> (
    openssl::x509::X509,
    openssl::x509::X509,
    openssl::pkey::PKey<openssl::pkey::Private>,
) {
    use openssl::{
        asn1::Asn1Time,
        hash::MessageDigest,
        pkey::PKey,
        rsa::Rsa,
        x509::{
            X509, X509NameBuilder,
            extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
        },
    };
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "advertised-status-root")
        .unwrap();
    let name = name.build();
    let mut root = X509::builder().unwrap();
    root.set_version(2).unwrap();
    root.set_subject_name(&name).unwrap();
    root.set_issuer_name(&name).unwrap();
    root.set_pubkey(&key).unwrap();
    root.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    root.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    root.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    root.append_extension(KeyUsage::new().key_cert_sign().build().unwrap())
        .unwrap();
    root.sign(&key, MessageDigest::sha256()).unwrap();
    let root = root.build();
    let leaf_key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut leaf_name = X509NameBuilder::new().unwrap();
    leaf_name
        .append_entry_by_text("CN", "advertised-status-peer")
        .unwrap();
    let leaf_name = leaf_name.build();
    let mut leaf = X509::builder().unwrap();
    leaf.set_version(2).unwrap();
    leaf.set_subject_name(&leaf_name).unwrap();
    leaf.set_issuer_name(&name).unwrap();
    leaf.set_pubkey(&leaf_key).unwrap();
    leaf.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    leaf.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    leaf.append_extension(BasicConstraints::new().critical().build().unwrap())
        .unwrap();
    leaf.append_extension(
        KeyUsage::new()
            .digital_signature()
            .key_encipherment()
            .build()
            .unwrap(),
    )
    .unwrap();
    leaf.append_extension(
        ExtendedKeyUsage::new()
            .server_auth()
            .client_auth()
            .build()
            .unwrap(),
    )
    .unwrap();
    let san = SubjectAlternativeName::new()
        .ip("127.0.0.1")
        .build(&leaf.x509v3_context(Some(&root), None))
        .unwrap();
    leaf.append_extension(san).unwrap();
    leaf.sign(&key, MessageDigest::sha256()).unwrap();
    (root, leaf.build(), leaf_key)
}

#[test]
fn http_client_preserves_tls_ca_and_mutual_client_certificate() {
    use openssl::ssl::{SslAcceptor, SslMethod, SslVerifyMode};
    let (root, cert, key) = tls_material();
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
    acceptor.set_certificate(&cert).unwrap();
    acceptor.set_private_key(&key).unwrap();
    acceptor.cert_store_mut().add_cert(root.clone()).unwrap();
    acceptor.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
    let acceptor = acceptor.build();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut stream = acceptor.accept(stream).unwrap();
        assert!(stream.ssl().peer_certificate().is_some());
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 21\r\nConnection: close\r\n\r\n{\"ddl_id\":\"local-id\"}").unwrap();
    });
    let directory = std::env::temp_dir().join(format!(
        "astersql-advertised-status-tls-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let ca = directory.join("ca.pem");
    let certificate = directory.join("cert.pem");
    let private_key = directory.join("key.pem");
    std::fs::write(&ca, root.to_pem().unwrap()).unwrap();
    std::fs::write(&certificate, cert.to_pem().unwrap()).unwrap();
    std::fs::write(&private_key, key.private_key_to_pem_pkcs8().unwrap()).unwrap();
    let tls = TlsOptions {
        ca: Some(ca.to_string_lossy().into_owned()),
        certificate: Some(certificate.to_string_lossy().into_owned()),
        key: Some(private_key.to_string_lossy().into_owned()),
    };
    assert_eq!(tls.scheme(), "https");
    let builder = tls.builder().unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    let (_tx, rx) = watch::channel(false);
    let result = runtime().block_on(async {
        let client = new_http_client(builder).unwrap();
        check_endpoint(&client, &format!("https://{address}/info"), "local-id", rx).await
    });
    worker.join().unwrap();
    assert_eq!(result.reason, "", "{:?}", result.error);
}

#[test]
fn endpoint_tls_verification_failure_is_a_request_failure() {
    use openssl::ssl::{SslAcceptor, SslMethod};
    let (_root, cert, key) = tls_material();
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
    acceptor.set_certificate(&cert).unwrap();
    acceptor.set_private_key(&key).unwrap();
    let acceptor = acceptor.build();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        assert!(acceptor.accept(stream).is_err());
    });
    let result = check(&format!("https://{address}/info"));
    worker.join().unwrap();
    assert_eq!(result.reason, "request-failed");
    assert!(result.error.is_some());
}

#[test]
fn inflight_cancellation_returns_request_failure_and_closes_connection() {
    let mut endpoint = Endpoint::bind("127.0.0.1", Vec::new(), true);
    let (tx, rx) = watch::channel(false);
    let url = endpoint.url();
    let requests = &mut endpoint.requests;
    let result = std::thread::scope(|scope| {
        scope.spawn(move || {
            requests.recv_timeout(Duration::from_secs(2)).unwrap();
            tx.send_replace(true);
        });
        runtime().block_on(async {
            let client = new_http_client(reqwest::Client::builder()).unwrap();
            check_endpoint(&client, &url, "local-id", rx).await
        })
    });
    assert_eq!(result.reason, "request-failed");
    assert_eq!(result.error.as_deref(), Some("context canceled"));
    endpoint
        .closed
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
}

#[test]
fn unexpected_status_preserves_remote_reason_phrase() {
    let endpoint = Endpoint::response("500 topology unavailable", b"unused");
    let result = check(&endpoint.url());
    assert_eq!(result.reason, "unexpected-status");
    assert_eq!(result.status, "500 topology unavailable");
}

#[test]
fn lifecycle_cancellation_does_not_wait_for_a_blocking_dns_job() {
    use reqwest::dns::{Addrs, Name, Resolve, Resolving};
    use std::sync::{Arc, Condvar, Mutex};
    struct HeldResolver {
        started: mpsc::Sender<()>,
        done: mpsc::Sender<()>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }
    impl Resolve for HeldResolver {
        fn resolve(&self, _: Name) -> Resolving {
            let started = self.started.clone();
            let done = self.done.clone();
            let release = self.release.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    started.send(()).unwrap();
                    let (lock, wake) = &*release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = wake.wait(released).unwrap();
                    }
                    done.send(()).unwrap();
                    Box::new(["127.0.0.1:0".parse::<SocketAddr>().unwrap()].into_iter()) as Addrs
                })
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            })
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let resolver = Arc::new(HeldResolver {
        started: started_tx,
        done: done_tx,
        release: release.clone(),
    });
    let options = Options {
        report_status: true,
        status_address: Some(listener.local_addr().unwrap()),
        advertise_address: "held.invalid".into(),
        local_id: "local-id".into(),
        tls: TlsOptions::default(),
    };
    let (reports_tx, reports_rx) = mpsc::channel();
    let handle = start_with_client_builder(
        options,
        move || Ok(reqwest::Client::builder().dns_resolver(resolver)),
        move |_, _, result| {
            reports_tx.send(result.clone()).unwrap();
        },
    )
    .unwrap();
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (closed_tx, closed_rx) = mpsc::channel();
    let closer = thread::spawn(move || {
        drop(handle);
        closed_tx.send(()).unwrap();
    });
    let closed = closed_rx.recv_timeout(Duration::from_secs(1));
    // Always release and join the substituted OS boundary before asserting,
    // including on the red path, so no DNS thread escapes the test process.
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_all();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    closer.join().unwrap();
    assert!(
        closed.is_ok(),
        "server cancellation waited for the OS DNS job"
    );
    assert!(
        reports_rx.try_recv().is_err(),
        "cancellation was reported as a warning"
    );
}
