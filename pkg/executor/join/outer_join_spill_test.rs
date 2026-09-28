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

// Outer Join spill（落盘）相关单元测试。
//
// 验证 probe 剩余行 spill/恢复后仍保持 Left Outer 未匹配语义，以及
// `HashJoinSpillHelper` 对 build/probe 两侧的写入、恢复与 round 限制。
// 块注释内保留 Go 版 failpoint / 内存 tracker 测试草稿。

/*
// outer join spill 测试如何构造 mock 数据源、内存 tracker、failpoint 和临时目录。

// prepareSimpleHashJoinEnv 对应 Go 的同名 helper：准备一个容易触发 spill fallback 的简单 hash join 环境。
pub fn prepare_simple_hash_join_env(file_name_prefix_for_test: &str) -> (
    testutil::MockDataSource,
    testutil::MockDataSource,
    HashJoinInfo,
    testutil::MockActionOnExceed,
) {
    let hard_limit_bytes_num: i64 = 5_000_000;
    let mut new_root_exceed_action = testutil::MockActionOnExceed::new();

    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    ctx.GetSessionVars().MemTracker = memory::NewTracker(memory::LabelForSession, hard_limit_bytes_num);
    ctx.GetSessionVars().MemTracker.SetActionOnExceed(&mut new_root_exceed_action);
    // Go 预先消耗几乎全部 session 内存，用来稳定触发 fallback action；仅保留触发条件。
    ctx.GetSessionVars().MemTracker.Consume((hard_limit_bytes_num as f64 * 0.99999) as i64);
    ctx.GetSessionVars().StmtCtx.MemTracker = memory::NewTracker(memory::LabelForSQLText, -1);
    ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker);

    let (left_data_source, right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, false);
    let (int_tp, string_tp) = not_null_int_and_string_types();
    let left_types = vec![int_tp.clone(), int_tp.clone(), int_tp.clone(), string_tp.clone(), int_tp.clone()];
    let right_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), int_tp.clone(), int_tp.clone()];
    let left_keys = vec![column(1, int_tp.clone()), column(3, string_tp.clone())];
    let right_keys = vec![column(0, int_tp.clone()), column(2, string_tp.clone())];

    let param = SpillTestParam {
        right_as_build_side: true,
        left_keys,
        right_keys,
        left_types,
        right_types,
        left_used: vec![0, 1, 3, 4],
        right_used: vec![0, 2, 3, 4],
        other_condition: None,
        left_used_by_other_condition: None,
        right_used_by_other_condition: None,
        memory_limits: Some(vec![5_000_000, 1_700_000, 6_000_000, 1_500_000, 10_000]),
        file_name_prefix_for_test: file_name_prefix_for_test.to_string(),
    };

    // spillChunkSize 是 Go 包级变量，测试把它固定为 100 以缩小 spill 粒度。
    spillChunkSize = 100;
    let join_type = base::InnerJoin;
    let return_types = getReturnTypes(join_type, &param);
    let (build_keys, probe_keys) = if param.right_as_build_side {
        (param.right_keys.clone(), param.left_keys.clone())
    } else {
        (param.left_keys.clone(), param.right_keys.clone())
    };

    let info = HashJoinInfo {
        ctx,
        schema: buildSchema(return_types),
        leftExec: left_data_source.clone(),
        rightExec: right_data_source.clone(),
        joinType: join_type,
        rightAsBuildSide: param.right_as_build_side,
        buildKeys: build_keys,
        probeKeys: probe_keys,
        lUsed: param.left_used,
        rUsed: param.right_used,
        otherCondition: param.other_condition,
        lUsedInOtherCondition: param.left_used_by_other_condition,
        rUsedInOtherCondition: param.right_used_by_other_condition,
        fileNamePrefixForTest: param.file_name_prefix_for_test,
    };

    (left_data_source, right_data_source, info, new_root_exceed_action)
}

// testRandomFail 对应 Go helper：在较低内存上限和 panic/error failpoint 下反复执行 hash join。
pub fn test_random_fail(
    t: &mut testing::T,
    ctx: &mut mock::Context,
    join_type: base::JoinType,
    param: SpillTestParam,
    left_data_source: &mut testutil::MockDataSource,
    right_data_source: &mut testutil::MockDataSource,
) {
    ctx.GetSessionVars().MemTracker = memory::NewTracker(memory::LabelForSQLText, 1_500_000);
    ctx.GetSessionVars().StmtCtx.MemTracker = memory::NewTracker(memory::LabelForSQLText, -1);
    ctx.GetSessionVars().StmtCtx.MemTracker.AttachTo(ctx.GetSessionVars().MemTracker);
    let return_types = getReturnTypes(join_type, &param);
    let (build_keys, probe_keys) = if param.right_as_build_side {
        (param.right_keys.clone(), param.left_keys.clone())
    } else {
        (param.left_keys.clone(), param.right_keys.clone())
    };

    let info = HashJoinInfo {
        ctx: ctx.clone(),
        schema: buildSchema(return_types),
        leftExec: left_data_source.clone(),
        rightExec: right_data_source.clone(),
        joinType: join_type,
        rightAsBuildSide: param.right_as_build_side,
        buildKeys: build_keys,
        probeKeys: probe_keys,
        lUsed: param.left_used,
        rUsed: param.right_used,
        otherCondition: param.other_condition,
        lUsedInOtherCondition: param.left_used_by_other_condition,
        rUsedInOtherCondition: param.right_used_by_other_condition,
        fileNamePrefixForTest: param.file_name_prefix_for_test,
    };

    // Go 在每次随机失败测试前重置 mock chunks，确保 Open/Next/Close 看到完整输入。
    left_data_source.PrepareChunks();
    right_data_source.PrepareChunks();
    let hash_join_exec = buildHashJoinV2Exec(&info);
    executeHashJoinExecForRandomFailTest(t, hash_join_exec);
}

// outer_join_spill_base_params 对应 Basic1/Basic2 中重复构造的 spillTestParam 列表。
fn outer_join_spill_base_params(test_func_name: &str, left_keys: Vec<expression::Column>, right_keys: Vec<expression::Column>, left_types: Vec<types::FieldType>, right_types: Vec<types::FieldType>) -> Vec<SpillTestParam> {
    vec![
        // Normal case：左右侧输出列都存在。
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], None, None, None, vec![3_000_000, 2_000_000, 5_000_000, 400_000, 10_000], test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], None, None, None, vec![3_000_000, 2_000_000, 5_000_000, 400_000, 10_000], test_func_name),
        // rightUsed is empty：验证右侧输出列为空时结果拼接和 spill 仍保持稳定。
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![], None, None, None, vec![2_000_000, 2_000_000, 3_300_000, 200_000, 10_000], test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![], None, None, None, vec![3_000_000, 2_000_000, 5_300_000, 400_000, 10_000], test_func_name),
        // leftUsed is empty：验证左侧输出列为空时 outer join 补空行路径。
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![], vec![0, 2, 3, 4], None, None, None, vec![3_000_000, 2_000_000, 5_000_000, 400_000, 10_000], test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![], vec![0, 2, 3, 4], None, None, None, vec![2_000_000, 2_000_000, 3_300_000, 200_000, 10_000], test_func_name),
    ]
}

#[test]
pub fn test_outer_join_spill_basic1() {
    // Go defer config.RestoreFunc()()，这里保留临时目录配置恢复语义，不实际触碰全局配置。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (left_data_source, right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, false);
    let (left_keys, right_keys, left_types, right_types) = outer_join_keys_and_types();
    let params = outer_join_spill_base_params(&test_func_name, left_keys, right_keys, left_types, right_types);

    // slowWorkers failpoint 强制 worker 慢路径，更容易覆盖 spill/restore 分支。
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/slowWorkers", "return(true)");
    spillChunkSize = 100;
    for param in params {
        testSpill(&mut testing::T::new(), &ctx, base::LeftOuterJoin, &left_data_source, &right_data_source, param);
    }
    util::CheckNoLeakFiles(&test_func_name);
}
*/

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::hash_join_spill_helper::HashJoinSpillHelper;
use crate::join_row_table::RowTableSegment;
use crate::join_table_meta::EncodedRow;
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::atomic::AtomicBool;

