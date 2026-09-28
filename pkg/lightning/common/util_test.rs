// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `util` 模块单元测试。
//
// 覆盖目录判断、MySQL 连接（含 base64 密码重试）、SQL 重试、标识符/字符串插值，
// 以及 AUTO_RANDOM 列定位、ADD INDEX SQL 生成与 SkipReadRowCount 判定。

use crate::{
    AUTO_INCREMENT_FLAG, BuildAddIndexSQL, CIStr, ColumnInfo, CommonError, ConnectMySQL, Context,
    DBExecutor, GetAutoRandomColumn, IndexColumn, IndexInfo, InterpolateMySQLString,
    IsContextCanceledError, IsDirExists, MySQLConfig, MySQLConnectParam, MySQLConnector,
    PRI_KEY_FLAG, QueryRows, SQLValue, SQLWithRetry, SchemaState, SchemaTableInfo,
    SkipReadRowCount, Transaction, UniqueTable, UnspecifiedLength,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 空实现事务，Commit/Rollback 恒成功。
struct MockTx;
impl Transaction for MockTx {
    fn Commit(&mut self) -> Result<(), CommonError> {
        Ok(())
    }
    fn Rollback(&mut self) -> Result<(), CommonError> {
        Ok(())
    }
}

/// 队列驱动的 Mock DB：按调用顺序弹出预设的 QueryRow/Exec 结果。
struct MockDB {
    query_row: Mutex<VecDeque<Result<Vec<String>, CommonError>>>,
    exec: Mutex<VecDeque<Result<u64, CommonError>>>,
}

impl MockDB {
    /// 构造空队列的 MockDB。
    fn new() -> Arc<Self> {
        Arc::new(Self {
            query_row: Mutex::new(VecDeque::new()),
            exec: Mutex::new(VecDeque::new()),
        })
    }
}

impl DBExecutor for MockDB {
    fn QueryRowContext(
        &self,
        _ctx: &Context,
        _query: &str,
        _args: &[SQLValue],
    ) -> Result<Vec<String>, CommonError> {
        self.query_row
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(CommonError::new("sql", "unexpected QueryRow")))
    }

    fn QueryContext(
        &self,
        ctx: &Context,
        query: &str,
        args: &[SQLValue],
    ) -> Result<QueryRows, CommonError> {
        let row = self.QueryRowContext(ctx, query, args)?;
        Ok(QueryRows {
            Columns: vec!["a".to_owned()],
            Rows: vec![row],
            IterationError: None,
        })
    }

    fn BeginTx(&self, _ctx: &Context) -> Result<Box<dyn Transaction>, CommonError> {
        Ok(Box::new(MockTx))
    }

    fn ExecContext(
        &self,
        _ctx: &Context,
        _query: &str,
        _args: &[SQLValue],
    ) -> Result<u64, CommonError> {
        self.exec
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(CommonError::new("sql", "unexpected Exec")))
    }
}

#[test]
/// 验证 IsDirExists 对当前目录与不存在路径的判断。
fn test_dir_not_exist() {
    assert!(IsDirExists("."));
    assert!(!IsDirExists("not-exists"));
}

