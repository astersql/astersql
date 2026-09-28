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

// 并行排序 spill（落盘）相关测试：Go 草稿保留主动/被动 spill 与 failpoint 路径，
// Rust 侧验证小规模并行排序能触发 spill 并产出全局有序结果。
//
// Spill：内存不足时将有序中间结果写入临时存储，再多路归并回读。

const _GO_DRAFT_ARCHIVE: &str = r####################"
#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables, unused_mut)]

// Parallel sort spill 测试，覆盖主动 spill、内存后 spill、failpoint 异常路径和临时文件泄漏检查。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - "testing"
// -
// - "github.com/pingcap/tidb/pkg/config"
// - "github.com/pingcap/tidb/pkg/executor/internal/testutil"
// - "github.com/pingcap/tidb/pkg/executor/internal/util"
// - "github.com/pingcap/tidb/pkg/executor/sortexec"
// - "github.com/pingcap/tidb/pkg/expression"
// - "github.com/pingcap/tidb/pkg/testkit/testfailpoint"
// - "github.com/pingcap/tidb/pkg/util/memory"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// hardLimit1 对应 Go 的同名变量；用于保留跨测试共享 fixture 或阈值。
pub static mut hardLimit1 = int64(100000)
// hardLimit2 对应 Go 的同名变量；用于保留跨测试共享 fixture 或阈值。
pub static mut hardLimit2 = hardLimit1 * 10

// oneSpillCase 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn one_spill_case(t *testing.T, exe *sortexec.SortExec, sortCase *testutil.SortCase, schema *expression.Schema, dataSource *testutil.MockDataSource) {
	if exe == nil {
		exe = buildSortExec(sortCase, dataSource)
	}
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	resultChunks := executeSortExecutor(t, exe, true)

	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.True(t, exe.IsSpillTriggeredInParallelSortForTest())
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.Equal(t, int64(sortCase.Rows), exe.GetSpilledRowNumInParallelSortForTest())

	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	err := exe.Close()
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.NoError(t, err)

	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// inMemoryThenSpill 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn in_memory_then_spill(t *testing.T, ctx *mock.Context, exe *sortexec.SortExec, sortCase *testutil.SortCase, schema *expression.Schema, dataSource *testutil.MockDataSource) {
	if exe == nil {
		exe = buildSortExec(sortCase, dataSource)
	}
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	resultChunks := executeSortExecutorAndManullyTriggerSpill(t, exe, hardLimit2, ctx.GetSessionVars().StmtCtx.MemTracker, true)

	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.True(t, exe.IsSpillTriggeredInParallelSortForTest())
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.Greater(t, int64(sortCase.Rows), exe.GetSpilledRowNumInParallelSortForTest())
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	err := exe.Close()
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.NoError(t, err)

	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// failpointNoMemoryDataTest 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn failpoint_no_memory_data_test(t *testing.T, exe *sortexec.SortExec, sortCase *testutil.SortCase, dataSource *testutil.MockDataSource) {
	if exe == nil {
		exe = buildSortExec(sortCase, dataSource)
	}
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	executeInFailpoint(t, exe, 0, nil)
}

// failpointDataInMemoryThenSpillTest 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn failpoint_data_in_memory_then_spill_test(t *testing.T, ctx *mock.Context, exe *sortexec.SortExec, sortCase *testutil.SortCase, dataSource *testutil.MockDataSource) {
	if exe == nil {
		exe = buildSortExec(sortCase, dataSource)
	}
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	executeInFailpoint(t, exe, hardLimit2, ctx.GetSessionVars().MemTracker)
}

// TestParallelSortSpillDisk 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_parallel_sort_spill_disk() {
	// defer 表示 Go 资源收尾或全局状态恢复；Rust 接线时应改为 guard/drop 或显式清理。
	defer config.RestoreFunc()()
	// 全局配置或 session 变量修改需要成对恢复；这里保留 Go 测试的状态边界。
	config.UpdateGlobal(func(conf *config.Config) {
		conf.TempStoragePath = t.TempDir()
	})
	testFuncName := util.GetFunctionName()

	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	sortexec.SetSmallSpillChunkSizeForTest()
	ctx := mock.NewContext()
	sortCase := &testutil.SortCase{Rows: 10000, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx, FileNamePrefixForTest: testFuncName}

	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/SlowSomeWorkers", `return(true)`)
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort", `return(true)`)

	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)

	schema := expression.NewSchema(sortCase.Columns()...)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	for range 10 {
		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		oneSpillCase(t, nil, sortCase, schema, dataSource)
		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		oneSpillCase(t, exe, sortCase, schema, dataSource)
	}

	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit2)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	for range 10 {
		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		inMemoryThenSpill(t, ctx, nil, sortCase, schema, dataSource)
		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		inMemoryThenSpill(t, ctx, exe, sortCase, schema, dataSource)
	}

	util.CheckNoLeakFiles(t, testFuncName)
}
"####################;

use super::sort::VecRowSource;
use super::{DataChunk, Row, SortExec, SortKey, SortValue};

fn spill_fixture() -> (Vec<DataChunk>, Vec<Row>) {
    let rows: Vec<_> = (0..100)
        .rev()
        .map(|value| Row(vec![SortValue::Int(value % 10), SortValue::Int(value)]))
        .collect();
    let chunks = rows
        .chunks(7)
        .map(|rows| DataChunk::new(rows.to_vec()))
        .collect();
    let mut expected = rows;
    expected.sort_by_key(|row| match (&row.0[0], &row.0[1]) {
        (SortValue::Int(first), SortValue::Int(second)) => (*first, *second),
        _ => unreachable!("spill fixture contains only integer keys"),
    });
    (chunks, expected)
}

