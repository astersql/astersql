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
use crate::nvidia::NvidiaEmbedder;

#[test]
fn go_merge_43_nvidia_provider_posts_base64_and_validates_type() {
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
        assert!(header.starts_with("post /v1/embeddings http/1.1"));
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
        assert_eq!(body["input"], serde_json::json!(["hello"]));
        assert_eq!(body["encoding_format"], "base64");
        assert_eq!(body["embedding_type"], "float");
        let encoded = base64::engine::general_purpose::STANDARD.encode(1.0_f32.to_le_bytes());
        let response = serde_json::json!({"data":[{"index":0,"embedding":encoded}]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let embedder = NvidiaEmbedder::new(
        || "test-key".into(),
        move || format!("http://{address}/v1/embeddings"),
    );
    assert!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "baai/bge-m3",
                &["hello".into()],
                &Options::from([("embedding_type".into(), serde_json::json!("int8"))]),
            )
            .unwrap_err()
            .contains("must be")
    );
    assert_eq!(
        embedder
            .create_embeddings(
                &AtomicBool::new(false),
                "baai/bge-m3",
                &["hello".into()],
                &Options::from([("embedding_type".into(), serde_json::json!("float"))]),
            )
            .unwrap(),
        vec![vec![1.0]]
    );
    server.join().unwrap();
}

#[test]
fn nvidia_shared_contract_preserves_limits_validation_and_causes() {
    crate::cohere_test::provider_contract(
        |cfg| Box::new(crate::nvidia::NvidiaEmbedder::with_config(cfg)),
        "NVIDIA NIM",
    );
}

#[test]
fn nvidia_protocol_covers_float_options_error_schemas_and_status_overrides() {
    use crate::cohere_test::{protocol_call, request_payload};
    let factory =
        |cfg| Box::new(crate::nvidia::NvidiaEmbedder::with_config(cfg)) as Box<dyn Embedder>;
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("input".into(), serde_json::json!(["wrong"])),
        ("encoding_format".into(), serde_json::json!("float")),
        ("embedding_type".into(), serde_json::json!("float")),
        ("input_type".into(), serde_json::json!("query")),
    ]);
    let (result, request) = protocol_call(
        factory,
        200,
        r#"{"data":[{"index":0,"embedding":"oTwEP2H/Kz4Jwho/Gf2RPvl5lb3N1IU+z+t6Pb9Sej5h/6u+UXO5vQ=="}]}"#,
        "baai/bge-m3",
        &["test".into()],
        &opts,
    );
    assert_eq!(
        result.unwrap(),
        vec![vec![
            0.5165501,
            0.16796638,
            0.60452324,
            0.2851341,
            -0.07298655,
            0.26138917,
            0.06126004,
            0.24445628,
            -0.33593276,
            -0.09055198
        ]]
    );
    assert_eq!(
        request_payload(&request),
        serde_json::json!({"model":"baai/bge-m3","input":["test"],"encoding_format":"base64","embedding_type":"float","input_type":"query"})
    );
    for invalid in [
        serde_json::json!("int8"),
        serde_json::json!("uint8"),
        serde_json::json!("binary"),
        serde_json::json!(1),
    ] {
        let provider = crate::nvidia::NvidiaEmbedder::new(
            || panic!("validation precedes key getter"),
            || panic!("validation precedes endpoint getter"),
        );
        assert_eq!(
            provider
                .create_embeddings(
                    &AtomicBool::new(false),
                    "model",
                    &["a".into()],
                    &Options::from([("embedding_type".into(), invalid)])
                )
                .unwrap_err(),
            r#"NVIDIA NIM embedding_type must be "float""#
        );
    }
    for (code, text) in [(401, "unauthorized"), (403, "forbidden")] {
        assert_eq!(
            protocol_call(
                factory,
                code,
                r#"{"detail":"Authorization failed"}"#,
                "model",
                &["a".into()],
                &Options::new()
            )
            .0
            .unwrap_err(),
            format!("NVIDIA NIM returns status {text}, check API key")
        );
    }
    assert_eq!(
        protocol_call(
            factory,
            404,
            "404 page not found",
            "missing",
            &["a".into()],
            &Options::new()
        )
        .0
        .unwrap_err(),
        "NVIDIA NIM model 'missing' does not exist or is not available"
    );
    for body in [
        r#"{"detail":"first","message":"second","error":"third"}"#,
        r#"{"detail":"","message":"first","error":"third"}"#,
        r#"{"error":"first"}"#,
    ] {
        assert_eq!(
            protocol_call(factory, 400, body, "model", &["a".into()], &Options::new())
                .0
                .unwrap_err(),
            "NVIDIA NIM: status code 400, message: first"
        );
    }
}

#[test]
fn nvidia_endpoints_validate_configuration_and_keep_default_protocol_routes() {
    crate::cohere_test::invalid_endpoints_and_missing_keys(
        |cfg| Box::new(crate::nvidia::NvidiaEmbedder::with_config(cfg)),
        "NVIDIA NIM",
    );
    let provider = crate::nvidia::NvidiaEmbedder::with_config(Default::default());
    assert_eq!(
        provider.endpoint("model").unwrap().as_str(),
        "https://integrate.api.nvidia.com/v1/embeddings"
    );
}
