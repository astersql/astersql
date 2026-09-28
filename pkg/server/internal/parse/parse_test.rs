// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// COM_STMT_FETCH 与连接属性解析的单元测试。
//
// 覆盖 fetch 包长度/字段解析与大小上限，以及下划线自定义属性弃用警告
// 与连接属性超限截断（ConnectAttrsSize / ConnectAttrsLost）行为。

use super::parse::{ParseError, parse_attrs, stmt_fetch_cmd};
use super::sessionctx::vardef::{ConnectAttrsLost, ConnectAttrsSize};

/// COM_STMT_FETCH 允许的最大 fetch_size（与服务端解析上限对齐）。
const MAX_FETCH_SIZE: u32 = 1024;

/// 校验 `stmt_fetch_cmd`：合法小端 stmt_id/fetch_size、超限截断到 MAX_FETCH_SIZE，
/// 以及过短/过长包报 MalformedPacket。
#[test]
pub fn TestParseStmtFetchCmd() {
    let tests = vec![
        // 正常：stmt_id=3，fetch_size=50。
        StmtFetchCase {
            arg: vec![3, 0, 0, 0, 50, 0, 0, 0],
            stmt_id: 3,
            fetch_size: 50,
            err: None,
        },
        // 正常：fetch_size=1000（小端 0xe8 0x03）。
        StmtFetchCase {
            arg: vec![5, 0, 0, 0, 232, 3, 0, 0],
            stmt_id: 5,
            fetch_size: 1000,
            err: None,
        },
        // 超过上限时截断到 MAX_FETCH_SIZE（2048 -> 1024）。
        StmtFetchCase {
            arg: vec![5, 0, 0, 0, 0, 8, 0, 0],
            stmt_id: 5,
            fetch_size: MAX_FETCH_SIZE,
            err: None,
        },
        // 包过短。
        StmtFetchCase {
            arg: vec![5, 0, 0],
            stmt_id: 0,
            fetch_size: 0,
            err: Some(ParseError::MalformedPacket),
        },
        // 包过长（超出 8 字节固定载荷）。
        StmtFetchCase {
            arg: vec![1, 0, 0, 0, 3, 2, 0, 0, 3, 5, 6],
            stmt_id: 0,
            fetch_size: 0,
            err: Some(ParseError::MalformedPacket),
        },
        // 空包。
        StmtFetchCase {
            arg: vec![],
            stmt_id: 0,
            fetch_size: 0,
            err: Some(ParseError::MalformedPacket),
        },
    ];

    for tc in tests {
        // 错误时用 (0,0) 占位，便于与期望的 err 一并比对。
        let (stmt_id, fetch_size, err) = match stmt_fetch_cmd(&tc.arg) {
            Ok((stmt_id, fetch_size)) => (stmt_id, fetch_size, None),
            Err(error) => (0, 0, Some(error)),
        };
        assert_eq!(tc.stmt_id, stmt_id);
        assert_eq!(tc.fetch_size, fetch_size);
        assert_eq!(tc.err, err);
    }
}

/// 校验 `parse_attrs`：非白名单下划线 key 产生弃用警告；
/// 白名单属性无警告；超出 ConnectAttrsSize 时截断并累加 ConnectAttrsLost。
#[test]
pub fn TestParseAttrsUnderscoreWarning() {
    let _globals = super::migration_aster_unit_test::globals_lock();
    let _guard = ConnectAttrsGuard::capture();
    // -1 表示不限制大小，便于先测下划线警告逻辑。
    ConnectAttrsSize.Store(-1);

    // `_custom` 不在白名单，应触发弃用警告。
    let payload = build_attrs_payload(&[
        ("_client_name", "libmysql"),
        ("_custom", "val"),
        ("_program_name", "mysql"),
        ("app_name", "myapp"),
    ]);
    let (attrs, warning) = parse_attrs(&payload).expect("parseAttrs should not error");
    assert_eq!(
        b"libmysql",
        attrs.get(b"_client_name".as_slice()).unwrap().as_slice()
    );
    assert_eq!(b"val", attrs.get(b"_custom".as_slice()).unwrap().as_slice());
    assert_eq!(
        b"mysql",
        attrs.get(b"_program_name".as_slice()).unwrap().as_slice()
    );
    assert_eq!(
        b"myapp",
        attrs.get(b"app_name".as_slice()).unwrap().as_slice()
    );
    assert!(warning.contains("custom connection attributes with leading underscore are deprecated and will be rejected in a future release"));

    // 仅白名单下划线属性时不应警告。
    let payload = build_attrs_payload(&[
        ("_client_name", "libmysql"),
        ("_client_version", "8.0.33"),
        ("_os", "linux"),
        ("_pid", "123"),
        ("_platform", "x86_64"),
        ("app_name", "myapp"),
    ]);
    let (_, warning) = parse_attrs(&payload).expect("parseAttrs should not error");
    assert!(warning.is_empty());

    // 限制总字节后，超长属性被截断并计入 Lost。
    ConnectAttrsLost.Store(0);
    ConnectAttrsSize.Store(20);
    let payload =
        build_attrs_payload(&[("_truncated", "client-value"), ("app_name", "my_service")]);
    let (attrs, warning) = parse_attrs(&payload).expect("parseAttrs should not error");
    assert!(warning.contains("session connection attributes truncated"));
    assert_ne!(
        b"client-value",
        attrs.get(b"_truncated".as_slice()).unwrap().as_slice()
    );
    assert_eq!(1, ConnectAttrsLost.Load());
}

/// COM_STMT_FETCH 解析用例：输入字节与期望 stmt_id / fetch_size / 错误。
struct StmtFetchCase {
    arg: Vec<u8>,
    stmt_id: u32,
    fetch_size: u32,
    err: Option<ParseError>,
}

/// 测试结束时恢复 ConnectAttrsSize / ConnectAttrsLost 全局指标。
struct ConnectAttrsGuard {
    size: i64,
    lost: i64,
}

impl ConnectAttrsGuard {
    /// 捕获当前全局连接属性指标快照。
    fn capture() -> Self {
        Self {
            size: ConnectAttrsSize.Load(),
            lost: ConnectAttrsLost.Load(),
        }
    }
}

impl Drop for ConnectAttrsGuard {
    fn drop(&mut self) {
        ConnectAttrsSize.Store(self.size);
        ConnectAttrsLost.Store(self.lost);
    }
}

/// 按 MySQL 连接属性线格式编码：每个键值对为 `len(key)|key|len(value)|value`。
fn build_attrs_payload(kvs: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (key, value) in kvs {
        buf.push(key.len() as u8);
        buf.extend_from_slice(key.as_bytes());
        buf.push(value.len() as u8);
        buf.extend_from_slice(value.as_bytes());
    }
    buf
}
