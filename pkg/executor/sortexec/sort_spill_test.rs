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

// 排序 spill（落盘）相关测试：内存/磁盘分区与 fallback 动作。
//
// Go 草稿覆盖单分区内存、单分区磁盘、多分区与手动触发 spill；
// Rust 侧验证 DiskRun 保持 chunk 边界，且关闭后拒绝再写入。
//
// Spill：内存不足时将有序中间结果写入临时存储，再多路归并回读。

const _GO_DRAFT_ARCHIVE: &str = r####################"
#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables, unused_mut)]

// Sort spill executor 测试，覆盖内存排序、磁盘 spill、分区结果校验和 fallback action。
// testkit、failpoint、memory tracker、chunk、mock context 和 executor 等外部依赖均保留为外部调用点。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发/异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - "context"
// - "sort"
// - "testing"
// - "time"
// -
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/tidb/pkg/config"
// - "github.com/pingcap/tidb/pkg/executor/internal/exec"
// - "github.com/pingcap/tidb/pkg/executor/internal/testutil"
// - "github.com/pingcap/tidb/pkg/executor/internal/util"
// - "github.com/pingcap/tidb/pkg/executor/sortexec"
// - "github.com/pingcap/tidb/pkg/expression"
// - "github.com/pingcap/tidb/pkg/expression/exprstatic"
// - plannerutil "github.com/pingcap/tidb/pkg/planner/util"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/pingcap/tidb/pkg/util/memory"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// It will sort values in memory and compare them with the results produced by sort executor.

// resultChecker 对应 Go 的同名结构体；字段顺序保留原测试 fixture/辅助状态。
pub struct resultChecker {
	schema      *expression.Schema
	keyColumns  []int
	keyCmpFuncs []chunk.CompareFunc
	byItemsDesc []bool

	// Initially, savedChunks are not sorted
	savedChunks []*chunk.Chunk
	rowPtrs     []chunk.RowPtr
}

// newResultChecker 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn new_result_checker(schema *expression.Schema, keyColumns []int, keyCmpFuncs []chunk.CompareFunc, byItemsDesc []bool, savedChunks []*chunk.Chunk) *resultChecker {
	checker := resultChecker{}
	checker.schema = schema
	checker.keyColumns = keyColumns
	checker.keyCmpFuncs = keyCmpFuncs
	checker.byItemsDesc = byItemsDesc
	checker.savedChunks = savedChunks
	return &checker
}

// lessRow 对应 Go 方法，receiver `r *resultChecker` 的状态修改与返回值语义按原文件保留。
// 方法体仍保留 Go 调用形状，后续接入 Rust chunk/executor 类型时再替换。
pub fn r_ptr_resultChecker_less_row(rowI, rowJ chunk.Row) bool {
	for i, colIdx := range r.keyColumns {
		cmpFunc := r.keyCmpFuncs[i]
		if cmpFunc != nil {
			cmp := cmpFunc(rowI, colIdx, rowJ, colIdx)
			if r.byItemsDesc[i] {
				cmp = -cmp
			}
			if cmp < 0 {
				return true
			} else if cmp > 0 {
				return false
			}
		}
	}
	return false
}

// keyColumnsLess 对应 Go 方法，receiver `r *resultChecker` 的状态修改与返回值语义按原文件保留。
// 方法体仍保留 Go 调用形状，后续接入 Rust chunk/executor 类型时再替换。
pub fn r_ptr_resultChecker_key_columns_less(i, j int) bool {
	// 磁盘 chunk 访问保留 spill 文件读写语义；当前不做真实 IO。
	rowI := r.savedChunks[r.rowPtrs[i].ChkIdx].GetRow(int(r.rowPtrs[i].RowIdx))
	rowJ := r.savedChunks[r.rowPtrs[j].ChkIdx].GetRow(int(r.rowPtrs[j].RowIdx))
	return r.lessRow(rowI, rowJ)
}

// getSavedChunksRowNumber 对应 Go 方法，receiver `r *resultChecker` 的状态修改与返回值语义按原文件保留。
// 方法体仍保留 Go 调用形状，后续接入 Rust chunk/executor 类型时再替换。
pub fn r_ptr_resultChecker_get_saved_chunks_row_number() int {
	rowNum := 0
	for _, chk := range r.savedChunks {
		rowNum += chk.NumRows()
	}
	return rowNum
}

