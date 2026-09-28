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

// 关系逻辑算子行为单元测试。
//
// 覆盖聚合统计/谓词拆分、Expand rollup、窗口谓词下推与 Frame 比较、
// 外连接谓词不下推空值供给侧、连接基数上下界，以及 Lateral Apply 行数缩放。
// 使用 RecordingPlan 记录下推到子节点的谓词列 ID。

use crate::*;
use std::any::Any;

/// 构造仅含 UniqueID 的测试列。
fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

/// 将列包装为表达式。
fn expression(column: &Column) -> Expression {
    Box::new(column.clone())
}

/// 构造 COUNT 聚合描述符。
fn count(argument: Expression) -> AggFuncDesc {
    let ctx = exprstatic::NewExprContext(Vec::new());
    aggregation::NewAggFuncDesc(&ctx, aggregation::ast::AggFuncCount, vec![argument], false)
        .expect("build count descriptor")
}

/// 记录谓词下推列 ID 的测试子计划桩。
struct RecordingPlan {
    base: BaseLogicalPlan,
    /// 已下推谓词中出现的列 UniqueID 列表。
    pushed: Vec<i64>,
}

impl RecordingPlan {
    /// 按给定列、行数与 NDV 构造桩计划。
    fn new(columns: Vec<Column>, row_count: f64, ndvs: &[(i64, f64)]) -> Self {
        let mut plan = Self {
            base: BaseLogicalPlan::default(),
            pushed: Vec::new(),
        };
        plan.SetSchema(expression::NewSchema(columns));
        let mut stats = StatsInfo {
            RowCount: row_count,
            ..StatsInfo::default()
        };
        stats.ColNDVs.extend(ndvs.iter().copied());
        plan.SetStats(stats);
        plan
    }
}

impl LogicalPlan for RecordingPlan {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.base
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.base
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        // 记录下推谓词引用的列 ID，并吞掉谓词（视为已消费）。
        self.pushed.extend(
            predicates
                .iter()
                .flat_map(|predicate| expression::ExtractColumns(predicate.as_ref()))
                .map(|column| column.UniqueID),
        );
        Ok(Vec::new())
    }
    fn DeriveStats(&mut self, _reload: bool) -> Result<(StatsInfo, bool)> {
        Ok((self.StatsInfo().cloned().unwrap_or_default(), false))
    }
}

/// 投影通过 LogicalPlan 接口推导输出列 NDV，不能只继承子节点列 ID。
#[test]
fn projection_trait_derives_output_column_statistics() {
    let input = column(1);
    let output = column(2);
    let child = RecordingPlan::new(vec![input.clone()], 100.0, &[(1, 10.0)]);
    let mut projection = LogicalProjection {
        Exprs: vec![expression(&input)],
        ..LogicalProjection::default()
    };
    projection.SetSchema(expression::NewSchema(vec![output.clone()]));
    projection.SetChildren(vec![Box::new(child)]);

    let plan: &mut dyn LogicalPlan = &mut projection;
    let (stats, changed) = plan
        .DeriveStats(true)
        .expect("derive projection statistics");

    assert!(changed);
    assert_eq!(stats.RowCount, 100.0);
    assert_eq!(stats.ColNDVs[&output.UniqueID], 10.0);
    assert!(!stats.ColNDVs.contains_key(&input.UniqueID));
}

/// 计算投影列在子节点中不存在；MPP hash 分区应放宽为 Any，
/// 由 Projection 计算列后再执行 Exchange，不能剪掉整个物理候选。
#[test]
fn projection_keeps_mpp_hash_property_candidate_for_computed_output() {
    let input = column(1);
    let output = column(2);
    let mut projection = LogicalProjection {
        Exprs: vec![Box::new(expression::Constant::null(
            expression::mysql::TypeNull,
        ))],
        ..LogicalProjection::default()
    };
    projection.SetSchema(expression::NewSchema(vec![output.clone()]));

    let mut required = property::PhysicalProperty::default();
    required.TaskTp = property::MppTaskType;
    required.MPPPartitionTp = property::HashType;
    required.MPPPartitionCols = vec![property::MPPPartitionColumn {
        Col: output,
        CollateID: 0,
    }];

    let (child, ok) = projection.TryToGetChildProp(&required);
    assert!(
        ok,
        "computed projection must not eliminate the MPP candidate"
    );
    let child = child.expect("projection returns a child property");
    assert_eq!(child.TaskTp, property::MppTaskType);
    assert_eq!(child.MPPPartitionTp, property::AnyType);
    assert!(child.MPPPartitionCols.is_empty());
    assert_eq!(input.UniqueID, 1);
}

