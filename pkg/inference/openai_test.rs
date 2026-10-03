// Copyright 2026 AsterSQL.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::thread;

use base64::Engine;

use crate::embed_fn::{Embedder, Options};
use crate::openai::OpenAIEmbedder;

#[test]
fn go_merge_43_openai_compatible_provider_posts_and_reorders_indexed_vectors() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        let body_start = loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let header = String::from_utf8_lossy(&request[..body_start]).to_ascii_lowercase();
        let content_length = header
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap();
        while request.len() - body_start < content_length {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
        }
        assert!(header.starts_with("post /v1/embeddings http/1.1"));
        assert!(header.contains("authorization: bearer test-key"));
        let body: serde_json::Value =
            serde_json::from_slice(&request[body_start..body_start + content_length]).unwrap();
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["input"], serde_json::json!(["first", "second"]));
        assert_eq!(body["encoding_format"], "base64");
        assert_eq!(body["dimensions"], 2);
        let encoded = |values: &[f32]| {
            base64::engine::general_purpose::STANDARD.encode(
                values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
        };
        let response = serde_json::json!({"data": [
            {"index": 1, "embedding": encoded(&[3.0, 4.0])},
            {"index": 0, "embedding": encoded(&[1.0, 2.0])}
        ]})
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });

    let embedder = OpenAIEmbedder::new(
        || "test-key".to_owned(),
        move || format!("http://{address}/v1"),
    );
    let result = embedder
        .create_embeddings(
            &AtomicBool::new(false),
            "text-embedding-3-small",
            &["first".into(), "second".into()],
            &Options::from([("dimensions".into(), serde_json::json!(2))]),
        )
        .unwrap();
    assert_eq!(result, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    server.join().unwrap();
}

#[test]
fn decoder_accepts_empty_embedding_and_real_jina_fixture() {
    use crate::openai::decode_indexed_base64_embeddings as decode;
    assert_eq!(
        decode(br#"{"data":[{"index":0,"embedding":""}]}"#, 1).unwrap(),
        vec![Vec::<f32>::new()]
    );
    let body = br#"{"data":[{"index":0,"embedding":"AAAYPgAAEb8AACq+AAAXPgAA4b0AAP0+AACUvQAA4TwAAC67AAAVPw=="}]}"#;
    assert_eq!(
        decode(body, 1).unwrap()[0],
        [
            0.1484375,
            -0.56640625,
            -0.166015625,
            0.1474609375,
            -0.10986328125,
            0.494140625,
            -0.072265625,
            0.0274658203125,
            -0.002655029296875,
            0.58203125
        ]
    );
}

#[test]
fn endpoint_normalizes_existing_suffix_and_keeps_query() {
    let provider = OpenAIEmbedder::new(
        || "key".into(),
        || "http://localhost/v1/embeddings/?api-version=x#section".into(),
    );
    // Exercise the provider's actual endpoint resolution without making a request.
    assert_eq!(
        provider.endpoint().unwrap().as_str(),
        "http://localhost/v1/embeddings?api-version=x#section"
    );
}

fn http_fixture(
    status: u16,
    response: String,
    delay: std::time::Duration,
) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 2048];
        let end = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let header = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
        let length = header
            .lines()
            .find_map(|v| v.strip_prefix("content-length: "))
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap();
        while request.len() < end + length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        thread::sleep(delay);
        let _ = write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        );
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}/v1"), worker)
}

#[test]
fn openai_fixed_fields_options_and_endpoint_variants_match_wire_contract() {
    for suffix in [
        "",
        "/",
        "/embeddings",
        "/embeddings/",
        "/v1?api-version=x",
        "/v1/embeddings?api-version=x",
        "/v1/embeddings/?api-version=x#section",
    ] {
        let base = format!("http://localhost{suffix}");
        let provider = OpenAIEmbedder::new(|| "key".into(), move || base.clone());
        let endpoint = provider.endpoint().unwrap();
        assert!(!endpoint.path().ends_with('/'));
        assert!(endpoint.path().ends_with("/embeddings"));
        if suffix.contains('?') {
            assert_eq!(endpoint.query(), Some("api-version=x"));
        }
    }
    let (url, server) = http_fixture(
        200,
        r#"{"data":[{"index":0,"embedding":"AACAPw=="}]}"#.into(),
        std::time::Duration::ZERO,
    );
    let provider = OpenAIEmbedder::new(|| "test-key".into(), move || url.clone());
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("input".into(), serde_json::json!(["wrong"])),
        ("encoding_format".into(), serde_json::json!("float")),
        ("my_opt".into(), serde_json::json!("abc")),
    ]);
    assert_eq!(
        provider
            .create_embeddings(&AtomicBool::new(false), "right", &["hello".into()], &opts)
            .unwrap(),
        vec![vec![1.0]]
    );
    let request = server.join().unwrap();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    let payload: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        payload,
        serde_json::json!({"model":"right","input":["hello"],"encoding_format":"base64","my_opt":"abc"})
    );
}

