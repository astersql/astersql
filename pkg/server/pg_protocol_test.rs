// Copyright 2026 AsterSQL.
use crate::pg_protocol::{
    MAX_STARTUP_LENGTH, PROTOCOL_VERSION, PROTOCOL_VERSION_30, StartupError, parse_startup,
    read_startup,
};
use std::io::{Cursor, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

// AsterSQL raw TCP client baseline v1: explicit protocol 3.2, no libpq defaults.
fn packet(version: u32, parameters: &[u8]) -> Vec<u8> {
    let mut p = ((8 + parameters.len()) as u32).to_be_bytes().to_vec();
    p.extend_from_slice(&version.to_be_bytes());
    p.extend_from_slice(parameters);
    p
}

#[test]
fn startup_packet_bounds() {
    let p = packet(PROTOCOL_VERSION, b"user\0root\0database\0test\0\0");
    let m = parse_startup(&p).unwrap();
    assert_eq!(m.parameters.get("user").map(String::as_str), Some("root"));
    assert_eq!(
        m.parameters.get("database").map(String::as_str),
        Some("test")
    );
    for version in [PROTOCOL_VERSION_30, PROTOCOL_VERSION] {
        let parsed = parse_startup(&packet(version, b"user\0root\0\0")).unwrap();
        assert_eq!(parsed.protocol_version, version);
        assert_eq!(parsed.parameters["user"], "root");
    }
    for v in [0, 131072, 196609, 196611, 262144, u32::MAX] {
        assert_eq!(
            parse_startup(&packet(v, b"\0")),
            Err(StartupError::UnsupportedVersion(v))
        );
    }
    for version in [PROTOCOL_VERSION_30, PROTOCOL_VERSION] {
        let p = packet(version, b"user\0root\0database\0test\0\0");
        for len in [0u32, 3, 4, 7, 8, (MAX_STARTUP_LENGTH + 1) as u32, u32::MAX] {
            let mut invalid = p.clone();
            invalid[..4].copy_from_slice(&len.to_be_bytes());
            assert!(parse_startup(&invalid).is_err());
        }
        for end in 0..p.len() {
            assert!(parse_startup(&p[..end]).is_err());
        }
        for params in [
            b"".as_slice(),
            b"user\0root\0",
            b"user\0",
            b"user\0root\0\0x",
            b"\0\0",
            b"user\0\xff\0\0",
            b"user\0a\0user\0b\0\0",
        ] {
            assert!(parse_startup(&packet(version, params)).is_err());
        }
        assert!(parse_startup(&packet(version, b"application_name\0\0\0")).is_ok());
        let mut params = b"user\0".to_vec();
        params.resize(MAX_STARTUP_LENGTH - 10, b'x');
        params.extend_from_slice(b"\0\0");
        assert_eq!(packet(version, &params).len(), MAX_STARTUP_LENGTH);
        assert!(parse_startup(&packet(version, &params)).is_ok());
        params.insert(5, b'x');
        assert!(parse_startup(&packet(version, &params)).is_err());
    }
}

#[test]
fn startup_raw_tcp_client_v1_requests_32() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(&packet(PROTOCOL_VERSION, b"user\0root\0\0"))
        .unwrap();
    assert_eq!(
        read_startup(&mut server)
            .unwrap()
            .parameters
            .get("user")
            .map(String::as_str),
        Some("root")
    );
    client
        .write_all(&packet(196608, b"user\0root\0\0"))
        .unwrap();
    assert_eq!(
        read_startup(&mut server).unwrap().protocol_version,
        PROTOCOL_VERSION_30
    );
}

#[test]
fn startup_reader_bounds() {
    for length in [0u32, 8, (MAX_STARTUP_LENGTH + 1) as u32, u32::MAX] {
        let mut reader = Cursor::new(length.to_be_bytes());
        assert!(matches!(
            read_startup(&mut reader),
            Err(StartupError::InvalidLength(_))
        ));
        assert_eq!(reader.position(), 4);
    }
    let p = packet(PROTOCOL_VERSION, b"user\0root\0\0");
    for end in 0..p.len() {
        assert!(read_startup(&mut Cursor::new(&p[..end])).is_err());
    }
    let mut both = p.clone();
    both.extend_from_slice(&p);
    let mut reader = Cursor::new(both);
    assert!(read_startup(&mut reader).is_ok());
    assert_eq!(reader.position(), p.len() as u64);
    assert!(read_startup(&mut reader).is_ok());
}
