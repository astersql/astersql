// Copyright 2026 AsterSQL.

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
