// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{Options, base::*};

#[test]
fn float_decoder_rejects_empty_and_preserves_little_endian_values() {
    assert_eq!(
        decode_float32_array_bytes(&[]).unwrap_err(),
        "embedding data is empty"
    );
    for data in [vec![0; 3], vec![0; 5]] {
        assert_eq!(
            decode_float32_array_bytes(&data).unwrap_err(),
            "invalid embedding data"
        );
    }
    assert_eq!(
        decode_float32_array_bytes(&1.5f32.to_le_bytes()).unwrap(),
        [1.5]
    );
}

#[test]
fn fixed_request_fields_override_owned_options() {
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("dimensions".into(), serde_json::json!(512)),
    ]);
    let fields = Options::from([("model".into(), serde_json::json!("right"))]);
    let merged = json_fields_with_options(fields, &opts);
    assert_eq!(merged["model"], "right");
    assert_eq!(merged["dimensions"], 512);
    assert_eq!(opts["model"], "wrong");
}

#[test]
fn credentials_are_redacted_before_error_truncation() {
    let text = r#"{"authorization":"Bearer secret-token","api_key":"plain-key","message":"Bearer another-secret sk-proj-super-secret-value dash\"secret"}"#;
    let sanitized = sanitize_error_text(text, &[r#"dash\"secret"#]);
    for secret in [
        "secret-token",
        "plain-key",
        "another-secret",
        "sk-proj-super-secret-value",
        r#"dash\"secret"#,
    ] {
        assert!(!sanitized.contains(secret), "{sanitized}");
    }
    assert!(sanitized.contains("[REDACTED]"));
    for field in ["TOKEN", "access_token", "api-key", "credentials"] {
        assert_eq!(
            sanitize_error_text(&format!(r#"{{"{field}":"secret"}}"#), &[]),
            format!(r#"{{"{field}":"[REDACTED]"}}"#)
        );
    }
    assert!(
        sanitize_error_text(&format!(r#"{{"api_key":"{}"}}"#, "s".repeat(4224)), &[]).len() < 100
    );
    let long = sanitize_error_text(&"s".repeat(4224), &[]);
    assert_eq!(long.len(), 4096 + "...[truncated]".len());
    assert!(long.ends_with("...[truncated]"));
}

#[test]
fn bounded_response_reader_matches_limits_and_preserves_io_causes() {
    use std::io::{Error, ErrorKind, Read};
    assert_eq!(read_response_body(&b"abcd"[..], 4).unwrap(), b"abcd");
    assert_eq!(
        read_response_body(&b"abcde"[..], 4)
            .unwrap_err()
            .to_string(),
        "response body exceeds maximum size of 4 bytes"
    );
    assert_eq!(read_response_body(&b"x"[..], i64::MAX).unwrap(), b"x");
    assert_eq!(
        read_response_body(&b""[..], -1).unwrap_err().to_string(),
        "maximum response body size must not be negative"
    );
    assert!(read_response_body(&b""[..], 0).unwrap().is_empty());
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(Error::new(ErrorKind::BrokenPipe, "reader failed"))
        }
    }
    let error = read_response_body(Broken, 64).unwrap_err();
    assert_eq!(error.to_string(), "reader failed");
    assert_eq!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<Error>()
            .unwrap()
            .kind(),
        ErrorKind::BrokenPipe
    );
}

#[test]
fn provider_config_defaults_and_custom_errors_preserve_identity() {
    use std::sync::Arc;
    let cause: Arc<dyn std::error::Error + Send + Sync> =
        Arc::new(std::io::Error::other("custom cause"));
    let original = APIKeyProviderConfig {
        missing_key_error: Some(ProviderError::redacted("configure me", cause.clone())),
        unauthorized_error: Some(ProviderError::redacted("replace key", cause.clone())),
        ..Default::default()
    };
    let normalized = original.clone().with_defaults();
    assert_eq!(original.max_response_bytes, 0);
    assert_eq!(normalized.max_response_bytes, DEFAULT_MAX_RESPONSE_BYTES);
    assert!(
        normalized
            .resolve_api_key_error(None)
            .unwrap_err()
            .has_cause(&cause)
    );
    assert!(
        normalized
            .unauthorized_error("provider", 401)
            .has_cause(&cause)
    );
    assert_eq!(
        APIKeyProviderConfig::default()
            .resolve_api_key_error(None)
            .unwrap_err()
            .to_string(),
        "API key is not configured"
    );
    assert!(
        APIKeyProviderConfig::default()
            .resolve_api_key_error(Some(ProviderError::redacted("fallback", cause.clone())))
            .unwrap_err()
            .has_cause(&cause)
    );
    for (code, text) in [(401, "unauthorized"), (403, "forbidden")] {
        assert_eq!(
            APIKeyProviderConfig::default()
                .unauthorized_error("provider", code)
                .to_string(),
            format!("provider returns status {text}, check API key")
        );
    }
    assert_eq!(
        APIKeyProviderConfig {
            max_response_bytes: 64,
            ..Default::default()
        }
        .with_defaults()
        .max_response_bytes,
        64
    );
    assert_eq!(escape_url_path_segment("model/name"), "model%2Fname");
    assert_eq!(escape_url_path_segment("."), "%2E");
    assert_eq!(escape_url_path_segment(".."), "%2E%2E");
    assert!(
        parse_http_url("https://example.com/%zz?token=secret", "provider URL")
            .unwrap_err()
            .to_string()
            == "invalid provider URL"
    );
    for raw in ["ftp://example.com", "/relative"] {
        assert!(parse_http_url(raw, "provider URL").is_err());
    }
    let safe = ProviderError::redacted("safe message", cause.clone());
    assert_eq!(safe.to_string(), "safe message");
    assert!(safe.has_cause(&cause));
}

fn proxy_fixture(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 2048];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(bytes).unwrap()
    });
    (url, worker)
}

#[test]
fn escaped_paths_preserve_proxy_absolute_targets_and_connect_authentication() {
    use hyper_util::client::proxy::matcher::Matcher;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (url, server) =
        proxy_fixture("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    let proxy = url.replace("http://", "http://user:password@");
    let result = runtime
        .block_on(crate::raw_http::post_json_with_matcher(
            "http://unused.example/a/%2E%2E/b?tenant=x",
            &serde_json::json!({"texts":["first","second"]}),
            reqwest::header::HeaderMap::new(),
            64,
            "provider",
            Matcher::builder().http(proxy).build(),
            std::time::Duration::from_secs(2),
        ))
        .unwrap();
    assert_eq!(result.0, reqwest::StatusCode::OK);
    assert_eq!(result.1, b"{}");
    let request = server.join().unwrap();
    assert!(
        request.starts_with("POST http://unused.example/a/%2E%2E/b?tenant=x HTTP/1.1"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("proxy-authorization: basic dxnlcjpwyxnzd29yza==")
    );
    let (url, server) =
        proxy_fixture("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let proxy = url.replace("http://", "http://user:password@");
    let error = runtime
        .block_on(crate::raw_http::post_json_with_matcher(
            "https://unused.example/a/%2E%2E/b",
            &serde_json::json!({}),
            reqwest::header::HeaderMap::new(),
            64,
            "provider",
            Matcher::builder().https(proxy).build(),
            std::time::Duration::from_secs(2),
        ))
        .unwrap_err();
    assert_eq!(error.to_string(), "provider request failed");
    assert!(std::error::Error::source(&error).is_some());
    let request = server.join().unwrap();
    assert!(
        request.starts_with("CONNECT unused.example:443 HTTP/1.1"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("proxy-authorization: basic dxnlcjpwyxnzd29yza==")
    );
}

#[test]
fn escaped_request_timeouts_and_body_bounds_keep_the_real_error_lifecycle() {
    use hyper_util::client::proxy::matcher::Matcher;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (url, server) =
        crate::openai_test::http_fixture(200, "{}".into(), std::time::Duration::from_millis(100));
    let error = runtime
        .block_on(crate::raw_http::post_json_with_matcher(
            &format!("{url}/%2E"),
            &serde_json::json!({}),
            reqwest::header::HeaderMap::new(),
            64,
            "provider",
            Matcher::builder().build(),
            std::time::Duration::from_millis(20),
        ))
        .unwrap_err();
    assert_eq!(error.to_string(), "provider request failed");
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .is::<tokio::time::error::Elapsed>()
    );
    server.join().unwrap();
    for status in [200, 400] {
        let (url, server) =
            crate::openai_test::http_fixture(status, "x".repeat(65), std::time::Duration::ZERO);
        let error = runtime
            .block_on(crate::raw_http::post_json_with_matcher(
                &format!("{url}/%2E"),
                &serde_json::json!({}),
                reqwest::header::HeaderMap::new(),
                64,
                "provider",
                Matcher::builder().build(),
                std::time::Duration::from_secs(2),
            ))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "response body exceeds maximum size of 64 bytes"
        );
        server.join().unwrap();
    }
}

#[test]
fn json_lifecycle_validates_decoder_configuration_and_preserves_custom_decode_causes() {
    use std::sync::{Arc, atomic::AtomicBool};
    type Decode = fn(&[u8], usize) -> Result<Vec<Vec<f32>>, ProviderError>;
    type DecodeError = fn(&serde_json::Value) -> Result<String, String>;
    let client = http_client("test provider");
    let cancelled = AtomicBool::new(false);
    let context = ProviderContext::new(&cancelled);
    let payload = serde_json::json!({"input":["hello"]});
    let endpoint = || reqwest::Url::parse("http://127.0.0.1:1").unwrap();
    let result = execute_json_embedding_call(
        &context,
        &client,
        "test provider",
        endpoint(),
        &payload,
        reqwest::header::HeaderMap::new(),
        64,
        &[],
        0,
        None::<DecodeError>,
        |_| None,
        None::<Decode>,
    );
    assert_eq!(
        result.unwrap_err().to_string(),
        "test provider error response decoder is not configured"
    );
    let result = execute_json_embedding_call(
        &context,
        &client,
        "test provider",
        endpoint(),
        &payload,
        reqwest::header::HeaderMap::new(),
        64,
        &[],
        0,
        Some(|value: &serde_json::Value| string_field(&value["message"])),
        |_| None,
        None::<Decode>,
    );
    assert_eq!(
        result.unwrap_err().to_string(),
        "test provider success response decoder is not configured"
    );
    let cause: Arc<dyn std::error::Error + Send + Sync> =
        Arc::new(std::io::Error::other("success decoder failed"));
    let (url, server) =
        crate::openai_test::http_fixture(200, r#"{"ok":true}"#.into(), std::time::Duration::ZERO);
    let result = execute_json_embedding_call(
        &context,
        &client,
        "test provider",
        reqwest::Url::parse(&url).unwrap(),
        &payload,
        reqwest::header::HeaderMap::new(),
        64,
        &[],
        2,
        Some(|value: &serde_json::Value| string_field(&value["message"])),
        |_| None,
        Some(|body: &[u8], expected| {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(body).unwrap(),
                serde_json::json!({"ok":true})
            );
            assert_eq!(expected, 2);
            Err::<Vec<Vec<f32>>, ProviderError>(ProviderError::redacted(
                "success decoder failed",
                cause.clone(),
            ))
        }),
    );
    assert!(result.unwrap_err().has_cause(&cause));
    server.join().unwrap();
}

#[test]
fn json_lifecycle_fallback_status_messages_match_go_http_status_text() {
    use std::sync::atomic::AtomicBool;
    let client = http_client("test provider");
    for (status, text) in [
        (413, "Request Entity Too Large"),
        (414, "Request URI Too Long"),
        (416, "Requested Range Not Satisfiable"),
        (503, "Service Unavailable"),
    ] {
        let (url, server) =
            crate::openai_test::http_fixture(status, "{".into(), std::time::Duration::ZERO);
        let error = execute_json_embedding_call(
            &ProviderContext::new(&AtomicBool::new(false)),
            &client,
            "test provider",
            reqwest::Url::parse(&url).unwrap(),
            &serde_json::json!({}),
            reqwest::header::HeaderMap::new(),
            64,
            &[],
            1,
            Some(|value: &serde_json::Value| string_field(&value["message"])),
            |_| None,
            Some(|_: &[u8], _| Ok::<_, ProviderError>(vec![vec![1.0]])),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("test provider: status code {status}, message: {text}")
        );
        server.join().unwrap();
    }
}
