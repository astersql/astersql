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

// Semi Join（半连接）探测逻辑的单元测试。
//
// 覆盖：仅输出命中的 probe 行、重复 build 键不重复外表行、other condition / NULL
// 结果拒绝候选，以及左侧 build 时只扫描已使用的 build 行。上方注释块保留 Go 版
// 大数据源与 spill 用例的对照实现。

/*
// semi/anti-semi join probe 如何构造随机输入、期望结果、other condition 和 spill 参数。

pub const MAX_CHUNK_SIZE_IN_TEST: i32 = 32;

// semiJoinleftCols 对应 Go 包级变量：semi join 左侧两列均为 longlong。
pub fn semi_join_left_cols() -> Vec<expression::Column> {
    vec![column(0, types::NewFieldType(mysql::TypeLonglong)), column(1, types::NewFieldType(mysql::TypeLonglong))]
}

// semiJoinrightCols 对应 Go 包级变量：semi join 右侧两列均为 longlong。
pub fn semi_join_right_cols() -> Vec<expression::Column> {
    vec![column(0, types::NewFieldType(mysql::TypeLonglong)), column(1, types::NewFieldType(mysql::TypeLonglong))]
}

// semiJoinRetTypes 对应 Go 包级变量：semi/anti-semi join 输出仍为左侧两列。
pub fn semi_join_ret_types() -> Vec<types::FieldType> {
    vec![types::NewFieldType(mysql::TypeLonglong), types::NewFieldType(mysql::TypeLonglong)]
}

// buildLeftAndRightSemiDataSource 对应 Go helper：构造两侧 50000 行 mock 数据源。
pub fn build_left_and_right_semi_data_source(
    ctx: sessionctx::Context,
    left_cols: Vec<expression::Column>,
    right_cols: Vec<expression::Column>,
    has_sel: bool,
) -> (testutil::MockDataSource, testutil::MockDataSource) {
    let left_schema = expression::NewSchema(left_cols);
    let right_schema = expression::NewSchema(right_cols);
    let join_key_left_int_datums = buildJoinKeyIntDatums(10_000);
    let join_key_right_int_datums = buildJoinKeyIntDatums(10_000);
    let left_param = testutil::MockDataSourceParameters {
        DataSchema: left_schema,
        Ctx: ctx.clone(),
        Rows: 50_000,
        Ndvs: vec![-1, -1],
        Datums: vec![join_key_left_int_datums.clone(), join_key_left_int_datums],
        HasSel: has_sel,
    };
    let right_param = testutil::MockDataSourceParameters {
        DataSchema: right_schema,
        Ctx: ctx,
        Rows: 50_000,
        Ndvs: vec![-1, -1],
        Datums: vec![join_key_right_int_datums.clone(), join_key_right_int_datums],
        HasSel: has_sel,
    };
    (testutil::BuildMockDataSource(left_param), testutil::BuildMockDataSource(right_param))
}

// buildSemiDataSourceAndExpectResult 对应 Go 的复杂数据生成器。
// 分支含义：duplicate key、other condition、build side 和 anti-semi 共同决定期望输出。
pub fn build_semi_data_source_and_expect_result(
    ctx: sessionctx::Context,
    left_cols: Vec<expression::Column>,
    right_cols: Vec<expression::Column>,
    right_as_build_side: bool,
    has_other_condition: bool,
    has_duplicate_key: bool,
    is_anti_semi_join: bool,
) -> (testutil::MockDataSource, testutil::MockDataSource, Vec<chunk::Row>) {
    let left_schema = expression::NewSchema(left_cols);
    let right_schema = expression::NewSchema(right_cols);
    let row_num: i64 = 50_000;
    let mut left_col0_datums = Vec::with_capacity(row_num as usize);
    let mut left_col1_datums = Vec::with_capacity(row_num as usize);
    let mut right_col0_datums = Vec::with_capacity(row_num as usize);
    let mut right_col1_datums = Vec::with_capacity(row_num as usize);
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let mut expect_result_chunk = chunk::NewChunkWithCapacity(vec![int_tp.clone(), int_tp], 10_000);

    if has_duplicate_key {
        if has_other_condition {
            build_duplicate_key_with_condition(
                right_as_build_side,
                is_anti_semi_join,
                &mut left_col0_datums,
                &mut left_col1_datums,
                &mut right_col0_datums,
                &mut right_col1_datums,
                &mut expect_result_chunk,
            );
        } else {
            // 无 other condition 时，偶数 key 有右侧匹配；semi 输出偶数左行，anti-semi 输出奇数左行。
            for i in 0..10_000_i64 {
                let left_single_key_num = rand::Int31n(2 * MAX_CHUNK_SIZE_IN_TEST) + 1;
                let right_single_key_num = rand::Int31n(2 * MAX_CHUNK_SIZE_IN_TEST) + 1;
                for _ in 0..left_single_key_num {
                    left_col0_datums.push(i);
                    left_col1_datums.push(0_i64);
                }
                if i % 2 == 0 {
                    for _ in 0..right_single_key_num {
                        right_col0_datums.push(i);
                        right_col1_datums.push(0_i64);
                    }
                    if !is_anti_semi_join {
                        append_expected_rows(&mut expect_result_chunk, i, 0, left_single_key_num);
                    }
                } else if is_anti_semi_join {
                    append_expected_rows(&mut expect_result_chunk, i, 0, left_single_key_num);
                }
            }
        }
    } else {
        // 非重复 key 场景让左侧从 30000 开始，只有 [30000, 49999] 能匹配右侧 [0, 49999]。
        let left_col0_start_num = 30_000_i64;
        for i in 0..row_num {
            let left_value = left_col0_start_num + i;
            left_col0_datums.push(left_value);
            if has_other_condition {
                if left_value % 2 == 0 {
                    left_col1_datums.push(1_i64);
                    append_expected_for_condition(&mut expect_result_chunk, left_value, 1, row_num, is_anti_semi_join, true);
                } else {
                    left_col1_datums.push(0_i64);
                    if is_anti_semi_join {
                        expect_result_chunk.AppendInt64(0, left_value);
                        expect_result_chunk.AppendInt64(1, 0);
                    }
                }
            } else {
                left_col1_datums.push(1_i64);
                append_expected_for_condition(&mut expect_result_chunk, left_value, 1, row_num, is_anti_semi_join, false);
            }
            right_col0_datums.push(i);
            right_col1_datums.push(0_i64);
        }
    }

    // Go 使用 Fisher-Yates 洗牌，避免 hash join 依赖输入顺序；保留 shuffle 的数据依赖。
    shuffle_two_columns(&mut left_col0_datums, &mut left_col1_datums);
    shuffle_two_columns(&mut right_col0_datums, &mut right_col1_datums);
    let expect_result = if is_anti_semi_join {
        sortRows(vec![expect_result_chunk], semi_join_ret_types())
    } else {
        (0..expect_result_chunk.NumRows()).map(|i| expect_result_chunk.GetRow(i)).collect()
    };

    let left_param = testutil::MockDataSourceParameters { DataSchema: left_schema, Ctx: ctx.clone(), Rows: left_col0_datums.len(), Ndvs: vec![-2, -2], Datums: vec![left_col0_datums, left_col1_datums], HasSel: false };
    let right_param = testutil::MockDataSourceParameters { DataSchema: right_schema, Ctx: ctx, Rows: right_col0_datums.len(), Ndvs: vec![-2, -2], Datums: vec![right_col0_datums, right_col1_datums], HasSel: false };
    (testutil::BuildMockDataSource(left_param), testutil::BuildMockDataSource(right_param), expect_result)
}

// testSemiJoin 对应 Go helper：只切换 isAntiSemiJoin=false。
pub fn test_semi_join(t: &mut testing::T, right_as_build_side: bool, has_other_condition: bool, has_duplicate_key: bool) {
    test_semi_or_anti_semi_join(t, right_as_build_side, has_other_condition, has_duplicate_key, false);
}

// testSemiOrAntiSemiJoin 对应 Go 主 helper：构造执行器并比较排序后的结果。
pub fn test_semi_or_anti_semi_join(
    t: &mut testing::T,
    right_as_build_side: bool,
    has_other_condition: bool,
    has_duplicate_key: bool,
    is_anti_semi_join: bool,
) {
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = MAX_CHUNK_SIZE_IN_TEST;
    ctx.GetSessionVars().MaxChunkSize = MAX_CHUNK_SIZE_IN_TEST;
    let (mut left_data_source, mut right_data_source, expected_result) =
        build_semi_data_source_and_expect_result(ctx.clone(), semi_join_left_cols(), semi_join_right_cols(), right_as_build_side, has_other_condition, has_duplicate_key, is_anti_semi_join);
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let left_keys = vec![column(0, int_tp.clone())];
    let right_keys = vec![column(0, int_tp.clone())];
    let (build_keys, probe_keys) = if right_as_build_side { (right_keys.clone(), left_keys.clone()) } else { (left_keys.clone(), right_keys.clone()) };

    let mut other_condition = None;
    let mut l_used_in_other_condition = vec![];
    let mut r_used_in_other_condition = vec![];
    if has_other_condition {
        l_used_in_other_condition.push(1);
        r_used_in_other_condition.push(1);
        // other condition 对应 concat row 中左第 1 列 > 右第 1 列。
        other_condition = Some(new_gt_condition(1, 3));
    }
    let join_type = if is_anti_semi_join { base::AntiSemiJoin } else { base::SemiJoin };
    let info = HashJoinInfo {
        ctx,
        schema: buildSchema(semi_join_ret_types()),
        leftExec: left_data_source.clone(),
        rightExec: right_data_source.clone(),
        joinType: join_type,
        rightAsBuildSide: right_as_build_side,
        buildKeys: build_keys,
        probeKeys: probe_keys,
        lUsed: vec![0, 1],
        rUsed: vec![],
        otherCondition: other_condition,
        lUsedInOtherCondition: Some(l_used_in_other_condition),
        rUsedInOtherCondition: Some(r_used_in_other_condition),
        ..Default::default()
    };

    left_data_source.PrepareChunks();
    right_data_source.PrepareChunks();
    let hash_join_exec = buildHashJoinV2Exec(&info);
    let result = getSortedResults(t, hash_join_exec, semi_join_ret_types());
    checkResults(t, semi_join_ret_types(), result, expected_result);
}

#[test]
pub fn test_semi_join_basic() {
    test_semi_join(&mut testing::T::new(), false, false, false); // Left side build without other condition
    test_semi_join(&mut testing::T::new(), false, true, false);  // Left side build with other condition
    test_semi_join(&mut testing::T::new(), true, false, false);  // Right side build without other condition
    test_semi_join(&mut testing::T::new(), true, true, false);   // Right side build with other condition
}

#[test]
pub fn test_semi_join_duplicate_keys() {
    test_semi_join(&mut testing::T::new(), false, false, true); // Left side build without other condition
    test_semi_join(&mut testing::T::new(), false, true, true);  // Left side build with other condition
    test_semi_join(&mut testing::T::new(), true, false, true);  // Right side build without other condition
    test_semi_join(&mut testing::T::new(), true, true, true);   // Right side build with other condition
}

#[test]
pub fn test_semi_and_anti_semi_join_spill() {
    // Go 测试名包含 anti-semi，但当前 joinTypes 只枚举 SemiJoin；保持原样。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let left_cols = semi_join_left_cols();
    let right_cols = semi_join_right_cols();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (left_data_source, right_data_source) =
        build_left_and_right_semi_data_source(ctx.clone(), left_cols, right_cols, false);
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let left_types = vec![int_tp.clone(), int_tp.clone()];
    let right_types = vec![int_tp.clone(), int_tp.clone()];
    let left_keys = vec![column(0, int_tp.clone())];
    let right_keys = vec![column(0, int_tp.clone())];
    let other_condition = new_gt_condition(1, 3);
    spillChunkSize = 100;
    let params = vec![
        // basic case
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1], vec![], None, None, None, vec![1_500_000, 1_700_000, 2_700_000, 100_000, 10_000], &test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1], vec![], None, None, None, vec![1_500_000, 1_700_000, 3_300_000, 100_000, 10_000], &test_func_name),
        // with other condition
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1], vec![], Some(other_condition.clone()), Some(vec![1]), Some(vec![1]), vec![1_500_000, 1_700_000, 3_300_000, 100_000, 10_000], &test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1], vec![], Some(other_condition), Some(vec![1]), Some(vec![1]), vec![1_500_000, 1_700_000, 3_300_000, 100_000, 10_000], &test_func_name),
    ];
    for param in params {
        testSpill(&mut testing::T::new(), &ctx, base::SemiJoin, &left_data_source, &right_data_source, param);
    }
    util::CheckNoLeakFiles(&test_func_name);
}

#[test]
pub fn test_semi_join_probe_basic() {
    testLeftOuterSemiOrSemiJoinProbeBasic(&mut testing::T::new(), false, false);
}

#[test]
pub fn test_semi_join_probe_all_join_keys() {
    testLeftOuterSemiJoinProbeAllJoinKeys(&mut testing::T::new(), false, false);
}

#[test]
pub fn test_semi_join_probe_with_sel() {
    testLeftOuterSemiJoinProbeWithSel(&mut testing::T::new(), false, false);
}
*/

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造两列整数行，用作连接键与载荷。
fn row(key: i64, payload: i64) -> Row {
    vec![Value::Int(key), Value::Int(payload)]
}

