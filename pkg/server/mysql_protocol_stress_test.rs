// Copyright 2026 AsterSQL.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use crate::mysql_compat_test_support::{MysqlCompatServer, TextValue, WireResponse};

const CLIENT_COMPRESS: u32 = 1 << 5;
const CLIENT_PROTOCOL_41: u32 = 1 << 9;
const CLIENT_SECURE_CONNECTION: u32 = 1 << 15;
const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
const MALFORMED_IO_TIMEOUT: Duration = Duration::from_secs(5);

fn read_raw_packet(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .map_err(|error| format!("read packet header: {error}"))?;
    let length = header[0] as usize | (header[1] as usize) << 8 | (header[2] as usize) << 16;
    let mut payload = vec![0; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("read packet payload: {error}"))?;
    Ok((header[3], payload))
}

fn write_raw_packet(stream: &mut TcpStream, sequence: u8, payload: &[u8]) -> Result<(), String> {
    let header = [
        payload.len() as u8,
        (payload.len() >> 8) as u8,
        (payload.len() >> 16) as u8,
        sequence,
    ];
    stream
        .write_all(&header)
        .and_then(|_| stream.write_all(payload))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("write packet: {error}"))
}

fn raw_connection(server: &MysqlCompatServer) -> TcpStream {
    let mut stream = TcpStream::connect(server.mysql_addr()).expect("connect raw malformed client");
    stream
        .set_read_timeout(Some(MALFORMED_IO_TIMEOUT))
        .expect("set malformed read timeout");
    stream
        .set_write_timeout(Some(MALFORMED_IO_TIMEOUT))
        .expect("set malformed write timeout");
    let (sequence, handshake) = read_raw_packet(&mut stream).expect("read server handshake");
    assert_eq!(sequence, 0);
    assert_eq!(handshake.first(), Some(&10));
    stream
}

fn assert_closed_or_protocol_error(label: &str, stream: &mut TcpStream) {
    stream
        .shutdown(Shutdown::Write)
        .expect("shutdown malformed client writes");
    let mut response = [0_u8; 1];
    match stream.read(&mut response) {
        Ok(0) => {}
        Ok(_) => assert_eq!(
            response[0], 0xff,
            "{label}: server returned a non-error byte before closing"
        ),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Err(error) => panic!("{label}: malformed connection did not close in time: {error}"),
    }
}

fn assert_healthy_connection(server: &MysqlCompatServer, label: &str) {
    let mut client = server
        .connect_root(0, None)
        .unwrap_or_else(|error| panic!("{label}: reconnect after malformed packet: {error}"));
    assert!(
        matches!(client.ping(), Ok(WireResponse::Ok(_))),
        "{label}: PING failed after malformed packet"
    );
    let WireResponse::ResultSet(result) = client
        .query("SELECT 1")
        .unwrap_or_else(|error| panic!("{label}: SELECT failed after malformed packet: {error}"))
    else {
        panic!("{label}: SELECT did not return a result set");
    };
    assert_eq!(result.rows, vec![vec![TextValue::Bytes(b"1".to_vec())]]);
}

fn authenticate_with_compression(stream: &mut TcpStream) {
    let capabilities =
        CLIENT_COMPRESS | CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH;
    let mut response = Vec::new();
    response.extend_from_slice(&capabilities.to_le_bytes());
    response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
    response.push(45);
    response.extend_from_slice(&[0; 23]);
    response.extend_from_slice(b"root\0");
    response.push(0);
    response.extend_from_slice(b"mysql_native_password\0");
    write_raw_packet(stream, 1, &response).expect("write compressed handshake response");
    let (sequence, auth) = read_raw_packet(stream).expect("read compressed authentication result");
    assert_eq!(sequence, 2);
    assert_eq!(
        auth.first(),
        Some(&0x00),
        "compression authentication failed"
    );
}

fn verify_compressed_ping(stream: &mut TcpStream) {
    let mut frame = vec![5, 0, 0, 0, 0, 0, 0];
    frame.extend_from_slice(&[1, 0, 0, 0, 0x0e]);
    stream
        .write_all(&frame)
        .and_then(|_| stream.flush())
        .expect("write compressed PING");

    let mut header = [0_u8; 7];
    stream
        .read_exact(&mut header)
        .expect("read compressed PING response header");
    let compressed_length =
        header[0] as usize | (header[1] as usize) << 8 | (header[2] as usize) << 16;
    assert_eq!(header[3], 0, "compressed PING outer sequence");
    assert_eq!(&header[4..7], &[0, 0, 0], "short PING response is plain");
    let mut payload = vec![0; compressed_length];
    stream
        .read_exact(&mut payload)
        .expect("read compressed PING response payload");
    assert!(payload.len() >= 5, "compressed PING response is truncated");
    assert_eq!(payload[3], 1, "compressed PING inner sequence");
    assert_eq!(payload[4], 0x00, "compressed PING did not return OK");
}

#[test]
fn malformed_connections_are_isolated_from_following_ping_and_select() {
    let server = MysqlCompatServer::start().expect("start compatibility server");
    let cases: &[(&str, &[u8])] = &[
        ("truncated_header", &[1, 0]),
        ("truncated_payload", &[4, 0, 0, 1, 0, 0]),
        ("wrong_sequence", &[0, 0, 0, 9]),
        (
            "max_payload_declared_then_truncated",
            &[0xff, 0xff, 0xff, 1],
        ),
    ];

    for (label, bytes) in cases {
        let mut stream = raw_connection(&server);
        stream
            .write_all(bytes)
            .expect("write malformed packet bytes");
        assert_closed_or_protocol_error(label, &mut stream);
        assert_healthy_connection(&server, label);
    }

    let mut compressed = raw_connection(&server);
    authenticate_with_compression(&mut compressed);
    verify_compressed_ping(&mut compressed);
    compressed
        .write_all(&[0, 0, 0, 9, 0, 0, 0])
        .expect("write invalid compressed sequence");
    assert_closed_or_protocol_error("invalid_compressed_sequence", &mut compressed);
    assert_healthy_connection(&server, "invalid_compressed_sequence");
}