#[test]
fn openai_http_errors_redact_messages_and_log_only_safe_fields() {
    use logutil::log::{BgLogger, LogField};
    let secret = "dash\"secret";
    for (status, body, expected) in [
        (
            400,
            serde_json::json!({"error":{"message":format!("invalid api key: {secret}")}})
                .to_string(),
            "OpenAI: status code 400, message: invalid api key: [REDACTED]",
        ),
        (
            502,
            "{\"error\":".into(),
            "OpenAI: status code 502, message: Bad Gateway",
        ),
        (
            404,
            r#"{"error":{"message":"The model 'missing' does not exist"}}"#.into(),
            "OpenAI: status code 404, message: The model 'missing' does not exist",
        ),
        (
            201,
            "{}".into(),
            "OpenAI: status code 201, message: Created",
        ),
    ] {
        let (url, server) = http_fixture(status, body, std::time::Duration::ZERO);
        let provider = OpenAIEmbedder::new(move || secret.into(), move || url.clone());
        let error = provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["text".into()],
                &Options::new(),
            )
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!error.contains(secret));
        server.join().unwrap();
    }
    let entries = BgLogger().entries();
    let errors = entries
        .iter()
        .filter(|entry| entry.message == "OpenAI API request failed")
        .collect::<Vec<_>>();
    assert!(
        errors
            .iter()
            .any(|entry| entry.fields.contains(&LogField::String(
                "message".into(),
                "invalid api key: [REDACTED]".into()
            )))
    );
    assert!(errors.iter().any(|entry| {
        entry
            .fields
            .iter()
            .any(|field| field.key() == "parse_error")
    }));
    for entry in errors {
        assert!(!entry.fields.iter().any(|field| field.key() == "body"));
        assert!(!format!("{:?}", entry.fields).contains(secret));
    }
}

#[test]
fn openai_body_limits_apply_to_success_and_error_responses() {
    use crate::openai::OpenAIConfig;
    use std::sync::Arc;
    for status in [200, 400] {
        let (url, server) = http_fixture(status, "x".repeat(65), std::time::Duration::ZERO);
        let provider = OpenAIEmbedder::with_config(OpenAIConfig {
            api_key: Some(Arc::new(|| "key".into())),
            base_url: Some(Arc::new(move || url.clone())),
            max_response_bytes: 64,
            ..Default::default()
        });
        assert_eq!(
            provider
                .create_embeddings(
                    &AtomicBool::new(false),
                    "model",
                    &["text".into()],
                    &Options::new()
                )
                .unwrap_err(),
            "response body exceeds maximum size of 64 bytes"
        );
        server.join().unwrap();
    }
    let body = format!(
        "{}{}",
        r#"{"data":[{"index":0,"embedding":"AACAPw=="}]}"#,
        " ".repeat(20)
    );
    let (url, server) = http_fixture(200, body.clone(), std::time::Duration::ZERO);
    let provider = OpenAIEmbedder::with_config(OpenAIConfig {
        api_key: Some(Arc::new(|| "key".into())),
        base_url: Some(Arc::new(move || url.clone())),
        max_response_bytes: body.len() as i64,
        ..Default::default()
    });
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["text".into()],
                &Options::new()
            )
            .unwrap(),
        vec![vec![1.0]]
    );
    server.join().unwrap();
}

