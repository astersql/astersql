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

// 写路径测试，对应 Go `pkg/executor/test/writetest/write_test.go`。
//
// 这些用例使用真实的 Rust session/executor 与 mock store，保留 Go testkit
// 的 SQL 顺序、错误路径和结果断言；不以记录 SQL 或恒真断言替代行为测试。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use astersql_executor::load_data::{
    ColumnInfo as LoadColumnInfo, DataParser, Datum as LoadDatum, FieldMapping, LoadDataBackend,
    LoadDataController, LoadDataError, LoadDataReaderInfo, NewLoadDataWorker,
    OnDuplicateKeyHandling, planInfo,
};
use astersql_session::runtime::{AddRecordWithoutAutoIDRebaseForTest, CreateDanglingIndexForTest};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows, RowsWithSep};

fn write_testkit() -> astersql_testkit::TestKit {
    let (store, domain) = CreateMockStoreAndDomain();
    NewTestKit(store)
}

/// Minimal real LOAD DATA backend used to exercise the production worker's
/// field mapping, warning accounting, and commit lifecycle without inventing
/// a SQL-side reader injection API.
#[derive(Clone, Default)]
struct LoadDataCaptureBackend {
    rows: Arc<Mutex<Vec<Vec<LoadDatum>>>>,
}

struct LoadDataRowsParser {
    rows: std::vec::IntoIter<Vec<LoadDatum>>,
}

impl DataParser for LoadDataRowsParser {
    fn read_row(&mut self) -> Result<Vec<LoadDatum>, LoadDataError> {
        self.rows.next().ok_or(LoadDataError::EndOfFile)
    }

    fn recycle_row(&mut self, _row: Vec<LoadDatum>) {}

    fn close(&mut self) -> Result<(), LoadDataError> {
        Ok(())
    }
}

impl LoadDataBackend for LoadDataCaptureBackend {
    fn init_data_files(&self, _controller: &LoadDataController) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn remote_reader_infos(
        &self,
        _controller: &LoadDataController,
    ) -> Result<Vec<LoadDataReaderInfo>, LoadDataError> {
        Ok(Vec::new())
    }

    fn local_reader_info(
        &self,
        path: &str,
        _reader: Box<dyn astersql_executor::load_data::LoadDataReader>,
    ) -> Result<LoadDataReaderInfo, LoadDataError> {
        Ok(LoadDataReaderInfo {
            path: path.to_owned(),
            offset: 0,
            length: None,
        })
    }

    fn open_parser(
        &self,
        _reader: &LoadDataReaderInfo,
    ) -> Result<Box<dyn DataParser>, LoadDataError> {
        Err(LoadDataError::Backend(
            "parser is supplied by the test".into(),
        ))
    }

    fn close_controller(&self, _controller: &LoadDataController) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn evaluate_assignment(
        &self,
        _assignment: usize,
        variables: &BTreeMap<String, LoadDatum>,
    ) -> Result<LoadDatum, LoadDataError> {
        Ok(variables
            .values()
            .next()
            .cloned()
            .unwrap_or(LoadDatum::Null))
    }

    fn normalize_row(
        &self,
        _row_number: u64,
        row: Vec<LoadDatum>,
    ) -> Result<Vec<LoadDatum>, LoadDataError> {
        Ok(row)
    }

    fn current_timestamp(&self, _column: &LoadColumnInfo) -> LoadDatum {
        LoadDatum::Timestamp(1_722_000_000)
    }

    fn begin_transaction(&self) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn set_transaction_low_priority(&self) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn add_record(
        &self,
        row: &[LoadDatum],
        _duplicate_check: bool,
        _size_hint: Option<usize>,
    ) -> Result<(), LoadDataError> {
        self.rows
            .lock()
            .expect("load rows mutex")
            .push(row.to_vec());
        Ok(())
    }

    fn batch_check_and_insert(
        &self,
        rows: &[Vec<LoadDatum>],
        _replace: bool,
    ) -> Result<(u64, u64), LoadDataError> {
        self.rows
            .lock()
            .expect("load rows mutex")
            .extend(rows.iter().cloned());
        Ok((rows.len() as u64, 0))
    }

