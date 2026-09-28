// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

const _GO_DRAFT_ARCHIVE: &str = r####################"
#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables, unused_mut)]

// Parallel sort 测试，覆盖正常并行排序、随机 failpoint 和 ORDER BY 常量表达式回归。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - "context"
// - "fmt"
// - "math/rand"
// - "sort"
// - "sync"
// - "testing"
// -
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/tidb/pkg/executor/internal/exec"
// - "github.com/pingcap/tidb/pkg/executor/internal/testutil"
// - "github.com/pingcap/tidb/pkg/executor/sortexec"
// - "github.com/pingcap/tidb/pkg/expression"
// - "github.com/pingcap/tidb/pkg/testkit"
// - "github.com/pingcap/tidb/pkg/util/memory"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// Test is successful if there is no hang
// executeInFailpoint 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
pub fn execute_in_failpoint(t *testing.T, exe *sortexec.SortExec, hardLimit int64, tracker *memory.Tracker) {
	// context 表示请求生命周期、取消或异步边界；当前不创建真实运行时。
	tmpCtx := context.Background()
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	err := exe.Open(tmpCtx)
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.NoError(t, err)

	// goroutine/channel/sync 保留 Go 并发协作语义；Rust 接线时需映射为 async/channel 或线程同步。
	once := sync.Once{}
	chk := exec.NewFirstChunk(exe)
	for i := 0; i >= 0; i++ {
		// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
		err := exe.Next(tmpCtx, chk)
		if err != nil {
			once.Do(func() {
				// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
				err = exe.Close()
				// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
				require.Equal(t, nil, err)
			})
			break
		}
		if chk.NumRows() == 0 {
			break
		}

		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		if i == 10 && hardLimit > 0 {
			// Trigger the spill
			// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
			tracker.Consume(hardLimit)
			// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
			tracker.Consume(-hardLimit)
		}
	}
	once.Do(func() {
		// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
		err = exe.Close()
		// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
		require.Equal(t, nil, err)
	})
}

// parallelSortTest 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn parallel_sort_test(t *testing.T, ctx *mock.Context, exe *sortexec.SortExec, schema *expression.Schema, dataSource *testutil.MockDataSource, sortCase *testutil.SortCase) {
	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)

	if exe == nil {
		exe = buildSortExec(sortCase, dataSource)
	}
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	resultChunks := executeSortExecutor(t, exe, true)

	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	err := exe.Close()
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.NoError(t, err)
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// failpointTest 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn failpoint_test(t *testing.T, ctx *mock.Context, exe *sortexec.SortExec, sortCase *testutil.SortCase, dataSource *testutil.MockDataSource) {
	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	if exe == nil {
		exe = buildSortExec(sortCase, dataSource)
	}
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	executeInFailpoint(t, exe, 0, nil)
}

// TestParallelSort 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_parallel_sort() {
	ctx := mock.NewContext()
	rowNum := 30000
	nvd := 100 // we have two column and should ensure that nvd*nvd is less than rowNum.
	sortCase := &testutil.SortCase{Rows: rowNum, OrderByIdx: []int{0, 1}, Ndvs: []int{nvd, nvd}, Ctx: ctx}
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/SlowSomeWorkers", `return(true)`))
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/SlowSomeWorkers")
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort", `return(true)`))
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort")

	schema := expression.NewSchema(sortCase.Columns()...)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	for range 10 {
		parallelSortTest(t, ctx, nil, schema, dataSource, sortCase)
		parallelSortTest(t, ctx, exe, schema, dataSource, sortCase)
	}
}

// TestFailpoint 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_failpoint() {
	ctx := mock.NewContext()
	ctx.GetSessionVars().ExecutorConcurrency = sortexec.ResultChannelCapacity * 2
	rowNum := 65536
	sortCase := &testutil.SortCase{Rows: rowNum, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx}
	schema := expression.NewSchema(sortCase.Columns()...)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource := buildDataSource(sortCase, schema)
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/ParallelSortRandomFail", `return(true)`))
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/ParallelSortRandomFail")
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/SlowSomeWorkers", `return(true)`))
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/SlowSomeWorkers")
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort", `return(true)`))
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort")

	testNum := 30
	exe := buildSortExec(sortCase, dataSource)
	for range testNum {
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointTest(t, ctx, nil, sortCase, dataSource)
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointTest(t, ctx, exe, sortCase, dataSource)
	}
}

