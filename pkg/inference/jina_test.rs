// Copyright 2026 AsterSQL.

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
