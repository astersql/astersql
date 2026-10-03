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

use crate::embed_fn::{Embedder, Options};
use crate::huggingface::HuggingFaceEmbedder;

#[test]
fn go_merge_43_huggingface_provider_escapes_model_path_and_decodes_vectors() {
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
        assert!(header.starts_with(
            "post /hf-inference/models/team/model%20name/pipeline/feature-extraction http/1.1"
        ));
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
        assert_eq!(body["inputs"], serde_json::json!(["hello"]));
        assert_eq!(body["truncate"], true);
        let response = "[[1.0,2.0]]";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let embedder = HuggingFaceEmbedder::new(
        || "test-key".into(),
        move || format!("http://{address}/hf-inference"),
    );
    assert_eq!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "team/model name",
                &["hello".into()],
                &Options::from([("truncate".into(), serde_json::json!(true))]),
            )
            .unwrap(),
        vec![vec![1.0, 2.0]]
    );
    server.join().unwrap();
}

#[test]
fn model_dot_segments_remain_escaped_in_the_actual_http_request() {
    let (url, server) =
        crate::openai_test::http_fixture(200, "[[1.0]]".into(), std::time::Duration::ZERO);
    let provider =
        crate::huggingface::HuggingFaceEmbedder::new(|| "key".into(), move || url.clone());
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "org/../model",
                &["text".into()],
                &Options::new()
            )
            .unwrap(),
        vec![vec![1.0]]
    );
    let request = server.join().unwrap();
    assert!(
        request
            .starts_with("POST /v1/models/org/%2E%2E/model/pipeline/feature-extraction HTTP/1.1"),
        "{request}"
    );
}

#[test]
fn huggingface_shared_contract_preserves_limits_validation_and_causes() {
    crate::cohere_test::provider_contract(
        |cfg| Box::new(crate::huggingface::HuggingFaceEmbedder::with_config(cfg)),
        "HuggingFace",
    );
}

#[test]
fn escaped_model_requests_abort_io_with_the_callers_original_cause() {
    use std::sync::{Arc, atomic::Ordering};
    let (url, server) = crate::openai_test::http_fixture(
        200,
        "[[1.0]]".into(),
        std::time::Duration::from_millis(200),
    );
    let provider =
        crate::huggingface::HuggingFaceEmbedder::new(|| "key".into(), move || url.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        signal.store(true, Ordering::Release);
    });
    let cause: Arc<dyn std::error::Error + Send + Sync> =
        Arc::new(std::io::Error::other("caller stopped"));
    let observer = || {
        cancelled
            .load(Ordering::Acquire)
            .then(|| crate::base::ProviderError::redacted("caller stopped", cause.clone()))
    };
    let context = crate::base::ProviderContext {
        cancel: &cancelled,
        cancellation_cause: Some(&observer),
    };
    let start = std::time::Instant::now();
    let error = provider
        .create_embeddings_with_context(&context, "org/../model", &["text".into()], &Options::new())
        .unwrap_err();
    assert!(error.has_cause(&cause));
    assert!(start.elapsed() < std::time::Duration::from_millis(150));
    worker.join().unwrap();
    server.join().unwrap();
}

#[test]
fn huggingface_protocol_preserves_fixed_inputs_count_and_special_statuses() {
    use crate::cohere_test::{protocol_call, request_payload};
    let factory = |cfg| {
        Box::new(crate::huggingface::HuggingFaceEmbedder::with_config(cfg)) as Box<dyn Embedder>
    };
    let opts = Options::from([
        ("inputs".into(), serde_json::json!(["wrong"])),
        ("normalize".into(), serde_json::json!(true)),
    ]);
    let (result, request) = protocol_call(
        factory,
        200,
        "[[0.022240305319428444,-0.004567116964608431,0.15847662091255188,-0.08124932646751404],[-0.013980913907289505,-0.058682069182395935,0.23456789012345678,0.98765432109876543]]",
        "org/model?revision=1",
        &["hello".into(), "world".into()],
        &opts,
    );
    assert_eq!(
        result.unwrap(),
        vec![
            vec![
                0.022240305319428444,
                -0.004567116964608431,
                0.15847662091255188,
                -0.08124932646751404
            ],
            vec![
                -0.013980913907289505,
                -0.058682069182395935,
                0.23456789012345678,
                0.98765432109876543
            ]
        ]
    );
    assert!(request.starts_with(
        "POST /v1/models/org/model%3Frevision=1/pipeline/feature-extraction HTTP/1.1"
    ));
    assert_eq!(
        request_payload(&request),
        serde_json::json!({"inputs":["hello","world"],"normalize":true})
    );
    for (status, body, expected) in [
        (
            401,
            "{}",
            "HuggingFace returns status unauthorized, check API key",
        ),
        (
            404,
            "{",
            "HuggingFace model 'missing' does not exist or is not available",
        ),
        (
            503,
            r#"{"error":"Model is currently loading"}"#,
            "HuggingFace: status code 503, message: Model is currently loading",
        ),
    ] {
        assert_eq!(
            protocol_call(
                factory,
                status,
                body,
                "missing",
                &["a".into()],
                &Options::new()
            )
            .0
            .unwrap_err(),
            expected
        );
    }
    assert!(
        protocol_call(
            factory,
            200,
            "[[1]]",
            "model",
            &["a".into(), "b".into()],
            &Options::new()
        )
        .0
        .unwrap_err()
        .contains("length 1")
    );
}

#[test]
fn huggingface_endpoints_validate_configuration_and_keep_default_protocol_routes() {
    crate::cohere_test::invalid_endpoints_and_missing_keys(
        |cfg| Box::new(crate::huggingface::HuggingFaceEmbedder::with_config(cfg)),
        "HuggingFace",
    );
    let provider = crate::huggingface::HuggingFaceEmbedder::with_config(Default::default());
    assert_eq!(
        provider.endpoint("model").unwrap().url.as_str(),
        "https://router.huggingface.co/hf-inference/models/model/pipeline/feature-extraction"
    );
}