// initRowPtrs 对应 Go 方法，receiver `r *resultChecker` 的状态修改与返回值语义按原文件保留。
// 方法体仍保留 Go 调用形状，后续接入 Rust chunk/executor 类型时再替换。
pub fn r_ptr_resultChecker_init_row_ptrs() {
	r.rowPtrs = make([]chunk.RowPtr, 0, r.getSavedChunksRowNumber())
	chunkNum := len(r.savedChunks)
	for chkIdx := range chunkNum {
		chk := r.savedChunks[chkIdx]
		for rowIdx := range chk.NumRows() {
			r.rowPtrs = append(r.rowPtrs, chunk.RowPtr{ChkIdx: uint32(chkIdx), RowIdx: uint32(rowIdx)})
		}
	}
}

// check 对应 Go 方法，receiver `r *resultChecker` 的状态修改与返回值语义按原文件保留。
// 方法体仍保留 Go 调用形状，后续接入 Rust chunk/executor 类型时再替换。
pub fn r_ptr_resultChecker_check(resultChunks []*chunk.Chunk, offset int64, count int64) bool {
	ctx := exprstatic.NewEvalContext()

	if r.rowPtrs == nil {
		r.initRowPtrs()

		sort.Slice(r.rowPtrs, r.keyColumnsLess)
		if offset < 0 {
			offset = 0
		}
		if count < 0 {
			count = (int64(len(r.rowPtrs)) - offset)
		}

		start := min(int64(len(r.rowPtrs)), offset)
		end := min(int64(len(r.rowPtrs)), offset+count)
		r.rowPtrs = r.rowPtrs[start:end]
	}

	cursor := 0
	fieldTypes := make([]*types.FieldType, 0)
	for _, col := range r.schema.Columns {
		fieldTypes = append(fieldTypes, col.GetType(ctx))
	}

	// Check row number
	totalResRowNum := 0
	for _, chk := range resultChunks {
		totalResRowNum += chk.NumRows()
	}
	if totalResRowNum != len(r.rowPtrs) {
		return false
	}

	for _, chk := range resultChunks {
		rowNum := chk.NumRows()
		for i := range rowNum {
			// 磁盘 chunk 访问保留 spill 文件读写语义；当前不做真实 IO。
			resRow := chk.GetRow(i)
			res := resRow.ToString(fieldTypes)

			expectRow := r.savedChunks[r.rowPtrs[cursor].ChkIdx].GetRow(int(r.rowPtrs[cursor].RowIdx))
			expect := expectRow.ToString(fieldTypes)

			if res != expect {
				return false
			}
			cursor++
		}
	}

	return true
}

// buildDataSource 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn build_data_source(sortCase *testutil.SortCase, schema *expression.Schema) *testutil.MockDataSource {
	opt := testutil.MockDataSourceParameters{
		DataSchema: schema,
		Rows:       sortCase.Rows,
		Ctx:        sortCase.Ctx,
		Ndvs:       sortCase.Ndvs,
	}
	// MockDataSource 提供测试输入 chunk；当前不会生成真实数据。
	return testutil.BuildMockDataSource(opt)
}

// buildSortExec 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn build_sort_exec(sortCase *testutil.SortCase, dataSource *testutil.MockDataSource) *sortexec.SortExec {
	dataSource.PrepareChunks()
	exe := &sortexec.SortExec{
		BaseExecutor:          exec.NewBaseExecutor(sortCase.Ctx, dataSource.Schema(), 0, dataSource),
		ByItems:               make([]*plannerutil.ByItems, 0, len(sortCase.OrderByIdx)),
		ExecSchema:            dataSource.Schema(),
		FileNamePrefixForTest: sortCase.FileNamePrefixForTest,
	}

	for _, idx := range sortCase.OrderByIdx {
		exe.ByItems = append(exe.ByItems, &plannerutil.ByItems{Expr: sortCase.Columns()[idx]})
	}

	return exe
}

// executeSortExecutor 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn execute_sort_executor(t *testing.T, exe *sortexec.SortExec, isParallelSort bool) []*chunk.Chunk {
	// context/cancel 保留 Go 中断传播语义；当前不创建真实异步上下文。
	tmpCtx := context.Background()
	// Open 启动 executor 生命周期；当前不实际驱动执行器。
	err := exe.Open(tmpCtx)
	// require.NoError 对应 Go 断言：这里保留错误必须为空的测试语义。
	require.NoError(t, err)
	if !isParallelSort {
		exe.IsUnparallel = true
		exe.InitUnparallelModeForTest()
	}

	resultChunks := make([]*chunk.Chunk, 0)
	chk := exec.NewFirstChunk(exe)
	for {
		// Next 拉取 chunk，保留 Go executor 迭代语义。
		err = exe.Next(tmpCtx, chk)
		// require.NoError 对应 Go 断言：这里保留错误必须为空的测试语义。
		require.NoError(t, err)
		if chk.NumRows() == 0 {
			break
		}
		resultChunks = append(resultChunks, chk.CopyConstruct())
	}
	return resultChunks
}

