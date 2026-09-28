// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// TLS 模块单元测试：Mock HTTP GetJSON、WithHost 拼 URL、以及非法 PEM 构造失败。

use crate::{Context, GetMockTLSUrl, MockTLSServer, NewTLS, NewTLSFromMockServer, TLSConfig};
use std::sync::Arc;

const VALID_CA_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----
MIIBITCBxwIUf04/Hucshr7AynmgF8JeuFUEf9EwCgYIKoZIzj0EAwIwEzERMA8G
A1UEAwwIYnJfdGVzdHMwHhcNMjIwNDEzMDcyNDQxWhcNMjIwNDE1MDcyNDQxWjAT
MREwDwYDVQQDDAhicl90ZXN0czBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABL+X
wczUg0AbaFFaCI+FAk3K9vbB9JeIORgGKS+F1TKip5tvm96g7S5lq8SgY38SXVc3
0yS3YqWZqnRjWi+sLwIwCgYIKoZIzj0EAwIDSQAwRgIhAJcpSwsUhqkM08LK1gYC
ze4ZnCkwJdP2VdpI3WZsoI7zAiEAjP8X1c0iFwYxdAbQAveX+9msVrzyUpZOohi4
RtgQTNI=
-----END CERTIFICATE-----
";

/// Mock HTTP 处理：从完整 URL 抽出 path，返回 JSON `{"path":"..."}`。
fn respond_path_handler(_ctx: &Context, full_url: &str) -> Result<Vec<u8>, crate::CommonError> {
    let path = full_url
        .find("://")
        .and_then(|scheme| {
            let rest = &full_url[scheme + 3..];
            rest.find('/').map(|slash| rest[slash..].to_owned())
        })
        .unwrap_or_else(|| full_url.to_owned());
    Ok(format!(r#"{{"path":"{path}"}}"#).into_bytes())
}

/// 构造 MockTLSServer：`secure` 为真时附带占位 CA PEM，并注入 path 回显 Client。
fn mock_server(secure: bool, base: &str) -> MockTLSServer {
    MockTLSServer {
        TLS: if secure {
            Some(TLSConfig {
                CA: b"-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_vec(),
                Cert: Vec::new(),
                Key: Vec::new(),
            })
        } else {
            None
        },
        URL: base.to_owned(),
        Client: Arc::new(respond_path_handler),
    }
}

/// 明文 HTTP：GetJSON 应带回请求 path。
#[test]
fn test_get_json_insecure() {
    let server = mock_server(false, "http://127.0.0.1:18080");
    let ctx = Context::Background();
    let tls = NewTLSFromMockServer(&server);
    let result = tls.GetJSON(&ctx, "/aaa").unwrap();
    assert!(
        String::from_utf8_lossy(&result).contains(r#""/aaa""#)
            || String::from_utf8_lossy(&result).contains("/aaa")
    );
    let result = tls.GetJSON(&ctx, "/bbbb").unwrap();
    assert!(String::from_utf8_lossy(&result).contains("/bbbb"));
}

/// HTTPS Mock：GetJSON 同样按 path 回显。
#[test]
fn test_get_json_secure() {
    let server = mock_server(true, "https://127.0.0.1:18443");
    let ctx = Context::Background();
    let tls = NewTLSFromMockServer(&server);
    let result = tls.GetJSON(&ctx, "/ccc").unwrap();
    assert!(String::from_utf8_lossy(&result).contains("/ccc"));
    let result = tls.GetJSON(&ctx, "/dddd").unwrap();
    assert!(String::from_utf8_lossy(&result).contains("/dddd"));
}

/// WithHost：按是否有 TLS 决定最终 scheme，并剥离输入中的 scheme 前缀。
#[test]
fn test_with_host() {
    let mock_tls_server = mock_server(true, "https://127.0.0.1:18443");
    let mock_server_plain = mock_server(false, "http://127.0.0.1:18080");

    // (期望 URL, 传入 host, 是否 secure mock)
    let test_cases = [
        ("https://127.0.0.1:2379", "http://127.0.0.1:2379", true),
        ("http://127.0.0.1:2379", "https://127.0.0.1:2379", false),
        (
            "http://127.0.0.1:2379/pd/api/v1/stores",
            "127.0.0.1:2379/pd/api/v1/stores",
            false,
        ),
        ("https://127.0.0.1:2379", "127.0.0.1:2379", true),
    ];

    for (expected, host, secure) in test_cases {
        let server = if secure {
            &mock_tls_server
        } else {
            &mock_server_plain
        };
        let tls = NewTLSFromMockServer(server);
        assert_eq!(expected, GetMockTLSUrl(&tls.WithHost(host)));
    }
}

/// NewTLS：证书/私钥非 PEM 时应返回 PEM 解析失败错误。
#[test]
fn test_invalid_tls() {
    let temp_dir =
        std::env::temp_dir().join(format!("lightning-common-tls-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let ca_path = temp_dir.join("ca.pem");
    let cert_path = temp_dir.join("test.pem");
    let key_path = temp_dir.join("test.key");

    // 合法 CA PEM + 非法 cert/key 内容，触发 PEM 校验失败
    let ca_content = VALID_CA_PEM;
    std::fs::write(&ca_path, ca_content).unwrap();
    let cert_content = b"invalid cert content";
    let key_content = b"invalid key content";
    std::fs::write(&cert_path, cert_content).unwrap();
    std::fs::write(&key_path, key_content).unwrap();

    let err = match NewTLS(
        ca_path.to_string_lossy().into_owned(),
        String::new(),
        String::new(),
        "localhost".to_owned(),
        ca_content.to_vec(),
        cert_content.to_vec(),
        key_content.to_vec(),
    ) {
        Err(err) => err,
        Ok(_) => panic!("invalid cert should fail"),
    };
    assert!(
        err.to_string()
            .contains("tls: failed to find any PEM data in certificate input"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(temp_dir);
}

/// Go `NewTLSConfig` gives CA paths precedence over in-memory CA content.
#[test]
fn test_ca_path_takes_precedence_over_content() {
    let temp_dir = std::env::temp_dir().join(format!(
        "lightning-common-tls-priority-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let ca_path = temp_dir.join("ca.pem");
    let ca_content = VALID_CA_PEM;
    std::fs::write(&ca_path, ca_content).unwrap();

    let tls = NewTLS(
        ca_path.to_string_lossy().into_owned(),
        String::new(),
        String::new(),
        "localhost".to_owned(),
        b"invalid in-memory CA".to_vec(),
        Vec::new(),
        Vec::new(),
    )
    .expect("the valid CA path must override invalid in-memory content");

    assert_eq!(
        Some(ca_content),
        tls.TLSConfig().map(|config| config.CA.as_slice())
    );
    let _ = std::fs::remove_dir_all(temp_dir);
}

/// Go ignores an incomplete client key pair but still creates a TLS config.
#[test]
fn test_incomplete_key_pair_is_ignored() {
    let tls = NewTLS(
        String::new(),
        "missing-cert.pem".to_owned(),
        String::new(),
        "localhost".to_owned(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .expect("an isolated certificate path is not loaded");

    let config = tls.TLSConfig().expect("non-empty input enables TLS");
    assert!(config.Cert.is_empty());
    assert!(config.Key.is_empty());
}
