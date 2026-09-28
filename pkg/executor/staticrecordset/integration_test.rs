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

// staticrecordset 集成测试：校验 RecordSet 对执行器 Next/Close 的转发与资源收尾。

use std::sync::{Arc, Mutex};

use astersql_executor_internal_exec::executor::{
    Chunk, Error, ExecContext, Executor, FieldType, RUV2Metrics, Schema,
};

use crate::{ChunkAllocator, CursorHandle, New, RecordContext, RecordSet, ResultField};

struct TestExecutor {
    events: Arc<Mutex<Vec<&'static str>>>,
    fields: Vec<FieldType>,
    init_cap: usize,
    max_chunk_size: usize,
    next_rows: usize,
    next_error: Option<Error>,
    close_error: Option<Error>,
    panic_on_next: bool,
    seen_metrics: Arc<Mutex<Option<Arc<RUV2Metrics>>>>,
}

impl TestExecutor {
    fn new(events: Arc<Mutex<Vec<&'static str>>>, fields: Vec<FieldType>) -> Self {
        Self {
            events,
            fields,
            init_cap: 1,
            max_chunk_size: 32,
            next_rows: 0,
            next_error: None,
            close_error: None,
            panic_on_next: false,
            seen_metrics: Arc::new(Mutex::new(None)),
        }
    }

    fn record(&self, event: &'static str) {
        self.events.lock().unwrap().push(event);
    }
}

impl Executor for TestExecutor {
    fn executorType(&self) -> &'static str {
        "*executor.PointGetExecutor"
    }

    fn Open(&mut self, _: &ExecContext) -> Result<(), Error> {
        self.record("executor.open");
        Ok(())
    }

    fn Next(&mut self, ctx: &ExecContext, req: &mut Chunk) -> Result<(), Error> {
        self.record("executor.next");
        *self.seen_metrics.lock().unwrap() = ctx.metrics.clone();
        if self.panic_on_next {
            panic!("staticrecordset test panic");
        }
        if let Some(error) = &self.next_error {
            return Err(error.clone());
        }
        req.SetNumRows(self.next_rows);
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        self.record("executor.close");
        self.close_error.clone().map_or(Ok(()), Err)
    }

    fn Schema(&self) -> Schema {
        Schema {
            fields: self.fields.clone(),
        }
    }

    fn RetFieldTypes(&self) -> Vec<FieldType> {
        self.fields.clone()
    }

    fn InitCap(&self) -> usize {
        self.init_cap
    }

    fn MaxChunkSize(&self) -> usize {
        self.max_chunk_size
    }

    fn AllChildren(&self) -> &[Box<dyn Executor>] {
        &[]
    }

    fn SetAllChildren(&mut self, children: Vec<Box<dyn Executor>>) {
        assert!(children.is_empty());
    }
}

struct TestCursor {
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl CursorHandle for TestCursor {
    fn Close(&mut self) {
        self.events.lock().unwrap().push("cursor.close");
    }
}

struct RecordingAllocator {
    request: Arc<Mutex<Option<(usize, usize, usize)>>>,
}

impl ChunkAllocator for RecordingAllocator {
    fn Alloc(&self, fields: &[FieldType], capacity: usize, max_size: usize) -> Chunk {
        *self.request.lock().unwrap() = Some((fields.len(), capacity, max_size));
        Chunk::new(fields, capacity, max_size)
    }
}

fn fields() -> Vec<ResultField> {
    vec![ResultField { name: "id".into() }]
}

fn executor(events: Arc<Mutex<Vec<&'static str>>>, rows: usize) -> TestExecutor {
    let mut executor = TestExecutor::new(events, vec![FieldType { type_code: 3 }]);
    executor.next_rows = rows;
    executor
}

/// 单行执行器经 New 包装后：Next 返回 1 行，Close 可重复调用且不 panic。
#[test]
fn canonical_static_recordset_forwards_next_and_closes_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut recordset = New(
        fields(),
        Box::new(executor(events, 1)),
        "select 1".into(),
        None,
    );
    let mut chunk = recordset.NewChunk(None);
    // 第一次 Next 应填入一行
    recordset
        .Next(&RecordContext::default(), &mut chunk)
        .unwrap();
    assert_eq!(chunk.NumRows(), 1);
    // Close 应幂等：连续两次均成功
    recordset.Close().unwrap();
    recordset.Close().unwrap();
}

/// 对应 Go TestStaticRecordSet：字段元信息和一次批量 Next 的行数均保留。
#[test]
fn static_recordset_fields_and_batch_rows_match_go() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = executor(events, 3);
    executor.max_chunk_size = 3;
    let mut recordset = New(fields(), Box::new(executor), "select * from t".into(), None);

    assert_eq!(recordset.Fields(), fields());
    let mut chunk = recordset.NewChunk(None);
    recordset
        .Next(&RecordContext::default(), &mut chunk)
        .unwrap();
    assert_eq!(chunk.NumRows(), 3);
}

