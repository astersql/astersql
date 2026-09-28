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

// resultset 模块的 Aster 对照单元测试。
//
// 覆盖 TidbResultSet 的列缓存/chunk 拉取、Finish/Detach/Close 生命周期、
// Finish 锁竞争跳过、lazy 游标跨 chunk 迭代，以及 CursorRUV2Tracker
// 仅上报正增量的行为。

use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_resourcegroup::ConsumptionReporter;
use astersql_server_internal_column as column;
use astersql_util_chunk as chunk;
use astersql_util_execdetails::ruv2_metrics::{RUV2Metrics, RUV2Weights, tikvutil};
use astersql_util_sqlexec as sqlexec;

use crate::*;

/// 构造单列 LONGLONG 的 ResultField，供脚本化 RecordSet 使用。
fn result_field(field_type: u8) -> sqlexec::resolve::ResultField {
    let mut column_info = column::model::ColumnInfo::default();
    column_info.FieldType = *chunk::types::NewFieldType(field_type);
    sqlexec::resolve::ResultField {
        column: Some(Rc::new(column_info)),
        ..sqlexec::resolve::ResultField::default()
    }
}

/// 统计底层 Close / Finish / OnFetchReturned 调用次数。
#[derive(Default)]
struct Calls {
    close: AtomicUsize,
    finish: AtomicUsize,
    fetch_returned: AtomicUsize,
}

/// 可脚本化的 RecordSet 测试替身：按预置批次产出行，并可控 detach/close 错误。
struct ScriptedRecordSet {
    fields: Vec<sqlexec::resolve::ResultField>,
    /// 每次 Next 返回的一批 i64 值；空向量表示无更多行。
    batches: Vec<Vec<i64>>,
    next_batch: usize,
    detachable: bool,
    detach_error: bool,
    close_error: bool,
    calls: Arc<Calls>,
}

impl ScriptedRecordSet {
    fn new(batches: Vec<Vec<i64>>, detachable: bool, calls: Arc<Calls>) -> Self {
        Self {
            fields: vec![result_field(chunk::mysql::TypeLonglong)],
            batches,
            next_batch: 0,
            detachable,
            detach_error: false,
            close_error: false,
            calls,
        }
    }
}

impl sqlexec::RecordSet for ScriptedRecordSet {
    fn Fields(&self) -> &[sqlexec::resolve::ResultField] {
        &self.fields
    }

    fn Next(
        &mut self,
        _ctx: &sqlexec::context::Context,
        req: &mut sqlexec::RecordChunk,
    ) -> Result<(), sqlexec::GoError> {
        // 超出预置批次时返回空 chunk，表示 EOF。
        let batch = self
            .batches
            .get(self.next_batch)
            .cloned()
            .unwrap_or_default();
        self.next_batch += 1;
        req.with_chunk_mut(|chunk| {
            chunk.Reset();
            for value in batch {
                chunk.AppendInt64(0, value);
            }
        });
        Ok(())
    }

    fn NewChunk(&self, allocator: Option<&mut dyn chunk::Allocator>) -> sqlexec::RecordChunk {
        let fields = vec![chunk::types::NewFieldType(chunk::mysql::TypeLonglong)];
        match allocator {
            Some(allocator) => sqlexec::RecordChunk::from_allocated(allocator.Alloc(&fields, 2, 2)),
            None => sqlexec::RecordChunk::from_boxed(chunk::New(fields, 2, 2)),
        }
    }

    fn Close(&mut self) -> Result<(), sqlexec::GoError> {
        self.calls.close.fetch_add(1, Ordering::SeqCst);
        if self.close_error {
            Err(std::io::Error::other("close failed").into())
        } else {
            Ok(())
        }
    }

