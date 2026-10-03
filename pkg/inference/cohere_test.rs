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

use crate::cohere::CohereEmbedder;
use crate::embed_fn::{Embedder, Options};

#[test]
fn go_merge_43_cohere_provider_posts_texts_and_decodes_typed_float() {
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
        assert!(header.starts_with("post /v1/embed http/1.1"));
        assert!(header.contains("authorization: bearer test-key"));
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
        assert_eq!(body["model"], "embed-v4");
        assert_eq!(body["texts"], serde_json::json!(["hello"]));
        assert_eq!(body["input_type"], "search_query");
        assert_eq!(body["embedding_types"], serde_json::json!(["float"]));
        let response = r#"{"embeddings":{"float":[[1.0,2.0]]}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let embedder = CohereEmbedder::new(
        || "test-key".into(),
        move || format!("http://{address}/v1/embed"),
    );
    assert!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "embed-v4",
                &["hello".into()],
                &Options::from([("embedding_types".into(), serde_json::json!(["int8"]))]),
            )
            .unwrap_err()
            .contains("exactly")
    );
    let embeddings = embedder
        .create_embeddings(
            &AtomicBool::new(false),
            "embed-v4",
            &["hello".into()],
            &Options::from([
                ("embedding_types".into(), serde_json::json!(["float"])),
                ("input_type".into(), serde_json::json!("search_query")),
            ]),
        )
        .unwrap();
    assert_eq!(embeddings, vec![vec![1.0, 2.0]]);
    server.join().unwrap();
}

#[test]
fn provider_errors_share_status_fallback_and_secret_redaction() {
    for provider in [
        "cohere",
        "gemini",
        "huggingface",
        "jina",
        "nvidia",
        "tidbcloud",
    ] {
        for status in [201, 400, 502] {
            let body = if status == 400 {
                serde_json::json!({"message":"invalid provider-secret Bearer other-token", "detail":"invalid provider-secret Bearer other-token", "error":{"message":"invalid provider-secret Bearer other-token"}}).to_string()
            } else {
                "{".into()
            };
            let body =
                if status == 400 && ["huggingface", "tidbcloud", "nvidia"].contains(&provider) {
                    serde_json::json!({"error":"invalid provider-secret Bearer other-token"})
                        .to_string()
                } else {
                    body
                };
            let (url, server) =
                crate::openai_test::http_fixture(status, body, std::time::Duration::ZERO);
            let embedder: Box<dyn Embedder> = match provider {
                "cohere" => Box::new(CohereEmbedder::new(
                    || "provider-secret".into(),
                    move || url.clone(),
                )),
                "gemini" => Box::new(crate::gemini::GeminiEmbedder::new(
                    || "provider-secret".into(),
                    move || url.clone(),
                )),
                "huggingface" => Box::new(crate::huggingface::HuggingFaceEmbedder::new(
                    || "provider-secret".into(),
                    move || url.clone(),
                )),
                "jina" => Box::new(crate::jina::JinaEmbedder::new(
                    || "provider-secret".into(),
                    move || url.clone(),
                )),
                "nvidia" => Box::new(crate::nvidia::NvidiaEmbedder::new(
                    || "provider-secret".into(),
                    move || url.clone(),
                )),
                _ => Box::new(crate::tidbcloud::TiDBCloudFreeEmbedder::new(
                    || "".into(),
                    || "provider-secret".into(),
                    move || url.clone(),
                )),
            };
            let result = embedder.create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["text".into()],
                &Options::new(),
            );
            server.join().unwrap();
            let label = match provider {
                "cohere" => "Cohere",
                "gemini" => "Gemini",
                "huggingface" => "HuggingFace",
                "jina" => "JinaAI",
                "nvidia" => "NVIDIA NIM",
                _ => "TiDB Cloud Inference",
            };
            let message = match status {
                201 => "Created",
                502 => "Bad Gateway",
                _ => "invalid [REDACTED] Bearer [REDACTED]",
            };
            assert_eq!(
                result.unwrap_err(),
                format!("{label}: status code {status}, message: {message}")
            );
        }
    }
}

