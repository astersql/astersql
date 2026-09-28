// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// RankTopN 相关测试：前缀键截断与并列边界行为。
//
// Go 草稿覆盖字符串前缀 key、collation（校对规则）与 TopN offset/count；
// Rust 侧验证在 LIMIT 边界处并列（tied）行全部保留。
//
// RankTopN：按排序前缀判定“排名”，同秩行在截断边界处一并返回。

const _GO_DRAFT_ARCHIVE: &str = r####################"
#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables, unused_mut)]

// RankTopN 测试，覆盖字符串前缀截断 key、collation、TopN offset/count 和结果正确性检查。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - crand "crypto/rand"
// - "encoding/base64"
// - "fmt"
// - "math/rand"
// - "testing"
// -
// - "github.com/pingcap/tidb/pkg/executor/internal/exec"
// - "github.com/pingcap/tidb/pkg/executor/internal/testutil"
// - "github.com/pingcap/tidb/pkg/executor/sortexec"
// - "github.com/pingcap/tidb/pkg/expression"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/planner/core/operator/physicalop"
// - plannerutil "github.com/pingcap/tidb/pkg/planner/util"
// - "github.com/pingcap/tidb/pkg/sessionctx"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/pingcap/tidb/pkg/util/memory"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// rankTopNCase is the sort case for RankTopN tests with string prefix key
// rankTopNCase 对应 Go 同名结构体；字段顺序保留测试 fixture 或辅助状态。
pub struct rankTopNCase {
	ctx                         sessionctx.Context
	rowCount                    int
	cols                        []*expression.Column
	orderByIdx                  []int
	truncateKeyExprs            []expression.Expression
	truncateKeyPrefixCharCounts []int
}

// buildRankTopNDataSource 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn build_rank_top_n_data_source(rankTopNCase *rankTopNCase, schema *expression.Schema) *testutil.MockDataSource {
	// Generate prefix-ordered data: rows are ordered by the first column (prefix key)
	opt := testutil.MockDataSourceParameters{
		DataSchema: schema,
		Rows:       rankTopNCase.rowCount,
		Ctx:        rankTopNCase.ctx,
		Ndvs:       make([]int, len(rankTopNCase.truncateKeyExprs)+1),
		Datums:     make([][]any, len(rankTopNCase.truncateKeyExprs)+1),
	}

	for i := range rankTopNCase.truncateKeyExprs {
		// -2 means use provided data
		opt.Ndvs[i] = -2
	}

	outputs := make([]string, rankTopNCase.rowCount)

	opt.Ndvs[len(rankTopNCase.truncateKeyExprs)] = 0
	// Generate prefix key data: strings that are pre-ordered.
	// Each prefix group has multiple rows; the group size is variable and each group
	// size is randomized in [1, 200].
	for i, ft := range rankTopNCase.truncateKeyExprs {
		prefixData := make([]any, rankTopNCase.rowCount)
		groupIdx := 0
		var bufLen int
		if rankTopNCase.truncateKeyPrefixCharCounts[i] == -1 {
			bufLen = 0
		} else {
			bufLen = 5
		}
		buf := make([]byte, bufLen)
		isCI := false
		switch ft.GetType(nil).GetCollate() {
		case "utf8mb4_bin":
		case "utf8mb4_general_ci":
			isCI = true
		default:
			panic(fmt.Sprintf("Unconsidered collator %s", ft.GetType(nil).GetCollate()))
		}

		for i := 0; i < rankTopNCase.rowCount; {
			// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
			groupSize := rand.Intn(200) + 1
			for j := 0; j < groupSize && i < rankTopNCase.rowCount; j++ {
				// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
				_, err := crand.Read(buf)
				if err != nil {
					// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
					panic("rand.Read returns error")
				}
				// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
				if isCI && rand.Intn(10) < 5 {
					// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
					prefixData[i] = fmt.Sprintf("PREFIX前缀_%05d_%s", groupIdx, base64.RawURLEncoding.EncodeToString(buf))
				} else {
					// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
					prefixData[i] = fmt.Sprintf("prefix前缀_%05d_%s", groupIdx, base64.RawURLEncoding.EncodeToString(buf))
				}
				outputs[i] = fmt.Sprintf("%s, %s", outputs[i], prefixData[i])
				i++
			}
			groupIdx++
		}
		opt.Datums[i] = prefixData
	}

	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	return testutil.BuildMockDataSource(opt)
}

