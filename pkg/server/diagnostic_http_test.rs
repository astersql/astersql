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

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};

use crate::server::{Domain, Server, ServerConfig, ServerDriver, StatusConfig};

static DIAGNOSTIC_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

struct Driver;

impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "diagnostic-http-test"
    }
}

struct DiagnosticDomain;

impl Domain for DiagnosticDomain {
    fn server_id(&self) -> u64 {
        11
    }

    fn start_timestamp(&self) -> i64 {
        1
    }

    fn local_ddl_id(&self) -> String {
        panic!("diagnostic status must not start the advertised /info identity check")
    }

    fn global_system_variables(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
}

struct RestoreMode<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for RestoreMode<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            restore();
        }
    }
}

struct RestoreInTest(bool);

impl Drop for RestoreInTest {
    fn drop(&mut self) {
        astersql_util_intest::InTest.store(self.0, Ordering::SeqCst);
    }
}

fn enable_in_test() -> RestoreInTest {
    RestoreInTest(astersql_util_intest::InTest.swap(true, Ordering::SeqCst))
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "astersql-diagnostic-http-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create diagnostic HTTP TLS directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn path_text(path: &Path) -> String {
    path.to_str().expect("UTF-8 test path").to_owned()
}

fn read_http_response<S: Read + Write>(stream: &mut S, method: &str, path: &str) -> String {
    stream
        .write_all(
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .expect("write diagnostic HTTP request");
    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof,
            "read diagnostic HTTP response: {error}"
        );
    }
    response
}

fn plain_request(address: SocketAddr, method: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).expect("connect diagnostic HTTP listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    read_http_response(&mut stream, method, path)
}

fn tls_request(
    address: SocketAddr,
    client_config: Arc<rustls::ClientConfig>,
    method: &str,
    path: &str,
) -> (String, Option<Vec<u8>>) {
    let socket = TcpStream::connect(address).expect("connect diagnostic HTTPS listener");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let connection = ClientConnection::new(
        client_config,
        ServerName::try_from("localhost").unwrap().to_owned(),
    )
    .expect("create diagnostic HTTPS client");
    let mut stream = StreamOwned::new(connection, socket);
    let response = read_http_response(&mut stream, method, path);
    (response, stream.conn.alpn_protocol().map(<[u8]>::to_vec))
}

fn response_status(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .expect("HTTP status code")
        .parse()
        .expect("numeric HTTP status code")
}

#[test]
fn diagnostic_http_exposes_only_local_get_endpoints_plain_and_tls() {
    let _lock = DIAGNOSTIC_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _restore_in_test = enable_in_test();
    let _restore = RestoreMode(Some(astersql_config::diagnosticmode::SetForTest(true)));

    for use_tls in [false, true] {
        let temp = TempDir::new();
        let certificate = temp.0.join("server-cert.pem");
        let key = temp.0.join("server-key.pem");
        let client_config = if use_tls {
            astersql_util::misc::CreateCertificates(
                &certificate,
                &key,
                2048,
                astersql_util::misc::PublicKeyAlgorithm::Rsa,
                astersql_util::misc::SignatureAlgorithm::Unspecified,
            )
            .unwrap();
            Some(
                astersql_util::security::NewTLSConfig(vec![astersql_util::security::WithCAPath(
                    path_text(&certificate),
                )])
                .unwrap()
                .unwrap()
                .client_config()
                .unwrap(),
            )
        } else {
            None
        };
        let server = Server::new_test(
            ServerConfig {
                host: "127.0.0.1".into(),
                port: 0,
                status: StatusConfig {
                    host: "127.0.0.1".into(),
                    port: 0,
                    tls_certificate: use_tls.then(|| path_text(&certificate)),
                    tls_key: use_tls.then(|| path_text(&key)),
                    ..StatusConfig::default()
                },
                ..ServerConfig::default()
            },
            Arc::new(Driver),
        );
        server.run(Arc::new(DiagnosticDomain)).unwrap();
        assert_eq!(server.status_server_addr(), None);
        let address = server
            .status_listener_addr()
            .expect("diagnostic status listener address");

        for (method, path, want_status) in [
            ("GET", "/status", 200),
            ("GET", "/metrics", 200),
            ("GET", "/debug/pprof/", 200),
            ("GET", "/debug/pprof/cmdline", 200),
            ("GET", "/info", 404),
            ("GET", "/settings", 404),
            ("POST", "/status", 405),
            ("POST", "/debug/pprof/symbol", 405),
        ] {
            let (response, alpn) = match &client_config {
                Some(config) => tls_request(address, Arc::clone(config), method, path),
                None => (plain_request(address, method, path), None),
            };
            assert_eq!(response_status(&response), want_status, "{method} {path}");
            if use_tls {
                assert_eq!(alpn.as_deref(), Some(b"http/1.1".as_slice()));
            }
        }

        server.close();
        assert_eq!(server.status_listener_addr(), None);
        assert!(TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err());
    }
}

#[test]
fn diagnostic_http_status_disabled_and_full_status_visibility_match_go() {
    let _lock = DIAGNOSTIC_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _restore_in_test = enable_in_test();
    for diagnostic in [false, true] {
        let _restore = RestoreMode(Some(astersql_config::diagnosticmode::SetForTest(
            diagnostic,
        )));
        let disabled = Server::new_test(
            ServerConfig {
                status: StatusConfig {
                    report_status: false,
                    ..StatusConfig::default()
                },
                ..ServerConfig::default()
            },
            Arc::new(Driver),
        );
        disabled.start_status_http().unwrap();
        assert_eq!(disabled.status_listener_addr(), None);
        assert_eq!(disabled.status_server_addr(), None);

        let enabled = Server::new_test(ServerConfig::default(), Arc::new(Driver));
        assert_eq!(enabled.status_server_addr().is_some(), !diagnostic);
    }
}