pub(super) fn provider_contract(
    factory: fn(crate::base::APIKeyProviderConfig) -> Box<dyn Embedder>,
    label: &str,
) {
    use crate::base::{APIKeyProviderConfig, ProviderContext, ProviderError};
    use std::error::Error;
    use std::sync::Arc;
    let call = |provider: &dyn Embedder| {
        provider.create_embeddings_with_context(
            &ProviderContext::new(&AtomicBool::new(false)),
            "model",
            &["text".into()],
            &Options::new(),
        )
    };
    let provider = factory(APIKeyProviderConfig::default());
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
    for status in [200, 400] {
        let (url, server) =
            crate::openai_test::http_fixture(status, "x".repeat(65), std::time::Duration::ZERO);
        let provider = factory(APIKeyProviderConfig {
            api_key: Some(Arc::new(|| "key".into())),
            base_url: Some(Arc::new(move || url.clone())),
            max_response_bytes: 64,
            ..Default::default()
        });
        assert_eq!(
            call(provider.as_ref()).unwrap_err().to_string(),
            "response body exceeds maximum size of 64 bytes"
        );
        server.join().unwrap();
    }
    let cause: Arc<dyn Error + Send + Sync> =
        Arc::new(std::io::Error::other("caller stopped request"));
    let provider = factory(APIKeyProviderConfig {
        api_key: Some(Arc::new(|| "key".into())),
        base_url: Some(Arc::new(|| "http://127.0.0.1:1?token=super-secret".into())),
        ..Default::default()
    });
    let callback = || {
        Some(ProviderError::redacted(
            "caller stopped request",
            cause.clone(),
        ))
    };
    let context = ProviderContext {
        cancel: &AtomicBool::new(true),
        cancellation_cause: Some(&callback),
    };
    let error = provider
        .create_embeddings_with_context(&context, "model", &["text".into()], &Options::new())
        .unwrap_err();
    assert!(error.has_cause(&cause));
    let transport_error = call(provider.as_ref()).unwrap_err();
    assert_eq!(
        transport_error.to_string(),
        format!("{label} request failed")
    );
    let transport = transport_error
        .source()
        .unwrap()
        .downcast_ref::<reqwest::Error>()
        .unwrap();
    assert!(transport.url().is_none());
    assert!(!format!("{transport_error}").contains("super-secret"));
    for status in [401, 403] {
        if label == "TiDB Cloud Inference" || (status == 403 && label != "NVIDIA NIM") {
            continue;
        }
        let (url, server) =
            crate::openai_test::http_fixture(status, "{".into(), std::time::Duration::ZERO);
        let provider = factory(APIKeyProviderConfig {
            api_key: Some(Arc::new(|| "key".into())),
            base_url: Some(Arc::new(move || url.clone())),
            unauthorized_error: Some(ProviderError::redacted(
                "custom unauthorized",
                cause.clone(),
            )),
            ..Default::default()
        });
        assert!(call(provider.as_ref()).unwrap_err().has_cause(&cause));
        server.join().unwrap();
    }
    if label != "TiDB Cloud Inference" {
        let provider = factory(APIKeyProviderConfig {
            missing_key_error: Some(ProviderError::redacted("custom missing key", cause.clone())),
            ..Default::default()
        });
        assert!(call(provider.as_ref()).unwrap_err().has_cause(&cause));
    }
}

#[test]
fn cohere_shared_contract_preserves_limits_validation_and_causes() {
    provider_contract(|cfg| Box::new(CohereEmbedder::with_config(cfg)), "Cohere");
}

pub(super) fn protocol_call(
    factory: fn(crate::base::APIKeyProviderConfig) -> Box<dyn Embedder>,
    status: u16,
    response: &str,
    model: &str,
    texts: &[String],
    opts: &Options,
) -> (Result<Vec<Vec<f32>>, String>, String) {
    use std::sync::Arc;
    let (url, server) =
        crate::openai_test::http_fixture(status, response.into(), std::time::Duration::ZERO);
    let provider = factory(crate::base::APIKeyProviderConfig {
        api_key: Some(Arc::new(|| "test-api-key".into())),
        base_url: Some(Arc::new(move || url.clone())),
        ..Default::default()
    });
    let result = provider.create_embeddings(&AtomicBool::new(false), model, texts, opts);
    (result, server.join().unwrap())
}