/// 聚合用分组 NDV 估行数，清空 GROUP BY 后全局聚合行数为 1。
#[test]
fn aggregation_uses_group_ndv_and_preserves_global_group() {
    let input = column(1);
    let output = column(2);
    let child = RecordingPlan::new(vec![input.clone()], 100.0, &[(1, 10.0)]);
    let mut aggregation = LogicalAggregation::default();
    aggregation.GroupByItems = vec![expression(&input)];
    aggregation.AggFuncs = vec![count(expression(&input))];
    aggregation.SetSchema(expression::NewSchema(vec![output.clone()]));
    aggregation.SetChildren(vec![Box::new(child)]);

    let (stats, changed) = aggregation
        .DeriveStats(false)
        .expect("derive aggregation statistics");
    assert!(changed);
    assert_eq!(stats.RowCount, 10.0);
    assert_eq!(stats.ColNDVs[&output.UniqueID], 10.0);
    assert_eq!(aggregation.InputCount, 100.0);

    aggregation.GroupByItems.clear();
    let (global, _) = aggregation
        .DeriveStats(true)
        .expect("derive global aggregation statistics");
    assert_eq!(global.RowCount, 1.0);
}

/// 仅分组不变谓词可下推，聚合输出列谓词保留。
#[test]
fn aggregation_only_pushes_group_invariant_predicates() {
    let group = column(10);
    let aggregate_output = column(11);
    let aggregation = LogicalAggregation {
        GroupByItems: vec![expression(&group)],
        ..LogicalAggregation::default()
    };
    let (pushed, retained) = aggregation
        .splitCondForAggregation(vec![expression(&group), expression(&aggregate_output)]);
    assert_eq!(pushed.len(), 1);
    assert_eq!(retained.len(), 1);
    assert_eq!(
        expression::ExtractColumns(pushed[0].as_ref())[0].UniqueID,
        group.UniqueID
    );
}

/// Expand 为 rollup 层级生成投影，缺失分组列填 NULL，并产出 grouping id。
#[test]
fn expand_generates_null_filled_rollup_levels_and_grouping_ids() {
    let group = column(20);
    let gid = column(21);
    let mut expand = LogicalExpand {
        DistinctGroupByCol: vec![group.clone()],
        RollupGroupingSets: GroupingSets(vec![
            GroupingSet {
                ColumnIDs: [group.UniqueID].into_iter().collect(),
            },
            GroupingSet::default(),
        ]),
        GID: Some(gid.clone()),
        ..LogicalExpand::default()
    };
    expand.SetSchema(expression::NewSchema(vec![group, gid]));
    expand.GenLevelProjections();

    assert_eq!(expand.RollupGroupingIDs, vec![1, 0]);
    assert_eq!(expand.LevelExprs.len(), 2);
    assert!(expand.LevelExprs[0][0].as_column().is_some());
    assert!(
        expand.LevelExprs[1][0]
            .as_any()
            .is::<expression::Constant>()
    );
}

/// Expand 复制行后丢弃子节点唯一键，且 MaxOneRow 为 false。
#[test]
fn expand_build_key_info_drops_child_uniqueness_after_replication() {
    let key = column(22);
    let mut child = RecordingPlan::new(vec![key.clone()], 1.0, &[(22, 1.0)]);
    child.Schema_mut().PKOrUK.push(vec![key.clone()]);
    let mut expand = LogicalExpand {
        RollupGroupingSets: GroupingSets(vec![GroupingSet::default(), GroupingSet::default()]),
        ..LogicalExpand::default()
    };
    expand.SetSchema(expression::NewSchema(vec![key]));
    expand.SetChildren(vec![Box::new(child)]);
    expand.BuildKeyInfo();
    assert!(expand.Schema().PKOrUK.is_empty());
    assert!(!expand.MaxOneRow());
}