    fn statement_commit(&self) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn commit_transaction(&self) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn rollback_transaction(&self) -> Result<(), LoadDataError> {
        Ok(())
    }

    fn allow_write_row_id(&self) -> bool {
        true
    }

    fn killed(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
}

fn run_load_rows(
    controller: LoadDataController,
    rows: Vec<Vec<LoadDatum>>,
) -> (String, Vec<Vec<LoadDatum>>) {
    let backend = Arc::new(LoadDataCaptureBackend::default());
    let captured = Arc::clone(&backend.rows);
    let mut worker = NewLoadDataWorker(
        backend,
        controller,
        planInfo {
            ID: 1,
            Columns: Vec::new(),
            GenColExprs: Vec::new(),
        },
        "test.load_data".into(),
    )
    .expect("create LOAD DATA worker");
    worker
        .TestLoadLocal(Box::new(LoadDataRowsParser {
            rows: rows.into_iter(),
        }))
        .expect("run LOAD DATA rows");
    let stats = worker
        .stats
        .lock()
        .expect("load stats mutex")
        .message
        .clone();
    let rows = captured.lock().expect("captured rows mutex").clone();
    (stats, rows)
}

fn load_column(name: &str, time_type: bool, not_null: bool) -> LoadColumnInfo {
    LoadColumnInfo {
        name: name.into(),
        generated: false,
        time_type,
        not_null,
        extra_handle: false,
    }
}

/// Go TestLoadDataMissingColumn: a missing NOT NULL timestamp column is
/// filled by the worker's current-timestamp path and counted as one warning.
#[test]
fn TestLoadDataMissingColumn() {
    let id = load_column("id", false, false);
    let timestamp = load_column("t", true, true);
    let controller = |columns: Vec<LoadColumnInfo>| LoadDataController {
        path: "/tmp/nonexistence.csv".into(),
        restrictive: false,
        ignore_lines: 0,
        field_count: columns.len(),
        field_mappings: columns.iter().cloned().map(FieldMapping::Column).collect(),
        insert_columns: columns,
        assignment_count: 0,
        expression_warnings: Vec::new(),
        on_duplicate: OnDuplicateKeyHandling::Error,
        max_rows_in_batch: 32,
        low_priority: false,
        shard_allocate_step: 0,
    };

    let (empty_message, empty_rows) =
        run_load_rows(controller(vec![id.clone(), timestamp.clone()]), vec![]);
    assert_eq!(
        empty_message,
        "Records: 0  Deleted: 0  Skipped: 0  Warnings: 0"
    );
    assert!(empty_rows.is_empty());

    let (message, rows) = run_load_rows(
        controller(vec![id, timestamp]),
        vec![vec![LoadDatum::Text("12".into())]],
    );
    assert_eq!(message, "Records: 1  Deleted: 0  Skipped: 0  Warnings: 1");
    assert_eq!(
        rows,
        vec![vec![
            LoadDatum::Text("12".into()),
            LoadDatum::Timestamp(1_722_000_000)
        ]]
    );

    let id = load_column("id", false, false);
    let timestamp = load_column("t", true, true);
    let nullable_timestamp = load_column("t2", true, false);
    let (message, rows) = run_load_rows(
        controller(vec![id, timestamp, nullable_timestamp]),
        vec![vec![LoadDatum::Text("12".into())]],
    );
    assert_eq!(message, "Records: 1  Deleted: 0  Skipped: 0  Warnings: 1");
    assert_eq!(
        rows,
        vec![vec![
            LoadDatum::Text("12".into()),
            LoadDatum::Timestamp(1_722_000_000),
            LoadDatum::Null,
        ]]
    );
}

/// Go TestIssue18681: all BIT fields survive LOAD DATA row mapping, including
/// a 32-bit value, without dropping columns or changing their order.
#[test]
fn TestIssue18681() {
    let columns = (b'a'..=b'f')
        .map(|name| load_column(&(name as char).to_string(), false, false))
        .collect::<Vec<_>>();
    let controller = LoadDataController {
        path: "/tmp/nonexistence.csv".into(),
        restrictive: false,
        ignore_lines: 0,
        field_count: 6,
        field_mappings: columns.iter().cloned().map(FieldMapping::Column).collect(),
        insert_columns: columns,
        assignment_count: 0,
        expression_warnings: vec!["bit conversion warning".into(); 5],
        on_duplicate: OnDuplicateKeyHandling::Error,
        max_rows_in_batch: 32,
        low_priority: false,
        shard_allocate_step: 0,
    };
    let input = vec![
        LoadDatum::Integer(1),
        LoadDatum::Integer(1),
        LoadDatum::Integer(1),
        LoadDatum::Integer(1),
        LoadDatum::Unsigned(0b1100010001001110011000100100111),
        LoadDatum::Integer(1),
    ];
    let (message, rows) = run_load_rows(controller, vec![input.clone()]);
    assert_eq!(message, "Records: 1  Deleted: 0  Skipped: 0  Warnings: 5");
    assert_eq!(rows, vec![input]);
}

/// Go TestIssue34358: NULL user variables used by SET assignments remain NULL
/// in the inserted row and the assignment warning is retained in statistics.
#[test]
fn TestIssue34358() {
    let controller = LoadDataController {
        path: "/tmp/nonexistence.csv".into(),
        restrictive: false,
        ignore_lines: 0,
        field_count: 2,
        field_mappings: vec![
            FieldMapping::UserVariable("v1".into()),
            FieldMapping::UserVariable("v2".into()),
        ],
        insert_columns: vec![
            load_column("a", false, false),
            load_column("b", false, false),
        ],
        assignment_count: 2,
        expression_warnings: vec!["NULL user variable".into()],
        on_duplicate: OnDuplicateKeyHandling::Error,
        max_rows_in_batch: 32,
        low_priority: false,
        shard_allocate_step: 0,
    };
    let (message, rows) = run_load_rows(controller, vec![vec![LoadDatum::Null, LoadDatum::Null]]);
    assert_eq!(message, "Records: 1  Deleted: 0  Skipped: 0  Warnings: 1");
    assert_eq!(rows, vec![vec![LoadDatum::Null, LoadDatum::Null]]);
}

/// Go TestInsertIgnore：唯一键、事务内重复行、转换告警、坏 NULL 与分区告警。
#[test]
fn TestInsertIgnore() {
    let mut tk = write_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "drop table if exists t; create table t (id int primary key auto_increment, c1 int unique key)",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1, 2)", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");

    tk.MustExec("insert ignore into t values (1, 3), (2, 3)", Vec::new());
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 2  Duplicates: 1  Warnings: 1"
    );
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 2", "2 3"]));

    tk.MustExec("insert ignore into t values (3, 4), (3, 4)", Vec::new());
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 2  Duplicates: 1  Warnings: 1"
    );
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 2", "2 3", "3 4"]));

    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert ignore into t values (4, 4), (4, 5), (4, 6)",
        Vec::new(),
    );
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 3  Duplicates: 2  Warnings: 2"
    );
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 2", "2 3", "3 4", "4 5"]));
    tk.MustExec("commit", Vec::new());

    tk.Session()
        .InjectNextDmlCommitErrorForTest("foo")
        .expect("inject one-shot write failure");
    assert!(
        tk.ExecToErr("insert ignore into t values (1, 3)")
            .to_string()
            .contains("foo")
    );

    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (a bigint)", Vec::new());
    tk.MustExec("insert ignore into t select '1a'", Vec::new());
    assert_eq!(
        tk.Session().LastMessage(),
        "Records: 1  Duplicates: 0  Warnings: 1"
    );
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1292|Truncated incorrect DOUBLE value: '1a'"],
    ));
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("insert ignore into t values ('1a')", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1366|Incorrect bigint value: '1a' for column 'a' at row 1"],
    ));
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1", "1"]));

    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (a int primary key, b int)", Vec::new());
    tk.MustExec("insert ignore into t values (1, 1)", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");
    tk.MustExec("insert ignore into t values (1, 1)", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1062|Duplicate entry '1' for key 't.PRIMARY'"],
    ));

    tk.MustExec(
        "drop table if exists test; create table test (i int primary key, j int unique); begin; insert into test values (1, 1); insert ignore into test values (2, 1); commit",
        Vec::new(),
    );
    tk.MustQuery("select * from test", Vec::new())
        .Check(Rows(&["1 1"]));

    tk.MustExec(
        "delete from test; insert into test values (1, 1); begin; delete from test where i = 1; insert ignore into test values (2, 1); commit",
        Vec::new(),
    );
    tk.MustQuery("select * from test", Vec::new())
        .Check(Rows(&["2 1"]));

    tk.MustExec(
        "delete from test; insert into test values (1, 1); begin; update test set i = 2, j = 2 where i = 1; insert ignore into test values (1, 3); insert ignore into test values (2, 4); commit",
        Vec::new(),
    );
    tk.MustQuery("select * from test order by i", Vec::new())
        .Check(Rows(&["1 3", "2 2"]));

    tk.MustExec("drop table if exists badnull", Vec::new());
    tk.MustExec("create table badnull (i int not null)", Vec::new());
    tk.MustExec("insert ignore into badnull values (null)", Vec::new());
    assert_eq!(tk.Session().LastMessage(), "");
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1048|Column 'i' cannot be null"],
    ));
    tk.MustQuery("select * from badnull", Vec::new())
        .Check(Rows(&["0"]));

    tk.MustExec(
        "create table tp (id int) partition by range (id) (partition p0 values less than (1), partition p1 values less than (2))",
        Vec::new(),
    );
    tk.MustExec("insert ignore into tp values (1), (3)", Vec::new());
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1526|Table has no partition for value 3"],
    ));
}

