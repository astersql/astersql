// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Plan Replayer 捕获逻辑单元测试。
//
// Plan Replayer 用于导出执行计划与表统计信息以便复现优化结果。
// 本文件验证 `capture_plan_replayer_table_stats` 是否按开关记录
// 逻辑计划（logical plan）中各 DataSource 的表统计。

use crate::main_test::build_logical_for_test;
use crate::optimizer_runtime::capture_plan_replayer_table_stats;
use logicalop_dependency::{DataSource, LogicalPlan, LogicalPlanRef, LogicalUnionAll};

/// 构造带指定 table_id 与行数估计的 DataSource 逻辑计划节点。
fn source(table_id: i64, row_count: f64) -> LogicalPlanRef {
    let mut source = DataSource::default();
    source.TableInfo.ID = table_id;
    source.TableStats.RowCount = row_count;
    Box::new(source)
}

/// 对齐 Go `TestPlanReplayerCaptureRecordJsonStats`：从 SQL 构建真实逻辑计划后，
/// 单表查询记录一个表，双表查询记录两个表。
#[test]
fn test_plan_replayer_capture_records_json_stats_for_each_sql_table() {
    // `main_test` 的 InfoSchema fixture 中，t 与 t2 的稳定表 ID 分别为 1、10001。
    let cases = [
        ("select * from t", &[1_i64][..], &[10_001_i64][..]),
        ("select * from t2", &[10_001_i64][..], &[1_i64][..]),
        ("select * from t, t2", &[1_i64, 10_001_i64][..], &[][..]),
    ];

    for (sql, expected, absent) in cases {
        let (_, plan) = build_logical_for_test(sql).unwrap_or_else(|error| panic!("{error}"));
        let mut variables = variable_dependency::session::SessionVars::default();
        variables.EnablePlanReplayerCapture = true;

        capture_plan_replayer_table_stats(plan.as_ref(), &variables);

        for table_id in expected {
            assert!(
                variables.StmtCtx.ContainsLogicalPlanTableStats(*table_id),
                "{sql:?} must capture table {table_id}"
            );
        }
        for table_id in absent {
            assert!(
                !variables.StmtCtx.ContainsLogicalPlanTableStats(*table_id),
                "{sql:?} must not capture table {table_id}"
            );
        }
        assert!(
            !variables.StmtCtx.ContainsLogicalPlanTableStats(4),
            "{sql:?} must not capture unrelated table t3"
        );
    }
}

/// 重复表只记录一次；开关关闭时不捕获；Clear 后状态清空。
#[test]
fn test_plan_replayer_capture_deduplicates_repeated_table_and_honors_switch() {
    let mut plan = LogicalUnionAll::default();
    plan.SetChildren(vec![source(1, 10.0), source(1, 30.0)]);

    let mut variables = variable_dependency::session::SessionVars::default();
    // 默认未开启捕获时不应写入任何表统计。
    capture_plan_replayer_table_stats(&plan, &variables);
    assert!(!variables.StmtCtx.ContainsLogicalPlanTableStats(1));

    variables.EnablePlanReplayerCapture = true;
    capture_plan_replayer_table_stats(&plan, &variables);
    assert!(variables.StmtCtx.ContainsLogicalPlanTableStats(1));

    variables.StmtCtx.ClearLogicalPlanTableStats();
    assert!(!variables.StmtCtx.ContainsLogicalPlanTableStats(1));
}
