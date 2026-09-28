// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// TiDB server 公共协议回归测试（part4）：预处理语句生命周期与连接状态查询。
//
// 覆盖 COM_STMT_PREPARE / SEND_LONG_DATA / EXECUTE / CLOSE 的 MySQL 协议流程，
// ConnectionCount 场景对 `Threads_connected` 的查询，以及 auth_socket OS 用户覆盖可恢复性。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use astersql_server::conn_stmt::{
    BinaryParam, ColumnInfo, Error, HandleStmtPrepare, ResultSet, StatementRuntime, clientConn,
    handleStmtClose, handleStmtExecute, handleStmtSendLongData,
};
use astersql_server_internal_testserverclient::{
    ExecuteResult, QueryResult, Scenario, SqlExecutor, SqlValue, TestServerClient,
};

/// 记录预处理执行参数与关闭语句 id 的内存 Runtime。
#[derive(Default)]
struct PreparedRuntime {
    /// 每次 EXECUTE 收到的二进制参数列表。
    executed: Mutex<Vec<Vec<BinaryParam>>>,
    /// 已关闭的 statement id 序列。
    closed: Mutex<Vec<u32>>,
}

impl StatementRuntime for PreparedRuntime {
    /// 固定准备 `insert into t values (?, ?)`，返回 statement_id=9、参数个数=2。
    fn prepare(&self, sql: &str) -> Result<(u32, usize, Vec<ColumnInfo>), Error> {
        assert_eq!(sql, "insert into t values (?, ?)");
        Ok((9, 2, Vec::new()))
    }

    /// 记录二进制参数；本用例无结果集（返回 None）。
    fn execute(
        &self,
        statement_id: u32,
        args: &[BinaryParam],
    ) -> Result<Option<Box<dyn ResultSet>>, Error> {
        assert_eq!(statement_id, 9);
        self.executed
            .lock()
            .expect("executed args lock poisoned")
            .push(args.to_vec());
        Ok(None)
    }

    /// 记录被关闭的 statement id。
    fn close_statement(&self, statement_id: u32) -> Result<(), Error> {
        self.closed
            .lock()
            .expect("closed statements lock poisoned")
            .push(statement_id);
        Ok(())
    }

    /// 仅 statement_id=9 在缓存中有文本。
    fn statement_cache_text(&self, statement_id: u32) -> Option<String> {
        (statement_id == 9).then(|| "insert into t values (?, ?)".to_owned())
    }

    /// 仅 statement_id=9 视为有效缓存项。
    fn statement_cache_valid(&self, statement_id: u32) -> bool {
        statement_id == 9
    }

    /// 游标 RU（Request Unit）增量回调；本用例无操作。
    fn cursor_ru_delta(&self, _statement_id: u32, _rows: usize) {}

    /// 语句错误后不重试。
    fn retry_after_statement_error(&self, _error: &Error) -> Result<bool, Error> {
        Ok(false)
    }

    /// 不回退到 TiFlash。
    fn should_fallback_tiflash(&self, _error: &Error) -> bool {
        false
    }

    /// TiFlash 开关占位。
    fn set_tiflash_enabled(&self, _enabled: bool) {}

    /// 警告追加占位。
    fn append_statement_warning(&self, _error: Error) {}
}

/// 验证 prepare → send_long_data → execute → close 按 MySQL 二进制协议拼装参数。
#[test]
fn prepared_count_and_send_long_data_follow_mysql_lifecycle() {
    let runtime = Arc::new(PreparedRuntime::default());
    let mut connection = clientConn {
        capability: 0,
        statements: Default::default(),
        output: Vec::new(),
        runtime: runtime.clone(),
    };

    // PREPARE 后连接上应多一条语句登记。
    let baseline = connection.statements.len();
    let global_baseline = astersql_server::conn_stmt::PreparedStmtCount.load(Ordering::Acquire);
    HandleStmtPrepare(&mut connection, "insert into t values (?, ?)").expect("prepare statement");
    assert_eq!(connection.statements.len(), baseline + 1);
    assert_eq!(
        astersql_server::conn_stmt::PreparedStmtCount.load(Ordering::Acquire),
        global_baseline + 1
    );

    // SEND_LONG_DATA：参数 0 写入 BLOB 字节，参数 1 分两包拼出 "hello"。
    handleStmtSendLongData(&mut connection, &[9, 0, 0, 0, 0, 0, 0xff, 0x00])
        .expect("binary long data");
    handleStmtSendLongData(&mut connection, &[9, 0, 0, 0, 1, 0, b'h', b'e'])
        .expect("first string long-data packet");
    handleStmtSendLongData(&mut connection, &[9, 0, 0, 0, 1, 0, b'l', b'l', b'o'])
        .expect("second string long-data packet");

    // EXECUTE 包：statement id、无游标、迭代次数、null bitmap、新类型标记、BLOB+STRING。
    let execute_packet = [
        9, 0, 0, 0, // statement id
        0, // no cursor
        1, 0, 0, 0, // iteration count
        0, // null bitmap
        1, // new parameter types follow
        252, 0, // MYSQL_TYPE_BLOB
        254, 0, // MYSQL_TYPE_STRING
    ];
    handleStmtExecute(&mut connection, &execute_packet).expect("execute long-data statement");

    let executions = runtime
        .executed
        .lock()
        .expect("executed args lock poisoned");
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0][0].tp, 252);
    assert_eq!(executions[0][0].value, [0xff, 0x00]);
    assert_eq!(executions[0][1].tp, 254);
    assert_eq!(executions[0][1].value, b"hello");
    drop(executions);

    // CLOSE 后连接语句表恢复基线，runtime 记录关闭 id=9。
    handleStmtClose(&mut connection, &[9, 0, 0, 0]).expect("close statement");
    assert_eq!(connection.statements.len(), baseline);
    assert_eq!(
        astersql_server::conn_stmt::PreparedStmtCount.load(Ordering::Acquire),
        global_baseline
    );
    assert_eq!(
        *runtime
            .closed
            .lock()
            .expect("closed statements lock poisoned"),
        [9]
    );
}

