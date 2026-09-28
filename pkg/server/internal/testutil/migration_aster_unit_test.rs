// Copyright 2026 AsterSQL.
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

// testutil 迁移对照单元测试。
//
// 校验 `BytesConn` 顺序读/EOF、写与连接元数据空操作，以及 IPv4/IPv6
// 地址端口提取与 Go 侧桩行为一致。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::SystemTime;

use super::{BytesConn, get_port_from_tcp_addr};

/// 分次读取应按缓冲顺序返回，耗尽后读长度为 0（EOF）。
#[test]
fn bytes_conn_reads_buffer_in_order_and_reports_eof() {
    let mut conn = BytesConn::new(b"packet".to_vec());
    let mut first = [0; 3];
    let mut rest = Vec::new();

    assert_eq!(conn.read(&mut first).unwrap(), 3);
    assert_eq!(&first, b"pac");
    assert_eq!(conn.read_to_end(&mut rest).unwrap(), 3);
    assert_eq!(rest, b"ket");
    assert_eq!(conn.read(&mut first).unwrap(), 0);
}

/// 写返回 0、close/deadline 成功、地址为 None，且写不破坏可读缓冲。
#[test]
fn bytes_conn_write_and_connection_metadata_match_go_stub() {
    let mut conn = BytesConn::new(b"input".to_vec());

    assert_eq!(conn.write(b"ignored").unwrap(), 0);
    assert!(conn.close().is_ok());
    assert_eq!(conn.local_addr(), None);
    assert_eq!(conn.remote_addr(), None);
    assert!(conn.set_deadline(SystemTime::now()).is_ok());
    assert!(conn.set_read_deadline(SystemTime::now()).is_ok());
    assert!(conn.set_write_deadline(SystemTime::now()).is_ok());

    // 写为空操作，原输入缓冲仍可完整读出。
    let mut remaining = Vec::new();
    conn.read_to_end(&mut remaining).unwrap();
    assert_eq!(remaining, b"input");
}

/// IPv4 / IPv6 SocketAddr 均应正确提取端口。
#[test]
fn tcp_port_is_extracted_for_ipv4_and_ipv6() {
    let ipv4 = SocketAddr::from((Ipv4Addr::LOCALHOST, 4000));
    let ipv6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 10080));

    assert_eq!(get_port_from_tcp_addr(ipv4), 4000);
    assert_eq!(get_port_from_tcp_addr(ipv6), 10080);
}