// TestIssue55344 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_issue55344() {
	// testkit SQL/mock store 调用；当前。
	store := testkit.CreateMockStore(t)
	// testkit SQL/mock store 调用；当前。
	tk := testkit.NewTestKit(t, store)
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("use test")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("set @@tidb_max_chunk_size=32")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("set @@tidb_init_chunk_size=1")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("drop table if exists t0;")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("CREATE TABLE t0(c0 BOOL);")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("INSERT INTO mysql.opt_rule_blacklist VALUES('predicate_push_down'),('column_prune');")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("ADMIN reload opt_rule_blacklist;")

	// Should not be panic
	// testkit SQL/mock store 调用；当前。
	tk.MustQuery("SELECT t0.c0 FROM t0 WHERE 0 ORDER BY -646041453 ASC;")

	// Test correctness
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("drop table if exists t1;")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("CREATE TABLE t1(c int);")
	valueNum := 1000
	insertedValues := make([]int, 0, valueNum)
	for range valueNum {
		// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
		insertedValues = append(insertedValues, rand.Intn(10000))
	}

	insertSQL := fmt.Sprintf("INSERT INTO t1 values (%d)", insertedValues[0])
	for i := 1; i < valueNum; i++ {
		insertSQL = fmt.Sprintf("%s, (%d)", insertSQL, insertedValues[i])
	}
	insertSQL += ";"

	// testkit SQL/mock store 调用；当前。
	tk.MustExec(insertSQL)
	sort.Ints(insertedValues)

	expectValue := fmt.Sprintf("%d", insertedValues[0])
	for i := 1; i < valueNum; i++ {
		expectValue = fmt.Sprintf("%s\n%d", expectValue, insertedValues[i])
	}

	// testkit SQL/mock store 调用；当前。
	result := tk.MustQuery("select c from t1 order by c, -646041453;")
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.Equal(t, expectValue, result.String())
	// testkit SQL/mock store 调用；当前。
	result = tk.MustQuery("select c from t1 order by -646041453, c;")
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.Equal(t, expectValue, result.String())
	// testkit SQL/mock store 调用；当前。
	result = tk.MustQuery("select c from t1 order by c, -646041453, c+1;")
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.Equal(t, expectValue, result.String())
	// testkit SQL/mock store 调用；当前。
	result = tk.MustQuery("select c from t1 order by c+1, -646041453, c;")
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.Equal(t, expectValue, result.String())

	// testkit SQL/mock store 调用；当前。
	tk.MustExec("delete from mysql.opt_rule_blacklist where name='column_prune' or name='predicate_push_down';")
	// testkit SQL/mock store 调用；当前。
	tk.MustExec("ADMIN reload opt_rule_blacklist;")
}
"####################;

use super::sort::{RowSource, VecRowSource};
use super::{DataChunk, Row, SortError, SortExec, SortKey, SortValue};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

fn collect_all(executor: &mut SortExec, chunk_size: usize) -> Result<Vec<Row>, SortError> {
    let mut rows = Vec::new();
    loop {
        let chunk = executor.Next(chunk_size)?;
        if chunk.rows.is_empty() {
            return Ok(rows);
        }
        rows.extend(chunk.rows);
    }
}

/// 两个 worker、两个乱序 chunk 时，并行排序应产出全局升序 1..4。
#[test]
fn parallel_sort_orders_across_worker_chunks() {
    // 分两个 chunk 模拟多路输入，迫使跨 worker 归并
    let source = VecRowSource::new(vec![
        DataChunk::new(vec![
            Row(vec![SortValue::Int(4)]),
            Row(vec![SortValue::Int(1)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(2)]),
        ]),
    ]);
    // concurrency=2 走并行路径；mem_limit=-1 表示不限制内存
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 2, 8, -1);
    let output = executor.Next(8).unwrap();
    assert_eq!(
        output
            .rows
            .iter()
            .map(|row| row.0[0].clone())
            .collect::<Vec<_>>(),
        vec![
            SortValue::Int(1),
            SortValue::Int(2),
            SortValue::Int(3),
            SortValue::Int(4)
        ]
    );
}