/// 仅允许查询、拒绝写语句的执行器，用于 ConnectionCount 场景。
#[derive(Default)]
struct ConnectionCountExecutor {
    /// 已发出的查询 SQL 列表。
    queried: Vec<String>,
}

impl SqlExecutor for ConnectionCountExecutor {
    /// ConnectionCount 场景不应执行变更语句。
    fn execute(&mut self, _sql: &str, _parameters: &[SqlValue]) -> Result<ExecuteResult, String> {
        Err("connection-count scenario must not execute a mutation".to_owned())
    }

    /// 记录查询并返回空的 Threads_connected 风格结果集。
    fn query(&mut self, sql: &str, _parameters: &[SqlValue]) -> Result<QueryResult, String> {
        self.queried.push(sql.to_owned());
        Ok(QueryResult {
            columns: vec!["Variable_name".to_owned(), "Value".to_owned()],
            rows: Vec::new(),
        })
    }
}

/// 验证 ConnectionCount 场景只查询 `SHOW STATUS LIKE 'Threads_connected'`。
#[test]
fn connection_count_scenario_queries_threads_connected_status() {
    let client = TestServerClient::new();
    let mut executor = ConnectionCountExecutor::default();
    client
        .run_named_scenario(&mut executor, Scenario::ConnectionCount)
        .expect("connection-count scenario");
    assert_eq!(executor.queried, ["SHOW STATUS LIKE 'Threads_connected'"]);

    let counts = astersql_server::server::ResourceGroupConnectionCount::default();
    for id in 0..100 {
        counts.open(id, "default");
    }
    assert_eq!(counts.count("default"), 100);
    for id in 0..50 {
        counts.close(id);
    }
    assert_eq!(counts.count("default"), 50);
    for id in 50..75 {
        counts.close(id);
    }
    assert_eq!(counts.count("default"), 25);
    for id in 75..100 {
        assert!(counts.move_to(id, "test"));
    }
    assert_eq!(counts.count("default"), 0);
    assert_eq!(counts.count("test"), 25);
    for id in 75..100 {
        counts.close(id);
    }
    counts.close(10_000);
    assert_eq!(counts.count("default"), 0);
    assert_eq!(counts.count("test"), 0);
}

/// 验证 auth_socket 的 OS 用户覆盖可被替换并可清除恢复。
#[test]
fn auth_socket_os_user_override_is_always_recoverable() {
    astersql_server::mock_conn::MockOSUserForAuthSocket("sockuser".to_owned())
        .expect("set auth_socket OS user");
    assert!(!astersql_server::mock_conn::AuthSocketUserMatches("sockuser", "", false).unwrap());
    assert!(astersql_server::mock_conn::AuthSocketUserMatches("sockuser", "", true).unwrap());
    assert!(astersql_server::mock_conn::AuthSocketUserMatches("u2", "sockuser", true).unwrap());
    assert!(!astersql_server::mock_conn::AuthSocketUserMatches("u1", "", true).unwrap());
    astersql_server::mock_conn::MockOSUserForAuthSocket("u2".to_owned())
        .expect("replace auth_socket OS user");
    assert!(astersql_server::mock_conn::AuthSocketUserMatches("u2", "sockuser", true).unwrap());
    astersql_server::mock_conn::ClearOSUserForAuthSocket().expect("clear auth_socket OS user");
}