/// 构造 Semi Join 探测器：`right_as_build` 决定 build 侧，`conditions` 为 other condition。
fn semi_probe(build: Vec<Row>, right_as_build: bool, conditions: Vec<Predicate>) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::Semi,
        false,
        vec![],
        conditions,
        Some([vec![0, 1], vec![0, 1]]),
        false,
        32,
    )
    .unwrap();
    let context = HashJoinContext::new(build, vec![0], vec![0], joiner, right_as_build, true, 32);
    new_join_probe(context, 2, JoinType::Semi, right_as_build, false).unwrap()
}

/// 右侧 build：只输出在 build 侧存在匹配的 probe 行。
#[test]
fn semi_join_returns_only_matched_probe_rows() {
    let mut probe = semi_probe(vec![row(1, 10), row(3, 30)], true, vec![]);
    probe
        .set_chunk_for_probe(vec![row(1, 100), row(2, 200)])
        .unwrap();
    // 键 1 命中，键 2 未命中，半连接只保留键 1。
    assert_eq!(probe.probe().rows, vec![row(1, 100)]);
}

/// 同一 build 键多行时，外表行仍只输出一次（半连接去重语义）。
#[test]
fn semi_join_duplicate_build_keys_do_not_duplicate_outer_row() {
    let mut probe = semi_probe(vec![row(5, 1), row(5, 2)], true, vec![]);
    probe.set_chunk_for_probe(vec![row(5, 9)]).unwrap();
    assert_eq!(probe.probe().rows, vec![row(5, 9)]);
}