/// 窗口算子下推分区列谓词，保留结果列谓词。
#[test]
fn window_pushes_partition_predicates_but_retains_result_predicates() {
    let partition = column(30);
    let result = column(31);
    let child = RecordingPlan::new(vec![partition.clone()], 8.0, &[(30, 4.0)]);
    let mut window = LogicalWindow {
        PartitionBy: vec![SortItem {
            Col: partition.clone(),
            Desc: false,
        }],
        WindowFuncDescs: vec![WindowFuncDesc {
            Name: "row_number".to_owned(),
            Args: Vec::new(),
        }],
        ..LogicalWindow::default()
    };
    window.SetSchema(expression::NewSchema(vec![
        partition.clone(),
        result.clone(),
    ]));
    window.SetChildren(vec![Box::new(child)]);

    let retained = window
        .PredicatePushDown(vec![expression(&partition), expression(&result)])
        .expect("window predicate pushdown");
    assert_eq!(retained.len(), 1);
    assert_eq!(
        expression::ExtractColumns(retained[0].as_ref())[0].UniqueID,
        result.UniqueID
    );
    let child = window.Children()[0]
        .as_any()
        .downcast_ref::<RecordingPlan>()
        .expect("recording child");
    assert_eq!(child.pushed, vec![partition.UniqueID]);
}

/// Frame/OrderBy 相等性比较包含边界与排序项。
#[test]
fn window_frame_comparison_includes_bounds_and_ordering() {
    let order = column(40);
    let frame = WindowFrame {
        Type: FrameType::Rows,
        Start: Some(FrameBound {
            Type: BoundType::Preceding,
            Num: 2,
            ..FrameBound::default()
        }),
        End: Some(FrameBound::default()),
    };
    let left = LogicalWindow {
        OrderBy: vec![SortItem {
            Col: order.clone(),
            Desc: false,
        }],
        Frame: Some(frame.clone()),
        ..LogicalWindow::default()
    };
    let mut right = LogicalWindow {
        OrderBy: vec![SortItem {
            Col: order,
            Desc: false,
        }],
        Frame: Some(frame),
        ..LogicalWindow::default()
    };
    assert!(left.equalFrame(&right));
    assert!(left.equalOrderBy(&right));
    right.Frame.as_mut().unwrap().Start.as_mut().unwrap().Num = 3;
    assert!(!left.equalFrame(&right));
}

/// 左外连接：谓词不下推到空值供给侧（右表）。
#[test]
fn outer_join_does_not_push_predicates_into_null_supplying_side() {
    let left_column = column(50);
    let right_column = column(51);
    let left = RecordingPlan::new(vec![left_column.clone()], 10.0, &[(50, 10.0)]);
    let right = RecordingPlan::new(vec![right_column.clone()], 5.0, &[(51, 5.0)]);
    let mut join = LogicalJoin {
        JoinType: JoinType::LeftOuterJoin,
        ..LogicalJoin::default()
    };
    join.SetSchema(expression::NewSchema(vec![
        left_column.clone(),
        right_column.clone(),
    ]));
    join.SetChildren(vec![Box::new(left), Box::new(right)]);

    let retained = join
        .PredicatePushDown(vec![expression(&left_column), expression(&right_column)])
        .expect("outer join predicate pushdown");
    assert_eq!(retained.len(), 1);
    assert_eq!(
        expression::ExtractColumns(retained[0].as_ref())[0].UniqueID,
        right_column.UniqueID
    );
    assert_eq!(
        join.Children()[0]
            .as_any()
            .downcast_ref::<RecordingPlan>()
            .unwrap()
            .pushed,
        vec![left_column.UniqueID]
    );
    assert!(
        join.Children()[1]
            .as_any()
            .downcast_ref::<RecordingPlan>()
            .unwrap()
            .pushed
            .is_empty()
    );
}

/// 内连接基数为乘积估计；外连接行数不低于保留侧。
#[test]
fn join_statistics_respect_inner_and_outer_cardinality_bounds() {
    let left_column = column(60);
    let right_column = column(61);
    let make_join = |join_type| {
        let mut join = LogicalJoin {
            JoinType: join_type,
            ..LogicalJoin::default()
        };
        join.SetSchema(expression::NewSchema(vec![
            left_column.clone(),
            right_column.clone(),
        ]));
        join.SetChildren(vec![
            Box::new(RecordingPlan::new(
                vec![left_column.clone()],
                10.0,
                &[(60, 10.0)],
            )),
            Box::new(RecordingPlan::new(
                vec![right_column.clone()],
                5.0,
                &[(61, 5.0)],
            )),
        ]);
        join
    };
    let mut inner = make_join(JoinType::InnerJoin);
    assert_eq!(inner.DeriveStats(false).unwrap().0.RowCount, 50.0);
    let mut outer = make_join(JoinType::LeftOuterJoin);
    assert!(outer.DeriveStats(false).unwrap().0.RowCount >= 10.0);
}

