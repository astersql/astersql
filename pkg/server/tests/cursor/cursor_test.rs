// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// MySQL 二进制协议游标回归测试。
//
// 通过可控的结果集与语句运行时替身，覆盖惰性/急切游标的分批拉取、同连接交错执行、
// 参数解码、错误清理，以及 COM_STMT_FETCH 包校验与单批行数上限。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use astersql_server::conn_stmt::{
    BinaryParam, ColumnInfo, Error, PreparedStatement, ProtocolEvent, ResultSet, StatementRuntime,
    clientConn, executePreparedStmtAndWriteResult, executeWithCursor, handleStmtExecute,
    handleStmtFetch,
};

// 测试结果集采用单个 i64 列，并保留协议层期望的原始字节表示。
type Row = Vec<Vec<u8>>;

fn row(value: i64) -> Row {
    vec![value.to_le_bytes().to_vec()]
}

fn paired_row(first: i64, second: i64) -> Row {
    vec![first.to_le_bytes().to_vec(), second.to_le_bytes().to_vec()]
}

fn cell_value(cell: &[u8]) -> i64 {
    i64::from_le_bytes(cell.try_into().expect("i64 cell"))
}

fn value(row: &Row) -> i64 {
    cell_value(&row[0])
}

fn column() -> Vec<ColumnInfo> {
    vec![ColumnInfo {
        name: "value".into(),
        column_type: 8,
    }]
}

// 可注入数据或错误的结果集替身，同时暴露关闭状态以验证资源释放。
struct Rows {
    columns: Vec<ColumnInfo>,
    rows: VecDeque<Result<Row, Error>>,
    lazy: bool,
    exhausted: bool,
    closed: Arc<AtomicBool>,
}

impl Rows {
    fn values(values: impl IntoIterator<Item = i64>, lazy: bool) -> (Self, Arc<AtomicBool>) {
        let closed = Arc::new(AtomicBool::new(false));
        (
            Self {
                columns: column(),
                rows: values.into_iter().map(|value| Ok(row(value))).collect(),
                lazy,
                exhausted: false,
                closed: Arc::clone(&closed),
            },
            closed,
        )
    }

    fn error(message: &str, lazy: bool) -> (Self, Arc<AtomicBool>) {
        let closed = Arc::new(AtomicBool::new(false));
        (
            Self {
                columns: column(),
                rows: VecDeque::from([Err(Error::Runtime(message.into()))]),
                lazy,
                exhausted: false,
                closed: Arc::clone(&closed),
            },
            closed,
        )
    }

    fn paired_values(
        values: impl IntoIterator<Item = (i64, i64)>,
        lazy: bool,
    ) -> (Self, Arc<AtomicBool>) {
        let closed = Arc::new(AtomicBool::new(false));
        (
            Self {
                columns: vec![
                    ColumnInfo {
                        name: "id".into(),
                        column_type: 8,
                    },
                    ColumnInfo {
                        name: "v".into(),
                        column_type: 8,
                    },
                ],
                rows: values
                    .into_iter()
                    .map(|(first, second)| Ok(paired_row(first, second)))
                    .collect(),
                lazy,
                exhausted: false,
                closed: Arc::clone(&closed),
            },
            closed,
        )
    }
}

impl ResultSet for Rows {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    fn next(&mut self) -> Result<Option<Row>, Error> {
        match self.rows.pop_front() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(error)) => Err(error),
            None => {
                self.exhausted = true;
                Ok(None)
            }
        }
    }

    fn close(&mut self) -> Result<(), Error> {
        self.closed.store(true, Ordering::SeqCst);
        self.exhausted = true;
        Ok(())
    }

    fn exhausted(&self) -> bool {
        self.exhausted || self.rows.is_empty()
    }

    fn supports_lazy_cursor(&self) -> bool {
        // false 会触发执行阶段物化，true 则保留源结果集供后续 FETCH 消费。
        self.lazy
    }
}