#[test]
/// 验证明文密码连接，以及 base64 密码在 1045 后解码重试。
fn test_connect() {
    let plain_psw = "dQAUoDiyb1ucWZk7";
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let connector: MySQLConnector = Arc::new(move |config: &MySQLConfig| {
        seen_clone.lock().unwrap().push(config.Passwd.clone());
        if config.Passwd == plain_psw {
            Ok(MockDB::new() as Arc<dyn DBExecutor>)
        } else {
            let mut err = CommonError::new("mysql", "access denied");
            err.Code = Some(1045);
            Err(err)
        }
    });

    let mut param = MySQLConnectParam {
        Host: "127.0.0.1".to_owned(),
        Port: 4000,
        User: "root".to_owned(),
        Password: plain_psw.to_owned(),
        SQLMode: "strict".to_owned(),
        MaxAllowedPacket: 1234,
        Connector: Some(Arc::clone(&connector)),
        ..Default::default()
    };
    param.Connect().unwrap();

    // 手工编码 base64，模拟配置中存放编码密码的场景
    // base64 of plain password
    let encoded = {
        // std without base64 crate: reuse ConnectMySQL's decode by encoding manually.
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = plain_psw.as_bytes();
        let mut out = Vec::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((n >> 18) & 63) as usize]);
            out.push(TABLE[((n >> 12) & 63) as usize]);
            if chunk.len() > 1 {
                out.push(TABLE[((n >> 6) & 63) as usize]);
            } else {
                out.push(b'=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(n & 63) as usize]);
            } else {
                out.push(b'=');
            }
        }
        String::from_utf8(out).unwrap()
    };
    param.Password = encoded;
    param.Connect().unwrap();
    let passwords = seen.lock().unwrap().clone();
    assert!(passwords.iter().any(|p| p == plain_psw));
}

#[test]
fn connect_mysql_rejects_data_after_base64_padding() {
    let attempts = Arc::new(Mutex::new(0));
    let attempts_clone = Arc::clone(&attempts);
    let connector: MySQLConnector = Arc::new(move |_config: &MySQLConfig| {
        *attempts_clone.lock().unwrap() += 1;
        let mut error = CommonError::new("mysql", "access denied");
        error.Code = Some(1045);
        Err(error)
    });
    let invalid_password = "dQ==AAAA";
    let mut config = MySQLConfig {
        Passwd: invalid_password.to_owned(),
        Connector: Some(connector),
        ..Default::default()
    };

    assert!(
        ConnectMySQL(&mut config).is_err(),
        "malformed base64 must not trigger a retry"
    );

    assert_eq!(*attempts.lock().unwrap(), 1);
    assert_eq!(config.Passwd, invalid_password);
}

#[test]
/// 验证上下文取消错误识别。
fn test_is_context_canceled_error() {
    assert!(IsContextCanceledError(Some(&CommonError::new(
        "cancelled",
        "context canceled"
    ))));
    assert!(!IsContextCanceledError(Some(&CommonError::new(
        "eof", "EOF"
    ))));
}

#[test]
/// 验证限定表名转义（含反引号加倍）。
fn test_unique_table() {
    assert_eq!(UniqueTable("test", "t1"), "`test`.`t1`");
    assert_eq!(UniqueTable("test", "t`1"), "`test`.`t``1`");
}

#[test]
/// 验证 SQLWithRetry：可重试失败耗尽、取消立即失败、最终成功与 Exec 路径。
fn test_sql_with_retry() {
    let db = MockDB::new();
    {
        let mut q = db.query_row.lock().unwrap();
        for _ in 0..3 {
            q.push_back(Err(
                CommonError::new("invalid-connection", "mock error").annotate("mock error")
            ));
        }
    }
    let sql_with_retry = SQLWithRetry {
        DB: db.clone() as Arc<dyn DBExecutor>,
        HideQueryLog: false,
    };
    let err = sql_with_retry
        .QueryRow(&Context::Background(), "", "select a from test.t1", &[])
        .expect_err("should fail after retries");
    assert!(err.to_string().contains("mock error"), "{err}");

    {
        let mut q = db.query_row.lock().unwrap();
        q.push_back(Err(CommonError::new("cancelled", "context canceled")));
    }
    let err = sql_with_retry
        .QueryRow(&Context::Background(), "", "select a from test.t1", &[])
        .expect_err("canceled");
    assert!(err.to_string().contains("canceled"), "{err}");

    {
        let mut q = db.query_row.lock().unwrap();
        q.push_back(Ok(vec!["1".to_owned()]));
    }
    let row = sql_with_retry
        .QueryRow(&Context::Background(), "", "select a from test.t1", &[])
        .unwrap();
    assert_eq!(row, vec!["1".to_owned()]);

    {
        let mut e = db.exec.lock().unwrap();
        e.push_back(Err(CommonError::new("cancelled", "context canceled")));
        e.push_back(Ok(1));
    }
    let err = sql_with_retry
        .Exec(
            &Context::Background(),
            "",
            "delete from test.t1 where id = ?",
            &[SQLValue::Integer(2)],
        )
        .expect_err("canceled exec");
    assert!(err.to_string().contains("canceled"), "{err}");
    sql_with_retry
        .Exec(
            &Context::Background(),
            "",
            "delete from test.t1 where id = ?",
            &[SQLValue::Integer(2)],
        )
        .unwrap();
}

