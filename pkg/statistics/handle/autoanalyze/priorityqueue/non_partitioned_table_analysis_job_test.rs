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

// 非分区表分析作业（NonPartitionedTableAnalysisJob）相关单元测试。
//
// 上方注释保留 Go 集成测试语义（整表/索引 ANALYZE、ValidateAndPrepare 失败冷却）；
// 可执行部分校验分析类型与 `%n` 参数化 SQL 生成。

// use std::collections::HashMap;
//
// NonPartitionedTableAnalysisJobDraft 对应 Go priorityqueue.NonPartitionedTableAnalysisJob 的测试字段。
// #[derive(Default)]
// struct NonPartitionedTableAnalysisJobDraft {
//     table_id: i64,
//     schema_name: String,
//     table_name: String,
//     index_ids: HashMap<i64, ()>,
//     table_stats_ver: i32,
//     weight: f64,
// }
//
// gen_sql_for_analyze_table 对应 Go GenSQLForAnalyzeTable，保留 %n 占位符和参数顺序。
// fn gen_sql_for_analyze_table(job: &NonPartitionedTableAnalysisJobDraft) -> (&'static str, Vec<String>) {
//     ("analyze table %n.%n", vec![job.schema_name.clone(), job.table_name.clone()])
// }
//
// gen_sql_for_analyze_index 对应 Go GenSQLForAnalyzeIndex，额外追加 index 参数。
// fn gen_sql_for_analyze_index(job: &NonPartitionedTableAnalysisJobDraft, index: &str) -> (&'static str, Vec<String>) {
//     ("analyze table %n.%n index %n", vec![job.schema_name.clone(), job.table_name.clone(), index.to_string()])
// }
//
// TestGenSQLForNonPartitionedTable 验证非分区表整表 analyze SQL 模板和参数。
// #[test]
// fn test_gen_sql_for_non_partitioned_table() {
//     let job = NonPartitionedTableAnalysisJobDraft {
//         schema_name: "test_schema".to_string(),
//         table_name: "test_table".to_string(),
//         ..Default::default()
//     };
//
//     let expected_sql = "analyze table %n.%n";
//     let expected_params = vec!["test_schema".to_string(), "test_table".to_string()];
//
//     let (sql, params) = gen_sql_for_analyze_table(&job);
//
//     assert_eq!(expected_sql, sql);
//     assert_eq!(expected_params, params);
// }
//
// TestGenSQLForNonPartitionedTableIndex 验证索引 analyze 的 SQL 模板和参数顺序。
// #[test]
// fn test_gen_sql_for_non_partitioned_table_index() {
//     let job = NonPartitionedTableAnalysisJobDraft {
//         schema_name: "test_schema".to_string(),
//         table_name: "test_table".to_string(),
//         ..Default::default()
//     };
//
//     let index = "test_index";
//
//     let expected_sql = "analyze table %n.%n index %n";
//     let expected_params = vec!["test_schema".to_string(), "test_table".to_string(), index.to_string()];
//
//     let (sql, params) = gen_sql_for_analyze_index(&job, index);
//
//     assert_eq!(expected_sql, sql);
//     assert_eq!(expected_params, params);
// }
//
// TestAnalyzeNonPartitionedTable 对应 Go 的 mock store 场景：建表、插入数据、Analyze 后统计行数变为 3。
// #[test]
// fn test_analyze_non_partitioned_table() {
//     let (store, dom) = testkit::create_mock_store_and_domain();
//     let tk = testkit::new_test_kit(store);
//     tk.must_exec("use test");
//
//     tk.must_exec("create table t (a int, b int, index idx(a))");
//     tk.must_exec("insert into t values (1, 1), (2, 2), (3, 3)");
//     let job = NonPartitionedTableAnalysisJobDraft {
//         schema_name: "test".to_string(),
//         table_name: "t".to_string(),
//         table_stats_ver: 2,
//         ..Default::default()
//     };
//
// Before analyze table.
//     let handle = dom.stats_handle();
//     let is = dom.info_schema();
//     let tbl = is.table_by_name(context::background(), ast::new_cistr("test"), ast::new_cistr("t")).expect("table exists");
//     let tbl_stats = handle.get_physical_table_stats(tbl.meta().id, tbl.meta());
//     assert!(tbl_stats.pseudo);
//
//     job.analyze(handle, dom.sys_proc_tracker());
// Check the result of analyze.
//     let is = dom.info_schema();
//     let tbl = is.table_by_name(context::background(), ast::new_cistr("test"), ast::new_cistr("t")).expect("table exists");
//     let tbl_stats = handle.get_physical_table_stats(tbl.meta().id, tbl.meta());
//     assert_eq!(3_i64, tbl_stats.realtime_count);
// }
//
// TestAnalyzeNonPartitionedIndexes 对应 Go 的多索引分析：只产生一条 analyze job，但两个索引都被分析。
// #[test]
// fn test_analyze_non_partitioned_indexes() {
//     let (store, dom) = testkit::create_mock_store_and_domain();
//     let tk = testkit::new_test_kit(store);
//     tk.must_exec("use test");
//
//     tk.must_exec("create table t (a int, b int, index idx(a), index idx1(b))");
//     tk.must_exec("insert into t values (1, 1), (2, 2), (3, 3)");
//     let tbl_info = dom.info_schema().table_by_name(context::background(), ast::new_cistr("test"), ast::new_cistr("t")).expect("table exists");
//     let mut job = NonPartitionedTableAnalysisJobDraft {
//         table_id: tbl_info.meta().id,
//         index_ids: HashMap::from([(1, ()), (2, ())]),
//         table_stats_ver: 2,
//         ..Default::default()
//     };
//     let handle = dom.stats_handle();
// Before analyze indexes.
//     let is = dom.info_schema();
//     let tbl = is.table_by_name(context::background(), ast::new_cistr("test"), ast::new_cistr("t")).expect("table exists");
//     let tbl_stats = handle.get_physical_table_stats(tbl.meta().id, tbl.meta());
//     assert!(!tbl_stats.get_idx(1).is_analyzed());
//
//     let (valid, fail_reason) = job.validate_and_prepare(tk.session());
//     assert!(valid);
//     assert_eq!("", fail_reason);
//     job.analyze(handle, dom.sys_proc_tracker());
// Check the result of analyze.
//     let is = dom.info_schema();
//     let tbl = is.table_by_name(context::background(), ast::new_cistr("test"), ast::new_cistr("t")).expect("table exists");
//     let tbl_stats = handle.get_physical_table_stats(tbl.meta().id, tbl.meta());
//     assert!(tbl_stats.get_idx(1).is_some());
//     assert!(tbl_stats.get_idx(1).is_analyzed());
//     assert!(tbl_stats.get_idx(2).is_some());
//     assert!(tbl_stats.get_idx(2).is_analyzed());
// Check analyze jobs are created.
//     let rows = tk.must_query("select * from mysql.analyze_jobs").rows();
// Because analyze one index will analyze all indexes and all columns together, so there is only 1 job.
//     assert_eq!(1, rows.len());
// }
//
// TestNonPartitionedTableValidateAndPrepare 覆盖最近失败记录对非分区表分析任务的跳过逻辑。
// #[test]
// fn test_non_partitioned_table_validate_and_prepare() {
//     let (store, dom) = testkit::create_mock_store_and_domain();
//     let tk = testkit::new_test_kit(store);
//     tk.must_exec(metadef::CREATE_ANALYZE_JOBS_TABLE);
//     tk.must_exec("create schema example_schema");
//     tk.must_exec("use example_schema");
//     tk.must_exec("create table example_table1 (a int, b int, index idx(a))");
//     let table_info = dom.info_schema().table_by_name(context::background(), ast::new_cistr("example_schema"), ast::new_cistr("example_table1")).expect("table exists");
//     let mut job = NonPartitionedTableAnalysisJobDraft {
//         table_id: table_info.meta().id,
//         table_stats_ver: 2,
//         weight: 3.0,
//         ..Default::default()
//     };
//     init_jobs(&tk);
//     insert_multiple_finished_jobs(&tk, "example_table1", "");
//
//     let se = tk.session();
//     let sctx = sessionctx::as_context(se);
//     let (mut valid, mut fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
//
// Insert some failed jobs.
// Just failed.
//     let now = tk.must_query("select now()").rows()[0][0].clone_string();
//     insert_failed_job_with_start_time(&tk, &job.schema_name, &job.table_name, "", &now);
// Note: The failure reason is not checked in this test because the time duration can sometimes be inaccurate.(not now)
//     (valid, _) = job.validate_and_prepare(sctx);
//     assert!(!valid);
// Failed 10 seconds ago.
//     let mut start_time = tk.must_query("select now() - interval 10 second").rows()[0][0].clone_string();
//     insert_failed_job_with_start_time(&tk, &job.schema_name, &job.table_name, "", &start_time);
//     (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(!valid);
//     assert_eq!("last failed analysis duration is less than 2 times the average analysis duration", fail_reason);
// Failed long long ago.
//     start_time = tk.must_query("select now() - interval 300 day").rows()[0][0].clone_string();
//     insert_failed_job_with_start_time(&tk, &job.schema_name, &job.table_name, "", &start_time);
//     (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
// }
//
// TestValidateAndPrepareWhenOnlyHasFailedAnalysisRecords 覆盖没有成功记录时使用默认等待时间的分支。
// #[test]
// fn test_validate_and_prepare_when_only_has_failed_analysis_records() {
//     let (store, dom) = testkit::create_mock_store_and_domain();
//     let tk = testkit::new_test_kit(store);
//     tk.must_exec(metadef::CREATE_ANALYZE_JOBS_TABLE);
//     tk.must_exec("create schema example_schema");
//     tk.must_exec("use example_schema");
//     tk.must_exec("create table example_table1 (a int, b int, index idx(a))");
//     let table_info = dom.info_schema().table_by_name(context::background(), ast::new_cistr("example_schema"), ast::new_cistr("example_table1")).expect("table exists");
//     let mut job = NonPartitionedTableAnalysisJobDraft {
//         table_id: table_info.meta().id,
//         weight: 2.0,
//         ..Default::default()
//     };
//     let se = tk.session();
//     let sctx = sessionctx::as_context(se);
//     let (mut valid, mut fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
// Failed long long ago.
//     let start_time = tk.must_query("select now() - interval 30 day").rows()[0][0].clone_string();
//     insert_failed_job_with_start_time(&tk, &job.schema_name, &job.table_name, "", &start_time);
//     (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
//
// Failed recently.
//     let ten_seconds_ago = tk.must_query("select now() - interval 10 second").rows()[0][0].clone_string();
//     insert_failed_job_with_start_time(&tk, &job.schema_name, &job.table_name, "", &ten_seconds_ago);
//     (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(!valid);
//     assert_eq!("last failed analysis duration is less than 30m0s", fail_reason);
// }
// */
use crate::{
    ANALYZE_INDEX, ANALYZE_TABLE, AnalysisJob, AnalysisRuntime, IndexMetadata,
    NewNonPartitionedTableAnalysisJob, TABLE_NOT_EXIST, TableMetadata,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Runtime {
    table: Option<TableMetadata>,
    executions: Mutex<Vec<(String, Vec<String>)>>,
    execute_result: Mutex<Result<bool, String>>,
    last_failed: Option<Duration>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            table: None,
            executions: Mutex::new(Vec::new()),
            execute_result: Mutex::new(Ok(true)),
            last_failed: None,
        }
    }
}

