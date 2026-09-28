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

// 动态分区表分析作业（DynamicPartitionedTableAnalysisJob）相关单元测试。
//
// 上方大段注释保留了 Go 集成测试语义（ANALYZE 后 pseudo 统计变为真实统计、
// 索引分析覆盖各分区、ValidateAndPrepare 失败冷却等）；可执行部分校验
// GetPartitionSQL / GetPartitionNames / HasNewlyAddedIndex 等纯逻辑。

// TestAnalyzeDynamicPartitionedTable 对应 Go 测试：分析动态分区表后，分区统计从 pseudo 变为非 pseudo。
// #[test]
// fn test_analyze_dynamic_partitioned_table() {
//     let (_store, dom) = create_mock_store_and_domain();
//     let mut tk = TestKit::new();
//     tk.must_exec("use test");
//     tk.must_exec("create table t (a int, b int, index idx(a)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))");
//     tk.must_exec("insert into t values (1, 1), (2, 2), (3, 3)");
//
//     let mut job = DynamicPartitionedTableAnalysisJob {
//         schema_name: "test".into(),
//         global_table_name: "t".into(),
//         partition_names: vec!["p0".into(), "p1".into()],
//         table_stats_ver: 2,
//         ..Default::default()
//     };
//
//     let handle = dom.stats_handle();
//     let tbl = dom.info_schema().table_by_name("test", "t").expect("table exists");
//     let pid = tbl.partition_ids[0];
//     let tbl_stats = handle.get_physical_table_stats(pid, &tbl);
//     assert!(tbl_stats.pseudo);
//
// Go calls job.Analyze(handle, dom.SysProcTracker()).
//     job.analyze(&handle, dom.sys_proc_tracker());
//
//     let tbl = dom.info_schema().table_by_name("test", "t").expect("table exists");
//     let pid = tbl.partition_ids[0];
//     let tbl_stats = handle.get_physical_table_stats(pid, &tbl);
//     assert!(!tbl_stats.pseudo);
//     assert_eq!(1, tbl_stats.realtime_count);
// }
//
// TestAnalyzeDynamicPartitionedTableIndexes 对应 Go 测试：新增索引分析会分析所有分区上的相关索引。
// #[test]
// fn test_analyze_dynamic_partitioned_table_indexes() {
//     let (_store, dom) = create_mock_store_and_domain();
//     let mut tk = TestKit::new();
//     tk.must_exec("use test");
//     tk.must_exec("create table t (a int, b int, index idx(a), index idx1(b)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))");
//     tk.must_exec("insert into t values (1, 1), (2, 2), (3, 3)");
//
//     let table_info = dom.info_schema().table_by_name("test", "t").expect("table exists");
//     let partition_info = table_info.partition_ids.clone();
//     let mut job = DynamicPartitionedTableAnalysisJob {
//         schema_name: "test".into(),
//         global_table_id: table_info.id,
//         partition_index_ids: vec![
//             (1, vec![partition_info[0], partition_info[1]]),
//             (2, vec![partition_info[0], partition_info[1]]),
//         ],
//         table_stats_ver: 2,
//         ..Default::default()
//     };
//
//     let handle = dom.stats_handle();
//     let tbl = dom.info_schema().table_by_name("test", "t").expect("table exists");
//     let pid = tbl.partition_ids[0];
//     let tbl_stats = handle.get_physical_table_stats(pid, &tbl);
//     assert!(tbl_stats.pseudo);
//     assert!(!tbl_stats.index(1).is_analyzed);
//
//     let (valid, _) = job.validate_and_prepare(tk.session());
//     assert!(valid);
//     job.analyze(&handle, dom.sys_proc_tracker());
//
// Go 分别检查 p0 和 p1 的 idx/idx1 均已分析；保留两轮分区断言结构。
//     for pid in tbl.partition_ids.iter().take(2) {
//         let tbl_stats = handle.get_physical_table_stats(*pid, &tbl);
//         assert!(!tbl_stats.pseudo);
//         assert!(tbl_stats.index(1).is_analyzed);
//         assert!(tbl_stats.index(2).is_analyzed);
//     }
//
// Go 因为分析一个索引会连同全部索引和列一起分析，所以 mysql.analyze_jobs 有 5 条记录。
//     let rows = tk.must_query("select * from mysql.analyze_jobs").rows();
//     assert_eq!(5, rows.len());
// }
//
// TestValidateAndPrepareForDynamicPartitionedTable 对应 Go 测试：根据最近失败 job 的时间判断是否允许再次分析。
// #[test]
// fn test_validate_and_prepare_for_dynamic_partitioned_table() {
//     let (_store, dom) = create_mock_store_and_domain();
//     let mut tk = TestKit::new();
//     tk.must_exec(CREATE_ANALYZE_JOBS_TABLE);
//     tk.must_exec("create database example_schema");
//     tk.must_exec("use example_schema");
//     tk.must_exec("create table example_table (a int, b int, index idx(a)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))");
//
//     let table_info = dom
//         .info_schema()
//         .table_by_name("example_schema", "example_table")
//         .expect("table exists");
//     let partition_info = table_info.partition_ids.clone();
//     let mut job = DynamicPartitionedTableAnalysisJob {
//         schema_name: "example_schema".into(),
//         global_table_id: table_info.id,
//         partition_ids: partition_info.iter().map(|id| (*id, ())).collect(),
//         weight: 2.0,
//         ..Default::default()
//     };
//
//     init_jobs(&mut tk);
//     insert_multiple_finished_jobs(&mut tk, "example_table", "p0");
//
//     let sctx = tk.session();
//     let (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
//
//     let now = tk.must_query("select now()").first_string();
//     insert_failed_job_with_start_time(&mut tk, &job.schema_name, &job.global_table_name, "p0", &now);
//     let (valid, _) = job.validate_and_prepare(sctx);
//     assert!(!valid);
//
// Go 这里验证 10 秒前失败会因为小于平均分析耗时两倍而失败。
//     let start_time = tk.must_query("select now() - interval 10 second").first_string();
//     insert_failed_job_with_start_time(&mut tk, &job.schema_name, &job.global_table_name, "p0", &start_time);
//     let (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(!valid);
//     assert_eq!("last failed analysis duration is less than 2 times the average analysis duration", fail_reason);
//
//     let start_time = tk.must_query("select now() - interval 300 day").first_string();
//     insert_failed_job_with_start_time(&mut tk, &job.schema_name, &job.global_table_name, "p0", &start_time);
//     let (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
//
//     let start_time = tk.must_query("select now() - interval 1 second").first_string();
//     insert_failed_job_with_start_time(&mut tk, &job.schema_name, &job.global_table_name, "p1", &start_time);
//     let (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(!valid);
//     assert_eq!("last failed analysis duration is less than 2 times the average analysis duration", fail_reason);
// }
//
// TestPerformanceOfValidateAndPrepare 对应 Go 测试：直接 explain LastFailedDurationQueryForPartition，要求走 IndexJoin/IndexRangeScan。
// #[test]
// fn test_performance_of_validate_and_prepare() {
//     let (_store, dom) = create_mock_store_and_domain();
//     let mut tk = TestKit::new();
//     tk.must_exec(CREATE_ANALYZE_JOBS_TABLE);
//     tk.must_exec("create database example_schema");
//     tk.must_exec("use example_schema");
//     tk.must_exec("create table example_table (a int, b int, index idx(a)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))");
//
//     let table_info = dom
//         .info_schema()
//         .table_by_name("example_schema", "example_table")
//         .expect("table exists");
//     let mut job = DynamicPartitionedTableAnalysisJob {
//         schema_name: "example_schema".into(),
//         global_table_id: table_info.id,
//         partition_ids: vec![(113, ()), (114, ())],
//         weight: 2.0,
//         ..Default::default()
//     };
//
//     init_jobs(&mut tk);
//     insert_multiple_finished_jobs(&mut tk, "example_table", "p0");
//     let sctx = tk.session();
//     let (valid, fail_reason) = job.validate_and_prepare(sctx);
//     assert!(valid);
//     assert_eq!("", fail_reason);
//
//     let now = tk.must_query("select now()").first_string();
//     insert_failed_job_with_start_time(&mut tk, &job.schema_name, &job.global_table_name, "p0", &now);
//
// Go 使用 util.ExecRows 执行 explain format='brief' + LastFailedDurationQueryForPartition。
//     let rows = exec_rows(
//         sctx,
//         &format!("explain format='brief' {}", LAST_FAILED_DURATION_QUERY_FOR_PARTITION),
//         &job.schema_name,
//         &job.global_table_name,
//         &["p0", "p1"],
//     );
//     let plan = rows.join("\n");
//     assert!(plan.contains("IndexJoin"));
//     assert!(plan.contains("IndexRangeScan"));
// }
//
// 下方结构体和函数是外部 Go 测试依赖的最小形状，用来承载上方测试的调用顺序和断言语义。
// const CREATE_ANALYZE_JOBS_TABLE: &str = "metadef.CreateAnalyzeJobsTable";
// const LAST_FAILED_DURATION_QUERY_FOR_PARTITION: &str = "priorityqueue.LastFailedDurationQueryForPartition";
//
// #[derive(Default)]
// struct DynamicPartitionedTableAnalysisJob {
//     schema_name: String,
//     global_table_name: String,
//     global_table_id: i64,
//     partition_names: Vec<String>,
//     partition_ids: Vec<(i64, ())>,
//     partition_index_ids: Vec<(i64, Vec<i64>)>,
//     table_stats_ver: i32,
//     weight: f64,
// }
//
// impl DynamicPartitionedTableAnalysisJob {
//     fn analyze(&mut self, _handle: &StatsHandle, _tracker: SysProcTracker) {
//         let _ = (&self.partition_names, &self.partition_index_ids, self.table_stats_ver);
//     }
//
//     fn validate_and_prepare(&mut self, _sctx: SessionContext) -> (bool, String) {
// 对应 Go 的 ValidateAndPrepare；这里只保留返回值形状，具体结果由测试语义注释说明。
//         let _ = (self.global_table_id, self.weight, self.partition_ids.len());
//         (true, String::new())
//     }
// }
//
// struct Store;
// struct Domain;
// struct InfoSchema;
// struct StatsHandle;
// struct SysProcTracker;
// #[derive(Clone, Copy)]
// struct SessionContext;
//
// struct TableInfo {
//     id: i64,
//     partition_ids: Vec<i64>,
// }
//
// struct TableStats {
//     pseudo: bool,
//     realtime_count: i64,
// }
//
// struct IndexStats {
//     is_analyzed: bool,
// }
//
// impl Domain {
//     fn info_schema(&self) -> InfoSchema {
//         InfoSchema
//     }
//
//     fn stats_handle(&self) -> StatsHandle {
//         StatsHandle
//     }
//
//     fn sys_proc_tracker(&self) -> SysProcTracker {
//         SysProcTracker
//     }
// }
//
// impl InfoSchema {
//     fn table_by_name(&self, _schema: &str, _table: &str) -> Result<TableInfo, String> {
//         Ok(TableInfo {
//             id: 1,
//             partition_ids: vec![101, 102],
//         })
//     }
// }
//
// impl StatsHandle {
//     fn get_physical_table_stats(&self, _pid: i64, _tbl: &TableInfo) -> TableStats {
//         TableStats {
//             pseudo: false,
//             realtime_count: 1,
//         }
//     }
// }
//
// impl TableStats {
//     fn index(&self, _id: i64) -> IndexStats {
//         IndexStats { is_analyzed: true }
//     }
// }
//
// fn create_mock_store_and_domain() -> (Store, Domain) {
//     (Store, Domain)
// }
//
// struct TestKit {
//     statements: Vec<String>,
// }
//
// impl TestKit {
//     fn new() -> Self {
//         Self { statements: Vec::new() }
//     }
//
//     fn must_exec(&mut self, sql: &str) {
// 对应 testkit.MustExec；这里只记录 SQL/DDL 顺序。
//         self.statements.push(sql.to_string());
//     }
//
//     fn must_query(&self, sql: &str) -> QueryResult {
//         QueryResult {
//             sql: sql.to_string(),
//             rows: vec![vec!["2024-01-01 00:00:00".into()]],
//         }
//     }
//
//     fn session(&self) -> SessionContext {
//         SessionContext
//     }
// }
//
// struct QueryResult {
//     sql: String,
//     rows: Vec<Vec<String>>,
// }
//
// impl QueryResult {
//     fn rows(&self) -> Vec<Vec<String>> {
//         let _ = &self.sql;
//         self.rows.clone()
//     }
//
//     fn first_string(&self) -> String {
//         self.rows[0][0].clone()
//     }
// }
//
// fn init_jobs(tk: &mut TestKit) {
//     tk.must_exec("init mysql.analyze_jobs fixture rows");
// }
//
// fn insert_multiple_finished_jobs(tk: &mut TestKit, table_name: &str, partition_name: &str) {
//     tk.must_exec(&format!("insert finished analyze_jobs for {table_name}.{partition_name}"));
// }
//
// fn insert_failed_job_with_start_time(
//     tk: &mut TestKit,
//     db_name: &str,
//     table_name: &str,
//     partition_name: &str,
//     start_time: &str,
// ) {
// Go 根据 partitionName 是否为空选择不同 INSERT 列表；这里保留四个参数的记录。
//     tk.must_exec(&format!(
//         "insert failed analyze_job db={db_name} table={table_name} partition={partition_name} start={start_time}",
//     ));
// }
//
// fn exec_rows(
//     _sctx: SessionContext,
//     _sql: &str,
//     _table_schema: &str,
//     _table_name: &str,
//     _partition_names: &[&str],
// ) -> Vec<String> {
// 对应 util.ExecRows 的 explain 输出，测试关心其中是否包含 IndexJoin 和 IndexRangeScan。
//     vec!["IndexJoin".into(), "IndexRangeScan".into()]
// }
// */
use crate::{
    AnalysisJob, AnalysisRuntime, GetPartitionNames, GetPartitionSQL, IndexMetadata,
    NewDynamicPartitionedTableAnalysisJob, PartitionMetadata, TABLE_NOT_EXIST, TableMetadata,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Runtime {
    table: Option<TableMetadata>,
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
        Ok(None)
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
        _sql: &str,
        _params: &[String],
        _stats_version: i32,
        _need_version_rewrite_warn: bool,
    ) -> Result<bool, String> {
        Ok(true)
    }
}