// buildRankTopNExec 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn build_rank_top_n_exec(rankTopNCase *rankTopNCase, dataSource *testutil.MockDataSource, offset uint64, count uint64) *sortexec.TopNExec {
	sortExec := sortexec.SortExec{
		BaseExecutor: exec.NewBaseExecutor(rankTopNCase.ctx, dataSource.Schema(), 0, dataSource),
		ByItems:      make([]*plannerutil.ByItems, 0, len(rankTopNCase.orderByIdx)),
		ExecSchema:   dataSource.Schema(),
	}

	for _, idx := range rankTopNCase.orderByIdx {
		sortExec.ByItems = append(sortExec.ByItems, &plannerutil.ByItems{Expr: rankTopNCase.cols[idx]})
	}

	topNexec := &sortexec.TopNExec{
		SortExec:    sortExec,
		Limit:       &physicalop.PhysicalLimit{Offset: offset, Count: count},
		Concurrency: 5,
	}

	topNexec.SetTruncateKeyMetasForTest(rankTopNCase.truncateKeyExprs, rankTopNCase.truncateKeyPrefixCharCounts)
	return topNexec
}

// checkRankTopNCorrectness 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn check_rank_top_n_correctness(schema *expression.Schema, exe *sortexec.TopNExec, dataSource *testutil.MockDataSource, resultChunks []*chunk.Chunk, offset uint64, count uint64) bool {
	keyColumns, keyCmpFuncs, byItemsDesc := exe.GetSortMetaForTest()
	checker := newResultChecker(schema, keyColumns, keyCmpFuncs, byItemsDesc, dataSource.GenData)
	return checker.check(resultChunks, int64(offset), int64(count))
}

// rankTopNBasicCase 对应 Go 同名辅助函数，保留参数、返回值和主要控制流。
// 外部 TiDB executor、chunk、mock context 与 failpoint 调用均为后续 Rust 接线线索。
pub fn rank_top_n_basic_case(t *testing.T, sortCase *rankTopNCase, schema *expression.Schema, dataSource *testutil.MockDataSource, offset uint64, count uint64) {
	exe := buildRankTopNExec(sortCase, dataSource, offset, count)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	dataSource.PrepareChunks()
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	resultChunks := executeTopNExecutor(t, exe)
	// mock datasource/executor 调用保留测试数据准备、Open/Next/Close 消费顺序。
	err := exe.Close()
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.NoError(t, err)
	// require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
	require.True(t, checkRankTopNCorrectness(schema, exe, dataSource, resultChunks, offset, count))
}

