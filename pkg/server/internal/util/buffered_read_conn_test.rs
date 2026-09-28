// Copyright 2026 AsterSQL.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use crate::NewBufferedReadConn;

#[test]
fn peek_waits_for_fragmented_bytes_without_consuming_them() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"a").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        stream.write_all(b"b").unwrap();
    });

    let stream = TcpStream::connect(address).unwrap();
    let mut conn = NewBufferedReadConn(stream).unwrap();
    assert_eq!(conn.Peek(2).unwrap(), b"ab");

    let mut bytes = [0; 2];
    conn.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ab");
    server.join().unwrap();
}