#[test]
/// 验证字符串字面量插值与单引号转义。
fn test_interpolate_mysql_string() {
    assert_eq!(InterpolateMySQLString("123"), "'123'");
    assert_eq!(InterpolateMySQLString("1'23"), "'1''23'");
    assert_eq!(InterpolateMySQLString("1'2''3"), "'1''2''''3'");
}

/// 构造测试用 ColumnInfo。
fn col(name: &str, flag: u64) -> ColumnInfo {
    ColumnInfo {
        Name: CIStr::new(name),
        Hidden: false,
        GeneratedExprString: String::new(),
        Flag: flag,
    }
}

/// 构造测试用 IndexInfo。
fn idx(
    name: &str,
    columns: &[(&str, usize, i32)],
    primary: bool,
    unique: bool,
    invisible: bool,
    comment: &str,
) -> IndexInfo {
    IndexInfo {
        Name: CIStr::new(name),
        Columns: columns
            .iter()
            .map(|(n, offset, length)| IndexColumn {
                Name: CIStr::new(*n),
                Offset: *offset,
                Length: *length,
            })
            .collect(),
        State: SchemaState::Public,
        Primary: primary,
        Unique: unique,
        Invisible: invisible,
        Comment: comment.to_owned(),
    }
}

#[test]
/// 覆盖无自随机、整型主键与 common handle 下定位 AUTO_RANDOM 列。
fn test_get_auto_random_column() {
    let tests = [
        (
            SchemaTableInfo {
                Columns: vec![col("c", 0)],
                ..Default::default()
            },
            None,
        ),
        (
            SchemaTableInfo {
                HasAutoIncrement: true,
                Columns: vec![col("c", AUTO_INCREMENT_FLAG)],
                ..Default::default()
            },
            None,
        ),
        (
            SchemaTableInfo {
                ContainsAutoRandomBits: true,
                PKIsHandle: true,
                Columns: vec![col("c", PRI_KEY_FLAG)],
                ..Default::default()
            },
            Some("c"),
        ),
        (
            SchemaTableInfo {
                ContainsAutoRandomBits: true,
                PKIsHandle: true,
                Columns: vec![col("a", 0), col("c", PRI_KEY_FLAG)],
                ..Default::default()
            },
            Some("c"),
        ),
        (
            SchemaTableInfo {
                ContainsAutoRandomBits: true,
                IsCommonHandle: true,
                Columns: vec![col("c", 0), col("a", 0)],
                Indices: vec![idx(
                    "primary",
                    &[("c", 0, UnspecifiedLength)],
                    true,
                    true,
                    false,
                    "",
                )],
                ..Default::default()
            },
            Some("c"),
        ),
        (
            SchemaTableInfo {
                ContainsAutoRandomBits: true,
                IsCommonHandle: true,
                Columns: vec![col("a", 0), col("c", 0)],
                Indices: vec![idx(
                    "primary",
                    &[("c", 1, UnspecifiedLength)],
                    true,
                    true,
                    false,
                    "",
                )],
                ..Default::default()
            },
            Some("c"),
        ),
    ];
    for (info, expect) in tests {
        let got = GetAutoRandomColumn(&info).map(|c| c.Name.L.as_str());
        assert_eq!(got, expect);
    }
}

