// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `misc` 模块单元测试。
//
// 覆盖重试、X509 名称序列化、panic 恢复、SQL 语法错误/警告包装、
// 会话进程信息行格式、随机缓冲以及列元数据到 tipb（TiDB 内部 protobuf）的转换。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::anyhow;
use chrono_tz::UTC;
use task_mysql::r#const as mysql;
use task_sessmgr::{
    ProcessInfo, ProcessListValue,
    stmtctx::{NewStmtCtx, ReferenceCount},
};

use crate::misc::{
    ColumnMetadata, ColumnToProto, ColumnsToProto, CommonName, ComposeURL, Country, Email,
    Locality, MockPkixAttribute, Organization, OrganizationalUnit, PkixName, Province,
    RunWithRetry, SqlSyntaxError, SyntaxError, SyntaxWarn, WithRecovery, X509NameOnline,
};

#[test]
/// 验证 `RunWithRetry`：成功重试、耗尽次数失败、以及不可重试立即返回。
fn TestRunWithRetry() {
    {
        let mut cnt = 0;
        let err = RunWithRetry(3, 1, || {
            cnt += 1;
            if cnt < 2 {
                return (true, Some(anyhow!("err")));
            }
            (true, None)
        });
        assert!(err.is_ok());
        assert_eq!(2, cnt);
    }

    {
        let mut cnt = 0;
        let err = RunWithRetry(3, 1, || {
            cnt += 1;
            if cnt < 4 {
                return (true, Some(anyhow!("err")));
            }
            (true, None)
        });
        assert!(err.is_err());
        assert_eq!(3, cnt);
    }

    {
        let mut cnt = 0;
        let err = RunWithRetry(3, 1, || {
            cnt += 1;
            if cnt < 2 {
                return (false, Some(anyhow!("err")));
            }
            (true, None)
        });
        assert!(err.is_err());
        assert_eq!(1, cnt);
    }
}

#[test]
/// 验证 `X509NameOnline` 将 PKIX 属性序列化为 OpenSSL 风格的斜杠分隔字符串。
fn TestX509NameParseMatch() {
    assert_eq!("", X509NameOnline(PkixName::default()));

    let check = PkixName {
        Names: vec![
            MockPkixAttribute(Country, "SE"),
            MockPkixAttribute(Province, "Stockholm2"),
            MockPkixAttribute(Locality, "Stockholm"),
            MockPkixAttribute(Organization, "MySQL demo client certificate"),
            MockPkixAttribute(OrganizationalUnit, "testUnit"),
            MockPkixAttribute(CommonName, "client"),
            MockPkixAttribute(Email, "client@example.com"),
        ],
    };
    let result = "/C=SE/ST=Stockholm2/L=Stockholm/O=MySQL demo client certificate/OU=testUnit/CN=client/emailAddress=client@example.com";
    assert_eq!(result, X509NameOnline(check));
}