#[derive(Default)]
// 按语句编号排队返回结果集，并记录参数执行与游标 RU 计量信息。
struct Runtime {
    results: Mutex<HashMap<u32, VecDeque<Box<dyn ResultSet>>>>,
    fetched: Mutex<Vec<(u32, usize)>>,
    executed: Mutex<Vec<(u32, Vec<BinaryParam>)>>,
}

impl Runtime {
    fn push(&self, statement_id: u32, result: impl ResultSet + 'static) {
        self.results
            .lock()
            .expect("result queue")
            .entry(statement_id)
            .or_default()
            .push_back(Box::new(result));
    }
}

impl StatementRuntime for Runtime {
    fn prepare(&self, _: &str) -> Result<(u32, usize, Vec<ColumnInfo>), Error> {
        unreachable!("these tests install prepared statements explicitly")
    }

    fn execute(
        &self,
        statement_id: u32,
        arguments: &[BinaryParam],
    ) -> Result<Option<Box<dyn ResultSet>>, Error> {
        self.executed
            .lock()
            .expect("executed statements")
            .push((statement_id, arguments.to_vec()));
        self.results
            .lock()
            .expect("result queue")
            .get_mut(&statement_id)
            .and_then(VecDeque::pop_front)
            .map(Some)
            .ok_or_else(|| Error::Runtime(format!("no result for statement {statement_id}")))
    }

    fn close_statement(&self, _: u32) -> Result<(), Error> {
        Ok(())
    }

    fn statement_cache_text(&self, _: u32) -> Option<String> {
        None
    }

    fn statement_cache_valid(&self, _: u32) -> bool {
        true
    }

    fn cursor_ru_delta(&self, statement_id: u32, rows: usize) {
        self.fetched
            .lock()
            .expect("RU samples")
            .push((statement_id, rows));
    }

    fn retry_after_statement_error(&self, _: &Error) -> Result<bool, Error> {
        Ok(false)
    }

    fn should_fallback_tiflash(&self, _: &Error) -> bool {
        false
    }

    fn set_tiflash_enabled(&self, _: bool) {}

    fn append_statement_warning(&self, _: Error) {}
}

fn statement(id: u32, num_params: usize) -> PreparedStatement {
    PreparedStatement {
        id,
        sql: "select value".into(),
        num_params,
        columns: column(),
        bound_params: vec![None; num_params],
        bound_params_too_large: false,
        max_allowed_packet: 64 << 20,
        params_type: Vec::new(),
        last_params: Vec::new(),
        cursor: None,
        cursor_active: false,
        protocol_cursor: None,
    }
}

fn connection(runtime: Arc<Runtime>, ids: impl IntoIterator<Item = u32>) -> clientConn {
    clientConn {
        capability: 0,
        statements: ids.into_iter().map(|id| (id, statement(id, 0))).collect(),
        output: Vec::new(),
        runtime,
    }
}

fn fetch_packet(statement_id: u32, fetch_size: u32) -> Vec<u8> {
    // COM_STMT_FETCH 负载固定为两个小端 u32：语句编号与期望拉取行数。
    let mut packet = statement_id.to_le_bytes().to_vec();
    packet.extend_from_slice(&fetch_size.to_le_bytes());
    packet
}

fn fetched_values(event: &ProtocolEvent) -> Vec<i64> {
    let ProtocolEvent::Result { rows, .. } = event else {
        panic!("expected result event: {event:?}");
    };
    rows.iter().map(value).collect()
}

fn fetched_row_values(event: &ProtocolEvent) -> Vec<Vec<i64>> {
    let ProtocolEvent::Result { rows, .. } = event else {
        panic!("expected result event: {event:?}");
    };
    rows.iter()
        .map(|row| row.iter().map(|cell| cell_value(cell)).collect())
        .collect()
}

#[test]
fn cursor_fetch_error_in_fetch_resets_cursor_and_releases_resources() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(Arc::clone(&runtime), [1]);
    let (rows, closed) = Rows::error("fail to get chunk for test", true);
    executeWithCursor(&mut connection, 1, Box::new(rows)).expect("open lazy cursor");

    assert_eq!(
        handleStmtFetch(&mut connection, &fetch_packet(1, 1024)),
        Err(Error::Runtime("fail to get chunk for test".into()))
    );
    let statement = &connection.statements[&1];
    assert!(
        !statement.cursor_active,
        "failed FETCH must reset cursor state"
    );
    assert!(
        statement.cursor.is_none(),
        "failed FETCH must release result set"
    );
    assert!(
        closed.load(Ordering::SeqCst),
        "failed FETCH must close resources"
    );
}

