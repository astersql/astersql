// Copyright 2023-2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// LDAP 公共层测试：DN 规范化、LDAPS/StartTLS 回退、TLS1.1 拒绝与超时。

#![allow(non_snake_case)]

use astersql_privilege_ldap::ldap_common::{
    LDAP_TIMEOUT, LdapAuthImpl, LdapConfig, LdapConnectionManager, search_filter,
};

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener};
    use std::sync::{Mutex, OnceLock, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    use astersql_privilege_ldap::native_tls::{Identity, Protocol, TlsAcceptor};
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose, PKCS_RSA_SHA256, SanType,
    };
    use time::OffsetDateTime;

    use super::{LDAP_TIMEOUT, LdapAuthImpl, LdapConfig, LdapConnectionManager, search_filter};

    /// 串行化 TLS 相关测试，避免 macOS Secure Transport 互相干扰。
    fn tls_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 生成短生命周期本地测试 CA/服务器证书与 TLS acceptor。
    fn tls_acceptor(max_protocol: Option<Protocol>) -> (TlsAcceptor, Vec<u8>) {
        // macOS rejects the Go fixture's 100-year lifetime, so generate the
        // same trusted localhost boundary with a short-lived RSA identity.
        let now = OffsetDateTime::now_utc();
        let mut ca_params =
            CertificateParams::new(Vec::new()).expect("create LDAP test CA parameters");
        ca_params.not_before = now - time::Duration::days(1);
        ca_params.not_after = now + time::Duration::days(30);
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "TiDB LDAP test CA");
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::CrlSign,
        ];
        let ca_key = KeyPair::generate_for(&PKCS_RSA_SHA256).expect("generate LDAP test CA key");
        let ca_certificate = ca_params
            .self_signed(&ca_key)
            .expect("generate LDAP test CA certificate");

        let key_pair =
            KeyPair::generate_for(&PKCS_RSA_SHA256).expect("generate LDAP test server key");
        // Bind/connect on 127.0.0.1 to avoid localhost IPv4/IPv6 races under load.
        let mut server_params = CertificateParams::new(vec!["localhost".to_owned()])
            .expect("create LDAP test server certificate parameters");
        server_params
            .subject_alt_names
            .push(SanType::IpAddress(std::net::IpAddr::V4(
                std::net::Ipv4Addr::LOCALHOST,
            )));
        server_params.not_before = now - time::Duration::days(1);
        server_params.not_after = now + time::Duration::days(30);
        server_params
            .distinguished_name
            .push(DnType::CommonName, "localhost");
        server_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let certificate = server_params
            .signed_by(&key_pair, &ca_certificate, &ca_key)
            .expect("generate LDAP test server certificate");
        let ca_pem = ca_certificate.pem();
        let certificate_chain_pem = format!("{}{}", certificate.pem(), ca_pem);
        let private_key_pem = key_pair.serialize_pem();
        let identity =
            Identity::from_pkcs8(certificate_chain_pem.as_bytes(), private_key_pem.as_bytes())
                .expect("import LDAP test identity");
        let mut builder = TlsAcceptor::builder(identity);
        builder.min_protocol_version(None);
        builder.max_protocol_version(max_protocol);
        (
            builder.build().expect("create LDAP test TLS acceptor"),
            ca_pem.into_bytes(),
        )
    }

    /// 构造指向 localhost 指定端口的启用 TLS 的连接管理器。
    fn tls_manager(port: u16, ca_pem: Option<Vec<u8>>) -> LdapConnectionManager {
        LdapConnectionManager {
            config: LdapConfig {
                search_attr: "cn".to_owned(),
                ldap_server_host: "localhost".to_owned(),
                ldap_server_port: port,
                enable_tls: true,
                ca_pem,
                ..LdapConfig::default()
            },
        }
    }

    // 对照 Go 的 TestCanonicalizeDN。
    // TestCanonicalizeDN mirrors Go's TestCanonicalizeDN.
    #[test]
    #[allow(non_snake_case)]
    fn TestCanonicalizeDN() {
        let impl_ = LdapAuthImpl::default();
        impl_.SetSearchAttr("cn");

        assert_eq!(
            impl_.canonicalize_dn("yka", "cn=y,dc=ping,dc=cap"),
            "cn=y,dc=ping,dc=cap"
        );
        assert_eq!(
            impl_.canonicalize_dn("yka", "+dc=ping,dc=cap"),
            "cn=yka,dc=ping,dc=cap"
        );
    }

    #[test]
    fn search_filter_matches_go_literal_formatting() {
        assert_eq!(
            search_filter("uid", "alice*)(uid=*)"),
            "(uid=alice*)(uid=*))"
        );
    }

    // Go's SetCAPath refreshes only the CA pool; it does not rebuild the
    // LDAP connection pool (unlike host/port/TLS/capacity setters).
    #[test]
    fn set_ca_path_does_not_rebuild_connection_pool() {
        let impl_ = LdapAuthImpl::default();
        impl_.SetInitCapacity(1);
        impl_.SetMaxCapacity(1);
        let generation = impl_.pool_generation();

        let (_, ca_pem) = tls_acceptor(Some(Protocol::Tlsv12));
        let mut ca_file = tempfile::NamedTempFile::new().expect("create temporary CA file");
        ca_file.write_all(&ca_pem).expect("write temporary CA");
        impl_
            .SetCAPath(ca_file.path().to_str().expect("temporary CA path is UTF-8"))
            .expect("load temporary CA");

        assert_eq!(impl_.pool_generation(), generation);
    }

    // 对照 Go：StartTLS 失败后回退到直接 LDAPS（636）。
    // TestConnectThrough636 mirrors the Go fallback from StartTLS to direct TLS.
    //
    // Go uses tls.Listen so StartTLS fails and connectionFactory falls back to
    // ldaps. On macOS, a failed StartTLS attempt via ldap3/native-tls can wedge
    // Secure Transport for later handshakes in the same process, so the direct
    // TLS leg is exercised first; StartTLS failure is checked afterward on a
    // fresh listener.
    #[test]
    #[allow(non_snake_case)]
    fn TestConnectThrough636() {
        let _guard = tls_test_lock();
        // --- Direct TLS (ldaps): ConnectLDAP fallback target. ---
        {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listen for ldaps");
            let port = listener.local_addr().expect("read listener address").port();
            let (acceptor, ca_pem) = tls_acceptor(Some(Protocol::Tlsv12));
            let (ready_tx, ready_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let _ = ready_tx.send(());
                // Accept a few times so client retries under workspace load still work.
                for _ in 0..5 {
                    let (stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(_) => break,
                    };
                    if acceptor.accept(stream).is_ok() {
                        break;
                    }
                }
            });
            ready_rx.recv().expect("ldaps server ready");
            let mut last_err = None;
            let mut connection = None;
            for _ in 0..5 {
                match tls_manager(port, Some(ca_pem.clone()))
                    .connect_url(&format!("ldaps://localhost:{port}"), false)
                {
                    Ok(conn) => {
                        connection = Some(conn);
                        break;
                    }
                    Err(err) => {
                        last_err = Some(err);
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
            let connection =
                connection.unwrap_or_else(|| panic!("direct TLS fallback target: {last_err:?}"));
            drop(connection);
            server.join().expect("join ldaps server");
        }

        // --- StartTLS against TLS-only listener must fail. ---
        {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listen for StartTLS fail");
            let port = listener.local_addr().expect("read listener address").port();
            let (acceptor, ca_pem) = tls_acceptor(Some(Protocol::Tlsv12));
            let (ready_tx, ready_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let _ = ready_tx.send(());
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(_) => return,
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut head = [0_u8; 1];
                if stream.peek(&mut head).unwrap_or(0) == 0 || head[0] != 0x16 {
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
                let _ = acceptor.accept(stream);
            });
            ready_rx.recv().expect("StartTLS-fail server ready");
            let err = tls_manager(port, Some(ca_pem))
                .connect_url(&format!("ldap://localhost:{port}"), true)
                .expect_err("StartTLS against TLS-only listener must fail");
            assert!(!err.to_string().is_empty());
            let _ = server.join();
        }
    }

    // 对照 Go：拒绝仅支持 TLS 1.1 的服务端。
    // TestConnectWithTLS11 mirrors Go's rejection of a TLS 1.1-only server.
    #[test]
    #[allow(non_snake_case)]
    fn TestConnectWithTLS11() {
        let _guard = tls_test_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen for TLS 1.1 test");
        let port = listener.local_addr().expect("read listener address").port();
        let (acceptor, ca_pem) = tls_acceptor(Some(Protocol::Tlsv11));
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let _ = ready_tx.send(());
            let mut errors = Vec::new();
            for _ in 0..2 {
                let (stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(_) => break,
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                if let Err(error) = acceptor.accept(stream) {
                    errors.push(format!("{error:?}"));
                }
            }
            errors
        });
        ready_rx.recv().expect("TLS 1.1 server thread is ready");

        let error = tls_manager(port, Some(ca_pem))
            .connect_ldap()
            .expect_err("TLS 1.1 must be rejected");
        let server_errors = server.join().expect("join TLS 1.1 server");
        assert_eq!(
            server_errors.len(),
            2,
            "both TLS 1.1 handshakes must fail: {server_errors:?}; client: {error:#}"
        );
    }

    // 对照 Go：对静默服务端的 StartTLS 超时。
    // TestLDAPStartTLSTimeout mirrors Go's silent-server StartTLS timeout.
    // Rust's production timeout is immutable, so the assertion uses LDAP_TIMEOUT.
    #[test]
    #[allow(non_snake_case)]
    fn TestLDAPStartTLSTimeout() {
        let _guard = tls_test_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen for timeout test");
        let port = listener.local_addr().expect("read listener address").port();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let _ = ready_tx.send(());
            let (mut stream, _) = listener.accept().expect("accept LDAP connection");
            let mut request = [0_u8; 64];
            let _ = stream.read(&mut request).expect("read StartTLS request");
            release_rx.recv().expect("wait for client timeout");
            let _ = stream.shutdown(Shutdown::Both);
        });
        ready_rx.recv().expect("timeout server thread is ready");

        let manager = tls_manager(port, None);
        let start = Instant::now();
        let error = manager
            .connect_url(&format!("ldap://localhost:{port}"), true)
            .expect_err("silent LDAP server must time out");
        let elapsed = start.elapsed();
        release_tx.send(()).expect("release timeout server");
        server.join().expect("join timeout server");

        assert!(elapsed >= LDAP_TIMEOUT, "returned too early: {elapsed:?}");
        assert!(
            elapsed < LDAP_TIMEOUT + Duration::from_secs(3),
            "returned too late: {elapsed:?}"
        );
        let message = format!("{error:#}");
        assert!(message.contains("timeout"), "unexpected error: {message}");
    }
}
