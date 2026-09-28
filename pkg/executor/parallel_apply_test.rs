// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Parallel Nested Loop Apply（并行相关子查询应用）构造期与内存跟踪的单元测试。
//
// 校验 worker 数必须为正，以及内存跟踪器初始字节数为 0。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_errors as errors;
use astersql_types::field::NewFieldType;
use astersql_util_chunk as chunk;

use crate::parallel_apply::{
    ApplyCorrelatedColumn, ApplyExpression, ParallelApplyCancellation, ParallelApplyExecutor,
    ParallelApplyJoiner, ParallelApplyMemoryTracker, ParallelApplyRuntimeContext,
    ParallelApplyRuntimeStats, ParallelNestedLoopApplyExec,
};

/// 空取消实现：测试中不需要真正中止。
#[derive(Default)]
struct Cancellation;
impl ParallelApplyCancellation for Cancellation {
    fn Cancel(&self) {}
}

/// 空执行器桩：Open/Close/Next 均为空操作，用于构造路径测试。
struct EmptyExecutor;
impl ParallelApplyExecutor for EmptyExecutor {
    fn Open(&mut self, _: Arc<dyn ParallelApplyRuntimeContext>) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn Close(&mut self) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn Cancellation(&self) -> Arc<dyn ParallelApplyCancellation> {
        Arc::new(Cancellation)
    }
    fn Next(
        &mut self,
        _: Arc<dyn ParallelApplyRuntimeContext>,
        request: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        request.Reset();
        Ok(())
    }
    fn NewFirstChunk(&mut self) -> chunk::Chunk {
        chunk::Chunk::default()
    }
    fn TryNewCacheChunk(&mut self) -> chunk::Chunk {
        chunk::Chunk::default()
    }
    fn HasRuntimeStats(&self) -> bool {
        true
    }
}

/// concurrency=0 应 panic；默认内存跟踪器消耗字节数为 0。
#[test]
fn parallel_apply_rejects_zero_workers_and_starts_with_zero_memory() {
    // worker 数为 0 违反构造不变量，catch_unwind 捕获断言失败。
    let panic = std::panic::catch_unwind(|| {
        ParallelNestedLoopApplyExec::new(
            Box::new(EmptyExecutor),
            Box::new(EmptyExecutor),
            Vec::new(),
            false,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            0,
            true,
            true,
        )
    });
    assert!(panic.is_err());
    assert_eq!(ParallelApplyMemoryTracker::default().BytesConsumed(), 0);
}

fn int_chunk(columns: usize, values: &[i64]) -> chunk::Chunk {
    let mut output = *chunk::New((0..columns).map(|_| NewFieldType(8)).collect(), 1, 2);
    for value in values {
        output.AppendInt64(0, *value);
    }
    output
}

#[derive(Default)]
struct TestCancellation(AtomicBool);

impl ParallelApplyCancellation for TestCancellation {
    fn Cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

struct RowsExecutor {
    rows: Vec<i64>,
    emitted: bool,
    columns: usize,
    opens: Arc<AtomicUsize>,
    cancellation: Arc<TestCancellation>,
}

impl RowsExecutor {
    fn new(rows: Vec<i64>, columns: usize, opens: Arc<AtomicUsize>) -> Self {
        Self {
            rows,
            emitted: false,
            columns,
            opens,
            cancellation: Arc::new(TestCancellation::default()),
        }
    }
}

impl ParallelApplyExecutor for RowsExecutor {
    fn Open(&mut self, _: Arc<dyn ParallelApplyRuntimeContext>) -> Result<(), errors::SharedError> {
        self.emitted = false;
        self.opens.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn Cancellation(&self) -> Arc<dyn ParallelApplyCancellation> {
        self.cancellation.clone()
    }

    fn Next(
        &mut self,
        _: Arc<dyn ParallelApplyRuntimeContext>,
        request: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        request.Reset();
        if !self.emitted {
            for value in &self.rows {
                request.AppendInt64(0, *value);
            }
            self.emitted = true;
        }
        Ok(())
    }

    fn NewFirstChunk(&mut self) -> chunk::Chunk {
        int_chunk(self.columns, &[])
    }

    fn TryNewCacheChunk(&mut self) -> chunk::Chunk {
        int_chunk(self.columns, &[])
    }

    fn HasRuntimeStats(&self) -> bool {
        true
    }
}

struct PairJoiner;

impl ParallelApplyJoiner for PairJoiner {
    fn TryToMatchInners(
        &mut self,
        outer: &chunk::Row,
        inners: &[chunk::Row],
        cursor: &mut usize,
        output: &mut chunk::Chunk,
    ) -> Result<(bool, bool), errors::SharedError> {
        while *cursor < inners.len() && !output.IsFull() {
            output.AppendInt64(0, outer.GetInt64(0));
            output.AppendInt64(1, inners[*cursor].GetInt64(0));
            *cursor += 1;
        }
        Ok((!inners.is_empty(), false))
    }