/// 构造单列 Int 行。
fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

/// 构造右表为 build 的 Left Outer Join probe。
fn outer_probe() -> Box<dyn Probe> {
    let joiner = Joiner::new(JoinType::LeftOuter, false, row(-1), vec![], None, false, 32).unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 32);
    new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap()
}

/// 构造最小可用的 build 侧 row table segment，供 spill helper 写入。
fn segment() -> RowTableSegment {
    RowTableSegment {
        rows: vec![EncodedRow {
            bytes: vec![1; 16],
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        }],
        hash_values: vec![7],
        valid_key_count: 1,
        ..Default::default()
    }
}

/// Spill 剩余 probe 行后恢复，匹配/未匹配语义与直接 probe 一致。
#[test]
fn outer_probe_spill_remaining_restores_unmatched_semantics() {
    let mut probe = outer_probe();
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    let spilled = probe.spill_remaining_probe_chunks();
    assert_eq!(spilled, vec![vec![row(1), row(2)]]);
    probe
        .set_restored_chunk_for_probe(spilled[0].clone())
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(-1)],
        ]
    );
}

/// Spill helper 同时落盘 build segment 与 probe chunk，恢复后两侧内容完整。
#[test]
fn outer_spill_writes_build_and_probe_rows_then_restores_both_sides() {
    let helper = HashJoinSpillHelper::new(2, 1, 2, 64).unwrap();
    helper.set_partition_spilled(&[1]).unwrap();
    helper.spill_build_segments(0, 1, &[segment()]).unwrap();
    let probe_chunk = vec![row(1), row(2)];
    helper.spill_probe_chunk(0, 1, &probe_chunk).unwrap();
    assert!(helper.build_spill_bytes() > 0);
    assert!(helper.probe_spill_bytes() > 0);
    helper.prepare_for_restoring(0).unwrap();
    let restored = helper.pop_restore_partition().unwrap();
    assert_eq!(restored.round, 1);
    assert_eq!(restored.build_side_chunks.len(), 1);
    assert_eq!(restored.build_side_chunks[0].len(), 1);
    let restored_build = &restored.build_side_chunks[0][0];
    assert_eq!(restored_build.hash_value, 7);
    assert!(restored_build.valid_join_key);
    assert_eq!(restored_build.row_bytes, vec![1; 16]);
    assert_eq!(restored.probe_side_chunks, vec![vec![row(1), row(2)]]);
}

