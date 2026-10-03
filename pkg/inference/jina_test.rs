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
use crate::jina::JinaEmbedder;

#[test]
fn go_merge_43_jina_provider_posts_base64_and_rejects_multivector() {
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
        let body: serde_json::Value =
            serde_json::from_slice(&request[body_start..body_start + content_length]).unwrap();
        assert_eq!(body["model"], "jina-embeddings-v3");
        assert_eq!(body["embedding_type"], "base64");
        let embedding = base64::engine::general_purpose::STANDARD
            .encode([1.0_f32.to_le_bytes(), 2.0_f32.to_le_bytes()].concat());
        let response =
            serde_json::json!({"data": [{"index": 0, "embedding": embedding}]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let embedder = JinaEmbedder::new(
        || "test-key".into(),
        move || format!("http://{address}/v1/embeddings"),
    );
    assert!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "jina-embeddings-v3",
                &["hello".into()],
                &Options::from([("return_multivector".into(), serde_json::json!(true))]),
            )
            .unwrap_err()
            .contains("not supported")
    );
    assert_eq!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "jina-embeddings-v3",
                &["hello".into()],
                &Options::new(),
            )
            .unwrap(),
        vec![vec![1.0, 2.0]]
    );
    server.join().unwrap();
}

#[test]
fn go_merge_43_jina_provider_reports_error_detail() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 4096];
        stream.read(&mut buffer).unwrap();
        let body = r#"{"detail":"Model missing"}"#;
        write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let embedder = JinaEmbedder::new(
        || "test-key".into(),
        move || format!("http://{address}/v1/embeddings"),
    );
    let error = embedder
        .create_embeddings(
            &AtomicBool::new(false),
            "missing",
            &["hello".into()],
            &Options::new(),
        )
        .unwrap_err();
    assert_eq!(error, "JinaAI: status code 404, message: Model missing");
    server.join().unwrap();
}

#[test]
fn jina_shared_contract_preserves_limits_validation_and_causes() {
    crate::cohere_test::provider_contract(
        |cfg| Box::new(crate::jina::JinaEmbedder::with_config(cfg)),
        "JinaAI",
    );
}

#[test]
fn jina_protocol_restores_indices_and_rejects_missing_dense_embeddings() {
    use crate::cohere_test::{protocol_call, request_payload};
    let factory = |cfg| Box::new(crate::jina::JinaEmbedder::with_config(cfg)) as Box<dyn Embedder>;
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("input".into(), serde_json::json!(["wrong"])),
        ("embedding_type".into(), serde_json::json!("float")),
        ("task".into(), serde_json::json!("retrieval.passage")),
    ]);
    let (result, request) = protocol_call(
        factory,
        200,
        r#"{"data":[{"index":1,"embedding":"39MmPZun+j7S4Gw+ZEDbvkeeKj5cVwa/96yDPjPxED6S+VW+3JGYPg=="},{"index":0,"embedding":"AACAPw=="}]}"#,
        "jina-embeddings-v3",
        &["first".into(), "second".into()],
        &opts,
    );
    assert_eq!(
        result.unwrap(),
        vec![
            vec![1.0],
            vec![
                0.0407294,
                0.48955998,
                0.23132637,
                -0.42822564,
                0.1666194,
                -0.5247705,
                0.257179,
                0.1415451,
                -0.20895985,
                0.29798782
            ]
        ]
    );
    assert_eq!(
        request_payload(&request),
        serde_json::json!({"model":"jina-embeddings-v3","input":["first","second"],"embedding_type":"base64","task":"retrieval.passage"})
    );
    for item in [
        r#"{"index":0,"embeddings":["AACAPw=="]}"#,
        r#"{"index":0}"#,
        r#"{"index":0,"embedding":null}"#,
        r#"{"index":0,"embedding":""}"#,
    ] {
        let body = format!(r#"{{"data":[{item}]}}"#);
        assert!(
            protocol_call(factory, 200, &body, "model", &["a".into()], &Options::new())
                .0
                .unwrap_err()
                .contains("embedding data is empty")
        );
    }
    assert_eq!(
        protocol_call(factory, 401, "{}", "model", &["a".into()], &Options::new())
            .0
            .unwrap_err(),
        "JinaAI returns status unauthorized, check API key"
    );
    assert!(
        protocol_call(
            factory,
            404,
            r#"{"detail":"Model not found"}"#,
            "model",
            &["a".into()],
            &Options::new()
        )
        .0
        .unwrap_err()
        .contains("Model not found")
    );
}

#[test]
fn jina_endpoints_validate_configuration_and_keep_default_protocol_routes() {
    crate::cohere_test::invalid_endpoints_and_missing_keys(
        |cfg| Box::new(crate::jina::JinaEmbedder::with_config(cfg)),
        "JinaAI",
    );
    let provider = crate::jina::JinaEmbedder::with_config(Default::default());
    assert_eq!(
        provider.endpoint("model").unwrap().as_str(),
        "https://api.jina.ai/v1/embeddings"
    );
}
