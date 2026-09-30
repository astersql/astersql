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
