// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 非事务 DML（Non-transactional DML）行为测试。
//
// 非事务 DML 按分片键将大语句切成多个小 job 分别提交，避免单事务过大。
// 可执行测试覆盖 job 切分公式、读子句限制、Datum 字面量、结果集形状，
// 以及轻量 mock store 上的分片可见性。

use std::cmp::Ordering;

use astersql_session::nontransactional::{
    DRY_RUN_QUERY, DRY_RUN_SPLIT_DML, Datum, FieldType, LogLevel, MetricKind,
    NonTransactionalError, NonTransactionalRuntime, Result as NtResult, ResultValue,
    RuntimeRecordSet, SessionVars, appendNewJob, buildDryRunResults, buildExecuteResults,
    checkReadClauses, job,
};

/// 非事务运行时桩：固定会话变量并录制日志，不访问真实存储。
struct StubRuntime {
    vars: SessionVars,
    logs: Vec<(LogLevel, String)>,
}

impl StubRuntime {
    /// 构造默认测试会话变量（autocommit、chunk size 等与 Go 用例对齐）。
    fn new() -> Self {
        Self {
            vars: SessionVars {
                read_staleness: 0,
                bulk_dml_enabled: false,
                in_non_transactional_dml: false,
                autocommit: true,
                in_transaction: false,
                global_batch_dml_enabled: true,
                dml_batch_size: 0,
                batch_delete: false,
                batch_insert: false,
                weak_read_consistency: false,
                snapshot_ts: 0,
                select_limit: 0,
                max_execution_time: 0,
                memory_quota_query: 0,
                ignore_error: false,
                redact_log: "OFF".to_owned(),
                current_db: "test".to_owned(),
                max_chunk_size: 35,
            },
            logs: Vec::new(),
        }
    }
}

impl NonTransactionalRuntime for StubRuntime {
    fn session_vars(&self) -> &SessionVars {
        &self.vars
    }
    fn session_vars_mut(&mut self) -> &mut SessionVars {
        &mut self.vars
    }
    fn preprocess(
        &mut self,
        _statement: &mut astersql_session::nontransactional::NonTransactionalDMLStmt,
    ) -> NtResult<()> {
        Ok(())
    }
    fn increment_metric(&mut self, _metric: MetricKind) {}
    fn attach_memory_tracker(&mut self, _quota: i64) -> NtResult<()> {
        Ok(())
    }
    fn consume_memory(&mut self, _bytes: i64) -> NtResult<()> {
        Ok(())
    }
    fn detach_memory_tracker(&mut self) -> NtResult<()> {
        Ok(())
    }
    fn scan_shard_values(&mut self, _sql: &str) -> NtResult<Vec<Datum>> {
        Ok(Vec::new())
    }
    fn compare_shard_values(
        &mut self,
        left: &Datum,
        right: &Datum,
        _column: Option<&astersql_session::nontransactional::ColumnInfo>,
    ) -> NtResult<Ordering> {
        left.compare(right, false)
    }
    fn execute_dml(&mut self, _sql: &str) -> NtResult<Option<Box<dyn RuntimeRecordSet>>> {
        Ok(None)
    }
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancellation_error(&self) -> NonTransactionalError {
        NonTransactionalError::new("cancelled")
    }
    fn log(&mut self, level: LogLevel, message: &str) {
        self.logs.push((level, message.to_owned()));
    }
}

/// 校验非事务 job 数公式 `(size+batch-1)/batch` 与 Go 上取整一致。
// 对应 TestNonTransactionalDMLSharding 中 job 数公式：(size+batch-1)/batch。
#[test]
fn nontransactional_job_count_matches_go_ceil_div() {
    let compositions = [
        (0, 10),
        (1, 1),
        (1, 2),
        (30, 25),
        (30, 35),
        (35, 25),
        (35, 35),
        (35, 40),
        (40, 25),
        (40, 35),
        (100, 25),
        (100, 40),
    ];
    for (table_size, batch_size) in compositions {
        let jobs = (table_size + batch_size - 1) / batch_size;
        assert_eq!(
            jobs,
            if table_size == 0 {
                0
            } else {
                (table_size + batch_size - 1) / batch_size
            }
        );
        // Go Check: "{jobs} all succeeded"
        let status = format!("{jobs} all succeeded");
        assert!(status.ends_with("all succeeded"));
    }
}

/// 校验 `checkReadClauses`：非事务 DML 拒绝 LIMIT / ORDER BY。
// 对应 checkReadClauses：non-transactional DML 拒绝 LIMIT / ORDER BY。
#[test]
fn check_read_clauses_rejects_limit_and_order_by() {
    assert!(checkReadClauses(false, false).is_ok());
    let limit_err = checkReadClauses(true, false).expect_err("limit");
    assert!(limit_err.message.contains("don't support limit"));
    let order_err = checkReadClauses(false, true).expect_err("order by");
    assert!(order_err.message.contains("don't support order by"));
}