/// reset 清空 spill 状态；超出 round 上限的恢复准备应失败。
#[test]
fn outer_spill_round_limit_and_reset_clear_state() {
    let helper = HashJoinSpillHelper::new(2, 1, 1, 64).unwrap();
    helper.set_partition_spilled(&[0, 1]).unwrap();
    assert!(helper.are_all_partitions_spilled());
    helper.reset();
    assert_eq!(helper.spilled_partition_count(), 0);
    assert!(helper.prepare_for_restoring(1).is_err());
    helper.close();
}

#[test]
/// Left outer probe returns a default inner row for every unmatched probe row.
fn outer_probe_reports_each_unmatched_probe_row_once() {
    let mut probe = outer_probe();
    probe.set_chunk_for_probe(vec![row(2), row(3)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Int(2), Value::Int(-1)],
            vec![Value::Int(3), Value::Int(-1)]
        ]
    );
}

#[test]
/// Capacity exhaustion resumes the same outer probe chunk on the following call.
fn outer_probe_resumes_after_capacity_boundary() {
    let joiner = Joiner::new(JoinType::LeftOuter, false, row(-1), vec![], None, false, 1).unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 1);
    let mut probe = new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(1)]]);
    assert_eq!(probe.probe().rows, [vec![Value::Int(2), Value::Int(-1)]]);
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
/// Right outer joins scan an outer build side after probing to emit only unused rows.
fn right_outer_probe_scans_unmatched_build_rows() {
    let joiner = Joiner::new(JoinType::RightOuter, true, row(-1), vec![], None, false, 8).unwrap();
    let context = HashJoinContext::new(
        vec![row(1), row(2)],
        vec![0],
        vec![0],
        joiner,
        true,
        true,
        8,
    );
    let mut probe = new_join_probe(context, 0, JoinType::RightOuter, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(1)]]);
    assert!(probe.need_scan_row_table());
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        [vec![Value::Int(-1), Value::Int(2)]]
    );
}