#[test]
fn openai_configuration_defaults_overrides_and_timeout() {
    use crate::openai::OpenAIConfig;
    use std::sync::Arc;
    let provider = OpenAIEmbedder::with_config(Default::default());
    assert_eq!(
        provider.endpoint().unwrap().as_str(),
        "https://api.openai.com/v1/embeddings"
    );
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["text".into()],
                &Options::new()
            )
            .unwrap_err(),
        "API key is not configured for OpenAI"
    );
    assert!(
        provider
            .create_embeddings(&AtomicBool::new(true), "", &[], &Options::new())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "",
                &["text".into()],
                &Options::new()
            )
            .unwrap_err(),
        "model name is required"
    );
    let provider = OpenAIEmbedder::with_config(OpenAIConfig {
        missing_key_error: Some("configure me".into()),
        ..Default::default()
    });
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["text".into()],
                &Options::new()
            )
            .unwrap_err(),
        "configure me"
    );
    for custom in [None, Some("new key please".to_owned())] {
        let (url, server) = http_fixture(401, "{}".into(), std::time::Duration::ZERO);
        let provider = OpenAIEmbedder::with_config(OpenAIConfig {
            api_key: Some(Arc::new(|| "key".into())),
            base_url: Some(Arc::new(move || url.clone())),
            unauthorized_error: custom.clone(),
            ..Default::default()
        });
        assert_eq!(
            provider
                .create_embeddings(
                    &AtomicBool::new(false),
                    "model",
                    &["text".into()],
                    &Options::new()
                )
                .unwrap_err(),
            custom.unwrap_or_else(|| "OpenAI returns status unauthorized, check API key".into())
        );
        server.join().unwrap();
    }
    let (url, server) = http_fixture(200, "{}".into(), std::time::Duration::from_millis(100));
    let mut provider = OpenAIEmbedder::new(|| "key".into(), move || url.clone());
    provider.client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(20))
        .build()
        .unwrap();
    assert!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["text".into()],
                &Options::new()
            )
            .unwrap_err()
            .contains("request failed")
    );
    server.join().unwrap();
    for base in ["://invalid", "localhost/v1"] {
        let provider = OpenAIEmbedder::new(|| "key".into(), move || base.into());
        assert!(provider.endpoint().is_err());
    }
}

#[test]
fn openai_indexed_decoder_checks_count_indices_base64_and_byte_length() {
    use crate::openai::decode_indexed_base64_embeddings as decode;
    for (body, count, expected) in [
        (r#"{"data":[]}"#, 1, "length 0"),
        (
            r#"{"data":[{"index":0,"embedding":"AACAPw=="},{"index":0,"embedding":"AACAPw=="}]}"#,
            2,
            "duplicate index 0",
        ),
        (
            r#"{"data":[{"index":2,"embedding":"AACAPw=="}]}"#,
            1,
            "out of range",
        ),
        (
            r#"{"data":[{"index":-1,"embedding":"AACAPw=="}]}"#,
            1,
            "index",
        ),
        (
            r#"{"data":[{"index":0,"embedding":"AAEC"}]}"#,
            1,
            "invalid embedding data",
        ),
        (
            r#"{"data":[{"index":0,"embedding":"!!!!"}]}"#,
            1,
            "decode embedding",
        ),
    ] {
        assert!(
            decode(body.as_bytes(), count)
                .unwrap_err()
                .contains(expected),
            "{body}"
        );
    }
    assert!(decode(b"{", 1).unwrap_err().contains("unmarshal"));
}

#[test]
fn openai_protocol_zero_values_and_byte_arrays_follow_go_json() {
    use crate::openai::decode_indexed_base64_embeddings as decode;
    for body in [
        r#"{"data":[{"index":0,"embedding":null}]}"#,
        r#"{"data":[{}]}"#,
    ] {
        assert_eq!(decode(body.as_bytes(), 1).unwrap(), vec![Vec::<f32>::new()]);
    }
    assert_eq!(
        decode(br#"{"data":[{"embedding":[0,0,128,63]}]}"#, 1).unwrap(),
        vec![vec![1.0]]
    );
    assert_eq!(
        decode(br#"{"data":[{"embedding":"AA\nCAPw=="}]}"#, 1).unwrap(),
        vec![vec![1.0]]
    );
}

#[test]
fn openai_cancellation_aborts_inflight_http_request() {
    use std::sync::{Arc, atomic::Ordering};
    let (url, server) = http_fixture(
        200,
        r#"{"data":[{"index":0,"embedding":"AACAPw=="}]}"#.into(),
        std::time::Duration::from_secs(1),
    );
    let provider = OpenAIEmbedder::new(|| "key".into(), move || url.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    let timer = thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(50));
        trigger.store(true, Ordering::Release);
    });
    let begin = std::time::Instant::now();
    let error = provider
        .create_embeddings(&cancel, "model", &["text".into()], &Options::new())
        .unwrap_err();
    let elapsed = begin.elapsed();
    timer.join().unwrap();
    server.join().unwrap();
    assert!(error.contains("canceled"));
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "cancellation waited {elapsed:?}"
    );
}