pub(super) fn request_payload(request: &str) -> serde_json::Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
fn cohere_protocol_covers_untyped_rows_fixed_options_and_rejected_types() {
    let factory = |cfg| Box::new(CohereEmbedder::with_config(cfg)) as Box<dyn Embedder>;
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("texts".into(), serde_json::json!(["wrong"])),
        ("input_type".into(), serde_json::json!("search_document")),
    ]);
    let (result, request) = protocol_call(
        factory,
        200,
        r#"{"embeddings":[[0.016296387,-0.008354187,0.12345678,-0.98765432,0.5],[0.04663086,-0.023239136,0.87654321,-0.11111111,0.3],[0.11111111,0.22222222,0.33333333,0.44444444,0.55555555]]}"#,
        "embed-v4.0",
        &[
            "hello world".into(),
            "test text".into(),
            "sample input".into(),
        ],
        &opts,
    );
    let values = result.unwrap();
    assert_eq!(
        values,
        vec![
            vec![0.016296387, -0.008354187, 0.12345678, -0.98765432, 0.5],
            vec![0.04663086, -0.023239136, 0.87654321, -0.11111111, 0.3],
            vec![0.11111111, 0.22222222, 0.33333333, 0.44444444, 0.55555555]
        ]
    );
    let payload = request_payload(&request);
    assert_eq!(payload["model"], "embed-v4.0");
    assert_eq!(
        payload["texts"],
        serde_json::json!(["hello world", "test text", "sample input"])
    );
    assert_eq!(payload["input_type"], "search_document");
    for invalid in [
        serde_json::json!(["int8"]),
        serde_json::json!(["float", "int8"]),
        serde_json::json!("float"),
        serde_json::json!(["float", 8]),
    ] {
        let provider = CohereEmbedder::new(
            || panic!("validation must precede key getter"),
            || panic!("validation must precede endpoint getter"),
        );
        assert_eq!(
            provider
                .create_embeddings(
                    &AtomicBool::new(false),
                    "model",
                    &["text".into()],
                    &Options::from([("embedding_types".into(), invalid)])
                )
                .unwrap_err(),
            r#"Cohere embedding_types must be exactly ["float"]"#
        );
    }
    for (body, error) in [
        (r#"{"embeddings":{"int8":[[1]]}}"#, "does not contain float"),
        (r#"{"embeddings":[[1]]}"#, "length 1"),
        ("{", "unmarshal"),
    ] {
        assert!(
            protocol_call(
                factory,
                200,
                body,
                "model",
                &["a".into(), "b".into()],
                &Options::new()
            )
            .0
            .unwrap_err()
            .contains(error)
        );
    }
    assert!(
        protocol_call(
            factory,
            404,
            r#"{"message":"model 'bad' not found"}"#,
            "bad",
            &["a".into()],
            &Options::new()
        )
        .0
        .unwrap_err()
        .contains("model 'bad' not found")
    );
}

#[test]
fn invalid_authentication_headers_report_safe_request_errors_and_context_causes() {
    use crate::base::{APIKeyProviderConfig, ProviderContext, ProviderError};
    use std::sync::Arc;
    let provider = CohereEmbedder::with_config(APIKeyProviderConfig {
        api_key: Some(Arc::new(|| "secret\nkey".into())),
        base_url: Some(Arc::new(|| "http://127.0.0.1:1".into())),
        ..Default::default()
    });
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["a".into()],
                &Options::new()
            )
            .unwrap_err(),
        "Cohere request failed"
    );
    let cause: Arc<dyn std::error::Error + Send + Sync> =
        Arc::new(std::io::Error::other("caller stopped"));
    let observer = || Some(ProviderError::redacted("caller stopped", cause.clone()));
    let context = ProviderContext {
        cancel: &AtomicBool::new(true),
        cancellation_cause: Some(&observer),
    };
    assert!(
        provider
            .create_embeddings_with_context(&context, "model", &["a".into()], &Options::new())
            .unwrap_err()
            .has_cause(&cause)
    );
}

pub(super) fn invalid_endpoints_and_missing_keys(
    factory: fn(crate::base::APIKeyProviderConfig) -> Box<dyn Embedder>,
    label: &str,
) {
    use std::sync::Arc;
    for url in [
        "://invalid",
        "/relative",
        "ftp://example.com/path",
        "https://example.com/%zz?token=super-secret",
    ] {
        let provider = factory(crate::base::APIKeyProviderConfig {
            api_key: Some(Arc::new(|| "key".into())),
            base_url: Some(Arc::new(move || url.into())),
            ..Default::default()
        });
        let error = provider
            .create_embeddings(
                &AtomicBool::new(false),
                "model",
                &["a".into()],
                &Options::new(),
            )
            .unwrap_err();
        assert!(error.starts_with("invalid "), "{error}");
        assert!(!error.contains("super-secret"));
    }
    if label != "TiDB Cloud Inference" {
        let provider = factory(Default::default());
        assert_eq!(
            provider
                .create_embeddings(
                    &AtomicBool::new(false),
                    "model",
                    &["a".into()],
                    &Options::new()
                )
                .unwrap_err(),
            format!("API key is not configured for {label}")
        );
    }
}

#[test]
fn cohere_endpoints_validate_configuration_and_keep_default_protocol_routes() {
    crate::cohere_test::invalid_endpoints_and_missing_keys(
        |cfg| Box::new(crate::cohere::CohereEmbedder::with_config(cfg)),
        "cohere",
    );
    let provider = crate::cohere::CohereEmbedder::with_config(Default::default());
    assert_eq!(
        provider.endpoint("model").unwrap().as_str(),
        "https://api.cohere.com/v1/embed"
    );
}

#[test]
fn cohere_typed_float_options_preserve_the_original_classification_fixture() {
    let opts = Options::from([
        ("input_type".into(), serde_json::json!("classification")),
        ("embedding_types".into(), serde_json::json!(["float"])),
        ("model".into(), serde_json::json!("wrong")),
        ("texts".into(), serde_json::json!(["wrong"])),
    ]);
    let (result, request) = protocol_call(
        |cfg| Box::new(CohereEmbedder::with_config(cfg)),
        200,
        r#"{"embeddings":{"float":[[0.1,0.2,0.3]]}}"#,
        "embed-v4.0",
        &["test".into()],
        &opts,
    );
    assert_eq!(result.unwrap(), vec![vec![0.1, 0.2, 0.3]]);
    assert_eq!(
        request_payload(&request),
        serde_json::json!({"model":"embed-v4.0","texts":["test"],"input_type":"classification","embedding_types":["float"]})
    );
}