#[test]
/// A rejected match still receives the outer-side default row.
fn outer_probe_condition_rejection_preserves_outer_row() {
    let condition: Predicate = std::sync::Arc::new(|_| Ok(Some(false)));
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        row(-1),
        vec![condition],
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 8);
    let mut probe = new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(-1)]]);
}

#[test]
/// Spill helper accepts an empty probe chunk without creating a bogus restore partition.
fn outer_spill_empty_probe_chunk_round_trips() {
    let helper = HashJoinSpillHelper::new(1, 1, 2, 64).unwrap();
    helper.set_partition_spilled(&[0]).unwrap();
    let empty: crate::row_table_builder::Chunk = Vec::new();
    helper.spill_probe_chunk(0, 0, &empty).unwrap();
    helper.prepare_for_restoring(0).unwrap();
    assert!(helper.pop_restore_partition().is_none());
}

/*
#[test]
pub fn test_outer_join_spill_basic2() {
    // 与 Basic1 相同的数据矩阵，但 joinType 切换为 RightOuterJoin。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (left_data_source, right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, false);
    let (left_keys, right_keys, left_types, right_types) = outer_join_keys_and_types();
    let params = outer_join_spill_base_params(&test_func_name, left_keys, right_keys, left_types, right_types);
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/slowWorkers", "return(true)");
    spillChunkSize = 100;
    for param in params {
        testSpill(&mut testing::T::new(), &ctx, base::RightOuterJoin, &left_data_source, &right_data_source, param);
    }
    util::CheckNoLeakFiles(&test_func_name);
}
*/