/// Go TestReplaceLog：悬空唯一索引必须拒绝 REPLACE，而不是静默覆盖索引。
#[test]
fn TestReplaceLog() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table testLog (a int not null primary key, b int unique key)",
        Vec::new(),
    );
    CreateDanglingIndexForTest(&domain, "test", "testLog", &[("a", "1"), ("b", "1")])
        .expect("create dangling unique index");
    let error = tk.ExecToErr("replace into testLog values (0, 0), (1, 1)");
    assert_eq!(
        error.to_string(),
        "can not be duplicated row, due to old row not found. handle 1 not found"
    );
    tk.MustQuery("admin cleanup index testLog b", Vec::new())
        .Check(Rows(&["1"]));
}

/// Go TestRebaseIfNeeded：更新自增列不应无条件 rebase allocator。
#[test]
fn TestRebaseIfNeeded() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t (a int not null primary key auto_increment, b int unique key)",
        Vec::new(),
    );
    tk.MustExec("insert into t (b) values (1)", Vec::new());
    AddRecordWithoutAutoIDRebaseForTest(&domain, "test", "t", &[("a", "30001"), ("b", "2")])
        .expect("direct AddRecord without allocator rebase");
    tk.MustExec("update t set b = 3 where a = 30001", Vec::new());
    tk.MustExec("insert into t (b) values (4)", Vec::new());
    tk.MustQuery("select a from t where b = 4", Vec::new())
        .Check(Rows(&["2"]));

    tk.MustExec(
        "insert into t set b = 3 on duplicate key update a = a",
        Vec::new(),
    );
    tk.MustExec("insert into t (b) values (5)", Vec::new());
    tk.MustQuery("select a from t where b = 5", Vec::new())
        .Check(Rows(&["4"]));

    tk.MustExec(
        "insert into t set b = 3 on duplicate key update a = a + 1",
        Vec::new(),
    );
    tk.MustExec("insert into t (b) values (6)", Vec::new());
    tk.MustQuery("select a from t where b = 6", Vec::new())
        .Check(Rows(&["30003"]));
}

