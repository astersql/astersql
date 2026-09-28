// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use std::net::TcpListener;
use std::thread;

#[test]
fn ok_packet_decodes_go_length_encoded_info() {
    let packet = parse_ok_packet(
        &[0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, b'o', b'k'],
        CLIENT_PROTOCOL_41,
    )
    .expect("parse OK packet with Go-compatible length-encoded info");

    assert_eq!(packet.info, b"ok");
}

#[test]
fn packet_reader_reassembles_go_multi_packet_payloads() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let address = listener.local_addr().expect("read listener address");
    let writer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept test client");
        let first = vec![b'a'; 0x00ff_ffff];
        write_packet_to(&mut stream, 7, &first).expect("write maximum-size first frame");
        write_packet_to(&mut stream, 8, b"tail").expect("write continuation frame");
    });
    let mut stream = TcpStream::connect(address).expect("connect test reader");

    let packet = read_packet_from(&mut stream).expect("read logical multi-packet payload");

    assert_eq!(packet.sequence, 7);
    assert_eq!(packet.payload.len(), 0x00ff_ffff + 4);
    assert_eq!(&packet.payload[..3], b"aaa");
    assert_eq!(&packet.payload[packet.payload.len() - 4..], b"tail");
    writer.join().expect("join packet writer");
}

#[test]
fn mysql_compat_test_support_round_trips_packets() {
    for value in [0, 250, 251, 0xffff, 0x1_0000, 0xff_ffff, 0x1_000000] {
        let mut encoded = Vec::new();
        put_lenenc_int(&mut encoded, value);
        assert_eq!(
            PacketCursor::new(&encoded)
                .read_lenenc_int()
                .expect("decode encoded length"),
            value
        );
    }

    let server = MysqlCompatServer::start().expect("start compatibility server");
    assert_ne!(server.mysql_addr().port(), 0);

    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("complete root handshake");
    assert_eq!(client.handshake.protocol_version, 10);
    assert!(
        client
            .handshake
            .server_version
            .contains("8.0.11-TiDB-AsterSQL")
    );
    assert_ne!(client.handshake.connection_id, 0);
    assert_eq!(
        client.handshake.negotiated_capabilities & CLIENT_DEPRECATE_EOF,
        CLIENT_DEPRECATE_EOF
    );

    let WireResponse::Ok(ping) = client.ping().expect("parse COM_PING OK packet") else {
        panic!("COM_PING did not return OK");
    };
    assert_eq!(ping.header, 0x00);
    assert_eq!(ping.affected_rows, 0);

    let WireResponse::Err(error) = client
        .command(0x7f, &[])
        .expect("parse unsupported-command ERR packet")
    else {
        panic!("unsupported command did not return ERR");
    };
    assert_eq!(error.code, 1047);
    assert_eq!(error.sql_state, Some(*b"08S01"));
    assert!(error.message.contains("is not supported"));

    let WireResponse::ResultSet(result) =
        client.query("SELECT 1").expect("parse SELECT result set")
    else {
        panic!("SELECT 1 did not return a result set");
    };
    assert_eq!(result.columns.len(), 1);
    assert_eq!(result.columns[0].catalog, "def");
    assert_eq!(result.columns[0].name, "1");
    assert_ne!(result.columns[0].character_set, 0);
    assert_eq!(result.rows, vec![vec![TextValue::Bytes(b"1".to_vec())]]);
    assert_eq!(result.status & 0x0002, 0x0002);

    let mut legacy_eof_client = server
        .connect_root(0, None)
        .expect("complete root handshake without deprecated EOF");
    let WireResponse::ResultSet(legacy_result) = legacy_eof_client
        .query("SELECT 1")
        .expect("parse legacy EOF result set")
    else {
        panic!("legacy EOF SELECT 1 did not return a result set");
    };
    assert_eq!(
        legacy_result.rows,
        vec![vec![TextValue::Bytes(b"1".to_vec())]]
    );
}