// TestRankTopN 对应 Go 同名测试，保留初始化、fixture、断言和资源收尾顺序。
#[test]
pub fn test_rank_top_n() {
	collationNames := []string{"utf8mb4_bin", "utf8mb4_general_ci"}
	ctx := mock.NewContext()
	rankTopNCases := make([]*rankTopNCase, 0)
	for _, collationName := range collationNames {
		truncateKeyField := types.NewFieldType(mysql.TypeVarString)
		truncateKeyField.SetCharset("utf8mb4")
		truncateKeyField.SetCollate(collationName)
		rankTopNCases = append(rankTopNCases, &rankTopNCase{
			// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
			rowCount:   rand.Intn(9000) + 1000,
			ctx:        ctx,
			orderByIdx: []int{0}, // Order by truncate key column
			truncateKeyExprs: []expression.Expression{
				&expression.Column{
					RetType: truncateKeyField,
					Index:   0,
				}},
			truncateKeyPrefixCharCounts: []int{14},
			cols: []*expression.Column{
				{Index: 0, RetType: truncateKeyField},
				{Index: 1, RetType: types.NewFieldType(mysql.TypeLonglong)}},
		})
		rankTopNCases = append(rankTopNCases, &rankTopNCase{
			// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
			rowCount:   rand.Intn(9000) + 1000,
			ctx:        ctx,
			orderByIdx: []int{0, 1}, // Order by truncate key column
			truncateKeyExprs: []expression.Expression{
				&expression.Column{
					RetType: truncateKeyField,
					Index:   0,
				},
				&expression.Column{
					RetType: truncateKeyField,
					Index:   1,
				}},
			truncateKeyPrefixCharCounts: []int{-1, 12},
			cols: []*expression.Column{
				{Index: 0, RetType: truncateKeyField},
				{Index: 1, RetType: truncateKeyField},
				{Index: 2, RetType: types.NewFieldType(mysql.TypeLonglong)}},
		})
	}

	for _, testCase := range rankTopNCases {
		ctx.GetSessionVars().InitChunkSize = 32
		ctx.GetSessionVars().MaxChunkSize = 32
		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		ctx.GetSessionVars().StmtCtx.MemTracker = memory.NewTracker(memory.LabelForSQLText, -1)
		// 内存 tracker/spill 相关调用保留资源阈值与落盘触发语义。
		ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker)

		schema := expression.NewSchema(testCase.cols...)
		dataSource := buildRankTopNDataSource(testCase, schema)

		// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
		randNum := rand.Intn(100)
		var offset uint64
		var count uint64
		if randNum < 10 {
			offset = 0
		} else {
			// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
			offset = uint64(rand.Intn(3000))
		}

		if randNum < 5 {
			count = 0
		} else {
			// 随机 fixture 生成保留 Go 测试的数据分布；Rust 接线时需控制可重复性和编码细节。
			count = uint64(rand.Intn(10000))
		}
		rankTopNBasicCase(t, testCase, schema, dataSource, offset, count)
	}
}
"####################;

use super::sort::VecRowSource;
use super::{DataChunk, Limit, RankInfo, Row, SortKey, SortValue, TopNExec};

fn drain(executor: &mut TopNExec, chunk_size: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    loop {
        let chunk = executor.Next(chunk_size).unwrap();
        if chunk.num_rows() == 0 {
            break;
        }
        rows.extend(chunk.rows);
    }
    executor.Close().unwrap();
    rows
}

/// Go RankTopN 仅用截断前缀扩大候选集，最终输出仍严格服从 LIMIT。
#[test]
fn rank_topn_trims_prefix_ties_to_limit() {
    // 两行前缀键均为 1，第三行前缀键为 2
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(2), SortValue::Bytes(b"c".to_vec())]),
        Row(vec![SortValue::Int(1), SortValue::Bytes(b"a".to_vec())]),
        Row(vec![SortValue::Int(1), SortValue::Bytes(b"b".to_vec())]),
    ])]);
    let key = SortKey::asc(0);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![key],
        Limit {
            Offset: 0,
            Count: 1,
        },
        Some(RankInfo {
            prefixKeys: vec![key],
            expectedCount: 1,
        }),
        1,
        8,
        -1,
    );
    let output = drain(&mut executor, 8);
    assert_eq!(
        output,
        vec![Row(vec![
            SortValue::Int(1),
            SortValue::Bytes(b"a".to_vec())
        ])]
    );
}

/// 对应 Go 随机 offset/count 场景：前缀并列不能改变窗口长度和偏移。
#[test]
fn rank_topn_applies_offset_and_count_after_candidate_expansion() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(2), SortValue::Bytes(b"d".to_vec())]),
        Row(vec![SortValue::Int(1), SortValue::Bytes(b"b".to_vec())]),
        Row(vec![SortValue::Int(1), SortValue::Bytes(b"a".to_vec())]),
        Row(vec![SortValue::Int(2), SortValue::Bytes(b"c".to_vec())]),
        Row(vec![SortValue::Int(3), SortValue::Bytes(b"e".to_vec())]),
    ])]);
    let prefix = SortKey::asc(0);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![prefix, SortKey::asc(1)],
        Limit {
            Offset: 1,
            Count: 2,
        },
        Some(RankInfo {
            prefixKeys: vec![prefix],
            expectedCount: 2,
        }),
        5,
        1,
        -1,
    );

    let output = drain(&mut executor, 1);
    assert_eq!(
        output,
        vec![
            Row(vec![SortValue::Int(1), SortValue::Bytes(b"b".to_vec())]),
            Row(vec![SortValue::Int(2), SortValue::Bytes(b"c".to_vec())]),
        ]
    );
}