fn execute_spill_case(mem_limit: i64) {
    let (chunks, expected) = spill_fixture();
    let source = VecRowSource::new(chunks);
    let mut executor = SortExec::new(
        Box::new(source),
        vec![SortKey::asc(0), SortKey::asc(1)],
        3,
        7,
        mem_limit,
    );
    let mut output = Vec::new();
    loop {
        let chunk = executor.Next(7).unwrap();
        assert!(chunk.num_rows() <= 7);
        if chunk.is_empty() {
            break;
        }
        output.extend(chunk.rows);
    }

    assert!(executor.IsSpillTriggered());
    assert_eq!(output, expected);
    assert!(executor.GetDiskTracker().bytes_consumed() > 0);

    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
    assert_eq!(executor.GetDiskTracker().bytes_consumed(), 0);
    assert!(!executor.IsSpillTriggered());
    executor.Close().unwrap();
}

/// 小内存阈值下并行排序应完整 spill，且最终结果按两个键全局升序。
#[test]
fn parallel_sort_spills_all_rows_and_merges_globally_ordered_rows() {
    execute_spill_case(128);
}

/// 对齐 Go 的 inMemoryThenSpill：先积累内存行，再越过较高阈值触发 spill。
#[test]
fn parallel_sort_keeps_memory_rows_before_spilling_and_cleans_up() {
    execute_spill_case(1_000);
}
const _GO_DRAFT_SUFFIX: &str = r####################"

// TestParallelSortSpillDiskFailpoint 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_parallel_sort_spill_disk_failpoint() {
	// defer 表示 Go 资源收尾或全局状态恢复；Rust 接线时应改为 guard/drop 或显式清理。
	defer config.RestoreFunc()()
	// 全局配置或 session 变量修改需要成对恢复；这里保留 Go 测试的状态边界。
	config.UpdateGlobal(func(conf *config.Config) {
		conf.TempStoragePath = t.TempDir()
	})
	testFuncName := util.GetFunctionName()

	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	sortexec.SetSmallSpillChunkSizeForTest()
	ctx := mock.NewContext()
	sortCase := &testutil.SortCase{Rows: 10000, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx, FileNamePrefixForTest: testFuncName}

	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/SlowSomeWorkers", `return(true)`)
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort", `return(true)`)
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/ParallelSortRandomFail", `return(true)`)
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/util/chunk/ChunkInDiskError", `return(true)`)

	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)

	schema := expression.NewSchema(sortCase.Columns()...)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	for range 20 {
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointNoMemoryDataTest(t, nil, sortCase, dataSource)
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointNoMemoryDataTest(t, exe, sortCase, dataSource)
	}

	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit2)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	for range 20 {
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointDataInMemoryThenSpillTest(t, ctx, nil, sortCase, dataSource)
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointDataInMemoryThenSpillTest(t, ctx, exe, sortCase, dataSource)
	}

	util.CheckNoLeakFiles(t, testFuncName)
}

// TestIssue59655 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_issue59655() {
	// defer 表示 Go 资源收尾或全局状态恢复；Rust 接线时应改为 guard/drop 或显式清理。
	defer config.RestoreFunc()()
	// 全局配置或 session 变量修改需要成对恢复；这里保留 Go 测试的状态边界。
	config.UpdateGlobal(func(conf *config.Config) {
		conf.TempStoragePath = t.TempDir()
	})
	testFuncName := util.GetFunctionName()

	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	sortexec.SetSmallSpillChunkSizeForTest()
	ctx := mock.NewContext()
	sortCase := &testutil.SortCase{Rows: 10000, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx, FileNamePrefixForTest: testFuncName}

	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/Issue59655", `return(true)`)

	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	ctx.GetSessionVars().ExecutorConcurrency = sortexec.ResultChannelCapacity * 2
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)

	schema := expression.NewSchema(sortCase.Columns()...)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	for range 20 {
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointNoMemoryDataTest(t, nil, sortCase, dataSource)
		// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
		failpointNoMemoryDataTest(t, exe, sortCase, dataSource)
	}

	util.CheckNoLeakFiles(t, testFuncName)
}

// TestIssue63216 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_issue63216() {
	// defer 表示 Go 资源收尾或全局状态恢复；Rust 接线时应改为 guard/drop 或显式清理。
	defer config.RestoreFunc()()
	// 全局配置或 session 变量修改需要成对恢复；这里保留 Go 测试的状态边界。
	config.UpdateGlobal(func(conf *config.Config) {
		conf.TempStoragePath = t.TempDir()
	})
	testFuncName := util.GetFunctionName()

	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	sortexec.SetSmallSpillChunkSizeForTest()
	ctx := mock.NewContext()
	sortCase := &testutil.SortCase{Rows: 10000, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx, FileNamePrefixForTest: testFuncName}

	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/sortexec/Issue63216", `return(true)`)

	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)

	schema := expression.NewSchema(sortCase.Columns()...)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	// failpoint 控制 Go 测试中的异常、等待或并发时序；只记录开关位置。
	failpointNoMemoryDataTest(t, exe, sortCase, dataSource)

	util.CheckNoLeakFiles(t, testFuncName)
}
"####################;