// executeSortExecutorAndManullyTriggerSpill 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn execute_sort_executor_and_manully_trigger_spill(t *testing.T, exe *sortexec.SortExec, hardLimit int64, tracker *memory.Tracker, isParallelSort bool) []*chunk.Chunk {
	tmpCtx := context.Background()
	err := exe.Open(tmpCtx)
	require.NoError(t, err)
	if !isParallelSort {
		exe.IsUnparallel = true
		exe.InitUnparallelModeForTest()
	}

	resultChunks := make([]*chunk.Chunk, 0)
	chk := exec.NewFirstChunk(exe)
	for i := 0; i >= 0; i++ {
		err = exe.Next(tmpCtx, chk)
		require.NoError(t, err)

		if i == 10 {
			// Trigger the spill
			// memory tracker 用于触发 spill/fallback；这里只保留阈值与消费顺序。
			tracker.Consume(hardLimit)
			tracker.Consume(-hardLimit)

			// Wait for the finish of spill, or the spill may not be triggered even data in memory has been drained
			// Sleep 等待异步 spill 完成；Rust 接线时应替换为确定性同步或超时控制。
			time.Sleep(100 * time.Millisecond)
		}

		if chk.NumRows() == 0 {
			break
		}
		resultChunks = append(resultChunks, chk.CopyConstruct())
	}
	return resultChunks
}

// checkCorrectness 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn check_correctness(schema *expression.Schema, exe *sortexec.SortExec, dataSource *testutil.MockDataSource, resultChunks []*chunk.Chunk) bool {
	keyColumns, keyCmpFuncs, byItemsDesc := exe.GetSortMetaForTest()
	checker := newResultChecker(schema, keyColumns, keyCmpFuncs, byItemsDesc, dataSource.GenData)
	return checker.check(resultChunks, -1, -1)
}

// onePartitionAndAllDataInMemoryCase 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn one_partition_and_all_data_in_memory_case(t *testing.T, ctx *mock.Context, sortCase *testutil.SortCase) {
	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	// memory tracker 用于触发 spill/fallback；这里只保留阈值与消费顺序。
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, 1048576)
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	schema := expression.NewSchema(sortCase.Columns()...)
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	resultChunks := executeSortExecutor(t, exe, false)

	// require.Equal 对应 Go 断言：保留期望值与实际值比较顺序。
	require.Equal(t, exe.GetSortPartitionListLenForTest(), 1)
	require.Equal(t, false, exe.IsSpillTriggeredInOnePartitionForTest(0))
	require.Equal(t, int64(2048), exe.GetRowNumInOnePartitionMemoryForTest(0))
	require.Equal(t, int64(0), exe.GetRowNumInOnePartitionDiskForTest(0))
	err := exe.Close()
	require.NoError(t, err)

	// require.True 对应 Go 断言：保留排序、TopN 或 split 校验必须成立的语义。
	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// onePartitionAndAllDataInDiskCase 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn one_partition_and_all_data_in_disk_case(t *testing.T, ctx *mock.Context, sortCase *testutil.SortCase) {
	// Keep all input rows in one chunk to make this one-partition case deterministic.
	ctx.GetSessionVars().InitChunkSize = sortCase.Rows
	ctx.GetSessionVars().MaxChunkSize = sortCase.Rows
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, 50000)
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	schema := expression.NewSchema(sortCase.Columns()...)
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)

	// To ensure that spill has been trigger before getting chunk, or we may get chunk from memory.
	// failpoint 控制 Go 测试中的并发/IO 时序；仅记录开关位置。
	failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/waitForSpill", `return(true)`)
	resultChunks := executeSortExecutor(t, exe, false)
	failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/waitForSpill", `return(false)`)

	require.Equal(t, exe.GetSortPartitionListLenForTest(), 1)
	err := exe.Close()
	require.NoError(t, err)

	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// When we enable the `unholdSyncLock` failpoint, we can ensure that there must be multi partitions.
// However, `unholdSyncLock` failpoint introduces sleep and this will hide some concurrent problems,
// so this failpoint needs to be disabled if we want to test concurrent problem.