/// 半连接按 Go 逻辑计划同时缩放保留侧行数与列 NDV。
#[test]
fn semi_join_scales_retained_column_ndv_with_row_count() {
    let left_column = column(62);
    let right_column = column(63);
    let mut join = LogicalJoin {
        JoinType: JoinType::AntiSemiJoin,
        ..LogicalJoin::default()
    };
    join.SetSchema(expression::NewSchema(vec![left_column.clone()]));
    join.SetChildren(vec![
        Box::new(RecordingPlan::new(
            vec![left_column.clone()],
            100.0,
            &[(left_column.UniqueID, 64.0)],
        )),
        Box::new(RecordingPlan::new(vec![right_column], 20.0, &[(63, 20.0)])),
    ]);

    let (stats, changed) = join
        .DeriveStats(false)
        .expect("derive anti semi join stats");
    assert!(changed);
    assert_eq!(stats.RowCount, 80.0);
    assert_eq!(stats.ColNDVs[&left_column.UniqueID], 51.2);
}

/// Pseudo join-key NDV zero means unknown and falls back to the input rows;
/// a real positive NDV remains authoritative.
#[test]
fn join_pseudo_zero_ndv_uses_input_cardinality() {
    let mut stats = StatsInfo {
        RowCount: 8_000.0,
        ..StatsInfo::default()
    };
    stats.ColNDVs.insert(60, 0.0);
    stats.ColNDVs.insert(61, 125.0);

    assert_eq!(effective_join_column_ndv(&stats, 60), 6_400.0);
    assert_eq!(effective_join_column_ndv(&stats, 61), 125.0);
    assert_eq!(effective_join_column_ndv(&stats, 62), 6_400.0);

    stats.RowCount = 10_000.0;
    assert_eq!(effective_join_column_ndv(&stats, 60), 8_000.0);
    assert_eq!(effective_join_column_ndv(&stats, 62), 8_000.0);
}

#[test]
fn aggregation_preserves_known_group_ndv_above_pseudo_cap() {
    let group = column(63);
    let output = column(64);
    let child = RecordingPlan::new(vec![group.clone()], 2_012_882.45, &[(63, 99_360.0)]);
    let mut aggregation = LogicalAggregation::default();
    aggregation.GroupByItems = vec![expression(&group)];
    aggregation.AggFuncs = vec![count(expression(&group))];
    aggregation.SetSchema(expression::NewSchema(vec![output.clone()]));
    aggregation.SetChildren(vec![Box::new(child)]);

    let (stats, _) = aggregation.DeriveStats(true).expect("derive GROUP BY NDV");
    assert_eq!(stats.RowCount, 99_360.0);
    assert_eq!(stats.ColNDVs[&output.UniqueID], 99_360.0);
}

/// Lateral Apply：内表行数按相关外表 NDV 缩放。
#[test]
fn lateral_apply_scales_inner_rows_by_correlated_outer_ndv() {
    let outer_column = column(70);
    let inner_column = column(71);
    let mut apply = LogicalApply {
        LogicalJoin: LogicalJoin {
            JoinType: JoinType::InnerJoin,
            ..LogicalJoin::default()
        },
        CorCols: vec![CorrelatedColumn {
            column: outer_column.clone(),
            data: None,
        }],
        IsLateral: true,
        ..LogicalApply::default()
    };
    apply.SetSchema(expression::NewSchema(vec![
        outer_column.clone(),
        inner_column.clone(),
    ]));
    apply.SetChildren(vec![
        Box::new(RecordingPlan::new(vec![outer_column], 100.0, &[(70, 10.0)])),
        Box::new(RecordingPlan::new(vec![inner_column], 20.0, &[(71, 20.0)])),
    ]);

    let (stats, changed) = apply
        .DeriveStats(false)
        .expect("derive lateral apply statistics");
    assert!(changed);
    assert_eq!(stats.RowCount, 200.0);
}