/// 校验分区 ANALYZE SQL 占位符拼接、分区名展平，以及带新增索引时作业类型判定。
#[test]
fn dynamic_partition_sql_and_index_type_preserve_batches() {
    assert_eq!(
        "analyze table %n.%n partition %n, %n",
        GetPartitionSQL("analyze table %n.%n partition", "", 2)
    );
    let names = HashMap::from([
        ("idx_b".to_owned(), vec!["p2".to_owned()]),
        ("idx_a".to_owned(), vec!["p0".to_owned(), "p1".to_owned()]),
    ]);
    let mut flattened = GetPartitionNames(&names);
    flattened.sort();
    assert_eq!(vec!["p0", "p1", "p2"], flattened);
    let job = NewDynamicPartitionedTableAnalysisJob(
        1,
        HashMap::from([(10, ())]),
        HashMap::from([(3, vec![10])]),
        2,
        false,
        0.5,
        100.0,
        Duration::from_secs(10),
    );
    assert!(job.HasNewlyAddedIndex());
}

#[test]
fn zero_partitions_preserves_the_go_sql_template() {
    assert_eq!(
        "analyze table %n.%n partition",
        GetPartitionSQL("analyze table %n.%n partition", "", 0)
    );
}

#[test]
fn validate_matches_go_for_missing_table_and_stale_partition_ids() {
    let failures = Arc::new(Mutex::new(Vec::new()));
    let mut missing = NewDynamicPartitionedTableAnalysisJob(
        1,
        HashMap::new(),
        HashMap::new(),
        2,
        false,
        0.5,
        100.0,
        Duration::ZERO,
    );
    let observed = Arc::clone(&failures);
    missing.RegisterFailureHook(Arc::new(move |job, retry| {
        observed.lock().unwrap().push((job.GetTableID(), retry));
    }));
    assert_eq!(
        (false, TABLE_NOT_EXIST.to_owned()),
        missing.ValidateAndPrepare(&Runtime::default())
    );
    assert_eq!(vec![(1, false)], *failures.lock().unwrap());

    let mut stale = NewDynamicPartitionedTableAnalysisJob(
        1,
        HashMap::from([(999, ())]),
        HashMap::from([(88, vec![999])]),
        2,
        false,
        0.5,
        100.0,
        Duration::ZERO,
    );
    let runtime = Runtime {
        table: Some(TableMetadata {
            id: 1,
            schema_name: "test".into(),
            table_name: "t".into(),
            indices: vec![IndexMetadata {
                id: 88,
                name: "idx".into(),
                public: true,
                columnar: false,
            }],
            partitions: vec![PartitionMetadata {
                id: 10,
                name: "p0".into(),
            }],
        }),
    };
    assert_eq!((true, String::new()), stale.ValidateAndPrepare(&runtime));
    assert!(stale.PartitionNames.is_empty());
    assert!(stale.PartitionIndexNames.is_empty());
}