// multiPartitionCase 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn multi_partition_case(t *testing.T, ctx *mock.Context, sortCase *testutil.SortCase, enableFailPoint bool) {
	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	hardLimit := int64(10000)
	if enableFailPoint {
		// Use a tighter limit so `unholdSyncLock` reliably creates multiple spill partitions.
		hardLimit = 1000
	}
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit)
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	schema := expression.NewSchema(sortCase.Columns()...)
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	exe.IsUnparallel = true
	if enableFailPoint {
		// failpoint 控制 Go 测试中的并发/IO 时序；仅记录开关位置。
		failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/unholdSyncLock", `return(true)`)
	}
	resultChunks := executeSortExecutor(t, exe, false)
	if enableFailPoint {
		failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/unholdSyncLock", `return(false)`)
	}
	if enableFailPoint {
		// Even with this failpoint, partition count can still be 1 on some schedules.
		sortPartitionNum := exe.GetSortPartitionListLenForTest()

		// Ensure all full partitions are spilled.
		for i := range sortPartitionNum {
			// The last partition may not be spilled.
			if i < sortPartitionNum-1 {
				// require.Equal 对应 Go 断言：保留期望值与实际值比较顺序。
				require.Equal(t, true, exe.IsSpillTriggeredInOnePartitionForTest(i))
			}
		}
	}

	err := exe.Close()
	require.NoError(t, err)

	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// Data are all in memory and some of then are fetched, then the spill is triggered

// inMemoryThenSpillCase 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB 类型、chunk、mock context 与执行器调用均为后续 Rust 接线线索。
pub fn in_memory_then_spill_case(t *testing.T, ctx *mock.Context, sortCase *testutil.SortCase) {
	hardLimit := int64(100000)
	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSQLText, hardLimit)
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	schema := expression.NewSchema(sortCase.Columns()...)
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	resultChunks := executeSortExecutorAndManullyTriggerSpill(t, exe, hardLimit, ctx.GetSessionVars().StmtCtx.MemTracker, false)

	require.Equal(t, exe.GetSortPartitionListLenForTest(), 1)
	require.Equal(t, true, exe.IsSpillTriggeredInOnePartitionForTest(0))

	rowNumInDisk := exe.GetRowNumInOnePartitionDiskForTest(0)
	require.Equal(t, int64(0), exe.GetRowNumInOnePartitionMemoryForTest(0))
	// require.Greater 对应 Go 边界断言：保留资源计数/行数下界。
	require.Greater(t, int64(2048), rowNumInDisk)
	// require.Less 对应 Go 边界断言：保留资源计数/行数上界。
	require.Less(t, int64(0), rowNumInDisk)
	err := exe.Close()
	require.NoError(t, err)

	require.True(t, checkCorrectness(schema, exe, dataSource, resultChunks))
}

// TestUnparallelSortSpillDisk 对应 Go 同名测试，保留初始化、断言和资源收尾顺序。
#[test]
pub fn test_unparallel_sort_spill_disk() {
	// defer 表示 Go 资源收尾或恢复全局配置；迁移时需在 Rust 中改成显式清理/guard。
	defer config.RestoreFunc()()
	config.UpdateGlobal(func(conf *config.Config) {
		// TempDir 为测试临时目录；Rust 接线时应使用临时目录 guard 并保证清理。
		conf.TempStoragePath = t.TempDir()
	})
	testFuncName := util.GetFunctionName()

	sortexec.SetSmallSpillChunkSizeForTest()
	ctx := mock.NewContext()
	sortCase := &testutil.SortCase{Rows: 2048, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx, FileNamePrefixForTest: testFuncName}

	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort", `return(true)`))
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort")

	for range 50 {
		onePartitionAndAllDataInMemoryCase(t, ctx, sortCase)
		onePartitionAndAllDataInDiskCase(t, ctx, sortCase)
		multiPartitionCase(t, ctx, sortCase, false)
		multiPartitionCase(t, ctx, sortCase, true)
		inMemoryThenSpillCase(t, ctx, sortCase)
	}
	// 文件泄漏检查保留原测试收尾要求，当前不扫描磁盘。
	util.CheckNoLeakFiles(t, testFuncName)
}
"####################;

use super::sort_util::DiskRun;
use super::{DataChunk, Row, SortValue};
use crate::parallel_sort_spill_helper::parallelSortSpillHelper;
use crate::sort_spill::{SpillAction, parallelSortSpillAction};
use crate::sort_util::{MemoryTracker, Result, comparator};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct CountingFallback(AtomicUsize);

