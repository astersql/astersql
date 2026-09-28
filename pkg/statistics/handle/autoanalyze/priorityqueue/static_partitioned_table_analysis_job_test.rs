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

// 静态分区表分析作业（StaticPartitionedTableAnalysisJob）相关单元测试。
//
// 上方注释保留 Go 集成测试（分区 ANALYZE、索引分析、失败冷却不影响其它分区）；
// 可执行部分校验分区/分区索引 SQL 模板与参数。

// TestKit、require、failpoint、DDL notifier、sessionctx、goroutine/channel 等外部依赖均按 Go 调用形状保留为后续接线点。
//
// TestGenSQLForAnalyzeStaticPartitionedTable 对应 Go 测试函数：保留 Test Gen S Q L For Analyze Static Partitioned Table 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestGenSQLForAnalyzeStaticPartitionedTable(t: &testing::T) {
// 	job := &priorityqueue.StaticPartitionedTableAnalysisJob{
// 		SchemaName:          "test_schema",
// 		GlobalTableName:     "test_table",
// 		StaticPartitionName: "p0",
// 	}
//
// 	expectedSQL := "analyze table %n.%n partition %n"
// 	expectedParams := []any{"test_schema", "test_table", "p0"}
//
// 	sql, params := job.GenSQLForAnalyzeStaticPartition()
//
// 	require.Equal(t, expectedSQL, sql)
// 	require.Equal(t, expectedParams, params)
// }
//
// TestGenSQLForAnalyzeStaticPartitionedTableIndex 对应 Go 测试函数：保留 Test Gen S Q L For Analyze Static Partitioned Table Index 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestGenSQLForAnalyzeStaticPartitionedTableIndex(t: &testing::T) {
// 	job := &priorityqueue.StaticPartitionedTableAnalysisJob{
// 		SchemaName:          "test_schema",
// 		GlobalTableName:     "test_table",
// 		StaticPartitionName: "p0",
// 	}
//
// 	index := "test_index"
//
// 	expectedSQL := "analyze table %n.%n partition %n index %n"
// 	expectedParams := []any{"test_schema", "test_table", "p0", index}
//
// 	sql, params := job.GenSQLForAnalyzeStaticPartitionIndex(index)
//
// 	require.Equal(t, expectedSQL, sql)
// 	require.Equal(t, expectedParams, params)
// }
//
// TestAnalyzeStaticPartitionedTable 对应 Go 测试函数：保留 Test Analyze Static Partitioned Table 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestAnalyzeStaticPartitionedTable(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
//
// 	tk.MustExec("create table t (a int, b int, index idx(a)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))")
// 	tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)")
//
// 	tableInfo, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	partitionInfo := tableInfo.Meta().GetPartitionInfo()
// 	require.NotNil(t, partitionInfo)
//
// 	job := &priorityqueue.StaticPartitionedTableAnalysisJob{
// 		GlobalTableID:     tableInfo.Meta().ID,
// 		StaticPartitionID: partitionInfo.Definitions[0].ID,
// 		TableStatsVer:     2,
// 	}
//
// Before analyze the partition.
// 	handle := dom.StatsHandle()
// 	is := dom.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	pid := tbl.Meta().GetPartitionInfo().Definitions[0].ID
// 	tblStats := handle.GetPhysicalTableStats(pid, tbl.Meta())
// 	require.True(t, tblStats.Pseudo)
//
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, failReason := job.ValidateAndPrepare(tk.Session())
// 	require.True(t, valid)
// 	require.Equal(t, "", failReason)
// Analyze 会执行真实 ANALYZE 路径；这里只保留测试对统计结果的预期。
// 	job.Analyze(handle, dom.SysProcTracker())
// Check the result of analyze.
// 	is = dom.InfoSchema()
// 	tbl, err = is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	pid = tbl.Meta().GetPartitionInfo().Definitions[0].ID
// 	tblStats = handle.GetPhysicalTableStats(pid, tbl.Meta())
// 	require.False(t, tblStats.Pseudo)
// 	require.Equal(t, int64(1), tblStats.RealtimeCount)
// }
//
// TestAnalyzeStaticPartitionedTableIndexes 对应 Go 测试函数：保留 Test Analyze Static Partitioned Table Indexes 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestAnalyzeStaticPartitionedTableIndexes(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	tk.MustExec("create table t (a int, b int, index idx(a), index idx1(b)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))")
// 	tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)")
// 	tableInfo, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	partitionInfo := tableInfo.Meta().GetPartitionInfo()
// 	require.NotNil(t, partitionInfo)
// 	job := &priorityqueue.StaticPartitionedTableAnalysisJob{
// 		GlobalTableID:     tableInfo.Meta().ID,
// 		StaticPartitionID: partitionInfo.Definitions[0].ID,
// 		IndexIDs:          map[int64]struct{}{1: {}, 2: {}},
// 		TableStatsVer:     2,
// 	}
// 	handle := dom.StatsHandle()
// Before analyze indexes.
// 	is := dom.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	pid := tbl.Meta().GetPartitionInfo().Definitions[0].ID
// 	tblStats := handle.GetPhysicalTableStats(pid, tbl.Meta())
// 	require.False(t, tblStats.GetIdx(1).IsAnalyzed())
//
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, failReason := job.ValidateAndPrepare(tk.Session())
// 	require.True(t, valid)
// 	require.Equal(t, "", failReason)
//
// Analyze 会执行真实 ANALYZE 路径；这里只保留测试对统计结果的预期。
// 	job.Analyze(handle, dom.SysProcTracker())
// Check the result of analyze.
// 	is = dom.InfoSchema()
// 	tbl, err = is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	pid = tbl.Meta().GetPartitionInfo().Definitions[0].ID
// 	tblStats = handle.GetPhysicalTableStats(pid, tbl.Meta())
// 	require.NotNil(t, tblStats.GetIdx(1))
// 	require.True(t, tblStats.GetIdx(1).IsAnalyzed())
// 	require.NotNil(t, tblStats.GetIdx(2))
// 	require.True(t, tblStats.GetIdx(2).IsAnalyzed())
// Check analyze jobs are created.
// 	rows := tk.MustQuery("select * from mysql.analyze_jobs").Rows()
// Because analyze one index will analyze all indexes and all columns together, so there are 4 jobs.
// 	require.Len(t, rows, 4)
// }
//
// TestStaticPartitionedTableValidateAndPrepare 对应 Go 测试函数：保留 Test Static Partitioned Table Validate And Prepare 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestStaticPartitionedTableValidateAndPrepare(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec(metadef.CreateAnalyzeJobsTable)
// 	tk.MustExec("create schema example_schema")
// 	tk.MustExec("use example_schema")
// 	tk.MustExec("create table example_table (a int, b int, index idx(a)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (4))")
// 	tableInfo, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("example_schema"), ast.NewCIStr("example_table"))
// 	require.NoError(t, err)
// 	partitionInfo := tableInfo.Meta().GetPartitionInfo()
// 	require.NotNil(t, partitionInfo)
// 	job := &priorityqueue.StaticPartitionedTableAnalysisJob{
// 		GlobalTableID:     tableInfo.Meta().ID,
// 		StaticPartitionID: partitionInfo.Definitions[0].ID,
// 		Weight:            2,
// 	}
// 	initJobs(tk)
// 	insertMultipleFinishedJobs(tk, "example_table", "p0")
// 	insertMultipleFinishedJobs(tk, "example_table", "p1")
//
// 	se := tk.Session()
// 	sctx := se.(sessionctx.Context)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, failReason := job.ValidateAndPrepare(sctx)
// 	require.True(t, valid)
// 	require.Equal(t, "", failReason)
//
// Insert some failed jobs.
// Just failed.
// 	now := tk.MustQuery("select now()").Rows()[0][0].(string)
// 	insertFailedJobWithStartTime(tk, job.SchemaName, "example_table", "p0", now)
// Note: The failure reason is not checked in this test because the time duration can sometimes be inaccurate.(not now)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ = job.ValidateAndPrepare(sctx)
// 	require.False(t, valid)
// Failed 10 seconds ago.
// 	startTime := tk.MustQuery("select now() - interval 10 second").Rows()[0][0].(string)
// 	insertFailedJobWithStartTime(tk, job.SchemaName, "example_table", "p0", startTime)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, failReason = job.ValidateAndPrepare(sctx)
// 	require.False(t, valid)
// 	require.Equal(t, "last failed analysis duration is less than 2 times the average analysis duration", failReason)
// Failed long long ago.
// 	startTime = tk.MustQuery("select now() - interval 300 day").Rows()[0][0].(string)
// 	insertFailedJobWithStartTime(tk, job.SchemaName, "example_table", "p0", startTime)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, failReason = job.ValidateAndPrepare(sctx)
// 	require.True(t, valid)
// 	require.Equal(t, "", failReason)
// Do not affect other partitions.
// 	job = &priorityqueue.StaticPartitionedTableAnalysisJob{
// 		GlobalTableID:     tableInfo.Meta().ID,
// 		StaticPartitionID: partitionInfo.Definitions[1].ID,
// 		Weight:            2,
// 	}
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, failReason = job.ValidateAndPrepare(sctx)
// 	require.True(t, valid)
// 	require.Equal(t, "", failReason)
// }
// */
use crate::{
    ANALYZE_STATIC_PARTITION, ANALYZE_STATIC_PARTITION_INDEX, AnalysisJob, AnalysisRuntime,
    IndexMetadata, NOT_PARTITIONED_TABLE, NewStaticPartitionTableAnalysisJob, PARTITION_NOT_EXIST,
    PartitionMetadata, TABLE_NOT_EXIST, TableMetadata,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Runtime {
    table: Option<TableMetadata>,
    executions: Mutex<Vec<(String, Vec<String>)>>,
    last_failed: Option<Duration>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            table: None,
            executions: Mutex::new(Vec::new()),
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
        Ok(true)
    }
}

