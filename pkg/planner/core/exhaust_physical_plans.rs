// Copyright 2017 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 穷举物理计划：将逻辑 Join/Apply/聚合/Limit 等展开为候选物理算子。
//
// 在给定物理属性（排序、任务类型等）下生成 HashJoin、MergeJoin、IndexJoin 家族、
// MPP Broadcast/Shuffle Join 等候选，并结合 Hint、伪统计与扫描比例做剪枝与告警。
// 「穷举」指枚举可行物理实现供后续 FindBestTask 按代价择优。

use crate::find_best_task::{
    AccessPath, DataSource, PhysicalProperty, PlanAlternative, candidatePath, convertToIndexScan,
    convertToTableScan, getIndexCandidate, getTableCandidate,
};
use crate::task::{Expression, JoinType, PlanKind, PlanNode, StatsInfo, StoreType, Task, TaskType};
use std::collections::{HashMap, HashSet};

/// Index Join 剪枝：探测侧行数低于此阈值不剪。
pub const indexJoinPruneMinProbeRows: f64 = 100_000.0;
/// Index Join 剪枝：构建侧行数低于此阈值不剪。
pub const indexJoinPruneMinBuildRows: f64 = 100.0;
/// Index Join 方法编号：普通 IndexJoin。
pub const indexJoinMethod: i32 = 0;
/// Index Join 方法编号：IndexHashJoin。
pub const indexHashJoinMethod: i32 = 1;
/// Index Join 方法编号：IndexMergeJoin。
pub const indexMergeJoinMethod: i32 = 2;

#[derive(Clone, Debug, Default)]
/// Join 相关优化器 Hint：偏好/禁止某种物理 Join，以及强制 Build/Probe 侧。
pub struct JoinHints {
    pub prefer_hash: bool,
    pub no_hash: bool,
    pub prefer_merge: bool,
    pub prefer_index: bool,
    pub prefer_index_hash: bool,
    pub prefer_index_merge: bool,
    pub force_left_build: bool,
    pub force_right_build: bool,
    pub force_left_probe: bool,
    pub force_right_probe: bool,
    pub prefer_broadcast: bool,
    pub prefer_shuffle: bool,
}

#[derive(Clone, Debug, Default)]
/// Index Join 运行时属性：内外 join key、其它条件与平均内表行数等。
pub struct IndexJoinRuntimeProp {
    pub inner_join_keys: Vec<usize>,
    pub outer_join_keys: Vec<usize>,
    pub other_conditions: Vec<Expression>,
    pub avg_inner_row_count: f64,
    pub table_range_scan: bool,
}

