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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：类型 `WriterState` 负责 WriterState。
// 中文总览：类型 `SharedIndexWriter` 负责 SharedIndexWriter。
// 中文总览：函数 `snapshot` 负责 snapshot。
// 中文总览：函数 `write` 负责 write。
// 中文总览：函数 `flush` 负责 flush。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use astersql_ddl::backfilling::KeyRange;
use astersql_ddl::backfilling_operators::{
    AddIndexIngestPipeline, BackfillOperatorContext, IndexRecord, IndexWriter, MOCK_FLUSH_ERROR,
    MOCK_SCAN_RECORD_ERROR, MOCK_WRITE_LOCAL_ERROR, NewAddIndexIngestPipeline, NewLocalWorkerCtx,
    OperatorError, SCAN_RECORD_EXEC, TableScanTaskSource, TableScanWorker, WRITE_LOCAL_EXEC,
};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{DbValue, NewTestKit, Rows, TestKit};
use astersql_tests_realtikvtest_addindextest3::serial_guard;

const REGION_COUNT: usize = 10;

// 该类型围绕 WriterState 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Default)]
struct WriterState {
    data: BTreeMap<Vec<u8>, Vec<u8>>,
    flush_count: usize,
    quota_checks: Vec<(usize, i64)>,
}

// 该类型围绕 SharedIndexWriter 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Default)]
struct SharedIndexWriter {
    state: Arc<Mutex<WriterState>>,
}

impl SharedIndexWriter {
    // 该辅助函数负责 snapshot。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn snapshot(&self) -> WriterState {
        self.state.lock().expect("index writer mutex").clone()
    }
}

impl IndexWriter for SharedIndexWriter {
    // 该辅助函数负责 write。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn write(&mut self, key: &[u8], value: &[u8]) -> Result<usize, String> {
        self.state
            .lock()
            .expect("index writer mutex")
            .data
            .insert(key.to_vec(), value.to_vec());
        Ok(key.len() + value.len())
    }

    // 该辅助函数负责 flush。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn flush(&mut self) -> Result<(), String> {
        self.state.lock().expect("index writer mutex").flush_count += 1;
        Ok(())
    }

    // 该辅助函数负责 ingest 回填 if quota exceeded。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn ingest_if_quota_exceeded(&mut self, task_id: usize, row_count: i64) -> Result<(), String> {
        self.state
            .lock()
            .expect("index writer mutex")
            .quota_checks
            .push((task_id, row_count));
        Ok(())
    }

    // 该辅助函数负责 total 键 count。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn total_key_count(&self) -> i64 {
        self.state.lock().expect("index writer mutex").data.len() as i64
    }
}

// 该辅助函数负责 构造 source。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn make_source() -> TableScanTaskSource {
    TableScanTaskSource {
        physical_table_id: 42,
        start_key: vec![0],
        end_key: vec![100],
        record_prefix: vec![0],
        checkpoint_key: None,
    }
}

// 该辅助函数负责 构造 ranges。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn make_ranges() -> Vec<KeyRange> {
    (0..REGION_COUNT)
        .map(|region| KeyRange {
            start_key: vec![(region * 10) as u8],
            end_key: vec![((region + 1) * 10) as u8],
        })
        .collect()
}

// 该辅助函数负责 构造 rows。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn make_rows() -> Vec<IndexRecord> {
    (0..REGION_COUNT)
        .map(|ordinal| IndexRecord {
            row_key: vec![(ordinal * 10 + 5) as u8],
            index_key: vec![b'i', ordinal as u8],
            value: vec![b'v', ordinal as u8],
            matches_partial_index: true,
        })
        .collect()
}

// 该辅助函数负责 构造 scanner。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn make_scanner() -> TableScanWorker {
    TableScanWorker {
        chunk_capacity: 100,
        condition_pushed: false,
        reorg_batch_size: None,
    }
}

// 该辅助函数负责 构造 pipeline。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn make_pipeline(
    context: BackfillOperatorContext,
    writer: SharedIndexWriter,
) -> AddIndexIngestPipeline<SharedIndexWriter> {
    NewAddIndexIngestPipeline(
        context,
        make_source(),
        make_ranges(),
        make_rows(),
        writer,
        1,
        2,
        2,
        make_scanner(),
    )
}

/// Build the same SQL/Domain fixture as Go `getRealAddIndexJob` + `prepare`.
// 该辅助函数负责 准备 sql fixture。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn prepare_sql_fixture(database: &str) -> (Arc<AnalyzeStatsStore>, TestKit) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());

    tk.MustExec("create table job_source(a int)", Vec::new());
    tk.MustExec("insert into job_source values (1)", Vec::new());
    tk.MustExec("alter table job_source add index idx_job(a)", Vec::new());
    tk.MustExec("admin check index job_source idx_job", Vec::new());

    tk.MustExec(
        "create table t(a int primary key, b int, index idx(b))",
        Vec::new(),
    );
    for ordinal in 0..REGION_COUNT {
        tk.MustExec(
            "insert into t values (?, ?)",
            vec![
                DbValue::I64((ordinal * 10_000) as i64),
                DbValue::I64(ordinal as i64),
            ],
        );
    }
    tk.MustQuery("select count(*) from t", Vec::new())
        .Check(Rows(&["10"]));
    tk.MustExec("admin check table t", Vec::new());
    (store, tk)
}