    fn Finish(&mut self) -> Result<(), sqlexec::GoError> {
        self.calls.finish.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn TryDetach(
        &mut self,
    ) -> Result<(Option<Box<dyn sqlexec::RecordSet>>, bool), sqlexec::GoError> {
        if self.detach_error {
            return Err(std::io::Error::other("detach failed").into());
        }
        if !self.detachable {
            return Ok((None, false));
        }
        // 分离出剩余未消费批次作为新 RecordSet。
        Ok((
            Some(Box::new(ScriptedRecordSet {
                fields: self.fields.clone(),
                batches: self.batches[self.next_batch..].to_vec(),
                next_batch: 0,
                detachable: false,
                detach_error: false,
                close_error: self.close_error,
                calls: Arc::clone(&self.calls),
            })),
            true,
        ))
    }

    fn OnFetchReturned(&mut self) {
        self.calls.fetch_returned.fetch_add(1, Ordering::SeqCst);
    }
}

/// 列信息应写入预编译缓存，且 NewChunk/Next 能读到脚本化批次。
#[test]
fn tidb_result_set_uses_real_fields_chunks_and_prepared_column_cache() {
    let calls = Arc::new(Calls::default());
    let prepared: PreparedStmtRef = Arc::new(astersql_planner_core::PlanCacheStmt::default());
    let mut result_set = New(
        Box::new(ScriptedRecordSet::new(vec![vec![7, 8]], false, calls)),
        Some(Arc::clone(&prepared)),
    );

    let columns = result_set.Columns();
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].Type, chunk::mysql::TypeLonglong);
    assert!(Arc::ptr_eq(
        &columns[0],
        &prepared.CachedColumnInfos().unwrap()[0]
    ));
    assert_eq!(
        result_set.FieldTypes()[0].GetType(),
        chunk::mysql::TypeLonglong
    );

    let mut request = result_set.NewChunk(None);
    result_set
        .Next(&sqlexec::context::Context::new(), &mut request)
        .unwrap();
    request.with_chunk(|chunk| {
        assert_eq!(chunk.NumRows(), 2);
        assert_eq!(chunk.GetRow(0).GetInt64(0), 7);
        assert_eq!(chunk.GetRow(1).GetInt64(0), 8);
    });
}

/// Finish/OnFetchReturned/TryDetach 应转发；Close 幂等且各实例各计一次。
#[test]
fn lifecycle_forwards_finish_detach_fetch_and_closes_once() {
    let calls = Arc::new(Calls::default());
    let prepared: PreparedStmtRef = Arc::new(astersql_planner_core::PlanCacheStmt::default());
    let mut result_set = New(
        Box::new(ScriptedRecordSet::new(
            vec![vec![11]],
            true,
            Arc::clone(&calls),
        )),
        Some(Arc::clone(&prepared)),
    );

    result_set.Finish().unwrap();
    result_set.OnFetchReturned();
    let (mut detached, ok) = result_set.TryDetach().unwrap();
    assert!(ok);
    let mut detached = detached.take().expect("detached result set");
    let detached_columns = detached.Columns();
    assert!(Arc::ptr_eq(
        &detached_columns[0],
        &prepared.CachedColumnInfos().unwrap()[0]
    ));

    result_set.Close();
    result_set.Close();
    assert!(result_set.IsClosed());
    detached.Close();
    assert_eq!(calls.finish.load(Ordering::SeqCst), 1);
    assert_eq!(calls.fetch_returned.load(Ordering::SeqCst), 1);
    assert_eq!(calls.close.load(Ordering::SeqCst), 2);
}

/// finish_lock 被占用时 Finish 应跳过，不调用底层 Finish。
#[test]
fn finish_try_lock_skips_busy_lifecycle_without_calling_underlying_finish() {
    let calls = Arc::new(Calls::default());
    let mut result_set = TidbResultSet::new(
        Box::new(ScriptedRecordSet::new(vec![], false, Arc::clone(&calls))),
        None,
    );
    let finish_lock = result_set.FinishLockForTest();
    let _busy = finish_lock.lock().unwrap();

    result_set.Finish().unwrap();
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
}

/// Close 幂等；底层 Close 出错也应只触发一次且标记已关闭。
#[test]
fn close_is_idempotent_and_ignores_record_set_close_error() {
    let calls = Arc::new(Calls::default());
    let mut record_set = ScriptedRecordSet::new(vec![], false, Arc::clone(&calls));
    record_set.close_error = true;
    let mut result_set = New(Box::new(record_set), None);

    result_set.Close();
    result_set.Close();
    assert!(result_set.IsClosed());
    assert_eq!(calls.close.load(Ordering::SeqCst), 1);
}