/// 对应 Go TestStaticRecordSetWithTxn：结果集持有执行器状态，不依赖调用方后续上下文。
#[test]
fn static_recordset_keeps_rows_after_context_replacement() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut recordset = New(
        fields(),
        Box::new(executor(events, 3)),
        "select * from t".into(),
        Some(RecordContext::default()),
    );
    let mut chunk = recordset.NewChunk(None);
    recordset
        .Next(&RecordContext::default(), &mut chunk)
        .unwrap();
    assert_eq!(chunk.NumRows(), 3);
}

/// 对应 Go TestStaticRecordSetExceedGCTime：Next 的快照/存储错误原样返回，并可收尾。
#[test]
fn static_recordset_propagates_snapshot_error_and_closes() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = executor(events, 0);
    executor.next_error = Some(Error::Other("GC safe point exceeded".into()));
    let mut recordset = New(fields(), Box::new(executor), "select * from t".into(), None);
    let mut chunk = recordset.NewChunk(None);

    assert_eq!(
        recordset.Next(&RecordContext::default(), &mut chunk),
        Err(Error::Other("GC safe point exceeded".into()))
    );
    recordset.Close().unwrap();
}

/// 对应 Go 的 NewChunk allocator 分支：字段数、初始容量和最大容量均传给分配器。
#[test]
fn static_recordset_new_chunk_uses_supplied_allocator() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = executor(events, 0);
    executor.init_cap = 4;
    executor.max_chunk_size = 64;
    let mut recordset = New(fields(), Box::new(executor), "select 1".into(), None);
    let request = Arc::new(Mutex::new(None));
    let allocator = RecordingAllocator {
        request: request.clone(),
    };

    let chunk = recordset.NewChunk(Some(&allocator));
    assert_eq!(chunk.NumCols(), 1);
    assert_eq!(*request.lock().unwrap(), Some((1, 4, 64)));
}

/// 对应 Go TestCursorWillBeClosed：游标先关闭，底层 RecordSet 再关闭。
#[test]
fn cursor_recordset_forwards_fetch_and_closes_cursor_first() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recordset = New(
        fields(),
        Box::new(executor(events.clone(), 2)),
        "select * from t".into(),
        None,
    );
    let mut recordset = crate::WrapRecordSetWithCursor(
        Box::new(TestCursor {
            events: events.clone(),
        }),
        recordset,
    );

    assert_eq!(recordset.Fields(), fields());
    let mut chunk = recordset.NewChunk(None);
    recordset
        .Next(&RecordContext::default(), &mut chunk)
        .unwrap();
    assert_eq!(chunk.NumRows(), 2);
    assert!(recordset.GetExecutor4Test().is_some());
    recordset.Close().unwrap();
    assert_eq!(
        events.lock().unwrap().as_slice(),
        &["executor.next", "cursor.close", "executor.close"]
    );
}

/// 对应游标关闭错误路径：底层 RecordSet 的错误是唯一对外返回值。
#[test]
fn cursor_recordset_returns_wrapped_close_error() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = executor(events.clone(), 0);
    executor.close_error = Some(Error::Other("close failed".into()));
    let recordset = New(fields(), Box::new(executor), "select 1".into(), None);
    let mut recordset = crate::WrapRecordSetWithCursor(Box::new(TestCursor { events }), recordset);

    assert_eq!(recordset.Close(), Err(Error::Other("close failed".into())));
}

/// 对应 Go 的 panic recover 分支：执行器 panic 在 RecordSet 边界被转换为错误。
#[test]
fn static_recordset_converts_next_panic_to_error() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = executor(events, 1);
    executor.panic_on_next = true;
    let mut recordset = New(fields(), Box::new(executor), "select 1".into(), None);
    let mut chunk = recordset.NewChunk(None);

    assert_eq!(
        recordset.Next(&RecordContext::default(), &mut chunk),
        Err(Error::Panic)
    );
}

/// 对应 sourceCtx 的 RU v2 继承：调用上下文没有 metrics 时仍使用来源 metrics。
#[test]
fn static_recordset_inherits_source_metrics() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let executor = executor(events, 1);
    let seen_metrics = executor.seen_metrics.clone();
    let metrics = Arc::new(RUV2Metrics::default());
    let source_context = RecordContext::default().withMetrics(metrics.clone());
    let mut recordset = New(
        fields(),
        Box::new(executor),
        "select 1".into(),
        Some(source_context),
    );
    let mut chunk = recordset.NewChunk(None);

    recordset
        .Next(&RecordContext::default(), &mut chunk)
        .unwrap();
    let inherited = seen_metrics.lock().unwrap().clone().unwrap();
    assert!(Arc::ptr_eq(&inherited, &metrics));
}
