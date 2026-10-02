// Copyright 2026 AsterSQL.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

#[test]
fn pd_split_config_uses_first_successful_tikv_and_decimal_go_units() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let stores = serde_json::json!({"stores": [
        {"store": {"status_address": ""}},
        {"store": {"status_address": "invalid", "labels": [{"key":"engine","value":"tiflash"}]}},
        {"store": {"status_address": "invalid", "state": 2}},
        {"store": {"status_address": address, "address": address, "state": 1}},
        {"store": {"status_address": address, "address": address}},
        {"store": {"status_address": address, "address": address}},
        {"store": {"status_address": "invalid", "address": "invalid"}}
    ]})
    .to_string();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = std::thread::spawn(move || {
        let replies = [
            ("200 OK", stores),
            ("500 Internal Server Error", "{}".into()),
            (
                "200 OK",
                r#"{"coprocessor":{"region-split-size":"bad","region-split-keys":17}}"#.into(),
            ),
            (
                "200 OK",
                r#"{"coprocessor":{"region-split-size":"0X_1.8p+1MB","region-split-keys":-42}}"#
                    .into(),
            ),
        ];
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0; 4096];
            let length = stream.read(&mut buffer).unwrap();
            captured.lock().unwrap().push(
                String::from_utf8_lossy(&buffer[..length])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            );
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(
        runtime
            .block_on(get_region_split_config(
                &kv::Context::default(),
                &[address],
                None
            ))
            .unwrap(),
        (3_000_000, -42)
    );
    server.join().unwrap();
    assert_eq!(
        *requests.lock().unwrap(),
        [
            "GET /pd/api/v1/stores HTTP/1.1",
            "GET /config HTTP/1.1",
            "GET /config HTTP/1.1",
            "GET /config HTTP/1.1"
        ]
    );
}

#[test]
fn pd_split_config_cancellation_interrupts_pending_http_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let context = kv::Context::default();
    let cancel = context.clone();
    let (cancelled_at, received) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0; 4096];
        stream.read(&mut buffer).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")
            .unwrap();
        cancelled_at.send(Instant::now()).unwrap();
        cancel.cancel();
        // Keep the peer open until the client has to observe cancellation.
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let _ = stream.read(&mut buffer);
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert!(
        runtime
            .block_on(get_region_split_config(&context, &[address], None))
            .unwrap_err()
            .to_string()
            .contains("context canceled")
    );
    assert!(received.recv().unwrap().elapsed() < Duration::from_millis(500));
    server.join().unwrap();
}

#[test]
fn pd_split_config_resolves_loopback_status_port_and_retains_unresolved_host() {
    assert_eq!(
        status_address("192.0.2.1:20160", "127.0.0.1:20180"),
        "192.0.2.1:20180"
    );
    assert_eq!(
        status_address("[2001:db8::1]:20160", "[::]:20180"),
        "[2001:db8::1]:20180"
    );
    assert_eq!(
        status_address("127.0.0.1:20160", "192.0.2.1:20180"),
        "192.0.2.1:20180"
    );
    assert_eq!(
        status_address("invalid", "127.0.0.1:20180"),
        "127.0.0.1:20180"
    );
}