/// 不可分离返回 (None,false)；detach 错误应原样向上传递。
#[test]
fn detach_false_and_error_are_preserved() {
    let calls = Arc::new(Calls::default());
    let mut not_detachable = New(
        Box::new(ScriptedRecordSet::new(vec![], false, Arc::clone(&calls))),
        None,
    );
    let (detached, ok) = not_detachable.TryDetach().unwrap();
    assert!(!ok);
    assert!(detached.is_none());

    let mut failing = ScriptedRecordSet::new(vec![], true, calls);
    failing.detach_error = true;
    let mut failing = New(Box::new(failing), None);
    let error = match failing.TryDetach() {
        Ok(_) => panic!("detach must fail"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "detach failed");
}

/// Lazy 游标应跨多个 chunk 顺序产出行，Close 关闭底层结果集。
#[test]
fn lazy_cursor_iterates_across_real_chunks_and_closes_result_set() {
    let calls = Arc::new(Calls::default());
    let result_set = New(
        Box::new(ScriptedRecordSet::new(
            vec![vec![1, 2], vec![3], vec![]],
            false,
            Arc::clone(&calls),
        )),
        None,
    );
    let mut cursor = WrapWithLazyCursor(result_set, 2, 2);
    let context = sqlexec::context::Context::new();
    let iterator = cursor.GetRowIterator();

    assert_eq!(iterator.Current(&context).GetInt64(0), 1);
    assert_eq!(iterator.Next(&context).GetInt64(0), 2);
    assert_eq!(iterator.Next(&context).GetInt64(0), 3);
    assert!(iterator.Next(&context).IsEmpty());
    assert!(iterator.Error().is_none());
    iterator.Close();
    assert_eq!(calls.close.load(Ordering::SeqCst), 1);
}

/// 记录 RU 上报调用的测试用 ConsumptionReporter。
#[derive(Default)]
struct Reporter {
    reports: Mutex<Vec<(String, f64, f64, f64)>>,
}

impl ConsumptionReporter for Reporter {
    type Consumption = ();

    fn report_consumption(&self, _resource_group_name: &str, _consumption: &Self::Consumption) {}

    fn report_ruv2_consumption(
        &self,
        resource_group_name: &str,
        tikv_ruv2: f64,
        tidb_ruv2: f64,
        tiflash_ruv2: f64,
    ) {
        self.reports.lock().unwrap().push((
            resource_group_name.to_owned(),
            tikv_ruv2,
            tidb_ruv2,
            tiflash_ruv2,
        ));
    }
}

/// 仅正增量触发上报；零/负的 chunk 增量不累计单元格。
#[test]
fn cursor_ruv2_tracker_reports_only_positive_deltas() {
    let reporter = Arc::new(Reporter::default());
    let reporter_boundary: Arc<dyn CursorRUV2Reporter> = reporter.clone();
    let metrics = Arc::new(RUV2Metrics::default());
    let details = Arc::new(tikvutil::RUDetails::default());
    let weights = RUV2Weights {
        RUScale: 1.0,
        ResultChunkCells: 2.0,
        ..RUV2Weights::default()
    };
    let tracker = NewCursorRUV2Tracker(
        Some(reporter_boundary),
        "rg1".to_owned(),
        Some(Arc::clone(&metrics)),
        Some(Arc::clone(&details)),
        weights,
    )
    .unwrap();
    let calls = Arc::new(Calls::default());
    let mut result_set = New(Box::new(ScriptedRecordSet::new(vec![], false, calls)), None);
    AttachCursorRUV2Tracker(result_set.as_mut(), Some(tracker));

    // 3 cells * 权重 2.0 = TiDB RU 6.0。
    ReportCursorRUV2Delta(result_set.as_mut(), 3);
    details.AddTiKVRUV2(5.0);
    // delta=0 / 负值不应再增加 ResultChunkCells。
    ReportCursorRUV2Delta(result_set.as_mut(), 0);
    ReportCursorRUV2Delta(result_set.as_mut(), -4);

    let reports = reporter.reports.lock().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0], ("rg1".to_owned(), 0.0, 6.0, 0.0));
    assert_eq!(reports[1], ("rg1".to_owned(), 5.0, 0.0, 0.0));
    assert_eq!(metrics.ResultChunkCells(), 3);
}