/// Go TestDeferConstraintCheckForInsert：立即/延迟唯一约束检查均须阻止冲突写入。
#[test]
fn TestDeferConstraintCheckForInsert() {
    let mut tk = write_testkit();
    // Go clears the global default before constructing this TestKit so the
    // deferred-constraint cases run in optimistic transactions.
    tk.MustExec("set tidb_txn_mode = ''", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "drop table if exists t; create table t (a int primary key, b int)",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1, 2), (2, 2)", Vec::new());
    assert!(
        tk.ExecToErr("update t set a = a + 1 where b = 2")
            .to_string()
            .contains("Duplicate")
    );

    tk.MustExec("drop table t; create table t (i int key)", Vec::new());
    tk.MustExec("insert t values (1)", Vec::new());
    tk.MustExec("set tidb_constraint_check_in_place = 1", Vec::new());
    tk.MustExec("begin", Vec::new());
    assert!(
        tk.ExecToErr("insert t values (1)")
            .to_string()
            .contains("Duplicate")
    );
    tk.MustExec("update t set i = 2 where i = 1", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["2"]));

    tk.MustExec("set tidb_constraint_check_in_place = 0", Vec::new());
    tk.MustExec("replace into t values (1), (2)", Vec::new());
    tk.MustExec("begin", Vec::new());
    assert!(
        tk.ExecToErr("update t set i = 2 where i = 1")
            .to_string()
            .contains("Duplicate")
    );
    assert!(
        tk.ExecToErr("insert into t values (1) on duplicate key update i = i + 1")
            .to_string()
            .contains("Duplicate")
    );
    tk.MustExec("rollback", Vec::new());

    tk.MustExec(
        "drop table t; create table t (id int primary key, v int unique)",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1, 1)", Vec::new());
    tk.MustExec("set tidb_constraint_check_in_place = 1", Vec::new());
    tk.MustExec("set @@autocommit = 0", Vec::new());
    assert!(
        tk.ExecToErr("insert into t values (3, 1)")
            .to_string()
            .contains("Duplicate")
    );
    assert!(
        tk.ExecToErr("insert into t values (1, 3)")
            .to_string()
            .contains("Duplicate")
    );
    tk.MustExec("commit", Vec::new());

    tk.MustExec("set tidb_constraint_check_in_place = 0", Vec::new());
    tk.MustExec("insert into t values (3, 1)", Vec::new());
    tk.MustExec("insert into t values (1, 3)", Vec::new());
    assert!(tk.ExecToErr("commit").to_string().contains("Duplicate"));

    for mode in [0, 1] {
        tk.MustExec(
            &format!("set tidb_constraint_check_in_place = {mode}"),
            Vec::new(),
        );

        tk.MustExec("drop table t", Vec::new());
        tk.MustExec(
            "create global temporary table t (a int primary key, b int) on commit delete rows",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustExec("insert into t values (1, 1)", Vec::new());
        assert!(
            tk.ExecToErr("insert into t values (1, 3)")
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("insert into t values (2, 2)", Vec::new());
        assert!(
            tk.ExecToErr("update t set a = a + 1 where a = 1")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("insert into t values (1, 3) on duplicate key update a = a + 1",)
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("commit", Vec::new());

        tk.MustExec("drop table t", Vec::new());
        tk.MustExec(
            "create global temporary table t (a int, b int unique) on commit delete rows",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustExec("insert into t values (1, 1)", Vec::new());
        assert!(
            tk.ExecToErr("insert into t values (3, 1)")
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("insert into t values (2, 2)", Vec::new());
        assert!(
            tk.ExecToErr("update t set b = b + 1 where a = 1")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("insert into t values (3, 1) on duplicate key update b = b + 1",)
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("commit", Vec::new());

        tk.MustExec("drop table if exists tl", Vec::new());
        tk.MustExec(
            "create temporary table tl (a int primary key, b int)",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustExec("insert into tl values (1, 1)", Vec::new());
        assert!(
            tk.ExecToErr("insert into tl values (1, 3)")
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("insert into tl values (2, 2)", Vec::new());
        assert!(
            tk.ExecToErr("update tl set a = a + 1 where a = 1")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("insert into tl values (1, 3) on duplicate key update a = a + 1",)
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("commit", Vec::new());

        tk.MustExec("begin", Vec::new());
        tk.MustQuery("select * from tl", Vec::new())
            .Check(Rows(&["1 1", "2 2"]));
        assert!(
            tk.ExecToErr("insert into tl values (1, 3)")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("update tl set a = a + 1 where a = 1")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("insert into tl values (1, 3) on duplicate key update a = a + 1",)
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("rollback", Vec::new());

        tk.MustExec("drop table tl", Vec::new());
        tk.MustExec(
            "create temporary table tl (a int, b int unique)",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustExec("insert into tl values (1, 1)", Vec::new());
        assert!(
            tk.ExecToErr("insert into tl values (3, 1)")
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("insert into tl values (2, 2)", Vec::new());
        assert!(
            tk.ExecToErr("update tl set b = b + 1 where a = 1")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("insert into tl values (3, 1) on duplicate key update b = b + 1",)
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("commit", Vec::new());

        tk.MustExec("begin", Vec::new());
        tk.MustQuery("select * from tl", Vec::new())
            .Check(Rows(&["1 1", "2 2"]));
        assert!(
            tk.ExecToErr("insert into tl values (3, 1)")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("update tl set b = b + 1 where a = 1")
                .to_string()
                .contains("Duplicate")
        );
        assert!(
            tk.ExecToErr("insert into tl values (3, 1) on duplicate key update b = b + 1",)
                .to_string()
                .contains("Duplicate")
        );
        tk.MustExec("rollback", Vec::new());
    }
}

/// Go TestPessimisticDeleteYourWrites：删除自己的写入后，另一会话可插入并提交。
#[test]
fn TestPessimisticDeleteYourWrites() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut session1 = NewTestKit(store.clone());
    let mut session2 = NewTestKit(store);
    session1.MustExec("use test", Vec::new());
    session2.MustExec("use test", Vec::new());
    session1.MustExec("drop table if exists x", Vec::new());
    session1.MustExec("create table x (id int primary key, c int)", Vec::new());
    session1.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
    session2.MustExec("set tidb_txn_mode = 'pessimistic'", Vec::new());
    session1.MustExec("begin", Vec::new());
    session1.MustExec("insert into x select 1, 1", Vec::new());
    session1.MustExec("delete from x where id = 1", Vec::new());
    session2.MustExec("begin", Vec::new());

    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        started_tx.send(()).expect("signal pessimistic writer");
        session2.MustExec("insert into x select 1, 2", Vec::new());
        session2.MustExec("commit", Vec::new());
        done_tx.send(()).expect("signal pessimistic commit");
    });
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("pessimistic writer started");
    assert!(
        done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "the conflicting insert must wait until the deleting transaction commits"
    );
    session1.MustExec("commit", Vec::new());
    writer.join().expect("pessimistic writer thread");
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("pessimistic writer committed");
    session1
        .MustQuery("select * from x", Vec::new())
        .Check(Rows(&["1 2"]));
}

/// Go TestLatch 的并发写冲突分支。
///
/// Rust mock-store 尚未把 Go `WithTxnLocalLatches(64)` 接到 TestKit 构造器，
/// 但这里走真实的双事务提交冲突路径，而不是只检查已提交行的重复键错误。
#[test]
fn TestLatch() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut first = NewTestKit(store.clone());
    let mut second = NewTestKit(store);
    first.MustExec("use test", Vec::new());
    second.MustExec("use test", Vec::new());
    first.MustExec("drop table if exists t", Vec::new());
    first.MustExec("create table t (id int)", Vec::new());
    first.MustExec("set @@tidb_disable_txn_auto_retry = true", Vec::new());
    second.MustExec("set @@tidb_disable_txn_auto_retry = true", Vec::new());
    first.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
    second.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());

    let run_non_overlapping =
        |first: &mut astersql_testkit::TestKit, second: &mut astersql_testkit::TestKit| {
            first.MustExec("begin", Vec::new());
            for value in 0..100 {
                first.MustExec(&format!("insert into t values ({value})"), Vec::new());
            }
            second.MustExec("begin", Vec::new());
            for value in 100..200 {
                first.MustExec(&format!("insert into t values ({value})"), Vec::new());
            }
            second.MustExec("commit", Vec::new());
        };

    run_non_overlapping(&mut first, &mut second);
    first.MustExec("commit", Vec::new());
    first.MustExec("truncate table t", Vec::new());
    run_non_overlapping(&mut first, &mut second);
    first.MustExec("commit", Vec::new());

    first.MustExec("begin", Vec::new());
    first.MustExec("update t set id = id + 1", Vec::new());
    second.MustExec("update t set id = id + 1", Vec::new());
    assert!(
        first
            .ExecToErr("commit")
            .to_string()
            .contains("Write conflict")
    );
}