#[test]
fn cursor_fetch_should_buffer_eager_results_and_close_the_source() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [1]);
    let (rows, source_closed) = Rows::paired_values([(1, 1), (1, 2)], false);
    executeWithCursor(&mut connection, 1, Box::new(rows)).expect("materialize eager cursor");
    assert!(source_closed.load(Ordering::SeqCst));

    handleStmtFetch(&mut connection, &fetch_packet(1, 2)).expect("fetch buffered rows");
    assert_eq!(
        fetched_row_values(connection.output.last().unwrap()),
        [vec![1, 1], vec![1, 2]]
    );
    assert!(!connection.statements[&1].cursor_active);
}

#[test]
fn cursor_fetch_execute_rejects_unknown_statement_and_unsupported_flags() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [7]);
    let mut packet = vec![0; 9];
    packet[..4].copy_from_slice(&8_u32.to_le_bytes());
    packet[4] = 1;
    assert_eq!(
        handleStmtExecute(&mut connection, &packet),
        Err(Error::StatementNotFound(8))
    );

    packet[..4].copy_from_slice(&7_u32.to_le_bytes());
    for flag in [1 | 2, 1 | 4] {
        packet[4] = flag;
        assert_eq!(
            handleStmtExecute(&mut connection, &packet),
            Err(Error::MalformedPacket)
        );
    }
}

fn interleaved_execute_and_fetch(lazy: bool) {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(Arc::clone(&runtime), [1, 2]);
    let (cursor_rows, _) = Rows::paired_values((0..1000).map(|value| (value, value)), lazy);
    executeWithCursor(&mut connection, 1, Box::new(cursor_rows)).expect("open cursor");

    let mut all = Vec::new();
    for _ in 0..100 {
        // 每拉取一批外层游标，就在同一连接上执行一次普通查询，验证输出与游标互不污染。
        handleStmtFetch(&mut connection, &fetch_packet(1, 10)).expect("cursor fetch");
        all.extend(fetched_row_values(connection.output.last().unwrap()));

        let (one_row, _) = Rows::paired_values([(0, 0)], false);
        executePreparedStmtAndWriteResult(&mut connection, 2, Some(Box::new(one_row)), false)
            .expect("interleaved query");
        assert_eq!(
            fetched_row_values(connection.output.last().unwrap()),
            [vec![0, 0]]
        );
    }
    assert_eq!(
        all,
        (0..1000)
            .map(|value| vec![value, value])
            .collect::<Vec<_>>()
    );
}

#[test]
fn concurrent_execute_and_fetch_preserves_eager_and_lazy_cursor_rows() {
    interleaved_execute_and_fetch(false);
    interleaved_execute_and_fetch(true);
}

#[test]
fn serial_lazy_execute_and_fetch_returns_every_row_in_order() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [1]);
    let (rows, _) = Rows::values(0..1000, true);
    executeWithCursor(&mut connection, 1, Box::new(rows)).unwrap();

    let mut actual = Vec::new();
    while connection.statements[&1].cursor_active {
        handleStmtFetch(&mut connection, &fetch_packet(1, 10)).unwrap();
        actual.extend(fetched_values(connection.output.last().unwrap()));
    }
    assert_eq!(actual, (0..1000).collect::<Vec<_>>());
}

#[test]
fn lazy_execute_projection_returns_computed_values() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [1]);
    let (rows, _) = Rows::values((0..1000).map(|value| value * 2), true);
    executeWithCursor(&mut connection, 1, Box::new(rows)).unwrap();
    handleStmtFetch(&mut connection, &fetch_packet(1, 1000)).unwrap();
    assert_eq!(
        fetched_values(connection.output.last().unwrap()),
        (0..1000).map(|v| v * 2).collect::<Vec<_>>()
    );
}