#[derive(Clone, Debug)]
/// 穷举用的逻辑 Join 摘要：统计、schema、Hint、MPP/TiFlash 能力等。
pub struct LogicalJoin {
    pub join_type: JoinType,
    pub children_stats: [StatsInfo; 2],
    pub children_schema_len: [usize; 2],
    pub join_keys: [Vec<usize>; 2],
    pub other_conditions: Vec<Expression>,
    pub stats: StatsInfo,
    pub hints: JoinHints,
    pub hash_join_v2: bool,
    pub mpp_enabled: bool,
    pub tiflash_replicas: [bool; 2],
    pub expressions_pushable: bool,
    pub index_join_scan_ratio: f64,
    pub pseudo_stats: [bool; 2],
    pub fine_grained_shuffle: bool,
    pub mpp_store_count: usize,
    pub broadcast_row_limit: f64,
    pub broadcast_size_limit: f64,
    pub warnings: Vec<String>,
}
impl Default for LogicalJoin {
    fn default() -> Self {
        Self {
            join_type: JoinType::Inner,
            children_stats: [StatsInfo::default(), StatsInfo::default()],
            children_schema_len: [0, 0],
            join_keys: [Vec::new(), Vec::new()],
            other_conditions: Vec::new(),
            stats: StatsInfo::default(),
            hints: JoinHints::default(),
            hash_join_v2: true,
            mpp_enabled: true,
            tiflash_replicas: [false, false],
            expressions_pushable: true,
            index_join_scan_ratio: 0.0,
            pseudo_stats: [false, false],
            fine_grained_shuffle: false,
            mpp_store_count: 1,
            broadcast_row_limit: 100_000.0,
            broadcast_size_limit: (100 << 20) as f64,
            warnings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 逻辑聚合摘要：分组列与 Hash/Stream/MPP 相位偏好。
pub struct LogicalAggregation {
    pub group_by_columns: Vec<usize>,
    pub preferred_hash: bool,
    pub preferred_stream: bool,
    pub mpp_1phase: bool,
    pub mpp_2phase: bool,
}
#[derive(Clone, Debug, Default)]
/// Limit/TopN 逻辑摘要：是否 TopN、下推偏好与 offset/count。
pub struct LogicalLimitTopN {
    pub is_topn: bool,
    pub prefer_push_down: bool,
    pub threshold: u64,
    pub offset: u64,
    pub count: u64,
}

#[derive(Clone, Debug)]
/// 参与穷举的逻辑算子变体。
pub enum LogicalOperator {
    Cte,
    Sort,
    TopN(LogicalLimitTopN),
    Lock,
    Join(LogicalJoin),
    Apply(LogicalJoin),
    Limit(LogicalLimitTopN),
    Window,
    Expand,
    UnionAll,
    Sequence,
    Selection,
    MaxOneRow,
    UnionScan,
    Projection,
    Aggregation(LogicalAggregation),
    PartitionUnionAll,
    Mock(Vec<PlanNode>),
}

#[derive(Clone, Debug)]
/// 待穷举的逻辑计划节点包装。
pub struct ExhaustLogicalPlan {
    pub operator: LogicalOperator,
    pub base_alternatives: Vec<PlanNode>,
    pub warnings: Vec<String>,
}

/// 构造带物理计划节点的 PlanAlternative。
fn alternative(
    plan: PlanNode,
    child_count: usize,
    prop: &PhysicalProperty,
    hint: bool,
    preferred: bool,
) -> PlanAlternative {
    PlanAlternative {
        plan,
        child_properties: vec![prop.clone(); child_count],
        hint,
        preferred,
    }
}

/// 对逻辑算子在给定物理属性下穷举物理候选计划。
pub fn exhaustPhysicalPlans(
    logical: &mut ExhaustLogicalPlan,
    prop: &PhysicalProperty,
) -> (Vec<Vec<PlanAlternative>>, bool) {
    match &mut logical.operator {
        LogicalOperator::Join(join) => exhaustPhysicalPlans4LogicalJoin(join, prop),
        LogicalOperator::Apply(join) => {
            let (plans, hint) = exhaustPhysicalPlans4LogicalApply(join, prop);
            (vec![plans], hint)
        }
        LogicalOperator::TopN(topn) | LogicalOperator::Limit(topn) => {
            let mut local = PlanNode::new(if topn.is_topn {
                PlanKind::TopN
            } else {
                PlanKind::Limit
            });
            local.offset = topn.offset;
            local.count = topn.count;
            let mut pushed = local.clone();
            pushed.offset = 0;
            pushed.count = topn.offset.saturating_add(topn.count);
            (
                vec![
                    vec![alternative(pushed, 1, prop, false, topn.prefer_push_down)],
                    vec![alternative(local, 1, prop, false, false)],
                ],
                true,
            )
        }
        LogicalOperator::Aggregation(agg) => {
            let mut hash = PlanNode::new(PlanKind::HashAgg);
            hash.group_items = agg
                .group_by_columns
                .iter()
                .map(|c| Expression {
                    column: Some(*c),
                    ..Expression::default()
                })
                .collect();
            let stream = PlanNode {
                kind: PlanKind::StreamAgg,
                group_items: hash.group_items.clone(),
                ..PlanNode::default()
            };
            (
                vec![vec![
                    alternative(hash, 1, prop, false, agg.preferred_hash),
                    alternative(stream, 1, prop, false, agg.preferred_stream),
                ]],
                !agg.preferred_hash || !agg.preferred_stream,
            )
        }
        LogicalOperator::Mock(plans) => (
            vec![
                plans
                    .iter()
                    .cloned()
                    .map(|p| alternative(p, 0, prop, false, false))
                    .collect(),
            ],
            true,
        ),
        operator => {
            let kind = match operator {
                LogicalOperator::Cte => PlanKind::Cte,
                LogicalOperator::Sort => PlanKind::Sort,
                LogicalOperator::Lock => PlanKind::Other("Lock".into()),
                LogicalOperator::Window => PlanKind::Window,
                LogicalOperator::Expand => PlanKind::Expand,
                LogicalOperator::UnionAll | LogicalOperator::PartitionUnionAll => {
                    PlanKind::UnionAll
                }
                LogicalOperator::Sequence => PlanKind::Sequence,
                LogicalOperator::Selection => PlanKind::Selection,
                LogicalOperator::MaxOneRow => PlanKind::Other("MaxOneRow".into()),
                LogicalOperator::UnionScan => PlanKind::UnionScan,
                LogicalOperator::Projection => PlanKind::Projection,
                _ => unreachable!(),
            };
            let plans = if logical.base_alternatives.is_empty() {
                vec![PlanNode::new(kind)]
            } else {
                logical.base_alternatives.clone()
            };
            (
                vec![
                    plans
                        .into_iter()
                        .map(|p| alternative(p, 1, prop, false, false))
                        .collect(),
                ],
                true,
            )
        }
    }
}

/// 构造 HashJoin 物理计划节点。
fn hash_join_plan(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    inner: usize,
    use_outer_to_build: bool,
    store: StoreType,
) -> PlanNode {
    let mut plan = PlanNode::new(PlanKind::HashJoin);
    plan.inner_child = inner;
    plan.join_type = join.join_type;
    plan.join_keys = join.join_keys[0].len();
    plan.stats = join.stats.clone();
    plan.store = store;
    plan.flags.fine_grained_shuffle = join.fine_grained_shuffle;
    if prop.expected_count < join.stats.row_count && join.stats.row_count > 0.0 {
        plan.expected_count = prop.expected_count;
    }
    plan.labels.insert(
        "use_outer_to_build".into(),
        if use_outer_to_build { 1.0 } else { 0.0 },
    );
    plan
}

/// 按属性生成单个 HashJoin 候选。
pub fn getHashJoin(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    inner_idx: usize,
    use_outer_to_build: bool,
) -> Vec<PlanNode> {
    let base = hash_join_plan(join, prop, inner_idx, use_outer_to_build, StoreType::TiDb);
    if prop.index_join_cols > 0 {
        let mut left = base.clone();
        left.labels.insert("index_join_prop_child".into(), 0.0);
        let mut right = base;
        right.labels.insert("index_join_prop_child".into(), 1.0);
        vec![left, right]
    } else {
        vec![base]
    }
}

/// 枚举 HashJoin（含左右 Build）候选；返回计划列表与是否满足属性。
pub fn getHashJoins(join: &mut LogicalJoin, prop: &PhysicalProperty) -> (Vec<PlanNode>, bool) {
    if !prop.is_sort_empty() {
        return (Vec::new(), false);
    }
    let mut force_left = join.hints.force_left_build || join.hints.force_right_probe;
    let mut force_right = join.hints.force_right_build || join.hints.force_left_probe;
    if force_left && force_right {
        join.warnings
            .push("conflicting HASH_JOIN_BUILD and HASH_JOIN_PROBE hints".into());
        force_left = false;
        force_right = false;
    }
    let mut plans = Vec::new();
    match join.join_type {
        JoinType::Semi | JoinType::AntiSemi => {
            if join.hash_join_v2 {
                if !force_left {
                    plans.extend(getHashJoin(join, prop, 1, false));
                }
                if !force_right {
                    plans.extend(getHashJoin(join, prop, 1, true));
                }
            } else {
                plans.extend(getHashJoin(join, prop, 1, false));
                if force_left || force_right {
                    join.warnings.push(format!(
                        "HASH_JOIN_BUILD and HASH_JOIN_PROBE hints are not supported for {:?} with hash join version 1",
                        join.join_type
                    ));
                    force_left = false;
                    force_right = false;
                }
            }
        }
        JoinType::LeftOuter => {
            if !force_left {
                plans.extend(getHashJoin(join, prop, 1, false));
            }
            if !force_right {
                plans.extend(getHashJoin(join, prop, 1, true));
            }
        }
        JoinType::RightOuter => {
            if !force_left {
                plans.extend(getHashJoin(join, prop, 0, true));
            }
            if !force_right {
                plans.extend(getHashJoin(join, prop, 0, false));
            }
        }
        JoinType::Inner => {
            if force_left {
                plans.extend(getHashJoin(join, prop, 0, false));
            } else if force_right {
                plans.extend(getHashJoin(join, prop, 1, false));
            } else {
                plans.extend(getHashJoin(join, prop, 1, false));
                plans.extend(getHashJoin(join, prop, 0, false));
            }
        }
    }
    let forced = join.hints.prefer_hash || force_left || force_right;
    if join.hints.no_hash && !forced {
        plans.clear();
    } else if join.hints.no_hash && forced {
        join.warnings
            .push("HASH_JOIN takes precedence over NO_HASH_JOIN".into());
    }
    (plans, forced)
}

/// 静态构造 IndexJoin 物理节点骨架。
pub fn constructIndexJoinStatic(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    outer_idx: usize,
    runtime: &IndexJoinRuntimeProp,
) -> Vec<PlanNode> {
    let mut plan = PlanNode::new(PlanKind::IndexJoin);
    plan.inner_child = 1 - outer_idx;
    plan.join_type = join.join_type;
    plan.join_keys = runtime.inner_join_keys.len();
    plan.stats = join.stats.clone();
    plan.expected_count = prop.expected_count;
    plan.ranges = if runtime.table_range_scan {
        1
    } else {
        runtime.inner_join_keys.len().max(1)
    };
    vec![plan]
}
/// 静态构造 IndexHashJoin 物理节点骨架。
pub fn constructIndexHashJoinStatic(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    outer_idx: usize,
    runtime: &IndexJoinRuntimeProp,
) -> Vec<PlanNode> {
    constructIndexJoinStatic(join, prop, outer_idx, runtime)
        .into_iter()
        .map(|mut p| {
            p.kind = PlanKind::IndexHashJoin;
            p
        })
        .collect()
}
/// 补全 Index Join 物理计划的反馈与路径信息。
pub fn completePhysicalIndexJoin(
    mut plan: PlanNode,
    inner: Task,
    extract_other_eq: bool,
) -> PlanNode {
    if let Some(child) = inner.plan() {
        plan.children.push(child.clone());
    }
    if extract_other_eq {
        plan.labels.insert("extracted_other_eq".into(), 1.0);
    }
    plan
}

/// 取探测侧全表扫描估计行数，供 Index Join 剪枝。
pub fn getProbeFullScanRowsForIndexJoinPrune(stats: &StatsInfo) -> f64 {
    stats.row_count.max(0.0)
}
/// 是否因伪统计而跳过 Index Join 扫描比例剪枝。
pub fn hasPseudoStatsForIndexJoinPrune(pseudo: bool) -> bool {
    pseudo
}
/// 按内表扫描比例与行数阈值决定是否剪掉 Index Join。
pub fn shouldPruneIndexJoinByScanRatio(
    threshold: f64,
    build_rows: f64,
    probe_rows_one: f64,
    build_pseudo: bool,
    probe_pseudo: bool,
    inner_full_scan_rows: f64,
) -> bool {
    if threshold <= 0.0
        || build_rows < indexJoinPruneMinBuildRows
        || build_pseudo
        || probe_pseudo
        || inner_full_scan_rows <= 0.0
    {
        return false;
    }
    let probe_rows = build_rows * probe_rows_one;
    if probe_rows < indexJoinPruneMinProbeRows {
        return false;
    }
    let index_rows = build_rows + probe_rows;
    let hash_rows = build_rows + inner_full_scan_rows;
    hash_rows > 0.0 && index_rows / hash_rows >= threshold
}

/// 以外表下标为 Probe 侧枚举 Index Join 候选。
pub fn enumerateIndexJoinByOuterIdx(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    outer_idx: usize,
    enable_ratio_prune: bool,
) -> Vec<PlanNode> {
    if prop.sort_items.windows(2).any(|v| v[0].desc != v[1].desc)
        || prop
            .sort_items
            .iter()
            .any(|s| s.column >= join.children_schema_len[outer_idx])
    {
        return Vec::new();
    }
    let build_rows = join.children_stats[outer_idx].row_count;
    let avg_inner = if build_rows > 0.0 {
        join.stats.row_count / build_rows
    } else {
        0.0
    };
    if enable_ratio_prune
        && shouldPruneIndexJoinByScanRatio(
            join.index_join_scan_ratio,
            build_rows,
            avg_inner,
            join.pseudo_stats[outer_idx],
            join.pseudo_stats[1 - outer_idx],
            join.children_stats[1 - outer_idx].row_count,
        )
    {
        return Vec::new();
    }
    let base = IndexJoinRuntimeProp {
        inner_join_keys: join.join_keys[1 - outer_idx].clone(),
        outer_join_keys: join.join_keys[outer_idx].clone(),
        other_conditions: join.other_conditions.clone(),
        avg_inner_row_count: avg_inner,
        table_range_scan: true,
    };
    let mut index = base.clone();
    index.table_range_scan = false;
    let mut plans = Vec::new();
    plans.extend(constructIndexJoinStatic(join, prop, outer_idx, &base));
    plans.extend(constructIndexJoinStatic(join, prop, outer_idx, &index));
    plans.extend(constructIndexHashJoinStatic(join, prop, outer_idx, &base));
    plans.extend(constructIndexHashJoinStatic(join, prop, outer_idx, &index));
    plans
}

/// 检查算子自身是否满足属性要求的任务类型（Root/Cop/MPP 等）。
pub fn checkOpSelfSatisfyPropTaskTypeRequirement(
    can_tikv: bool,
    can_tiflash: bool,
    prop: &PhysicalProperty,
) -> bool {
    match prop.task_type {
        TaskType::Mpp => can_tiflash,
        TaskType::CopSingleRead | TaskType::CopMultiRead => can_tikv,
        TaskType::Root => true,
    }
}
/// 检查 Index Join 内表任务是否允许携带聚合。
pub fn checkIndexJoinInnerTaskWithAgg(
    group_by: &[usize],
    data_source_schema: &[usize],
    runtime: &IndexJoinRuntimeProp,
) -> bool {
    let groups: HashSet<_> = group_by.iter().copied().collect();
    let keys: HashSet<_> = runtime
        .inner_join_keys
        .iter()
        .copied()
        .filter(|k| data_source_schema.contains(k))
        .collect();
    keys.len() <= groups.len() && keys.is_subset(&groups)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Index Join 内表子树可接受的模式（Scan/Selection/Agg 等）。
pub enum InnerPattern {
    DataSource,
    Projection,
    Selection,
    InnerJoin,
    OuterJoin,
    Aggregation,
    UnionScan,
    Fence,
}
/// 判定内表子计划模式是否可被 Index Join 接纳。
pub fn admitIndexJoinInnerChildPattern(
    pattern: InnerPattern,
    enable_multi: bool,
    aggregation_valid: bool,
    prefers_tiflash: bool,
) -> bool {
    match pattern {
        InnerPattern::DataSource => !prefers_tiflash,
        InnerPattern::Projection | InnerPattern::Selection | InnerPattern::InnerJoin => {
            enable_multi
        }
        InnerPattern::Aggregation => enable_multi && aggregation_valid,
        InnerPattern::UnionScan => true,
        InnerPattern::OuterJoin | InnerPattern::Fence => false,
    }
}

/// 按 Index Join 属性将 DataSource 转为 IndexScan 任务。
pub fn buildDataSource2IndexScanByIndexJoinProp(
    ds: &DataSource,
    prop: &PhysicalProperty,
    runtime: &IndexJoinRuntimeProp,
) -> Task {
    let mut best: Option<candidatePath> = None;
    for path in &ds.paths {
        if path.is_table_path() || path.index.as_ref().is_some_and(|i| i.multi_valued) {
            continue;
        }
        let candidate = getIndexCandidate(ds, path, prop);
        if best
            .as_ref()
            .is_none_or(|old| candidate.path.count_after_access < old.path.count_after_access)
        {
            best = Some(candidate);
        }
    }
    let Some(candidate) = best else {
        return Task::invalid("no index join index path");
    };
    let mut task = convertToIndexScan(ds, prop, &candidate);
    completeIndexJoinFeedBackInfo(&mut task, runtime, &candidate.path);
    task
}
/// 按 Index Join 属性将 DataSource 转为 TableScan 任务。
pub fn buildDataSource2TableScanByIndexJoinProp(
    ds: &DataSource,
    prop: &PhysicalProperty,
    runtime: &IndexJoinRuntimeProp,
) -> Task {
    let Some(path) = ds
        .paths
        .iter()
        .filter(|p| p.is_table_path())
        .min_by(|a, b| a.count_after_access.total_cmp(&b.count_after_access))
    else {
        return Task::invalid("no index join table path");
    };
    let candidate = getTableCandidate(ds, path, prop);
    let mut task = convertToTableScan(ds, prop, &candidate);
    completeIndexJoinFeedBackInfo(&mut task, runtime, path);
    task
}
/// 回填 Index Join 的路径/选择反馈信息。
pub fn completeIndexJoinFeedBackInfo(
    task: &mut Task,
    runtime: &IndexJoinRuntimeProp,
    path: &AccessPath,
) {
    if let Some(plan) = match task {
        Task::Root { plan, .. } | Task::Mpp { plan, .. } => plan.as_mut(),
        Task::Cop { index_plan, .. } => index_plan.as_mut(),
        Task::Invalid { .. } => None,
    } {
        plan.labels
            .insert("avg_inner_rows".into(), runtime.avg_inner_row_count);
        plan.labels
            .insert("index_columns".into(), path.index_columns.len() as f64);
    }
}

/// 构造 DataSource → TableScan 的物理任务。
pub fn constructDS2TableScanTask(
    ds: &DataSource,
    _path: &AccessPath,
    runtime: &IndexJoinRuntimeProp,
    keep_order: bool,
    desc: bool,
) -> Task {
    let prop = PhysicalProperty {
        sort_items: if keep_order {
            vec![crate::find_best_task::SortItem { column: 0, desc }]
        } else {
            Vec::new()
        },
        ..PhysicalProperty::default()
    };
    buildDataSource2TableScanByIndexJoinProp(ds, &prop, runtime)
}
/// 构造 DataSource → IndexScan 的物理任务。
pub fn constructDS2IndexScanTask(
    ds: &DataSource,
    path: &AccessPath,
    runtime: &IndexJoinRuntimeProp,
    keep_order: bool,
    desc: bool,
) -> Task {
    let prop = PhysicalProperty {
        sort_items: if keep_order {
            vec![crate::find_best_task::SortItem {
                column: path.index_columns.first().copied().unwrap_or(0),
                desc,
            }]
        } else {
            Vec::new()
        },
        ..PhysicalProperty::default()
    };
    let candidate = getIndexCandidate(ds, path, &prop);
    let mut task = convertToIndexScan(ds, &prop, &candidate);
    completeIndexJoinFeedBackInfo(&mut task, runtime, path);
    task
}

/// 若过滤为 IN 列表则返回元素个数，否则 0。
pub fn getInListLength(filter: &Expression) -> usize {
    filter
        .name
        .strip_prefix("in:")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}
/// 表达式树中是否含超过阈值的大 IN 列表。
pub fn containsLargeInList(expr: &Expression, threshold: usize) -> bool {
    getInListLength(expr) > threshold
}
/// 将大 IN 从 Index Join Probe 可下推过滤中拆出。
pub fn splitLargeInListFiltersForIndexJoinProbe(
    filters: &[Expression],
    threshold: usize,
) -> (Vec<Expression>, Vec<Expression>) {
    filters
        .iter()
        .cloned()
        .partition(|f| !containsLargeInList(f, threshold))
}
/// 从直方图 NDV 集合取多列 NDV 下界。
pub fn getColsNDVLowerBoundFromHistColl(columns: &[usize], ndv: &HashMap<usize, i64>) -> i64 {
    columns
        .iter()
        .filter_map(|c| ndv.get(c))
        .copied()
        .max()
        .unwrap_or(1)
}

/// 解析物理 Index Join 的内表侧下标与方法编号。
pub fn getIndexJoinSideAndMethod(join: &PlanNode) -> Option<(usize, i32)> {
    let method = match join.kind {
        PlanKind::IndexJoin => indexJoinMethod,
        PlanKind::IndexHashJoin => indexHashJoinMethod,
        PlanKind::IndexMergeJoin => indexMergeJoinMethod,
        _ => return None,
    };
    Some((join.inner_child, method))
}
/// 尝试枚举 Index Join 家族候选。
pub fn tryToEnumerateIndexJoin(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    ratio_prune: bool,
) -> Vec<PlanNode> {
    let mut plans = enumerateIndexJoinByOuterIdx(join, prop, 0, ratio_prune);
    plans.extend(enumerateIndexJoinByOuterIdx(join, prop, 1, ratio_prune));
    plans
}
/// 是否存在强制 Index Join 家族的 Hint。
pub fn hasForceIndexJoinFamilyHint(join: &LogicalJoin) -> bool {
    join.hints.prefer_index || join.hints.prefer_index_hash || join.hints.prefer_index_merge
}
/// 候选集合中是否已含 Index Join。
pub fn enumerationContainIndexJoin(candidates: &[Vec<PlanAlternative>]) -> bool {
    candidates.iter().flatten().any(|p| {
        matches!(
            p.plan.kind,
            PlanKind::IndexJoin | PlanKind::IndexHashJoin | PlanKind::IndexMergeJoin
        )
    })
}
/// 按 Hint 过滤/保留 Index Join 候选。
pub fn handleFilterIndexJoinHints(join: &LogicalJoin, candidates: Vec<PlanNode>) -> Vec<PlanNode> {
    candidates
        .into_iter()
        .filter(|p| match p.kind {
            PlanKind::IndexJoin => join.hints.prefer_index || !hasForceIndexJoinFamilyHint(join),
            PlanKind::IndexHashJoin => {
                join.hints.prefer_index_hash || !hasForceIndexJoinFamilyHint(join)
            }
            PlanKind::IndexMergeJoin => {
                join.hints.prefer_index_merge || !hasForceIndexJoinFamilyHint(join)
            }
            _ => true,
        })
        .collect()
}

/// 记录聚合 Hint 无法满足时的告警。
pub fn recordAggregationHintWarnings(agg: &LogicalAggregation) -> Option<String> {
    if agg.preferred_hash && agg.preferred_stream {
        Some("conflicting HASH_AGG and STREAM_AGG hints".into())
    } else {
        None
    }
}
/// 记录 Limit/TopN 下推到 Cop 相关告警。
pub fn recordLimitToCopWarnings(limit: &LogicalLimitTopN, pushed: bool) -> Option<String> {
    if limit.prefer_push_down && !pushed {
        Some("limit/topn cannot be pushed down".into())
    } else {
        None
    }
}
/// 记录 Index Join Hint 相关告警。
pub fn recordIndexJoinHintWarnings(join: &LogicalJoin, candidates: &[PlanNode]) -> Option<String> {
    if hasForceIndexJoinFamilyHint(join)
        && !candidates.iter().any(|p| {
            matches!(
                p.kind,
                PlanKind::IndexJoin | PlanKind::IndexHashJoin | PlanKind::IndexMergeJoin
            )
        })
    {
        Some("index join hint is inapplicable".into())
    } else {
        None
    }
}
/// 按算子类型汇总 Hint 告警。
pub fn recordWarnings(operator: &LogicalOperator, candidates: &[PlanNode]) -> Option<String> {
    match operator {
        LogicalOperator::Aggregation(a) => recordAggregationHintWarnings(a),
        LogicalOperator::Limit(l) | LogicalOperator::TopN(l) => {
            recordLimitToCopWarnings(l, !candidates.is_empty())
        }
        LogicalOperator::Join(j) => recordIndexJoinHintWarnings(j, candidates),
        _ => None,
    }
}

/// 当前计划是否符合 HashJoin 偏好 Hint。
pub fn preferHashJoin(join: &LogicalJoin, plan: &PlanNode) -> bool {
    join.hints.prefer_hash && plan.kind == PlanKind::HashJoin
}
/// 当前计划是否符合 MergeJoin 偏好 Hint。
pub fn preferMergeJoin(join: &LogicalJoin, plan: &PlanNode) -> bool {
    join.hints.prefer_merge && plan.kind == PlanKind::MergeJoin
}
/// 当前计划是否符合 Index Join 家族偏好 Hint。
pub fn preferIndexJoinFamily(join: &LogicalJoin, plan: &PlanNode) -> bool {
    matches!(plan.kind, PlanKind::IndexJoin) && join.hints.prefer_index
        || matches!(plan.kind, PlanKind::IndexHashJoin) && join.hints.prefer_index_hash
        || matches!(plan.kind, PlanKind::IndexMergeJoin) && join.hints.prefer_index_merge
}
/// 将逻辑 Join Hint 应用到物理计划选择。
pub fn applyLogicalJoinHint(join: &LogicalJoin, plan: &PlanNode) -> bool {
    preferHashJoin(join, plan)
        || preferMergeJoin(join, plan)
        || preferIndexJoinFamily(join, plan)
        || (join.hints.prefer_broadcast && plan.labels.get("broadcast").copied() == Some(1.0))
        || (join.hints.prefer_shuffle && plan.labels.get("broadcast").copied() == Some(0.0))
}
/// 将逻辑聚合 Hint 应用到物理计划。
pub fn applyLogicalAggregationHint(agg: &LogicalAggregation, plan: &PlanNode) -> bool {
    agg.preferred_hash && plan.kind == PlanKind::HashAgg
        || agg.preferred_stream && plan.kind == PlanKind::StreamAgg
}
/// 将 TopN/Limit Hint 应用到子任务。
pub fn applyLogicalTopNAndLimitHint(topn: &LogicalLimitTopN, child_tasks: &[Task]) -> bool {
    topn.prefer_push_down
        && child_tasks
            .iter()
            .any(|t| matches!(t, Task::Cop { .. } | Task::Mpp { .. }))
}
/// 应用与 var/eigen 相关的逻辑 Hint 偏好。
pub fn applyLogicalHintVarEigen(
    operator: &LogicalOperator,
    plan: &PlanNode,
    child_tasks: &[Task],
) -> bool {
    match operator {
        LogicalOperator::Join(j) | LogicalOperator::Apply(j) => applyLogicalJoinHint(j, plan),
        LogicalOperator::Aggregation(a) => applyLogicalAggregationHint(a, plan),
        LogicalOperator::TopN(t) | LogicalOperator::Limit(t) => {
            applyLogicalTopNAndLimitHint(t, child_tasks)
        }
        _ => false,
    }
}
/// 是否存在普通（非强制）的任务类型偏好。
pub fn hasNormalPreferTask(
    operator: &LogicalOperator,
    state: &mut crate::find_best_task::enumerateState,
    plan: &PlanNode,
    child_tasks: &[Task],
) -> bool {
    match operator {
        LogicalOperator::TopN(t) if child_tasks.iter().any(|c| matches!(c, Task::Cop { .. })) => {
            if state.topNCopExist {
                false
            } else {
                state.topNCopExist = true;
                t.prefer_push_down
            }
        }
        LogicalOperator::Limit(t) if child_tasks.iter().any(|c| matches!(c, Task::Cop { .. })) => {
            if state.limitCopExist {
                false
            } else {
                state.limitCopExist = true;
                t.prefer_push_down
            }
        }
        _ => matches!(plan.kind, PlanKind::HashJoin),
    }
}

/// 处理强制 Index Join 家族 Hint。
pub fn handleForceIndexJoinHints(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    candidates: Vec<PlanNode>,
) -> (Vec<PlanNode>, bool) {
    if !hasForceIndexJoinFamilyHint(join) {
        return (candidates, false);
    }
    let filtered = handleFilterIndexJoinHints(join, candidates);
    if filtered.is_empty() && !prop.can_add_enforcer {
        (Vec::new(), false)
    } else {
        (filtered, true)
    }
}
/// 检查子计划是否适合 Broadcast（广播）交换。
pub fn checkChildFitBC(
    stats: &StatsInfo,
    schema_columns: usize,
    stores: usize,
    row_limit: f64,
    size_limit: f64,
) -> bool {
    let stores = stores.max(1) as f64;
    stats.row_count <= row_limit
        && stats.row_count * stats.avg_row_size.max(schema_columns as f64 * 8.0) * stores
            <= size_limit
}
/// 估算 Broadcast Exchange 的数据量与是否可行。
pub fn calcBroadcastExchangeSize(plan: &PlanNode, stores: usize) -> (f64, f64, bool) {
    let rows = plan.stats.row_count;
    let size = rows * plan.row_size() * stores.max(1) as f64;
    (rows, size, rows.is_finite() && size.is_finite())
}
/// 按子节点估算 Broadcast Exchange 规模。
pub fn calcBroadcastExchangeSizeByChild(
    left: &PlanNode,
    right: &PlanNode,
    stores: usize,
) -> (f64, f64, bool) {
    let a = calcBroadcastExchangeSize(left, stores);
    let b = calcBroadcastExchangeSize(right, stores);
    (a.0 + b.0, a.1 + b.1, a.2 && b.2)
}
/// 估算 Hash（Shuffle）Exchange 规模。
pub fn calcHashExchangeSize(plan: &PlanNode, stores: usize) -> (f64, f64, bool) {
    let rows = plan.stats.row_count / stores.max(1) as f64;
    let size = rows * plan.row_size();
    (rows, size, rows.is_finite() && size.is_finite())
}
/// 按子节点估算 Hash Exchange 规模。
pub fn calcHashExchangeSizeByChild(
    left: &PlanNode,
    right: &PlanNode,
    stores: usize,
) -> (f64, f64, bool) {
    let a = calcHashExchangeSize(left, stores);
    let b = calcHashExchangeSize(right, stores);
    (a.0 + b.0, a.1 + b.1, a.2 && b.2)
}
/// Join 某一侧是否适合 MPP Broadcast Join。
pub fn isJoinChildFitMPPBCJ(join: &LogicalJoin, child: usize) -> bool {
    checkChildFitBC(
        &join.children_stats[child],
        join.children_schema_len[child],
        join.mpp_store_count,
        join.broadcast_row_limit,
        join.broadcast_size_limit,
    )
}
/// Join 整体是否适合 MPP Broadcast Join。
pub fn isJoinFitMPPBCJ(join: &LogicalJoin) -> bool {
    isJoinChildFitMPPBCJ(join, 0) || isJoinChildFitMPPBCJ(join, 1)
}
/// 取 Join 两侧统计与 schema 长度。
pub fn getJoinChildStatsAndSchema(join: &LogicalJoin) -> ([StatsInfo; 2], [usize; 2]) {
    (join.children_stats.clone(), join.children_schema_len)
}
/// 是否偏好 MPP Broadcast Join。
pub fn preferMppBCJ(join: &LogicalJoin) -> bool {
    join.hints.prefer_broadcast || (!join.hints.prefer_shuffle && isJoinFitMPPBCJ(join))
}
/// Join 中表达式是否可下推到指定存储引擎。
pub fn canExprsInJoinPushdown(join: &LogicalJoin, store: StoreType) -> bool {
    join.expressions_pushable && store == StoreType::TiFlash
}
/// Join 两侧是否具备 TiFlash 副本以跑 MPP。
pub fn hasTiFlashReplicaForMPP(join: &LogicalJoin) -> bool {
    join.tiflash_replicas.iter().all(|v| *v)
}
/// 是否允许尝试为该 Join 生成 MPP Join。
pub fn canTryMPPJoinForJoin(join: &LogicalJoin) -> bool {
    join.mpp_enabled
        && hasTiFlashReplicaForMPP(join)
        && canExprsInJoinPushdown(join, StoreType::TiFlash)
}

/// 尝试生成 MPP HashJoin（Broadcast/Shuffle）候选。
pub fn tryToGetMppHashJoin(
    join: &LogicalJoin,
    prop: &PhysicalProperty,
    use_bcj: bool,
) -> Vec<PlanNode> {
    if !prop.is_sort_empty() || !canTryMPPJoinForJoin(join) {
        return Vec::new();
    }
    let build = if use_bcj {
        if isJoinChildFitMPPBCJ(join, 1) {
            1
        } else if isJoinChildFitMPPBCJ(join, 0) {
            0
        } else {
            return Vec::new();
        }
    } else {
        1
    };
    let mut plan = hash_join_plan(join, prop, build, false, StoreType::TiFlash);
    plan.labels
        .insert("broadcast".into(), if use_bcj { 1.0 } else { 0.0 });
    vec![plan]
}

/// 逻辑 Join 的物理计划穷举入口。
pub fn exhaustPhysicalPlans4LogicalJoin(
    join: &mut LogicalJoin,
    prop: &PhysicalProperty,
) -> (Vec<Vec<PlanAlternative>>, bool) {
    let (hash, hash_forced) = getHashJoins(join, prop);
    let mut index = tryToEnumerateIndexJoin(join, prop, true);
    let (filtered, index_forced) = handleForceIndexJoinHints(join, prop, index);
    index = filtered;
    let mut merge = Vec::new();
    if prop.is_sort_empty() || join.hints.prefer_merge {
        let mut p = PlanNode::new(PlanKind::MergeJoin);
        p.join_type = join.join_type;
        p.join_keys = join.join_keys[0].len();
        p.stats = join.stats.clone();
        merge.push(p);
    }
    let mut mpp = tryToGetMppHashJoin(join, prop, preferMppBCJ(join));
    if mpp.is_empty() && canTryMPPJoinForJoin(join) {
        mpp = tryToGetMppHashJoin(join, prop, false);
    }
    let forced = hash_forced
        || index_forced
        || join.hints.prefer_merge
        || join.hints.prefer_broadcast
        || join.hints.prefer_shuffle;
    let wrap = |plans: Vec<PlanNode>| {
        plans
            .into_iter()
            .map(|p| {
                let hint = applyLogicalJoinHint(join, &p);
                alternative(p, 2, prop, hint, false)
            })
            .collect::<Vec<_>>()
    };
    let mut groups = Vec::new();
    if !hash.is_empty() {
        groups.push(wrap(hash));
    }
    if !index.is_empty() {
        groups.push(wrap(index));
    }
    if !merge.is_empty() {
        groups.push(wrap(merge));
    }
    if !mpp.is_empty() {
        groups.push(wrap(mpp));
    }
    let hint_can_work = !forced || groups.iter().flatten().any(|p| p.hint);
    (groups, hint_can_work)
}

/// 为 Apply 构造 HashJoin 形态物理计划。
pub fn GetHashJoin(apply: &LogicalJoin, prop: &PhysicalProperty) -> PlanNode {
    hash_join_plan(apply, prop, 1, false, StoreType::TiDb)
}
/// 逻辑 Apply（相关子查询）的物理计划穷举。
pub fn exhaustPhysicalPlans4LogicalApply(
    apply: &mut LogicalJoin,
    prop: &PhysicalProperty,
) -> (Vec<PlanAlternative>, bool) {
    let mut apply_plan = PlanNode::new(PlanKind::Apply);
    apply_plan.join_type = apply.join_type;
    apply_plan.stats = apply.stats.clone();
    let mut plans = vec![alternative(apply_plan, 2, prop, false, false)];
    if prop.is_sort_empty() {
        let hash = GetHashJoin(apply, prop);
        plans.push(alternative(hash, 2, prop, apply.hints.prefer_hash, false));
    }
    let hint = !apply.hints.prefer_hash || plans.iter().any(|p| p.hint);
    (plans, hint)
}

/// 是否强制下推 Limit/TopN；返回 (pushed, forcibly)。
pub fn pushLimitOrTopNForcibly(limit: &LogicalLimitTopN, plan: &PlanNode) -> (bool, bool) {
    let rows = limit.offset.saturating_add(limit.count);
    let meet = limit.threshold == 0 || rows <= limit.threshold;
    let pushable = meet
        && matches!(plan.store, StoreType::TiKv | StoreType::TiFlash)
        && !matches!(plan.kind, PlanKind::Apply | PlanKind::Window);
    (limit.prefer_push_down && pushable, meet)
}
