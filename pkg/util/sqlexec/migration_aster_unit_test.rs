// Copyright 2026 AsterSQL.
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

// sqlexec 迁移单元测试：ExecOption、SimpleRecordSet、Drain 与 ExecSQL 行为对照。
//
// 验证选项应用顺序、chunk 分批、Close 重置、以及 Close 错误不掩盖 Drain 结果。

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// 构造指定 FieldType 的 ResultField，供结果集测试使用。
fn result_field(tp: u8) -> resolve::ResultField {
    let mut column = resolve::model::ColumnInfo::default();
    column.FieldType = *types::NewFieldType(tp);
    resolve::ResultField {
        column: Some(Rc::new(column)),
        ..Default::default()
    }
}

/// 将任意值迭代器收集为一行。
fn row(values: impl IntoIterator<Item = Box<dyn Any>>) -> Vec<Box<dyn Any>> {
    values.into_iter().collect()
}

/// 选项按 Go 顺序叠加，后写覆盖先写；回调可被取出执行。
#[test]
fn options_apply_in_go_order_and_keep_callbacks() {
    let untracked = Arc::new(AtomicUsize::new(0));
    let untracked_by_callback = Arc::clone(&untracked);
    let option = GetExecOption(vec![
        Box::new(ExecOptionIgnoreWarning),
        Box::new(ExecOptionEnableDDLAnalyze),
        Box::new(ExecOptionAnalyzeVer2),
        GetPartitionPruneModeOption("dynamic".to_owned()),
        GetAnalyzeSnapshotOption(false),
        Box::new(ExecOptionUseCurSession),
        Box::new(ExecOptionUseSessionPool),
        ExecOptionWithSnapshot(42),
        ExecOptionWithSysProcTrack(
            99,
            Box::new(|_, _| Ok(())),
            Box::new(move |id| {
                untracked_by_callback.store(id as usize, Ordering::SeqCst);
            }),
        ),
    ]);

    assert!(option.IgnoreWarning);
    assert!(option.EnableDDLAnalyze);
    assert_eq!(option.AnalyzeVer, 2);
    assert_eq!(option.PartitionPruneMode, "dynamic");
    assert_eq!(option.AnalyzeSnapshot, Some(false));
    assert!(!option.UseCurSession, "the later pool option must win");
    assert_eq!(option.SnapshotTS, 42);
    assert_eq!(option.TrackSysProcID, 99);
    assert!(option.TrackSysProc.is_some());
    option.UnTrackSysProc.expect("untrack callback")(99);
    assert_eq!(untracked.load(Ordering::SeqCst), 99);
}

/// SimpleRecordSet 按 MaxChunkSize 分批，Close 后可重新从头读取。
#[test]
fn simple_record_set_chunks_rows_and_close_restarts() {
    let fields = vec![
        result_field(chunk::mysql::TypeLonglong),
        result_field(chunk::mysql::TypeVarString),
    ];
    let rows = vec![
        row([Box::new(1_i64) as Box<dyn Any>, Box::new("one".to_owned())]),
        row([Box::new(2_i64) as Box<dyn Any>, Box::new("two".to_owned())]),
        row([
            Box::new(3_i64) as Box<dyn Any>,
            Box::new("three".to_owned()),
        ]),
    ];
    let mut record_set = SimpleRecordSet::new(fields, rows, 2);
    let ctx = context::Context::new();
    let mut req = RecordSet::NewChunk(&record_set, None);

    RecordSet::Next(&mut record_set, &ctx, &mut req).unwrap();
    req.with_chunk(|chunk| {
        assert_eq!(chunk.NumRows(), 2);
        assert_eq!(chunk.GetRow(0).GetInt64(0), 1);
        assert_eq!(chunk.GetRow(1).GetString(1), "two");
    });

    RecordSet::Next(&mut record_set, &ctx, &mut req).unwrap();
    req.with_chunk(|chunk| {
        assert_eq!(chunk.NumRows(), 1);
        assert_eq!(chunk.GetRow(0).GetInt64(0), 3);
    });

    RecordSet::Next(&mut record_set, &ctx, &mut req).unwrap();
    assert_eq!(req.NumRows(), 0);
    assert_eq!(RecordSet::Fields(&record_set).len(), 2);

    RecordSet::Close(&mut record_set).unwrap();
    RecordSet::Next(&mut record_set, &ctx, &mut req).unwrap();
    req.with_chunk(|chunk| assert_eq!(chunk.GetRow(0).GetInt64(0), 1));
}

/// NewChunk 传入 Allocator 时应得到 Allocated 变体。
#[test]
fn simple_record_set_uses_the_supplied_chunk_allocator() {
    let fields = vec![result_field(chunk::mysql::TypeLonglong)];
    let rows = vec![row([Box::new(8_i64) as Box<dyn Any>])];
    let mut record_set = SimpleRecordSet::new(fields, rows, 4);
    let mut allocator = chunk::NewAllocator();
    let mut req = RecordSet::NewChunk(&record_set, Some(allocator.as_mut()));

    assert!(req.is_allocated());
    RecordSet::Next(&mut record_set, &context::Context::new(), &mut req).unwrap();
    req.with_chunk(|chunk| assert_eq!(chunk.GetRow(0).GetInt64(0), 8));
}