/// Go `TestParallelSort` 会复用同一个执行器；Close 后必须能再次完整执行。
#[test]
fn parallel_sort_can_be_closed_and_reopened_without_stale_results() {
    #[derive(Clone)]
    struct ReplaySource {
        template: Vec<DataChunk>,
        chunks: VecDeque<DataChunk>,
    }

    impl ReplaySource {
        fn new(chunks: Vec<DataChunk>) -> Self {
            Self {
                template: chunks.clone(),
                chunks: VecDeque::from(chunks),
            }
        }
    }

    impl RowSource for ReplaySource {
        fn open(&mut self) -> Result<(), SortError> {
            self.chunks = VecDeque::from(self.template.clone());
            Ok(())
        }

        fn next(&mut self) -> Result<Option<DataChunk>, SortError> {
            Ok(self.chunks.pop_front())
        }
    }

    let source = ReplaySource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(3), SortValue::Int(1)]),
        Row(vec![SortValue::Int(1), SortValue::Int(2)]),
        Row(vec![SortValue::Int(1), SortValue::Int(3)]),
        Row(vec![SortValue::Int(2), SortValue::Int(0)]),
    ])]);
    let mut executor = SortExec::new(
        Box::new(source),
        vec![SortKey::asc(0), SortKey::desc(1)],
        2,
        2,
        -1,
    );
    let expected = vec![
        Row(vec![SortValue::Int(1), SortValue::Int(3)]),
        Row(vec![SortValue::Int(1), SortValue::Int(2)]),
        Row(vec![SortValue::Int(2), SortValue::Int(0)]),
        Row(vec![SortValue::Int(3), SortValue::Int(1)]),
    ];

    for _ in 0..2 {
        assert_eq!(collect_all(&mut executor, 8).unwrap(), expected);
        executor.Close().unwrap();
    }
}

/// 对应 Go failpoint 测试的核心资源契约：子执行器报错时错误上传且仍可 Close。
#[test]
fn parallel_sort_propagates_child_error_and_closes_once() {
    struct FailingSource {
        close_count: Arc<Mutex<usize>>,
    }

    impl RowSource for FailingSource {
        fn next(&mut self) -> Result<Option<DataChunk>, SortError> {
            Err(SortError("injected parallel sort failure".into()))
        }

        fn close(&mut self) -> Result<(), SortError> {
            *self.close_count.lock().unwrap() += 1;
            Ok(())
        }
    }

    let close_count = Arc::new(Mutex::new(0));
    let source = FailingSource {
        close_count: close_count.clone(),
    };
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 2, 8, -1);

    assert_eq!(
        executor.Next(8).unwrap_err(),
        SortError("injected parallel sort failure".into())
    );
    executor.Close().unwrap();
    assert_eq!(*close_count.lock().unwrap(), 1);
}

/// Go issue 55344 验证常量 ORDER BY 项不改变有效键；越界列在 Rust 中等价为全 NULL 常量键。
#[test]
fn constant_ordering_items_do_not_change_parallel_sort_order() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(3)]),
        Row(vec![SortValue::Int(1)]),
        Row(vec![SortValue::Int(2)]),
    ])]);
    let mut executor = SortExec::new(
        Box::new(source),
        vec![SortKey::asc(99), SortKey::asc(0), SortKey::desc(98)],
        2,
        8,
        -1,
    );

    assert_eq!(
        collect_all(&mut executor, 8).unwrap(),
        vec![
            Row(vec![SortValue::Int(1)]),
            Row(vec![SortValue::Int(2)]),
            Row(vec![SortValue::Int(3)]),
        ]
    );
    executor.Close().unwrap();
}