/// 校验静态分区整分区与分区索引两类 SQL 的参数顺序。
#[test]
fn static_partition_job_generates_table_and_index_sql() {
    // 无 IndexIDs：analyzeStaticPartition，参数为 schema/table/partition。
    let mut table = NewStaticPartitionTableAnalysisJob(
        1,
        10,
        HashMap::new(),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    table.SchemaName = "db".into();
    table.GlobalTableName = "t".into();
    table.StaticPartitionName = "p0".into();
    assert_eq!(ANALYZE_STATIC_PARTITION, table.GetAnalyzeType());
    assert_eq!(10, table.AsJSON().TableID);
    assert_eq!(
        vec!["db", "t", "p0"],
        table.GenSQLForAnalyzeStaticPartition().1
    );

    // 有 IndexIDs：analyzeStaticPartitionIndex，额外追加 index 名。
    let mut index = NewStaticPartitionTableAnalysisJob(
        1,
        10,
        HashMap::from([(3, ())]),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    index.SchemaName = "db".into();
    index.GlobalTableName = "t".into();
    index.StaticPartitionName = "p0".into();
    assert_eq!(ANALYZE_STATIC_PARTITION_INDEX, index.GetAnalyzeType());
    assert_eq!(
        vec!["db", "t", "p0", "idx"],
        index.GenSQLForAnalyzeStaticPartitionIndex("idx").1
    );
}

#[test]
fn analyze_indexes_matches_go_by_executing_only_the_first_index() {
    let runtime = Runtime::default();
    let mut job = NewStaticPartitionTableAnalysisJob(
        1,
        10,
        HashMap::from([(1, ()), (2, ())]),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    job.SchemaName = "db".into();
    job.GlobalTableName = "t".into();
    job.StaticPartitionName = "p0".into();
    job.IndexNames = vec!["idx".into(), "idx1".into()];

    job.Analyze(&runtime).unwrap();

    assert_eq!(
        vec![(
            "analyze table %n.%n partition %n index %n".into(),
            vec!["db".into(), "t".into(), "p0".into(), "idx".into()]
        )],
        *runtime.executions.lock().unwrap()
    );
}

#[test]
fn stale_index_ids_do_not_fall_back_to_analyzing_the_whole_partition() {
    let runtime = Runtime::default();
    let mut job = NewStaticPartitionTableAnalysisJob(
        1,
        10,
        HashMap::from([(99, ())]),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    job.SchemaName = "db".into();
    job.GlobalTableName = "t".into();
    job.StaticPartitionName = "p0".into();

    job.Analyze(&runtime).unwrap();

    assert!(runtime.executions.lock().unwrap().is_empty());
}

#[test]
fn validate_and_prepare_matches_go_failure_hook_retry_semantics() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut job = NewStaticPartitionTableAnalysisJob(
        1,
        10,
        HashMap::new(),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    let recorded = Arc::clone(&calls);
    job.RegisterFailureHook(Arc::new(move |_, retry| {
        recorded.lock().unwrap().push(retry);
    }));

    assert_eq!(
        (false, TABLE_NOT_EXIST.into()),
        job.ValidateAndPrepare(&Runtime::default())
    );
    assert_eq!(vec![false], *calls.lock().unwrap());

    let runtime = Runtime {
        table: Some(TableMetadata {
            id: 1,
            schema_name: "db".into(),
            table_name: "t".into(),
            ..TableMetadata::default()
        }),
        ..Runtime::default()
    };
    assert_eq!(
        (false, NOT_PARTITIONED_TABLE.into()),
        job.ValidateAndPrepare(&runtime)
    );
    assert_eq!(vec![false, false], *calls.lock().unwrap());

    let runtime = Runtime {
        table: Some(TableMetadata {
            id: 1,
            schema_name: "db".into(),
            table_name: "t".into(),
            partitions: vec![PartitionMetadata {
                id: 11,
                name: "p1".into(),
            }],
            ..TableMetadata::default()
        }),
        ..Runtime::default()
    };
    assert_eq!(
        (false, PARTITION_NOT_EXIST.into()),
        job.ValidateAndPrepare(&runtime)
    );
    assert_eq!(vec![false, false, false], *calls.lock().unwrap());

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
            partitions: vec![PartitionMetadata {
                id: 10,
                name: "p0".into(),
            }],
        }),
        last_failed: Some(Duration::ZERO),
        ..Runtime::default()
    };
    assert!(!job.ValidateAndPrepare(&runtime).0);
    assert_eq!(vec![false, false, false, true], *calls.lock().unwrap());
}
