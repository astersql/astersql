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
use crate::gemini::GeminiEmbedder;

#[test]
fn go_merge_43_gemini_provider_batches_contents_and_decodes_values() {
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
        assert!(
            header
                .starts_with("post /v1beta/models/text-embedding-004:batchembedcontents http/1.1")
        );
        assert!(header.contains("x-goog-api-key: test-key"));
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
        assert_eq!(body["requests"][0]["model"], "models/text-embedding-004");
        assert_eq!(body["requests"][0]["content"]["parts"][0]["text"], "hello");
        assert_eq!(body["requests"][0]["taskType"], "RETRIEVAL_QUERY");
        let response = r#"{"embeddings":[{"values":[1.0,2.0]}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let embedder = GeminiEmbedder::new(
        || "test-key".into(),
        move || format!("http://{address}/v1beta/models"),
    );
    assert_eq!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "text-embedding-004",
                &["hello".into()],
                &Options::from([("taskType".into(), serde_json::json!("RETRIEVAL_QUERY"))]),
            )
            .unwrap(),
        vec![vec![1.0, 2.0]]
    );
    server.join().unwrap();
}

#[test]
fn gemini_shared_contract_preserves_limits_validation_and_causes() {
    crate::cohere_test::provider_contract(
        |cfg| Box::new(crate::gemini::GeminiEmbedder::with_config(cfg)),
        "Gemini",
    );
}

#[test]
fn gemini_protocol_keeps_per_text_fields_model_escaping_and_error_messages() {
    use crate::cohere_test::{protocol_call, request_payload};
    let factory =
        |cfg| Box::new(crate::gemini::GeminiEmbedder::with_config(cfg)) as Box<dyn Embedder>;
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("content".into(), serde_json::json!("wrong")),
        ("outputDimensionality".into(), serde_json::json!(10)),
    ]);
    let (result, request) = protocol_call(
        factory,
        200,
        r#"{"embeddings":[{"values":[-0.010632273,0.019375853,0.020965198,0.0007706437,-0.061464068]},{"values":[0.018468002,0.0054281265,-0.017658807,0.013859263,0.05341865]},{"values":[0.058089074,0.020941732,-0.10872878,-0.04039259,0.12345678]}]}"#,
        "text embedding/004?x=1",
        &[
            "hello world".into(),
            "test text".into(),
            "sample input".into(),
        ],
        &opts,
    );
    assert_eq!(
        result.unwrap(),
        vec![
            vec![
                -0.010632273,
                0.019375853,
                0.020965198,
                0.0007706437,
                -0.061464068
            ],
            vec![
                0.018468002,
                0.0054281265,
                -0.017658807,
                0.013859263,
                0.05341865
            ],
            vec![
                0.058089074,
                0.020941732,
                -0.10872878,
                -0.04039259,
                0.12345678
            ]
        ]
    );
    assert!(
        request.starts_with("POST /v1/text%20embedding%2F004%3Fx=1:batchEmbedContents HTTP/1.1")
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-goog-api-key: test-api-key")
    );
    let payload = request_payload(&request);
    for (i, text) in ["hello world", "test text", "sample input"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            payload["requests"][i],
            serde_json::json!({"model":"models/text embedding/004?x=1", "content":{"parts":[{"text":text}]}, "outputDimensionality":10})
        );
    }
    for (status, message) in [
        (400, "API key not valid"),
        (404, "model is not found for API version v1beta"),
    ] {
        let body = serde_json::json!({"error":{"code":status,"message":message,"status":"INVALID_ARGUMENT"}}).to_string();
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
    assert!(
        protocol_call(
            factory,
            200,
            r#"{"embeddings":[{"values":[1]}]}"#,
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
fn gemini_endpoints_validate_configuration_and_keep_default_protocol_routes() {
    crate::cohere_test::invalid_endpoints_and_missing_keys(
        |cfg| Box::new(crate::gemini::GeminiEmbedder::with_config(cfg)),
        "Gemini",
    );
    let provider = crate::gemini::GeminiEmbedder::with_config(Default::default());
    assert_eq!(
        provider.endpoint("model").unwrap().url.as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/model:batchEmbedContents"
    );
}

#[test]
fn gemini_options_preserve_the_original_ten_dimensional_response() {
    use crate::cohere_test::{protocol_call, request_payload};
    let opts = Options::from([
        ("outputDimensionality".into(), serde_json::json!(10)),
        ("model".into(), serde_json::json!("wrong")),
        ("content".into(), serde_json::json!("wrong")),
    ]);
    let (result, request) = protocol_call(
        |cfg| Box::new(crate::gemini::GeminiEmbedder::with_config(cfg)),
        200,
        r#"{"embeddings":[{"values":[-0.010632273,0.019375853,0.020965198,0.0007706437,-0.061464068,0.123456,0.789012,0.345678,0.901234,0.567890]}]}"#,
        "text-embedding-004",
        &["test".into()],
        &opts,
    );
    assert_eq!(
        result.unwrap(),
        vec![vec![
            -0.010632273,
            0.019375853,
            0.020965198,
            0.0007706437,
            -0.061464068,
            0.123456,
            0.789012,
            0.345678,
            0.901234,
            0.567890
        ]]
    );
    assert_eq!(
        request_payload(&request),
        serde_json::json!({"requests":[{"model":"models/text-embedding-004","content":{"parts":[{"text":"test"}]},"outputDimensionality":10}]})
    );
}