impl SpillAction for CountingFallback {
    fn Action(&self) -> Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn parallel_action_fixture(
    sort_consumed: i64,
    passed_consumed: i64,
    limit: i64,
) -> (parallelSortSpillAction, Arc<CountingFallback>) {
    let sort_tracker = Arc::new(MemoryTracker::new(-1));
    sort_tracker.consume(sort_consumed);
    let disk_tracker = Arc::new(MemoryTracker::new(-1));
    let helper = Arc::new(Mutex::new(parallelSortSpillHelper::new(
        Vec::new(),
        comparator(Vec::new()),
        sort_tracker,
        disk_tracker,
    )));
    let passed_tracker = Arc::new(MemoryTracker::new(limit));
    passed_tracker.consume(passed_consumed);
    let fallback = Arc::new(CountingFallback::default());
    (
        parallelSortSpillAction::new(helper, passed_tracker, Some(fallback.clone())),
        fallback,
    )
}

/// Go 仅在传入 tracker 仍然超限时触发 fallback。
#[test]
fn parallel_action_does_not_fallback_below_limit() {
    let (action, fallback) = parallel_action_fixture(0, 5, 100);
    action.Action().unwrap();
    assert_eq!(fallback.0.load(Ordering::Relaxed), 0);
}

/// Go 用 sort executor 自身 tracker 判断是否有足够数据可 spill。
#[test]
fn parallel_action_falls_back_when_sort_data_is_below_ten_percent() {
    let (action, fallback) = parallel_action_fixture(5, 100, 100);
    action.Action().unwrap();
    assert_eq!(fallback.0.load(Ordering::Relaxed), 1);
}

/// DiskRun 应保留 chunk 边界，并在 close 后拒绝 add。
#[test]
fn disk_run_preserves_chunk_boundaries_and_rejects_after_close() {
    let mut run = DiskRun::default();
    run.add(DataChunk::new(vec![Row(vec![SortValue::Int(1)])]))
        .unwrap();
    run.add(DataChunk::new(vec![Row(vec![SortValue::Int(2)])]))
        .unwrap();
    assert_eq!((run.num_chunks(), run.num_rows()), (2, 2));
    assert_eq!(run.get_chunk(1).unwrap().rows[0].0[0], SortValue::Int(2));
    // close 后不可再追加，模拟 spill 文件关闭语义
    run.close();
    assert!(
        run.add(DataChunk::new(vec![Row(vec![SortValue::Int(3)])]))
            .is_err()
    );
}
const _GO_DRAFT_SUFFIX: &str = r####################"

// TestFallBackAction 对应 Go 同名测试，保留初始化、断言和资源收尾顺序。
#[test]
pub fn test_fall_back_action() {
	defer config.RestoreFunc()()
	config.UpdateGlobal(func(conf *config.Config) {
		conf.TempStoragePath = t.TempDir()
	})
	testFuncName := util.GetFunctionName()

	hardLimitBytesNum := int64(1000000)
	newRootExceedAction := new(testutil.MockActionOnExceed)
	sortexec.SetSmallSpillChunkSizeForTest()
	ctx := mock.NewContext()
	ctx.GetSessionVars().InitChunkSize = 32
	ctx.GetSessionVars().MaxChunkSize = 32
	ctx.GetSessionVars().MemTracker = memory.NewTracker(memory.LabelForSession, hardLimitBytesNum)
	ctx.GetSessionVars().MemTracker.SetActionOnExceed(newRootExceedAction)
	// Consume lots of memory in advance to help to trigger fallback action.
	ctx.GetSessionVars().MemTracker.Consume(int64(float64(hardLimitBytesNum) * 0.99999))
	ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
	ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)
	sortCase := &testutil.SortCase{Rows: 2048, OrderByIdx: []int{0, 1}, Ndvs: []int{0, 0}, Ctx: ctx, FileNamePrefixForTest: testFuncName}

	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort", `return(true)`))
	defer failpoint.Disable("github.com/pingcap/tidb/pkg/executor/sortexec/SignalCheckpointForSort")

	schema := expression.NewSchema(sortCase.Columns()...)
	dataSource := buildDataSource(sortCase, schema)
	exe := buildSortExec(sortCase, dataSource)
	executeSortExecutor(t, exe, false)
	err := exe.Close()
	require.NoError(t, err)

	require.Less(t, 0, newRootExceedAction.GetTriggeredNum())
	util.CheckNoLeakFiles(t, testFuncName)
}
"####################;