/// other condition 为假或 NULL（三值逻辑）时应拒绝该候选匹配。
#[test]
fn semi_join_condition_and_null_result_reject_candidates() {
    // 载荷列比较：left > right 为真；相等返回 NULL；其余为假。
    let condition: Predicate = Arc::new(|joined| match (&joined[1], &joined[3]) {
        (Value::Int(left), Value::Int(right)) if left > right => Ok(Some(true)),
        (Value::Int(left), Value::Int(right)) if left == right => Ok(None),
        _ => Ok(Some(false)),
    });
    let mut probe = semi_probe(vec![row(1, 10), row(2, 20)], true, vec![condition]);
    probe
        .set_chunk_for_probe(vec![row(1, 11), row(2, 20)])
        .unwrap();
    // 11>10 命中；20==20 为 NULL，半连接不接受。
    assert_eq!(probe.probe().rows, vec![row(1, 11)]);
}

/// 左侧 build：探测阶段不产出行，随后扫描 row table 只输出已标记使用的 build 行。
#[test]
fn semi_join_with_left_build_scans_only_used_build_rows() {
    let mut probe = semi_probe(vec![row(1, 10), row(2, 20)], false, vec![]);
    probe
        .set_restored_chunk_for_probe(vec![row(1, 100), row(3, 300)])
        .unwrap();
    // 左侧 build 时 probe() 只打 used 标记，结果行延后到 scan_row_table。
    assert!(probe.probe().rows.is_empty());
    assert!(probe.need_scan_row_table());
    probe.init_for_scan_row_table();
    assert_eq!(probe.scan_row_table().rows, vec![row(1, 10)]);
}

