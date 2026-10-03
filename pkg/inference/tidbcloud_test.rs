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

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::thread;

use base64::Engine;

use crate::embed_fn::{Embedder, Options};
use crate::tidbcloud::TiDBCloudFreeEmbedder;

#[test]
fn go_merge_43_hosted_provider_uses_billing_path_and_optional_key() {
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
        assert!(header.starts_with("post /base/api/v1/inference/embeddings/cluster_test http/1.1"));
        assert!(!header.contains("authorization:"));
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap();
        while request.len() - body_start < length {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
        }
        let body: serde_json::Value =
            serde_json::from_slice(&request[body_start..body_start + length]).unwrap();
        assert_eq!(body["model"], "hosted-model");
        assert_eq!(body["texts"], serde_json::json!(["hello"]));
        let encoded = base64::engine::general_purpose::STANDARD.encode(1.0_f32.to_le_bytes());
        let response = serde_json::json!({"embeddings":[encoded]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let embedder = TiDBCloudFreeEmbedder::new(
        || "cluster_test".into(),
        String::new,
        move || format!("http://{address}/base"),
    );
    assert_eq!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "hosted-model",
                &["hello".into()],
                &Options::new(),
            )
            .unwrap(),
        vec![vec![1.0]]
    );
    server.join().unwrap();
}

#[test]
fn billing_dot_segments_remain_escaped_in_the_actual_http_request() {
    let (url, server) = crate::openai_test::http_fixture(
        200,
        r#"{"embeddings":["AACAPw=="]}"#.into(),
        std::time::Duration::ZERO,
    );
    let provider = crate::tidbcloud::TiDBCloudFreeEmbedder::new(
        || "..".into(),
        || "".into(),
        move || url.clone(),
    );
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
    let request = server.join().unwrap();
    assert!(
        request.starts_with("POST /v1/api/v1/inference/embeddings/%2E%2E HTTP/1.1"),
        "{request}"
    );
}

#[test]
fn tidbcloud_shared_contract_preserves_limits_validation_and_causes() {
    crate::cohere_test::provider_contract(
        |cfg| {
            Box::new(crate::tidbcloud::TiDBCloudFreeEmbedder::with_config(
                crate::tidbcloud::TiDBCloudConfig {
                    api_key: cfg.api_key,
                    base_url: cfg.base_url,
                    max_response_bytes: cfg.max_response_bytes,
                    ..Default::default()
                },
            ))
        },
        "TiDB Cloud Inference",
    );
}

#[test]
fn cloud_protocol_covers_default_billing_fixed_options_errors_and_byte_validation() {
    use crate::cohere_test::{protocol_call, request_payload};
    let factory = |cfg: crate::base::APIKeyProviderConfig| {
        Box::new(crate::tidbcloud::TiDBCloudFreeEmbedder::with_config(
            crate::tidbcloud::TiDBCloudConfig {
                api_key: cfg.api_key,
                base_url: cfg.base_url,
                max_response_bytes: cfg.max_response_bytes,
                ..Default::default()
            },
        )) as Box<dyn Embedder>
    };
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("texts".into(), serde_json::json!(["wrong"])),
        ("input_type".into(), serde_json::json!("search_document")),
    ]);
    let (result, request) = protocol_call(
        factory,
        200,
        r#"{"embeddings":["AACAPwAAAEA=","AABAQAAAgEA="]}"#,
        "cohere/embed-english-v3",
        &["first".into(), "second".into()],
        &opts,
    );
    assert_eq!(result.unwrap(), vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    assert!(
        request.starts_with("POST /v1/api/v1/inference/embeddings/default_billing_id HTTP/1.1")
    );
    assert_eq!(
        request_payload(&request),
        serde_json::json!({"model":"cohere/embed-english-v3","texts":["first","second"],"input_type":"search_document"})
    );
    for (status, message) in [
        (400, "Unknown model 'abc'"),
        (
            400,
            "Malformed input request: required key [input_type] not found",
        ),
        (403, "Exceeded quota limit"),
    ] {
        let body = serde_json::json!({"error":message}).to_string();
        assert!(
            protocol_call(
                factory,
                status,
                &body,
                "model",
                &["a".into()],
                &Options::new()
            )
            .0
            .unwrap_err()
            .contains(message)
        );
    }
    for (body, error) in [
        (r#"{"embeddings":[]}"#, "length 0"),
        (r#"{"embeddings":["AAEC"]}"#, "invalid embedding data"),
        (r#"{"embeddings":[null]}"#, "embedding data is empty"),
        (r#"{"embeddings":["!"]}"#, "decode embedding"),
    ] {
        assert!(
            protocol_call(factory, 200, body, "model", &["a".into()], &Options::new())
                .0
                .unwrap_err()
                .contains(error)
        );
    }
    let provider = crate::tidbcloud::TiDBCloudFreeEmbedder::with_config(Default::default());
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["a".into()],
                &Options::new()
            )
            .unwrap_err(),
        "base URL is not configured for TiDB Cloud Inference"
    );
}

#[test]
fn cloud_endpoints_validate_configuration_and_preserve_escaped_billing_queries() {
    use std::sync::Arc;
    crate::cohere_test::invalid_endpoints_and_missing_keys(
        |cfg| {
            Box::new(crate::tidbcloud::TiDBCloudFreeEmbedder::with_config(
                crate::tidbcloud::TiDBCloudConfig {
                    api_key: cfg.api_key,
                    base_url: cfg.base_url,
                    ..Default::default()
                },
            ))
        },
        "TiDB Cloud Inference",
    );
    let provider =
        crate::tidbcloud::TiDBCloudFreeEmbedder::with_config(crate::tidbcloud::TiDBCloudConfig {
            billing_id: Some(Arc::new(|| "billing/id?revision=1".into())),
            base_url: Some(Arc::new(|| " https://example.com/root/?tenant=x ".into())),
            ..Default::default()
        });
    assert_eq!(
        provider.endpoint().unwrap().url.as_str(),
        "https://example.com/root/api/v1/inference/embeddings/billing%2Fid%3Frevision=1?tenant=x"
    );
}