#[test]
fn lazy_execute_selection_returns_only_matching_rows() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [1]);
    let (rows, _) = Rows::values(500..1000, true);
    executeWithCursor(&mut connection, 1, Box::new(rows)).unwrap();
    handleStmtFetch(&mut connection, &fetch_packet(1, 1000)).unwrap();
    assert_eq!(
        fetched_values(connection.output.last().unwrap()),
        (500..1000).collect::<Vec<_>>()
    );
}

#[test]
fn lazy_execute_with_param_keeps_outer_cursor_during_nested_query() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(Arc::clone(&runtime), [1, 2]);
    connection.statements.insert(2, statement(2, 1));
    let (outer, _) = Rows::values(500..1000, true);
    executeWithCursor(&mut connection, 1, Box::new(outer)).unwrap();

    let mut actual = Vec::new();
    while connection.statements[&1].cursor_active {
        handleStmtFetch(&mut connection, &fetch_packet(1, 50)).unwrap();
        actual.extend(fetched_values(connection.output.last().unwrap()));
        if connection.statements[&1].cursor_active {
            // 外层游标尚未耗尽时，构造一个带参数的内层只读游标执行包。
            let (nested, _) = Rows::values([500], true);
            runtime.push(2, nested);
            let mut execute = 2_u32.to_le_bytes().to_vec();
            execute.push(1); // 只读游标标记 CURSOR_TYPE_READ_ONLY。
            execute.extend_from_slice(&1_u32.to_le_bytes());
            execute.push(0); // 参数非 NULL。
            execute.push(1); // 本次包携带新的参数类型。
            execute.extend_from_slice(&[8, 0]); // 有符号 MYSQL_TYPE_LONGLONG。
            execute.extend_from_slice(&500_i64.to_le_bytes());
            handleStmtExecute(&mut connection, &execute).unwrap();
            handleStmtFetch(&mut connection, &fetch_packet(2, 1)).unwrap();
            assert_eq!(fetched_values(connection.output.last().unwrap()), [500]);
        }
    }
    assert_eq!(actual, (500..1000).collect::<Vec<_>>());
    let executed = runtime.executed.lock().expect("executed statements");
    assert!(!executed.is_empty());
    assert!(executed.iter().all(|(statement_id, arguments)| {
        *statement_id == 2
            && arguments.len() == 1
            && arguments[0].tp == 8
            && arguments[0].value == 500_i64.to_le_bytes()
    }));
}

#[test]
fn cursor_exceed_quota_closes_materialization_source() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [1]);
    // 急切游标在执行阶段物化；即使临时空间超限，也必须关闭源结果集。
    let (rows, closed) = Rows::error("Out Of Quota For Local Temporary Space!", false);
    assert_eq!(
        executeWithCursor(&mut connection, 1, Box::new(rows)),
        Err(Error::Runtime(
            "Out Of Quota For Local Temporary Space!".into()
        ))
    );
    assert!(
        closed.load(Ordering::SeqCst),
        "quota failure must clean temporary resources"
    );
    assert!(!connection.statements[&1].cursor_active);
}

#[test]
fn cursor_fetch_packet_is_exactly_eight_bytes_and_caps_rows_at_1024() {
    let runtime = Arc::new(Runtime::default());
    let mut connection = connection(runtime, [1]);
    let (rows, _) = Rows::values(0..2000, true);
    executeWithCursor(&mut connection, 1, Box::new(rows)).unwrap();

    // 多余字节应被视为畸形包，而超过服务端上限的请求仅返回 1024 行并保持游标活跃。
    let mut oversized_packet = fetch_packet(1, 1);
    oversized_packet.push(0);
    assert_eq!(
        handleStmtFetch(&mut connection, &oversized_packet),
        Err(Error::MalformedPacket)
    );

    handleStmtFetch(&mut connection, &fetch_packet(1, 1025)).unwrap();
    assert_eq!(
        fetched_values(connection.output.last().unwrap()).len(),
        1024
    );
    assert!(connection.statements[&1].cursor_active);
}