#[test]
/// NULL keys never satisfy ordinary semi-join equality, even when both sides are NULL.
fn semi_join_null_keys_are_not_equal() {
    let mut probe = semi_probe(
        vec![vec![Value::Null, Value::Int(10)], row(1, 20)],
        true,
        vec![],
    );
    probe
        .set_chunk_for_probe(vec![vec![Value::Null, Value::Int(100)], row(1, 200)])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(1, 200)]);
}

#[test]
/// A left-build semi join marks only matching build rows before the row-table scan.
fn semi_join_left_build_skips_unmatched_duplicate_rows() {
    let mut probe = semi_probe(vec![row(4, 40), row(4, 41), row(5, 50)], false, vec![]);
    probe.set_chunk_for_probe(vec![row(4, 400)]).unwrap();
    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert_eq!(probe.scan_row_table().rows, [row(4, 40), row(4, 41)]);
}

#[test]
#[should_panic(expected = "should not reach here")]
fn semi_join_right_build_init_for_scan_row_table_panics_like_go() {
    let mut probe = semi_probe(vec![row(1, 10)], true, vec![]);
    probe.init_for_scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn semi_join_right_build_is_scan_row_table_done_panics_like_go() {
    let probe = semi_probe(vec![row(1, 10)], true, vec![]);
    probe.is_scan_row_table_done();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn semi_join_right_build_scan_row_table_panics_like_go() {
    let mut probe = semi_probe(vec![row(1, 10)], true, vec![]);
    probe.scan_row_table();
}