#[test]
/// 验证仅补主键，以及多索引（唯一/不可见/表达式/前缀）的 ADD SQL。
fn test_build_add_index_sql() {
    let current = SchemaTableInfo {
        Columns: vec![col("pk", 0), col("id", AUTO_INCREMENT_FLAG)],
        Indices: vec![idx(
            "uniq_id",
            &[("id", 1, UnspecifiedLength)],
            false,
            true,
            false,
            "",
        )],
        ..Default::default()
    };
    let desired = SchemaTableInfo {
        Columns: vec![col("pk", PRI_KEY_FLAG), col("id", AUTO_INCREMENT_FLAG)],
        Indices: vec![
            idx(
                "primary",
                &[("pk", 0, UnspecifiedLength)],
                true,
                true,
                false,
                "",
            ),
            idx(
                "uniq_id",
                &[("id", 1, UnspecifiedLength)],
                false,
                true,
                false,
                "",
            ),
        ],
        ..Default::default()
    };
    let (single, multi) = BuildAddIndexSQL("`test`.`non_pk_auto_inc`", &current, &desired);
    assert_eq!(
        single,
        "ALTER TABLE `test`.`non_pk_auto_inc` ADD PRIMARY KEY (`pk`)"
    );
    assert_eq!(
        multi,
        vec!["ALTER TABLE `test`.`non_pk_auto_inc` ADD PRIMARY KEY (`pk`)".to_owned()]
    );

    let current = SchemaTableInfo {
        Columns: (1..=11)
            .map(|i| col(&format!("c{i}"), if i == 1 { PRI_KEY_FLAG } else { 0 }))
            .collect(),
        Indices: vec![idx(
            "primary",
            &[("c1", 0, UnspecifiedLength)],
            true,
            true,
            false,
            "",
        )],
        HasClusteredIndex: true,
        PKIsHandle: true,
        ..Default::default()
    };
    let mut desired = current.clone();
    desired.Indices.extend([
        idx(
            "idx_c2",
            &[("c2", 1, UnspecifiedLength)],
            false,
            false,
            false,
            "single column index",
        ),
        idx(
            "idx_c2_c3",
            &[("c2", 1, UnspecifiedLength), ("c3", 2, UnspecifiedLength)],
            false,
            false,
            false,
            "multiple column index",
        ),
        idx(
            "uniq_c4",
            &[("c4", 3, UnspecifiedLength)],
            false,
            true,
            false,
            "single column unique key",
        ),
        idx(
            "uniq_c4_c5",
            &[("c4", 3, UnspecifiedLength), ("c5", 4, UnspecifiedLength)],
            false,
            true,
            false,
            "multiple column unique key",
        ),
        idx(
            "idx_c6",
            &[("c6", 5, UnspecifiedLength)],
            false,
            false,
            false,
            "single column index with asc order",
        ),
        idx(
            "idx_c7",
            &[("c7", 6, UnspecifiedLength)],
            false,
            false,
            false,
            "single column index with desc order",
        ),
        idx(
            "idx_c6_c7",
            &[("c6", 5, UnspecifiedLength), ("c7", 6, UnspecifiedLength)],
            false,
            false,
            false,
            "multiple column index with asc and desc order",
        ),
        idx(
            "idx_c8",
            &[("c8", 7, UnspecifiedLength)],
            false,
            false,
            false,
            "single column index with visible",
        ),
        idx(
            "idx_c9",
            &[("c9", 8, UnspecifiedLength)],
            false,
            false,
            true,
            "single column index with invisible",
        ),
        {
            let index = idx(
                "idx_lower_c10",
                &[("c10", 9, UnspecifiedLength)],
                false,
                false,
                false,
                "single column index with function",
            );
            // Hidden generated column for expression index.
            desired.Columns[9].Hidden = true;
            desired.Columns[9].GeneratedExprString = "lower(`c10`)".to_owned();
            index
        },
        idx(
            "idx_prefix_c11",
            &[("c11", 10, 3)],
            false,
            false,
            false,
            "single column index with prefix",
        ),
        idx(
            "c2",
            &[("c2", 1, UnspecifiedLength)],
            false,
            true,
            false,
            "",
        ),
    ]);
    let (single, multi) = BuildAddIndexSQL("`test`.`multi_indexes`", &current, &desired);
    assert!(single.starts_with("ALTER TABLE `test`.`multi_indexes` ADD KEY `idx_c2`(`c2`)"));
    assert!(single.contains("ADD UNIQUE KEY `c2`(`c2`)"));
    assert_eq!(multi.len(), 12);
    assert_eq!(
        multi[0],
        "ALTER TABLE `test`.`multi_indexes` ADD KEY `idx_c2`(`c2`) COMMENT 'single column index'"
    );
    assert!(
        multi
            .iter()
            .any(|sql| sql.contains("idx_lower_c10") && sql.contains("lower(`c10`)"))
    );
}