// 该辅助函数负责 断言 successful pipeline。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn assert_successful_pipeline(
    pipeline: &AddIndexIngestPipeline<SharedIndexWriter>,
    writer: &SharedIndexWriter,
) {
    let summary = pipeline.Summary();
    assert_eq!(REGION_COUNT, summary.task_count);
    assert_eq!(REGION_COUNT, summary.chunk_count);
    assert_eq!(REGION_COUNT as i64, summary.processed_rows);
    assert_eq!(REGION_COUNT as i64, summary.total_rows);
    assert_eq!(REGION_COUNT * 4, summary.written_bytes);

    let state = writer.snapshot();
    assert_eq!(REGION_COUNT, state.data.len());
    assert_eq!(REGION_COUNT, state.quota_checks.len());
    assert_eq!(1, state.flush_count);
    for ordinal in 0..REGION_COUNT {
        assert_eq!(
            Some(&vec![b'v', ordinal as u8]),
            state.data.get(&vec![b'i', ordinal as u8])
        );
    }
}

/// Go `TestBackfillOperators`.
// 该用例覆盖 backfill operators。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_backfill_operators() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare_sql_fixture("operator_stages");
    let context = NewLocalWorkerCtx();
    let writer = SharedIndexWriter::default();
    let mut pipeline = make_pipeline(context.clone(), writer.clone());

    pipeline.Execute().expect("open source/scan/ingest/sink");
    assert!(pipeline.IsStarted());
    pipeline.Close().expect("close all backfill operators");
    assert!(!pipeline.IsStarted());
    assert_eq!(None, context.OperatorErr());
    assert_successful_pipeline(&pipeline, &writer);

    tk.MustQuery("select count(*) from t force index(idx)", Vec::new())
        .Check(Rows(&["10"]));
}

/// Go `TestBackfillOperatorPipeline`.
// 该用例覆盖 backfill 算子 pipeline。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_backfill_operator_pipeline() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare_sql_fixture("operator_pipeline");
    let context = NewLocalWorkerCtx();
    let writer = SharedIndexWriter::default();
    let mut pipeline = make_pipeline(context.clone(), writer.clone());

    pipeline
        .Execute()
        .expect("execute ADD INDEX ingest pipeline");
    pipeline.Close().expect("pipeline close");
    assert_eq!(None, context.OperatorErr());
    assert_successful_pipeline(&pipeline, &writer);
    tk.MustExec("admin check table t", Vec::new());
}

/// Go `TestBackfillOperatorPipelineException`.
// 该用例覆盖 backfill 算子 pipeline exception。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_backfill_operator_pipeline_exception() {
    let _serial = serial_guard();
    let (_store, _tk) = prepare_sql_fixture("operator_exceptions");
    // 该类型围绕 ExceptionCase 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    struct ExceptionCase {
        failpoint: &'static str,
        close_error: OperatorError,
        operator_error: Option<OperatorError>,
    }
    let cases = [
        ExceptionCase {
            failpoint: MOCK_SCAN_RECORD_ERROR,
            close_error: OperatorError::Cancelled,
            operator_error: Some(OperatorError::Write("mock scan record error".to_owned())),
        },
        ExceptionCase {
            failpoint: SCAN_RECORD_EXEC,
            close_error: OperatorError::Cancelled,
            operator_error: Some(OperatorError::Cancelled),
        },
        ExceptionCase {
            failpoint: MOCK_WRITE_LOCAL_ERROR,
            close_error: OperatorError::Cancelled,
            operator_error: Some(OperatorError::Write("mock write local error".to_owned())),
        },
        ExceptionCase {
            failpoint: WRITE_LOCAL_EXEC,
            close_error: OperatorError::Cancelled,
            operator_error: None,
        },
        ExceptionCase {
            failpoint: MOCK_FLUSH_ERROR,
            close_error: OperatorError::Flush("mock flush error".to_owned()),
            operator_error: Some(OperatorError::Flush("mock flush error".to_owned())),
        },
    ];

    for case in cases {
        let context = NewLocalWorkerCtx();
        let mut pipeline = make_pipeline(context.clone(), SharedIndexWriter::default());
        pipeline
            .EnableFailpoint(case.failpoint)
            .expect("enable production failpoint");
        pipeline.Execute().expect("pipeline Execute");
        assert_eq!(
            case.close_error,
            pipeline.Close().expect_err(case.failpoint),
            "{} close error",
            case.failpoint
        );
        assert!(context.IsCancelled(), "{} must cancel", case.failpoint);
        assert_eq!(
            case.operator_error,
            context.OperatorErr(),
            "{} operator error",
            case.failpoint
        );
    }
}

/// Go `TestTuneWorkerPoolSize`.
// 该用例覆盖 tune worker pool size。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_tune_worker_pool_size() {
    let _serial = serial_guard();
    let (_store, _tk) = prepare_sql_fixture("operator_tune");
    let context = NewLocalWorkerCtx();
    let writer = SharedIndexWriter::default();
    let mut pipeline = make_pipeline(context.clone(), writer.clone());

    {
        let (scan, ingest) = pipeline.GetReaderAndWriter();
        assert_eq!(2, scan.GetWorkerPoolSize());
        assert_eq!(2, ingest.GetWorkerPoolSize());
        scan.TuneWorkerPoolSize(8, false);
        ingest.TuneWorkerPoolSize(8, false);
        assert_eq!(8, scan.GetWorkerPoolSize());
        assert_eq!(8, ingest.GetWorkerPoolSize());
        scan.TuneWorkerPoolSize(1, false);
        ingest.TuneWorkerPoolSize(1, false);
        assert_eq!(1, scan.GetWorkerPoolSize());
        assert_eq!(1, ingest.GetWorkerPoolSize());
    }

    pipeline.Execute().expect("execute tuned pipeline");
    pipeline.Close().expect("close tuned pipeline");
    assert_eq!(None, context.OperatorErr());
    assert_successful_pipeline(&pipeline, &writer);
}