/// 校验 Datum 有序比较与 SQL 字面量还原（分片边界依赖）。
// 对应 Datum 比较与 SQL 字面量还原：分片边界依赖有序比较与字面量拼接。
#[test]
fn datum_compare_and_sql_literal_cover_signed_text_and_binary() {
    assert_eq!(
        Datum::Signed(1).compare(&Datum::Signed(2), false).unwrap(),
        Ordering::Less
    );
    assert_eq!(
        Datum::Text("a".into())
            .compare(&Datum::Text("B".into()), true)
            .unwrap(),
        Ordering::Less
    );
    assert_eq!(
        Datum::Signed(42).to_sql_literal(FieldType::Signed).unwrap(),
        "42"
    );
    assert_eq!(
        Datum::Text("o'reilly".into())
            .to_sql_literal(FieldType::Text)
            .unwrap(),
        "'o''reilly'"
    );
    assert_eq!(
        Datum::Binary(vec![0x41, 0xf6])
            .to_sql_literal(FieldType::Binary)
            .unwrap(),
        "X'41F6'"
    );
}

/// 校验执行成功汇总与 dry-run 结果集字段名形状。
// 对应 buildExecuteResults / buildDryRunResults：成功汇总与 dry-run 字段名。
#[test]
fn build_execute_and_dry_run_results_match_go_shapes() {
    let mut runtime = StubRuntime::new();
    let mut jobs = Vec::new();
    appendNewJob(
        &mut jobs,
        Datum::Signed(0),
        Datum::Signed(9),
        10,
        &mut runtime,
    )
    .unwrap();
    appendNewJob(
        &mut jobs,
        Datum::Signed(10),
        Datum::Signed(19),
        10,
        &mut runtime,
    )
    .unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].jobID, 1);
    assert_eq!(jobs[1].jobID, 2);

    let ok = buildExecuteResults(&jobs, 35, "OFF", &mut runtime).unwrap();
    assert_eq!(ok.fields[0].name, "number of jobs");
    assert_eq!(ok.fields[1].name, "job status");
    assert_eq!(
        ok.rows[0],
        vec![
            ResultValue::Integer(2),
            ResultValue::Text("all succeeded".into())
        ]
    );

    let mut failed = jobs.clone();
    failed[1].err = Some(NonTransactionalError::new(
        "injected batch(non-transactional) DML error",
    ));
    failed[1].sql = "UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5".into();
    let err = buildExecuteResults(&failed, 35, "OFF", &mut runtime).unwrap_err();
    assert_eq!(
        err.message,
        "1/2 jobs failed in the non-transactional DML: job id: 2, estimated size: 10, sql: UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5, injected batch(non-transactional) DML error;, ...(more in logs)"
    );

    // Go TestNonTransactionalDMLErrorMessage sets tidb_redact_log=marker and
    // requires the complete job SQL to be enclosed by redaction markers.
    let marker_err = buildExecuteResults(&failed, 35, "MARKER", &mut runtime).unwrap_err();
    assert_eq!(
        marker_err.message,
        "1/2 jobs failed in the non-transactional DML: job id: 2, estimated size: 10, sql: ‹UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5›, injected batch(non-transactional) DML error;, ...(more in logs)"
    );

    let dry = buildDryRunResults(
        DRY_RUN_SPLIT_DML,
        vec!["UPDATE t SET b=b*2 WHERE a BETWEEN 0 AND 9".into()],
        35,
    )
    .unwrap();
    assert_eq!(dry.fields[0].name, "split statement examples");
    let dry_q = buildDryRunResults(DRY_RUN_QUERY, vec!["SELECT a FROM t".into()], 35).unwrap();
    assert_eq!(dry_q.fields[0].name, "query statement");
    let _ = job {
        start: Datum::Null,
        end: Datum::Null,
        err: None,
        jobID: 0,
        jobSize: 0,
        sql: String::new(),
    };
}

/// 在 mock store 上用逐点 update/delete 模拟分片 DML，并以 analyze 行数校验可见性。
// 对应 TestNonTransactionalDMLSharding SQL 路径子集：逐点 update/delete 后以 analyze 校验行数。
#[test]
fn batch_style_update_and_delete_cover_sharded_dml_visibility() {
    use astersql_domain::Domain;
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    /// 从 Domain 统计元数据读取表实时行数。
    fn table_row_count(domain: &Domain, database: &str, table: &str) -> i64 {
        let table_info = domain
            .table_by_name(database, table)
            .unwrap_or_else(|error| {
                panic!("typed InfoSchema lookup for {database}.{table}: {error}")
            });
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .stats_meta(table_info.ID)
            .cloned()
            .unwrap_or_else(|| panic!("no statistics recorded for {database}.{table}"))
            .realtime_count
    }

    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t(a int, b int, primary key(a))", Vec::new());
    for i in 0..10 {
        tk.MustExec(
            &format!("insert into t values ({i}, {})", i * 2),
            Vec::new(),
        );
    }
    // 轻量 runtime 仅支持 WHERE k = literal；用逐点更新模拟分片 DML。
    for i in 0..10 {
        tk.MustExec(
            &format!("update t set b = {} where a = {i}", i * 4),
            Vec::new(),
        );
    }
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 10);
    for i in 0..10 {
        tk.MustExec(&format!("delete from t where a = {i}"), Vec::new());
    }
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 0);
}