impl AnalysisRuntime for Runtime {
    fn table_by_id(&self, _table_id: i64) -> Option<TableMetadata> {
        self.table.clone()
    }

    fn last_failed_analysis_duration(
        &self,
        _schema: &str,
        _table: &str,
        _partitions: &[String],
    ) -> Result<Option<Duration>, String> {
        Ok(self.last_failed)
    }

    fn average_analysis_duration(
        &self,
        _schema: &str,
        _table: &str,
        _partitions: &[String],
    ) -> Result<Option<Duration>, String> {
        Ok(None)
    }

    fn execute_analyze(
        &self,
        sql: &str,
        params: &[String],
        _stats_version: i32,
        _need_version_rewrite_warn: bool,
    ) -> Result<bool, String> {
        self.executions
            .lock()
            .unwrap()
            .push((sql.to_owned(), params.to_vec()));
        self.execute_result.lock().unwrap().clone()
    }
}

/// 校验无索引时为 analyzeTable，有 IndexIDs 时为 analyzeIndex，且 SQL 参数顺序正确。
#[test]
fn non_partitioned_job_generates_identifier_parameterized_sql() {
    // 整表路径：IndexIDs 为空。
    let mut table =
        NewNonPartitionedTableAnalysisJob(1, HashMap::new(), 2, false, 0.5, 10.0, Duration::ZERO);
    table.SchemaName = "db".into();
    table.TableName = "t".into();
    assert_eq!(ANALYZE_TABLE, table.GetAnalyzeType());
    assert_eq!(
        ("analyze table %n.%n".into(), vec!["db".into(), "t".into()]),
        table.GenSQLForAnalyzeTable()
    );

    // 索引路径：IndexIDs 非空，参数追加 index 名。
    let mut index = NewNonPartitionedTableAnalysisJob(
        1,
        HashMap::from([(3, ())]),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    index.SchemaName = "db".into();
    index.TableName = "t".into();
    assert_eq!(ANALYZE_INDEX, index.GetAnalyzeType());
    assert_eq!(vec!["db", "t", "idx"], index.GenSQLForAnalyzeIndex("idx").1);
}

#[test]
fn analyze_indexes_matches_go_by_executing_only_the_first_index() {
    let runtime = Runtime {
        execute_result: Mutex::new(Ok(true)),
        ..Runtime::default()
    };
    let mut job = NewNonPartitionedTableAnalysisJob(
        1,
        HashMap::from([(1, ()), (2, ())]),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    job.SchemaName = "db".into();
    job.TableName = "t".into();
    job.IndexNames = vec!["idx".into(), "idx1".into()];

    job.Analyze(&runtime).unwrap();

    assert_eq!(
        vec![(
            "analyze table %n.%n index %n".into(),
            vec!["db".into(), "t".into(), "idx".into()]
        )],
        *runtime.executions.lock().unwrap()
    );
}

#[test]
fn stale_index_ids_do_not_fall_back_to_analyzing_the_whole_table() {
    let runtime = Runtime {
        execute_result: Mutex::new(Ok(true)),
        ..Runtime::default()
    };
    let mut job = NewNonPartitionedTableAnalysisJob(
        1,
        HashMap::from([(99, ())]),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    job.SchemaName = "db".into();
    job.TableName = "t".into();

    job.Analyze(&runtime).unwrap();

    assert!(runtime.executions.lock().unwrap().is_empty());
}

#[test]
fn validate_and_prepare_matches_go_failure_hook_retry_semantics() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut missing =
        NewNonPartitionedTableAnalysisJob(1, HashMap::new(), 2, false, 0.5, 10.0, Duration::ZERO);
    let missing_calls = Arc::clone(&calls);
    missing.RegisterFailureHook(Arc::new(move |_, retry| {
        missing_calls.lock().unwrap().push(retry);
    }));
    assert_eq!(
        (false, TABLE_NOT_EXIST.into()),
        missing.ValidateAndPrepare(&Runtime::default())
    );
    assert_eq!(vec![false], *calls.lock().unwrap());

    let retry_calls = Arc::new(Mutex::new(Vec::new()));
    let mut cooling_down =
        NewNonPartitionedTableAnalysisJob(1, HashMap::new(), 2, false, 0.5, 10.0, Duration::ZERO);
    let recorded = Arc::clone(&retry_calls);
    cooling_down.RegisterFailureHook(Arc::new(move |_, retry| {
        recorded.lock().unwrap().push(retry);
    }));
    let runtime = Runtime {
        table: Some(TableMetadata {
            id: 1,
            schema_name: "db".into(),
            table_name: "t".into(),
            indices: vec![IndexMetadata {
                id: 2,
                name: "idx".into(),
                public: true,
                columnar: false,
            }],
            ..TableMetadata::default()
        }),
        last_failed: Some(Duration::ZERO),
        execute_result: Mutex::new(Ok(true)),
        ..Runtime::default()
    };
    assert!(!cooling_down.ValidateAndPrepare(&runtime).0);
    assert_eq!(vec![true], *retry_calls.lock().unwrap());
}
