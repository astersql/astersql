// Copyright 2026 AsterSQL.

use crate::{New, config};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

#[test]
fn enabling_server_mode_after_http_start_updates_the_handler_state() {
    let mut global = config::GlobalConfig::default();
    global.App.StatusAddr = "127.0.0.1:0".to_string();
    let mut lightning = New(global);
    lightning.GoServe().expect("start HTTP server");
    lightning.enableServerMode();

    let addr = lightning.serverAddr.expect("published listener address");
    let mut stream = TcpStream::connect(addr).expect("connect HTTP server");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .write_all(
            b"POST /tasks HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 4\r\n\r\n????",
        )
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();

    assert!(
        response.starts_with("HTTP/1.1 400 "),
        "server mode must parse the posted task instead of returning 501: {response:?}"
    );
    lightning.Stop();
}