    fn OnMissMatch(&mut self, _: bool, outer: &chunk::Row, output: &mut chunk::Chunk) {
        output.AppendInt64(0, outer.GetInt64(0));
        output.AppendNull(1);
    }
}

#[derive(Default)]
struct TestRuntime {
    attached: AtomicUsize,
    detached: AtomicUsize,
    stats: Mutex<VecDeque<ParallelApplyRuntimeStats>>,
}

impl ParallelApplyRuntimeContext for TestRuntime {
    fn VectorizedFilterOuter(
        &self,
        input: &chunk::Chunk,
        _: &[ApplyExpression],
        mut reuse: Vec<bool>,
    ) -> Result<Vec<bool>, errors::SharedError> {
        reuse.clear();
        reuse.resize(input.NumRows(), true);
        Ok(reuse)
    }

    fn VectorizedFilterInner(
        &self,
        _: usize,
        input: &chunk::Chunk,
        _: &[ApplyExpression],
        mut reuse: Vec<bool>,
    ) -> Result<Vec<bool>, errors::SharedError> {
        reuse.clear();
        reuse.resize(input.NumRows(), true);
        Ok(reuse)
    }

    fn BindCorrelatedColumns(
        &self,
        _: usize,
        _: &chunk::Row,
        _: &[ApplyCorrelatedColumn],
    ) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn EncodeCorrelatedKey(
        &self,
        _: usize,
        outer: &chunk::Row,
        _: &[ApplyCorrelatedColumn],
    ) -> Result<Vec<u8>, errors::SharedError> {
        Ok(outer.GetInt64(0).to_le_bytes().to_vec())
    }

    fn InitializeApplyCache(&self) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn ApplyCacheGet(&self, _: &[u8]) -> Result<Option<Vec<chunk::Chunk>>, errors::SharedError> {
        Ok(None)
    }
    fn ApplyCacheSet(&self, _: Vec<u8>, _: Vec<chunk::Chunk>) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn AttachMemoryTracker(&self, _: Arc<ParallelApplyMemoryTracker>) {
        self.attached.fetch_add(1, Ordering::AcqRel);
    }
    fn DetachMemoryTracker(&self, _: Arc<ParallelApplyMemoryTracker>) {
        self.detached.fetch_add(1, Ordering::AcqRel);
    }
    fn RegisterRuntimeStats(&self, stats: ParallelApplyRuntimeStats) {
        self.stats.lock().unwrap().push_back(stats);
    }
    fn LogInnerCloseError(&self, _: usize, _: &errors::SharedError) {}
    fn TriggerOuterWorkerFailpoint(&self) {}
    fn TriggerInnerWorkerFailpoint(&self, _: usize) {}
    fn TriggerInnerWorkerOrderedFailpoint(&self, _: usize) {}
    fn TriggerOrderedSleepFailpoint(&self, _: usize) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn TriggerCacheGetFailpoint(&self) {}
    fn TriggerCacheSetFailpoint(&self) {}
    fn TriggerSlowInnerFailpoint(&self, _: usize) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

#[test]
fn ordered_parallel_apply_preserves_outer_order_and_closes_runtime_state() {
    let outer_opens = Arc::new(AtomicUsize::new(0));
    let inner_opens = Arc::new(AtomicUsize::new(0));
    let base_opens = Arc::new(AtomicUsize::new(0));
    let mut apply = ParallelNestedLoopApplyExec::new(
        Box::new(RowsExecutor::new(Vec::new(), 2, base_opens)),
        Box::new(RowsExecutor::new(vec![3, 1, 2], 1, outer_opens.clone())),
        Vec::new(),
        false,
        vec![Vec::new(), Vec::new()],
        vec![Vec::new(), Vec::new()],
        vec![
            Box::new(RowsExecutor::new(vec![10], 1, inner_opens.clone())),
            Box::new(RowsExecutor::new(vec![10], 1, inner_opens.clone())),
        ],
        vec![Box::new(PairJoiner), Box::new(PairJoiner)],
        2,
        true,
        false,
    );
    let runtime = Arc::new(TestRuntime::default());
    apply.Open(runtime.clone()).unwrap();

    let mut rows = Vec::new();
    loop {
        let mut output = int_chunk(2, &[]);
        apply.Next(runtime.clone(), &mut output).unwrap();
        if output.NumRows() == 0 {
            break;
        }
        for index in 0..output.NumRows() {
            rows.push((
                output.GetRow(index).GetInt64(0),
                output.GetRow(index).GetInt64(1),
            ));
        }
    }
    apply.Close().unwrap();

    assert_eq!(rows, vec![(3, 10), (1, 10), (2, 10)]);
    assert_eq!(outer_opens.load(Ordering::Acquire), 1);
    assert_eq!(inner_opens.load(Ordering::Acquire), 3);
    assert_eq!(runtime.attached.load(Ordering::Acquire), 1);
    assert_eq!(runtime.detached.load(Ordering::Acquire), 1);
    let stats = runtime.stats.lock().unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].concurrency, 2);
    assert!(!stats[0].cache_enabled);
    assert_eq!(stats[0].cache_hit_ratio, 0.0);
    assert_eq!(stats[0].memory_bytes, 0);
}