#[test]
/// 验证 `WithRecovery` 能捕获 panic 载荷并交给回调。
fn TestBasicFuncWithRecovery() {
    let mut recovery: Option<String> = None;
    WithRecovery(
        || {
            panic!("test");
        },
        Some(|r: Option<&(dyn std::any::Any + Send)>| {
            recovery = r.and_then(|payload| {
                payload
                    .downcast_ref::<&str>()
                    .map(|value| (*value).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
            });
        }),
    );
    assert_eq!(Some("test".to_owned()), recovery);
}

#[test]
/// 验证 `SyntaxError`：None、普通错误包装、以及已是 `SqlSyntaxError` 时保持原消息。
fn TestBasicFuncSyntaxError() {
    assert!(SyntaxError(None).is_none());
    let wrapped = SyntaxError(Some(anyhow!("test"))).unwrap();
    let syntax = wrapped.downcast_ref::<SqlSyntaxError>().unwrap();
    assert!(!syntax.warning);
    assert!(syntax.message.contains("test"));
    assert!(syntax.message.contains(crate::misc::SyntaxErrorPrefix));

    let existing = SyntaxError(Some(anyhow!(SqlSyntaxError {
        message: "keep-me".to_owned(),
        warning: false,
    })))
    .unwrap();
    assert_eq!(
        "keep-me",
        existing.downcast_ref::<SqlSyntaxError>().unwrap().message
    );
}

#[test]
/// 验证 `SyntaxWarn` 将错误包装为带 warning 标记的语法错误。
fn TestBasicFuncSyntaxWarn() {
    assert!(SyntaxWarn(None).is_none());
    let wrapped = SyntaxWarn(Some(anyhow!("test"))).unwrap();
    let syntax = wrapped.downcast_ref::<SqlSyntaxError>().unwrap();
    assert!(syntax.warning);
    assert!(syntax.message.contains("test"));
}

#[test]
/// 验证 `ProcessInfo` 转为 SHOW PROCESSLIST 行及带时区的完整行。
/// ProcessInfo：会话当前执行状态快照，对应 MySQL 进程列表一行。
fn TestBasicFuncProcessInfo() {
    let sc = Arc::from(NewStmtCtx());
    let mem: Arc<task_sessmgr::memory::Tracker> =
        Arc::from(task_sessmgr::memory::NewTracker(-1, -1));
    let pi = ProcessInfo {
        ID: 1,
        User: "test".to_owned(),
        Host: "www".to_owned(),
        DB: "db".to_owned(),
        Command: mysql::ComSleep,
        Plan: None,
        Time: SystemTime::now(),
        State: mysql::ServerStatusInTrans | mysql::ServerStatusAutocommit,
        Info: "test".to_owned(),
        StmtCtx: Some(Arc::clone(&sc)),
        MemTracker: Some(Arc::clone(&mem)),
        RefCountOfStmtCtx: Some(Arc::new(ReferenceCount::default())),
        ..ProcessInfo::default()
    };
    let row = pi.ToRowForShow(false);
    let row2 = pi.ToRowForShow(true);
    assert_eq!(row2, row);
    assert_eq!(8, row.len());
    assert_eq!(ProcessListValue::Unsigned(pi.ID), row[0]);
    assert_eq!(ProcessListValue::Text(pi.User.clone()), row[1]);
    assert_eq!(ProcessListValue::Text(pi.Host.clone()), row[2]);
    assert_eq!(ProcessListValue::Text(pi.DB.clone()), row[3]);
    assert_eq!(ProcessListValue::Text("Sleep".to_owned()), row[4]);
    assert!(matches!(row[5], ProcessListValue::Unsigned(0..=1)));
    assert_eq!(
        ProcessListValue::Text("in transaction; autocommit".to_owned()),
        row[6]
    );
    assert_eq!(ProcessListValue::Text("test".to_owned()), row[7]);

    let row3 = pi.ToRow(UTC);
    assert_eq!(row, row3[0..8]);
    assert_eq!(ProcessListValue::Signed(0), row3[9]);
}

#[test]
/// 验证快速随机缓冲长度与禁用字节约束。
fn TestBasicFuncRandomBuf() {
    let buf = astersql_util_fastrand::Buf(5);
    assert_eq!(5, buf.len());
    assert!(!buf.contains(&b'$'));
    assert!(!buf.contains(&0));
}

#[derive(Clone)]
/// 测试用列元数据桩，实现 `ColumnMetadata` 供 protobuf 转换断言。
struct TestColumn {
    id: i64,
    collation: i32,
    column_len: i32,
    decimal: i32,
}

impl ColumnMetadata for TestColumn {
    fn id(&self) -> i64 {
        self.id
    }
    fn collation_id(&self) -> i32 {
        self.collation
    }
    fn column_len(&self) -> i32 {
        self.column_len
    }
    fn decimal(&self) -> i32 {
        self.decimal
    }
    fn flags(&self) -> i32 {
        0
    }
    fn elements(&self) -> Vec<String> {
        Vec::new()
    }
    fn field_type(&self) -> i32 {
        0
    }
    fn array_element_type(&self) -> i32 {
        0
    }
    fn is_array(&self) -> bool {
        false
    }
    fn is_virtual_generated(&self) -> bool {
        false
    }
    fn is_primary_key(&self) -> bool {
        false
    }
}

#[test]
/// 验证单列与列切片转为 tipb ColumnInfo，collation id 与 Go 夹具一致。
fn TestToPB() {
    // utf8mb4_general_ci collation id is -45 in tipb; utf8mb4_bin shares the same
    // default length/decimal zeros used by Go's NewFieldType(0) fixture.
    let column = TestColumn {
        id: 1,
        collation: -45,
        column_len: -1,
        decimal: -1,
    };
    let column2 = TestColumn {
        id: 1,
        collation: -45,
        column_len: -1,
        decimal: -1,
    };

    let proto = ColumnToProto(&column, false, false);
    assert_eq!(
        "column_id:1 collation:-45 columnLen:-1 decimal:-1 ",
        format!(
            "column_id:{} collation:{} columnLen:{} decimal:{} ",
            proto.ColumnId, proto.Collation, proto.ColumnLen, proto.Decimal
        )
    );
    let protos = ColumnsToProto(&[column, column2], false, false, false);
    assert_eq!(
        "column_id:1 collation:-45 columnLen:-1 decimal:-1 ",
        format!(
            "column_id:{} collation:{} columnLen:{} decimal:{} ",
            protos[0].ColumnId, protos[0].Collation, protos[0].ColumnLen, protos[0].Decimal
        )
    );
}

#[test]
/// 验证 `ComposeURL` 对不同协议前缀与路径拼接规则。
fn TestComposeURL() {
    // TODO Setup config for TLS and verify https protocol output
    assert_eq!(
        ComposeURL("server.example.com", ""),
        "http://server.example.com"
    );
    assert_eq!(
        ComposeURL("httpserver.example.com", ""),
        "http://httpserver.example.com"
    );
    assert_eq!(
        ComposeURL("http://httpserver.example.com", "/"),
        "http://httpserver.example.com/"
    );
    assert_eq!(
        ComposeURL("https://httpserver.example.com", "/api/test"),
        "https://httpserver.example.com/api/test"
    );
    assert_eq!(
        ComposeURL("http://server.example.com", ""),
        "http://server.example.com"
    );
    assert_eq!(
        ComposeURL("https://server.example.com", ""),
        "https://server.example.com"
    );
}

#[test]
/// Go `net.IP.IsGlobalUnicast` excludes IPv4 and IPv6 link-local addresses.
fn TestGlobalUnicastExcludesLinkLocalAddresses() {
    assert!(!crate::misc::is_global_unicast(&IpAddr::V4(Ipv4Addr::new(
        169, 254, 1, 1
    ),)));
    assert!(!crate::misc::is_global_unicast(&IpAddr::V6(
        "fe80::1".parse::<Ipv6Addr>().unwrap(),
    )));
    assert!(crate::misc::is_global_unicast(&IpAddr::V4(Ipv4Addr::new(
        192, 168, 1, 1
    ),)));
}