#[test]
fn build_add_index_sql_escapes_comment_like_go_output_format() {
    let current = SchemaTableInfo {
        Columns: vec![col("c", 0)],
        ..Default::default()
    };
    let desired = SchemaTableInfo {
        Columns: vec![col("c", 0)],
        Indices: vec![idx(
            "idx",
            &[("c", 0, UnspecifiedLength)],
            false,
            false,
            false,
            "a\\b\nc\rd\0e'f",
        )],
        ..Default::default()
    };

    let (single, multi) = BuildAddIndexSQL("`test`.`t`", &current, &desired);
    let expected = "ALTER TABLE `test`.`t` ADD KEY `idx`(`c`) COMMENT 'a\\\\b\\nc\\rd\\0e''f'";
    assert_eq!(single, expected);
    assert_eq!(multi, vec![expected.to_owned()]);
}

#[test]
/// 验证在不同主键/自增/自随机形态下是否可跳过行数读取。
fn test_skip_read_row_count() {
    let cases = [
        (
            SchemaTableInfo {
                PKIsHandle: true,
                Columns: vec![
                    col("id", PRI_KEY_FLAG),
                    col("k", 0),
                    col("c", 0),
                    col("pad", 0),
                ],
                Indices: vec![idx(
                    "primary",
                    &[("id", 0, UnspecifiedLength)],
                    true,
                    true,
                    false,
                    "",
                )],
                ..Default::default()
            },
            true,
        ),
        (
            SchemaTableInfo {
                Columns: vec![col("id", 0), col("k", 0), col("c", 0), col("pad", 0)],
                ..Default::default()
            },
            false,
        ),
        (
            SchemaTableInfo {
                PKIsHandle: true,
                HasAutoIncrement: true,
                Columns: vec![col("id", PRI_KEY_FLAG | AUTO_INCREMENT_FLAG), col("k", 0)],
                Indices: vec![idx(
                    "primary",
                    &[("id", 0, UnspecifiedLength)],
                    true,
                    true,
                    false,
                    "",
                )],
                ..Default::default()
            },
            false,
        ),
        (
            SchemaTableInfo {
                PKIsHandle: true,
                ContainsAutoRandomBits: true,
                Columns: vec![col("id", PRI_KEY_FLAG), col("k", 0)],
                ..Default::default()
            },
            false,
        ),
        (
            SchemaTableInfo {
                PKIsHandle: true,
                Columns: vec![col("id", PRI_KEY_FLAG), col("k", AUTO_INCREMENT_FLAG)],
                Indices: vec![idx(
                    "primary",
                    &[("id", 0, UnspecifiedLength)],
                    true,
                    true,
                    false,
                    "",
                )],
                ..Default::default()
            },
            true,
        ),
        (
            SchemaTableInfo {
                IsCommonHandle: false,
                PKIsHandle: false,
                Columns: vec![col("id", 0), col("k", 0)],
                ..Default::default()
            },
            false,
        ),
    ];
    for (info, expected) in cases {
        assert_eq!(expected, SkipReadRowCount(Some(&info)));
    }
}
