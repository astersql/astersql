// Copyright 2026 AsterSQL.

use crate::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn get(addr: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("status service must accept connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn status_service_binds_and_exposes_go_routes() {
    let tctx = tcontext::Background();
    let handle = startDumplingService(&tctx, "127.0.0.1:0").unwrap();

    for path in [
        "/metrics",
        "/debug/pprof/",
        "/debug/pprof/cmdline",
        "/debug/pprof/profile",
        "/debug/pprof/symbol",
        "/debug/pprof/trace",
    ] {
        let response = get(&handle.addr, path);
        assert!(response.starts_with("HTTP/1.1 200"), "{path}: {response}");
    }

    handle.stop();
}

#[test]
fn status_service_reports_listen_failure_with_context() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = occupied.local_addr().unwrap().to_string();
    let err = startDumplingService(&tcontext::Background(), &addr).unwrap_err();
    assert!(err.msg.contains("start listening"), "{}", err.msg);
}

#[test]
fn net_closing_detection_matches_go_nil_and_substring_cases() {
    assert!(!isErrNetClosing_pub(None));
    assert!(isErrNetClosing_pub(Some(&errors_new(format!(
        "wrapped: {useOfClosedErrMsg}"
    )))));
    assert!(!isErrNetClosing_pub(Some(&errors_new("other error"))));
}