/*
#[test]
pub fn test_outer_join_spill_with_sel() {
    // hasSel=true 覆盖输入 chunk 带 selection vector 的路径。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (left_data_source, right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, true);
    let (left_keys, right_keys, left_types, right_types) = outer_join_keys_and_types();
    let params = vec![
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], None, None, None, vec![2_000_000, 1_000_000, 2_500_000, 200_000, 10_000], &test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], None, None, None, vec![2_000_000, 1_000_000, 2_500_000, 200_000, 10_000], &test_func_name),
    ];
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/slowWorkers", "return(true)");
    spillChunkSize = 100;
    for join_type in [base::LeftOuterJoin, base::RightOuterJoin] {
        for param in params.clone() {
            testSpill(&mut testing::T::new(), &ctx, join_type, &left_data_source, &right_data_source, param);
        }
    }
    util::CheckNoLeakFiles(&test_func_name);
}

#[test]
pub fn test_outer_join_spill_with_other_condition() {
    // otherCondition 使用 left 第 0 列和 right 第 4 列组合后的 schema 下标，覆盖 spill 后条件重算。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (left_data_source, right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, false);
    let (left_keys, right_keys, left_types, right_types) = outer_join_keys_and_types();
    let other_condition = new_gt_condition(0, 9);
    let params = vec![
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], Some(other_condition.clone()), Some(vec![0]), Some(vec![4]), vec![3_000_000, 2_000_000, 5_000_000, 400_000, 10_000], &test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], Some(other_condition), Some(vec![0]), Some(vec![4]), vec![3_000_000, 2_000_000, 5_000_000, 400_000, 10_000], &test_func_name),
    ];
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/slowWorkers", "return(true)");
    spillChunkSize = 100;
    for join_type in [base::LeftOuterJoin, base::RightOuterJoin] {
        for param in params.clone() {
            testSpill(&mut testing::T::new(), &ctx, join_type, &left_data_source, &right_data_source, param);
        }
    }
    util::CheckNoLeakFiles(&test_func_name);
}

#[test]
pub fn test_outer_join_under_apply_exec() {
    // Hash join executor may be repeatedly closed and opened.
    // Go 这里模拟 ApplyExec 反复 Open/Close 同一个 hash join；保留 info 复用和 joinType 轮换。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (left_data_source, right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, false);
    let mut info = HashJoinInfo {
        ctx,
        schema: buildSchema(retTypes),
        leftExec: left_data_source.clone(),
        rightExec: right_data_source.clone(),
        joinType: base::InnerJoin,
        rightAsBuildSide: true,
        buildKeys: vec![column(0, types::NewFieldType(mysql::TypeLonglong)), column(2, types::NewFieldType(mysql::TypeVarString))],
        probeKeys: vec![column(1, types::NewFieldType(mysql::TypeLonglong)), column(3, types::NewFieldType(mysql::TypeVarString))],
        lUsed: vec![0, 1, 3, 4],
        rUsed: vec![0, 2, 3, 4],
        otherCondition: Some(expression::CNFExprs::new()),
        lUsedInOtherCondition: Some(vec![0]),
        rUsedInOtherCondition: Some(vec![4]),
        fileNamePrefixForTest: test_func_name.clone(),
    };
    spillChunkSize = 100;
    for join_type in [base::LeftOuterJoin, base::RightOuterJoin] {
        info.joinType = join_type;
        let expected_result = getExpectedResults(&mut testing::T::new(), &info.ctx, &info, retTypes, &left_data_source, &right_data_source);
        testUnderApplyExec(&mut testing::T::new(), &info.ctx, expected_result, &info, retTypes, &left_data_source, &right_data_source);
    }
    util::CheckNoLeakFiles(&test_func_name);
}

#[test]
pub fn test_fall_back_action() {
    // 验证内存 action 至少触发一次；真实内存 tracker 行为由 Go 测试负责。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let (mut left_data_source, mut right_data_source, info, new_root_exceed_action) =
        prepare_simple_hash_join_env(&test_func_name);
    left_data_source.PrepareChunks();
    right_data_source.PrepareChunks();
    let hash_join_exec = buildHashJoinV2Exec(&info);
    let _ = executeHashJoinExec(&mut testing::T::new(), hash_join_exec);
    require::Less(&mut testing::T::new(), 0, new_root_exceed_action.GetTriggeredNum());
    util::CheckNoLeakFiles(&test_func_name);
}

#[test]
pub fn test_issue_59377() {
    // Issue59377 failpoint 在 Open/Next 之间注入异常，覆盖内存已清理后的错误返回路径。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let (mut left_data_source, mut right_data_source, mut info, _) =
        prepare_simple_hash_join_env(&test_func_name);
    left_data_source.PrepareChunks();
    right_data_source.PrepareChunks();
    let mut hash_join_exec = buildHashJoinV2Exec(&info);
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/Issue59377", "return");
    let tmp_ctx = context::Background();
    hash_join_exec.isMemoryClearedForTest = true;
    require::NoError(&mut testing::T::new(), hash_join_exec.Open(tmp_ctx));
    let mut chk = exec::NewFirstChunk(&hash_join_exec);
    let err = hash_join_exec.Next(tmp_ctx, &mut chk);
    require::True(&mut testing::T::new(), err.is_err());
    let _ = hash_join_exec.Close();
    util::CheckNoLeakFiles(&test_func_name);
}

#[test]
pub fn test_hash_join_random_fail() {
    // 同时打开 slowWorkers 与 panicOrError failpoint，循环覆盖随机 panic/error 及资源收尾。
    let _restore = config::RestoreFunc();
    config::UpdateGlobal(|conf| conf.TempStoragePath = testing::temp_dir());
    let test_func_name = util::GetFunctionName();
    let mut ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = 32;
    ctx.GetSessionVars().MaxChunkSize = 32;
    let (mut left_data_source, mut right_data_source) =
        buildLeftAndRightDataSource(&ctx, leftCols, rightCols, false);
    let (left_keys, right_keys, left_types, right_types) = outer_join_keys_and_types();
    let params = vec![
        spill_param(true, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], None, None, None, vec![], &test_func_name),
        spill_param(false, &left_keys, &right_keys, &left_types, &right_types, vec![0, 1, 3, 4], vec![0, 2, 3, 4], None, None, None, vec![], &test_func_name),
    ];
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/slowWorkers", "return(true)");
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/executor/join/panicOrError", "return(true)");
    spillChunkSize = 100;
    for _ in 0..15 {
        for join_type in [base::InnerJoin, base::LeftOuterJoin, base::RightOuterJoin] {
            for param in params.clone() {
                test_random_fail(&mut testing::T::new(), &mut ctx, join_type, param, &mut left_data_source, &mut right_data_source);
            }
        }
    }
    util::CheckNoLeakFiles(&test_func_name);
}
*/