/// 可脚本化的测试用 RecordSet：按批次吐行、可注入 Next/Close 失败。
struct ScriptedRecordSet {
    fields: Vec<resolve::ResultField>,
    batches: Vec<Vec<i64>>,
    next_call: usize,
    fail_at: Option<usize>,
    close_error: bool,
    close_count: Arc<AtomicUsize>,
}

impl ScriptedRecordSet {
    fn new(batches: Vec<Vec<i64>>, close_count: Arc<AtomicUsize>) -> Self {
        Self {
            fields: vec![result_field(chunk::mysql::TypeLonglong)],
            batches,
            next_call: 0,
            fail_at: None,
            close_error: false,
            close_count,
        }
    }
}

impl RecordSet for ScriptedRecordSet {
    fn Fields(&self) -> &[resolve::ResultField] {
        &self.fields
    }

    fn Next(&mut self, _ctx: &context::Context, req: &mut RecordChunk) -> Result<(), GoError> {
        // 在指定调用序号注入失败，模拟 Drain 中途错误。
        if self.fail_at == Some(self.next_call) {
            self.next_call += 1;
            return Err(std::io::Error::other("scripted next failure").into());
        }
        let values = self
            .batches
            .get(self.next_call)
            .cloned()
            .unwrap_or_default();
        self.next_call += 1;
        req.with_chunk_mut(|chunk| {
            chunk.Reset();
            for value in values {
                chunk.AppendInt64(0, value);
            }
        });
        Ok(())
    }

    fn NewChunk(&self, allocator: Option<&mut dyn chunk::Allocator>) -> RecordChunk {
        let fields = vec![chunk::types::NewFieldType(chunk::mysql::TypeLonglong)];
        match allocator {
            Some(allocator) => RecordChunk::from_allocated(allocator.Alloc(&fields, 0, 2)),
            None => RecordChunk::from_boxed(chunk::New(fields, 2, 2)),
        }
    }

    fn Close(&mut self) -> Result<(), GoError> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        if self.close_error {
            Err(std::io::Error::other("scripted close failure").into())
        } else {
            self.next_call = 0;
            Ok(())
        }
    }
}

/// Drain 失败时保留已读行，且 AndClose 路径仍会调用 Close。
#[test]
fn drain_preserves_partial_rows_and_close_runs_on_error() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut record_set = ScriptedRecordSet::new(vec![vec![7]], Arc::clone(&close_count));
    record_set.fail_at = Some(1);

    let error = DrainRecordSetAndClose(&context::Context::new(), &mut record_set, 2)
        .expect_err("second Next must fail");

    assert_eq!(error.rows.len(), 1);
    assert_eq!(error.rows[0].GetInt64(0), 7);
    assert_eq!(error.source.to_string(), "scripted next failure");
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}

/// Close 失败只记日志，不把成功的 Drain 结果替换为错误。
#[test]
fn drain_and_close_does_not_replace_success_with_a_close_error() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut record_set = ScriptedRecordSet::new(vec![vec![21]], Arc::clone(&close_count));
    record_set.close_error = true;

    let rows = DrainRecordSetAndClose(&context::Context::new(), &mut record_set, 2)
        .expect("close errors are logged without replacing the drain result");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].GetInt64(0), 21);
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}

/// 测试用 SQLExecutor：返回预设的下一个结果集。
struct MockExecutor {
    next: Option<Box<dyn RecordSet>>,
}

impl SQLExecutor for MockExecutor {
    fn Execute(
        &mut self,
        _ctx: &context::Context,
        _sql: &str,
    ) -> Result<Vec<Box<dyn RecordSet>>, GoError> {
        Ok(Vec::new())
    }

    fn ExecuteInternal(
        &mut self,
        _ctx: &context::Context,
        _sql: &str,
        _args: Vec<Box<dyn Any>>,
    ) -> Result<Option<Box<dyn RecordSet>>, GoError> {
        Ok(self.next.take())
    }

    fn ExecuteStmt(
        &mut self,
        _ctx: &context::Context,
        _stmt_node: ast::NodeRef,
    ) -> Result<Option<Box<dyn RecordSet>>, GoError> {
        Ok(self.next.take())
    }
}

/// ExecSQL：nil 结果返回 None；非空则 Drain 并 Close。
#[test]
fn exec_sql_handles_nil_result_and_closes_non_nil_result() {
    let ctx = context::Context::new();
    let mut empty_executor = MockExecutor { next: None };
    assert!(
        ExecSQL(&ctx, &mut empty_executor, "set x = 1", Vec::new())
            .unwrap()
            .is_none()
    );

    let close_count = Arc::new(AtomicUsize::new(0));
    let record_set = ScriptedRecordSet::new(vec![vec![11], vec![12]], Arc::clone(&close_count));
    let mut executor = MockExecutor {
        next: Some(Box::new(record_set)),
    };
    let rows = ExecSQL(&ctx, &mut executor, "select x", Vec::new())
        .unwrap()
        .expect("record set rows");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].GetInt64(0), 11);
    assert_eq!(rows[1].GetInt64(0), 12);
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}
