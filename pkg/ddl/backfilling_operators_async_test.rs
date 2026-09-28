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

// ADD INDEX 异步回填流水线的并发调优与故障传播回归测试。
//
// 测试用固定键范围和索引记录驱动“任务生成 → 扫描 → 写入 → 汇总”各阶段，
// 再通过共享写入器观测写入、配额检查和刷新副作用，避免依赖真实存储后端。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::backfilling::KeyRange;
use crate::backfilling_operators::{
    AddIndexIngestPipeline, BackfillOperatorContext, IndexRecord, IndexWriter, MOCK_FLUSH_ERROR,
    MOCK_SCAN_RECORD_ERROR, MOCK_WRITE_LOCAL_ERROR, NewAddIndexIngestPipeline, NewLocalWorkerCtx,
    OperatorError, SCAN_RECORD_EXEC, TableScanTaskSource, TableScanWorker, WRITE_LOCAL_EXEC,
};

const TASK_COUNT: usize = 10;

#[derive(Debug, Default)]
/// 汇总并发写入阶段产生的可观测副作用。
struct WriterState {
    data: BTreeMap<Vec<u8>, Vec<u8>>,
    flushes: usize,
    quota_checks: Vec<(usize, i64)>,
}

#[derive(Clone, Debug, Default)]
/// 让各写入工作线程落到同一份状态，便于在流水线关闭后统一断言。
struct SharedWriter(Arc<Mutex<WriterState>>);

impl IndexWriter for SharedWriter {
    fn write(&mut self, key: &[u8], value: &[u8]) -> Result<usize, String> {
        self.0
            .lock()
            .expect("writer mutex")
            .data
            .insert(key.to_vec(), value.to_vec());
        Ok(key.len() + value.len())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.0.lock().expect("writer mutex").flushes += 1;
        Ok(())
    }

    fn ingest_if_quota_exceeded(&mut self, task_id: usize, row_count: i64) -> Result<(), String> {
        self.0
            .lock()
            .expect("writer mutex")
            .quota_checks
            .push((task_id, row_count));
        Ok(())
    }

    fn total_key_count(&self) -> i64 {
        self.0.lock().expect("writer mutex").data.len() as i64
    }
}

/// 构造十个互不重叠的扫描任务，每个任务恰好命中一条索引记录。
fn pipeline(
    context: BackfillOperatorContext,
    writer: SharedWriter,
) -> AddIndexIngestPipeline<SharedWriter> {
    let ranges = (0..TASK_COUNT)
        .map(|ordinal| KeyRange {
            start_key: vec![(ordinal * 10) as u8],
            end_key: vec![((ordinal + 1) * 10) as u8],
        })
        .collect();
    let rows = (0..TASK_COUNT)
        .map(|ordinal| IndexRecord {
            row_key: vec![(ordinal * 10 + 1) as u8],
            index_key: vec![b'i', ordinal as u8],
            value: vec![b'v', ordinal as u8],
            matches_partial_index: true,
        })
        .collect();
    NewAddIndexIngestPipeline(
        context,
        TableScanTaskSource {
            physical_table_id: 1,
            start_key: vec![0],
            end_key: vec![100],
            record_prefix: vec![0],
            checkpoint_key: None,
        },
        ranges,
        rows,
        writer,
        1,
        3,
        3,
        TableScanWorker {
            chunk_capacity: 100,
            condition_pushed: false,
            reorg_batch_size: None,
        },
    )
}

#[test]
/// 验证执行前设置的读写并发度生效，且关闭流水线会等待各阶段完成并发布最终汇总。
fn asynchronous_pipeline_runs_all_stages_and_honors_tuning() {
    let context = NewLocalWorkerCtx();
    let writer = SharedWriter::default();
    let mut pipeline = pipeline(context.clone(), writer.clone());
    {
        let (reader, writer) = pipeline.GetReaderAndWriter();
        reader.TuneWorkerPoolSize(4, false);
        writer.TuneWorkerPoolSize(5, false);
        assert_eq!(4, reader.GetWorkerPoolSize());
        assert_eq!(5, writer.GetWorkerPoolSize());
    }

    pipeline.Execute().unwrap();
    assert!(pipeline.IsStarted());
    pipeline.Close().unwrap();
    assert!(!pipeline.IsStarted());
    assert_eq!(None, context.OperatorErr());
    assert_eq!(TASK_COUNT, pipeline.Summary().task_count);
    assert_eq!(TASK_COUNT as i64, pipeline.Summary().processed_rows);

    let state = writer.0.lock().expect("writer mutex");
    assert_eq!(TASK_COUNT, state.data.len());
    assert_eq!(TASK_COUNT, state.quota_checks.len());
    assert_eq!(1, state.flushes);
}

#[test]
/// 验证扫描、写入和刷新阶段的故障会分别传递到关闭结果、上下文错误与取消状态。
fn asynchronous_pipeline_failpoints_propagate_close_and_operator_errors() {
    // 关闭错误表示流水线最终返回值；上下文则保留操作器记录的错误。
    // `WRITE_LOCAL_EXEC` 仅触发取消，不写入操作器错误，因此其期望值为 `None`。
    let cases = [
        (
            MOCK_SCAN_RECORD_ERROR,
            OperatorError::Cancelled,
            Some(OperatorError::Write("mock scan record error".to_owned())),
        ),
        (
            SCAN_RECORD_EXEC,
            OperatorError::Cancelled,
            Some(OperatorError::Cancelled),
        ),
        (
            MOCK_WRITE_LOCAL_ERROR,
            OperatorError::Cancelled,
            Some(OperatorError::Write("mock write local error".to_owned())),
        ),
        (WRITE_LOCAL_EXEC, OperatorError::Cancelled, None),
        (
            MOCK_FLUSH_ERROR,
            OperatorError::Flush("mock flush error".to_owned()),
            Some(OperatorError::Flush("mock flush error".to_owned())),
        ),
    ];

    for (failpoint, close_error, operator_error) in cases {
        let context = NewLocalWorkerCtx();
        let mut pipeline = pipeline(context.clone(), SharedWriter::default());
        pipeline.EnableFailpoint(failpoint).unwrap();
        pipeline.Execute().unwrap();
        assert_eq!(close_error, pipeline.Close().unwrap_err(), "{failpoint}");
        assert_eq!(operator_error, context.OperatorErr(), "{failpoint}");
        assert!(context.IsCancelled(), "{failpoint}");
    }
}
