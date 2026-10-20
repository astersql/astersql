// Copyright 2026 AsterSQL.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::{BufferedReadTransport, NewBufferedReadConn};

#[test]
fn buffered_read_conn_is_alive_preserves_pending_data() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    let mut conn = NewBufferedReadConn(server).unwrap();

    assert_eq!(conn.IsAlive(), 1);
    client.write_all(b"ping").unwrap();
    assert_eq!(conn.IsAlive(), 1);

    let mut payload = [0; 4];
    conn.read_exact(&mut payload).unwrap();
    assert_eq!(&payload, b"ping");
}

#[test]
fn buffered_read_conn_concurrent_read_and_is_alive() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    let conn = Arc::new(NewBufferedReadConn(server).unwrap());
    let stopped = Arc::new(AtomicBool::new(false));

    let reader_conn = Arc::clone(&conn);
    let reader_stopped = Arc::clone(&stopped);
    let reader = std::thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        while !reader_stopped.load(Ordering::Acquire) {
            if reader_conn.Read(&mut buffer).is_err() {
                break;
            }
        }
    });
    let writer_stopped = Arc::clone(&stopped);
    let writer = std::thread::spawn(move || {
        let payload = [b'x'; 4096];
        while !writer_stopped.load(Ordering::Acquire) {
            if client.write_all(&payload).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_millis(500);
    let mut probes = 0;
    while Instant::now() < deadline {
        assert_eq!(conn.IsAlive(), 1);
        probes += 1;
    }
    stopped.store(true, Ordering::Release);
    drop(conn);
    writer.join().unwrap();
    reader.join().unwrap();
    assert!(probes > 0);
}

struct NetConnWrapper(TcpStream);

impl Read for NetConnWrapper {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(output)
    }
}

impl BufferedReadTransport for NetConnWrapper {
    fn net_stream(&self) -> Option<&TcpStream> {
        Some(&self.0)
    }
}

struct HiddenConnWrapper(TcpStream);

impl Read for HiddenConnWrapper {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(output)
    }
}

impl BufferedReadTransport for HiddenConnWrapper {}

#[test]
fn buffered_read_conn_unwraps_exposed_tcp_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let _client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    assert_eq!(NewBufferedReadConn(server).unwrap().IsAlive(), 1);

    let _client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    assert_eq!(
        NewBufferedReadConn(NetConnWrapper(server))
            .unwrap()
            .IsAlive(),
        1
    );

    let _client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    assert_eq!(
        NewBufferedReadConn(HiddenConnWrapper(server))
            .unwrap()
            .IsAlive(),
        -1
    );
}

#[test]
fn buffered_read_conn_is_alive_through_net_conn_wrapper() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    let conn = NewBufferedReadConn(NetConnWrapper(server)).unwrap();
    assert_eq!(conn.IsAlive(), 1);
    drop(client);

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && conn.IsAlive() != 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(conn.IsAlive(), 0);
}

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
