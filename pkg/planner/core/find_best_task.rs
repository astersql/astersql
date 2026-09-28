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

// FindBestTask：在物理属性约束下为逻辑计划选择代价最优的执行 Task。
//
// 核心流程包括：穷举/迭代子物理计划、AccessPath 的 skyline 剪枝、
// 路径到 TableScan/IndexScan/PointGet/IndexMerge 的转换、代价比较，
// 以及在需要时通过 Sort 等 enforcer 强制满足排序属性。

use crate::plan_cost_ver1::PlanCostOption;
use crate::plan_cost_ver2::GetPlanCost;
use crate::task::{
    Expression, FieldType, PlanKind, PlanNode, StatsInfo, StoreType, Task, TaskType, attach2Task,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
/// 访问路径与 range 比较用的简化 Datum。
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Default)]
/// 索引/表扫描的键范围上下界。
pub struct Range {
    pub low: Vec<Datum>,
    pub high: Vec<Datum>,
    pub low_exclusive: bool,
    pub high_exclusive: bool,
}
/// Range 辅助：判断是否为非空点查。
impl Range {
    pub fn is_point_non_nullable(&self) -> bool {
        !self.low.is_empty()
            && self.low == self.high
            && !self.low_exclusive
            && !self.high_exclusive
            && self.low.iter().all(|v| !matches!(v, Datum::Null))
    }
}

#[derive(Clone, Debug)]
/// 索引元信息：列序、前缀长度、唯一/全局/多值/向量等。
pub struct IndexInfo {
    pub columns: Vec<usize>,
    pub prefix_lengths: Vec<Option<usize>>,
    pub unique: bool,
    pub global: bool,
    pub multi_valued: bool,
    pub vector: bool,
}

#[derive(Clone, Debug, Default)]
/// 表或索引的一种访问路径，含 range、过滤与代价估计行数。
pub struct AccessPath {
    pub index: Option<IndexInfo>,
    pub ranges: Vec<Range>,
    pub access_conditions: Vec<Expression>,
    pub index_filters: Vec<Expression>,
    pub table_filters: Vec<Expression>,
    pub index_columns: Vec<usize>,
    pub partial_index_paths: Vec<AccessPath>,
    pub count_after_access: f64,
    pub count_after_index: f64,
    pub min_count_after_access: f64,
    pub max_count_after_access: f64,
    pub is_int_handle: bool,
    pub is_common_handle: bool,
    pub is_single_scan: bool,
    pub force_keep_order: bool,
    pub force_no_keep_order: bool,
    pub store: Option<StoreType>,
    pub grouped_ranges: Vec<Vec<Range>>,
    pub group_by_col_idxs: Vec<usize>,
    pub sample_path: bool,
}
/// AccessPath 路径类型判断。
impl AccessPath {
    pub fn is_table_path(&self) -> bool {
        self.index.is_none()
    }
    pub fn is_index_merge(&self) -> bool {
        !self.partial_index_paths.is_empty()
    }
    pub fn full_range(&self) -> bool {
        self.access_conditions.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 物理属性匹配结果：不匹配 / 匹配 / 匹配但需 MergeSort。
pub enum PropMatchResult {
    #[default]
    NotMatched,
    Matched,
    MatchedNeedMergeSort,
}
/// 是否属于匹配类结果。
impl PropMatchResult {
    pub fn matched(self) -> bool {
        self != Self::NotMatched
    }
}

#[derive(Clone, Debug, Default)]
/// 部分有序信息：排序项与已匹配前缀长度。
pub struct PartialOrderInfo {
    pub sort_items: Vec<SortItem>,
    pub prefix_length: usize,
}
#[derive(Clone, Debug, Default)]
/// 部分有序匹配结果。
pub struct PartialOrderMatchResult {
    pub matched: bool,
    pub matched_length: usize,
}

#[derive(Clone, Debug)]
/// 排序项：列下标与升降序。
pub struct SortItem {
    pub column: usize,
    pub desc: bool,
}

#[derive(Clone, Debug)]
/// 物理属性需求：排序、任务类型、是否允许加 enforcer 等。
pub struct PhysicalProperty {
    pub task_type: TaskType,
    pub sort_items: Vec<SortItem>,
    pub sort_item_hints: Vec<SortItem>,
    pub expected_count: f64,
    pub can_add_enforcer: bool,
    pub mpp_partition_any: bool,
    pub partial_order: Option<PartialOrderInfo>,
    pub index_join_cols: usize,
    pub vector_top_k: Option<u64>,
}
/// 默认物理属性。
impl Default for PhysicalProperty {
    fn default() -> Self {
        Self {
            task_type: TaskType::Root,
            sort_items: Vec::new(),
            sort_item_hints: Vec::new(),
            expected_count: f64::INFINITY,
            can_add_enforcer: false,
            mpp_partition_any: true,
            partial_order: None,
            index_join_cols: 0,
            vector_top_k: None,
        }
    }
}
/// 物理属性匹配与克隆辅助。
impl PhysicalProperty {
    pub fn is_sort_empty(&self) -> bool {
        self.sort_items.is_empty()
    }
    pub fn is_flash(&self) -> bool {
        self.task_type == TaskType::Mpp
    }
    pub fn without_order(&self) -> Self {
        let mut p = self.clone();
        p.sort_items.clear();
        p.expected_count = f64::INFINITY;
        p.mpp_partition_any = true;
        p
    }
}

#[derive(Clone, Debug, Default)]
/// 逻辑 DataSource：表扫描候选路径与统计信息。
pub struct DataSource {
    pub paths: Vec<AccessPath>,
    pub conditions: Vec<Expression>,
    pub pushed_down_conditions: Vec<Expression>,
    pub schema: Vec<FieldType>,
    pub stats: StatsInfo,
    pub table_pseudo: bool,
    pub index_pseudo: HashSet<usize>,
    pub partitioned: bool,
    pub has_tiflash: bool,
    pub mpp_allowed: bool,
    pub tiflash_cop_banned: bool,
    pub disaggregated_tiflash: bool,
    pub memory_db: bool,
    pub sample: bool,
    pub columns: Vec<usize>,
    pub new_collation_enabled: bool,
    pub common_handle_version0: bool,
}

#[derive(Clone, Debug)]
/// 一种物理计划候选及其附带元数据。
pub struct PlanAlternative {
    pub plan: PlanNode,
    pub child_properties: Vec<PhysicalProperty>,
    pub hint: bool,
    pub preferred: bool,
}

#[derive(Clone, Debug, Default)]
/// 逻辑计划节点（含子节点与穷举状态）。
pub struct LogicalPlan {
    pub children: Vec<LogicalPlan>,
    pub alternatives: Vec<Vec<PlanAlternative>>,
    pub data_source: Option<DataSource>,
    pub cache: HashMap<String, Task>,
    pub sequence: bool,
}

#[derive(Clone, Debug, Default)]
/// 物理计划枚举过程中的状态缓存。
pub struct enumerateState {
    pub topNCopExist: bool,
    pub limitCopExist: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 子计划迭代模式（普通 / GroupExpression / Sequence 等）。
pub enum IterationMode {
    BaseLogical,
    GroupExpression,
    LogicalSequence,
    LogicalSequenceGroup,
}

/// 迭代子物理计划的函数类型。
pub type iterFunc = fn(&LogicalPlan, &PlanAlternative, &PhysicalProperty, i32) -> Vec<Task>;

/// 若为 GroupExpression 则返回 GE 与 true，否则返回自身。
pub fn getGEAndSelf(plan: &LogicalPlan) -> (&LogicalPlan, bool) {
    (plan, !plan.alternatives.is_empty())
}
/// 准备向下迭代子计划所需的孩子数、模式与迭代函数。
pub fn prepareIterationDownElems(plan: &LogicalPlan) -> (usize, IterationMode, iterFunc) {
    let grouped = !plan.alternatives.is_empty();
    match (plan.sequence, grouped) {
        (true, true) => (
            plan.children.len(),
            IterationMode::LogicalSequenceGroup,
            iterateChildPlan4LogicalSequenceGE,
        ),
        (true, false) => (
            plan.children.len(),
            IterationMode::LogicalSequence,
            iterateChildPlan4LogicalSequence,
        ),
        (false, true) => (
            plan.children.len(),
            IterationMode::GroupExpression,
            iteratePhysicalPlan4GroupExpression,
        ),
        (false, false) => (
            plan.children.len(),
            IterationMode::BaseLogical,
            iteratePhysicalPlan4BaseLogical,
        ),
    }
}

/// 按模式迭代子计划并收集 Task。
fn iterate_children(
    plan: &LogicalPlan,
    alternative: &PlanAlternative,
    fallback: &PhysicalProperty,
    model: i32,
    sequence: bool,
) -> Vec<Task> {
    let mut tasks = Vec::new();
    for (idx, child) in plan.children.iter().enumerate() {
        let property = alternative.child_properties.get(idx).unwrap_or(fallback);
        let mut child = child.clone();
        let task = findBestTask(&mut child, property, model);
        if task.is_invalid() {
            return Vec::new();
        }
        if sequence
            && idx + 1 == plan.children.len()
            && property.task_type == TaskType::Mpp
            && !matches!(task, Task::Mpp { .. })
        {
            return Vec::new();
        }
        tasks.push(task);
    }
    tasks
}
/// 基类逻辑计划的子物理计划迭代。
pub fn iteratePhysicalPlan4BaseLogical(
    plan: &LogicalPlan,
    alternative: &PlanAlternative,
    prop: &PhysicalProperty,
    model: i32,
) -> Vec<Task> {
    iterate_children(plan, alternative, prop, model, false)
}
/// GroupExpression 的子物理计划迭代。
pub fn iteratePhysicalPlan4GroupExpression(
    plan: &LogicalPlan,
    alternative: &PlanAlternative,
    prop: &PhysicalProperty,
    model: i32,
) -> Vec<Task> {
    iterate_children(plan, alternative, prop, model, false)
}
/// LogicalSequence 子计划迭代。
pub fn iterateChildPlan4LogicalSequence(
    plan: &LogicalPlan,
    alternative: &PlanAlternative,
    prop: &PhysicalProperty,
    model: i32,
) -> Vec<Task> {
    iterate_children(plan, alternative, prop, model, true)
}
/// Sequence + GroupExpression 的子计划迭代。
pub fn iterateChildPlan4LogicalSequenceGE(
    plan: &LogicalPlan,
    alternative: &PlanAlternative,
    prop: &PhysicalProperty,
    model: i32,
) -> Vec<Task> {
    iterate_children(plan, alternative, prop, model, true)
}

#[derive(Clone, Debug)]
/// Skyline 剪枝用的候选访问路径包装。
pub struct candidatePath {
    pub path: AccessPath,
    pub accessCondsColMap: HashMap<usize, usize>,
    pub indexCondsColMap: HashMap<usize, usize>,
    pub matchPropResult: PropMatchResult,
    pub partialOrderMatchResult: PartialOrderMatchResult,
    pub matchWithAdvisorySortItems: bool,
    pub partialPathMatchResults: Vec<PropMatchResult>,
    pub indexJoinCols: usize,
    pub isFullRange: bool,
    pub eqOrInCount: usize,
}

/// 将物理属性编码为缓存键字符串。
fn property_key(prop: &PhysicalProperty, model_version: i32) -> String {
    format!(
        "{:?}:{:?}:{:?}:{}:{}:{}:{:?}:{}:{:?}:{}",
        prop.task_type,
        prop.sort_items
            .iter()
            .map(|i| (i.column, i.desc))
            .collect::<Vec<_>>(),
        prop.sort_item_hints
            .iter()
            .map(|i| (i.column, i.desc))
            .collect::<Vec<_>>(),
        prop.expected_count,
        prop.can_add_enforcer,
        prop.mpp_partition_any,
        prop.partial_order.as_ref().map(|partial| (
            partial
                .sort_items
                .iter()
                .map(|i| (i.column, i.desc))
                .collect::<Vec<_>>(),
            partial.prefix_length,
        )),
        prop.index_join_cols,
        prop.vector_top_k,
        model_version,
    )
}
/// 统计表达式引用的列及其出现次数。
fn expr_columns(exprs: &[Expression]) -> HashMap<usize, usize> {
    let mut out = HashMap::new();
    for expr in exprs {
        if let Some(column) = expr.column {
            *out.entry(column).or_insert(0) += 1;
        }
    }
    out
}

/// 构建分区相关物理计划信息（谓词与列）。
pub fn buildPhysPlanPartInfo(ds: &DataSource) -> (Vec<Expression>, Vec<usize>) {
    (ds.conditions.clone(), ds.columns.clone())
}

/// 任务类型是否满足属性要求。
pub fn taskTypeSatisfied(required: Option<&PhysicalProperty>, task: Option<&Task>) -> bool {
    let (Some(prop), Some(task)) = (required, task) else {
        return true;
    };
    match prop.task_type {
        TaskType::Root => !task.is_invalid(),
        TaskType::CopSingleRead | TaskType::CopMultiRead => matches!(task, Task::Cop { .. }),
        TaskType::Mpp => matches!(task, Task::Mpp { .. }),
    }
}

/// 计算 Task 代价；返回 (cost, invalid)。溢出时夹到 MAX 且 invalid=false。
pub fn getTaskPlanCost(task: &Task, model_version: i32) -> (f64, bool) {
    if task.is_invalid() {
        return (f64::MAX, true);
    }
    let option = PlanCostOption::default();
    match task.clone() {
        Task::Root { mut plan, .. } => (
            plan.as_mut()
                .map(|p| GetPlanCost(p, TaskType::Root, &option, model_version))
                .unwrap_or(0.0),
            false,
        ),
        Task::Mpp { mut plan, .. } => (
            plan.as_mut()
                .map(|p| GetPlanCost(p, TaskType::Mpp, &option, model_version))
                .unwrap_or(0.0),
            false,
        ),
        Task::Cop {
            mut index_plan,
            mut table_plan,
            index_finished,
            store,
            ..
        } => {
            let task_type = if store == StoreType::TiFlash {
                TaskType::Mpp
            } else if index_plan.is_some() && table_plan.is_some() {
                TaskType::CopMultiRead
            } else {
                TaskType::CopSingleRead
            };
            if !index_finished {
                return (
                    index_plan
                        .as_mut()
                        .map(|p| GetPlanCost(p, task_type, &option, model_version))
                        .unwrap_or(0.0),
                    false,
                );
            }
            let index = index_plan
                .as_mut()
                .map(|p| GetPlanCost(p, task_type, &option, model_version))
                .unwrap_or(0.0);
            let table = table_plan
                .as_mut()
                .map(|p| GetPlanCost(p, task_type, &option, model_version))
                .unwrap_or(0.0);
            (index + table, false)
        }
        Task::Invalid { .. } => (f64::MAX, true),
    }
}

/// 若 current 代价优于 best 则返回 true。
pub fn compareTaskCost(current: &Task, best: &Task, model_version: i32) -> bool {
    let (current_cost, current_invalid) = getTaskPlanCost(current, model_version);
    let (best_cost, best_invalid) = getTaskPlanCost(best, model_version);
    !current_invalid && (best_invalid || current_cost < best_cost)
}

/// 通过插入 Sort 等 enforcer 使 Task 满足物理属性。
fn enforce_property(prop: &PhysicalProperty, task: Task) -> Task {
    if task.is_invalid() || prop.is_sort_empty() {
        return task;
    }
    let mut sort = PlanNode::new(PlanKind::Sort);
    sort.by_items = prop
        .sort_items
        .iter()
        .map(|i| Expression {
            column: Some(i.column),
            name: if i.desc { "desc".into() } else { "asc".into() },
            ..Expression::default()
        })
        .collect();
    attach2Task(sort, vec![task.into_root()])
}

/// 枚举物理计划并挂到 Task 的辅助实现。
pub fn enumeratePhysicalPlans4TaskHelper(
    logical: &LogicalPlan,
    alternatives: &[PlanAlternative],
    prop: &PhysicalProperty,
    add_enforcer: bool,
    model_version: i32,
) -> (Task, bool) {
    let mut normal_iter = Task::invalid("no physical alternative");
    let mut normal_prefer = Task::invalid("no preferred alternative");
    let mut hint = Task::invalid("no hinted alternative");
    for alternative in alternatives {
        let (_, _, iteration) = prepareIterationDownElems(logical);
        let child_tasks = iteration(logical, alternative, prop, model_version);
        if child_tasks.len() != logical.children.len() {
            continue;
        }
        let mut task = attach2Task(alternative.plan.clone(), child_tasks);
        if prop.task_type == TaskType::Root {
            task = task.into_root();
        }
        if add_enforcer {
            task = enforce_property(prop, task);
        }
        let slot = if alternative.hint {
            &mut hint
        } else if alternative.preferred && hint.is_invalid() {
            &mut normal_prefer
        } else {
            &mut normal_iter
        };
        if compareTaskCost(&task, slot, model_version) {
            *slot = task;
        }
    }
    if !hint.is_invalid() {
        (hint, true)
    } else if !normal_prefer.is_invalid() {
        (normal_prefer, false)
    } else {
        (normal_iter, false)
    }
}

/// 为逻辑计划枚举物理计划并选出挂接后的 Task。
pub fn enumeratePhysicalPlans4Task(
    logical: &LogicalPlan,
    slices: &[Vec<PlanAlternative>],
    prop: &PhysicalProperty,
    add_enforcer: bool,
    model_version: i32,
) -> (Task, bool) {
    let mut normal = Task::invalid("no plan");
    let mut hint = Task::invalid("no hinted plan");
    for alternatives in slices {
        if alternatives.is_empty() {
            continue;
        }
        let (task, hinted) = enumeratePhysicalPlans4TaskHelper(
            logical,
            alternatives,
            prop,
            add_enforcer,
            model_version,
        );
        let best = if hinted { &mut hint } else { &mut normal };
        if compareTaskCost(&task, best, model_version) {
            *best = task;
        }
    }
    if !hint.is_invalid() {
        (hint, true)
    } else {
        (normal, false)
    }
}

/// 在给定物理属性下为逻辑计划寻找代价最优 Task（含 enforcer）。
pub fn findBestTask(
    logical: &mut LogicalPlan,
    prop: &PhysicalProperty,
    model_version: i32,
) -> Task {
    let key = property_key(prop, model_version);
    if let Some(task) = logical.cache.get(&key) {
        return task.clone();
    }
    if let Some(ds) = &logical.data_source {
        let task = findBestTask4LogicalDataSource(ds, prop, model_version);
        logical.cache.insert(key, task.clone());
        return task;
    }
    if prop.task_type != TaskType::Root && prop.task_type != TaskType::Mpp {
        let invalid = Task::invalid("logical operators cannot be wholly pushed to TiKV");
        logical.cache.insert(key, invalid.clone());
        return invalid;
    }
    let (mut best, prefer) =
        enumeratePhysicalPlans4Task(logical, &logical.alternatives, prop, false, model_version);
    if !prefer && (prop.can_add_enforcer || !prop.is_sort_empty()) {
        let empty = prop.without_order();
        let (enforced, enforced_prefer) = enumeratePhysicalPlans4Task(
            logical,
            &logical.alternatives,
            &empty,
            true,
            model_version,
        );
        if enforced_prefer || compareTaskCost(&enforced, &best, model_version) {
            best = enforced;
        }
    }
    logical.cache.insert(key, best.clone());
    best
}

/// 若谓词恒假则可返回 Dual（空结果）Task。
pub fn tryToGetDualTask(ds: &DataSource) -> Option<Task> {
    if ds
        .pushed_down_conditions
        .iter()
        .any(|c| c.name == "false" && c.column.is_none())
    {
        let mut dual = PlanNode::new(PlanKind::Other("TableDual".into()));
        dual.schema = ds.schema.clone();
        dual.stats.row_count = 0.0;
        Some(Task::root(dual))
    } else {
        None
    }
}

/// 布尔比较：true>false，返回 -1/0/1。
pub fn compareBool(left: bool, right: bool) -> i32 {
    match left.cmp(&right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}
/// 比较列频次 map；返回比较结果与是否可比较。
fn compare_maps(left: &HashMap<usize, usize>, right: &HashMap<usize, usize>) -> (i32, bool) {
    let l_covers = right
        .iter()
        .all(|(k, v)| left.get(k).is_some_and(|lv| lv >= v));
    let r_covers = left
        .iter()
        .all(|(k, v)| right.get(k).is_some_and(|rv| rv >= v));
    (
        match (l_covers, r_covers) {
            (true, false) => 1,
            (false, true) => -1,
            _ => 0,
        },
        l_covers || r_covers,
    )
}
/// 比较是否需要回表（index back）。
pub fn compareIndexBack(lhs: &candidatePath, rhs: &candidatePath) -> (i32, bool) {
    let result = compareBool(lhs.path.is_single_scan, rhs.path.is_single_scan);
    if result == 0 && !lhs.path.is_single_scan {
        compare_maps(&lhs.indexCondsColMap, &rhs.indexCondsColMap)
    } else {
        (result, true)
    }
}
/// 比较全局索引偏好。
pub fn compareGlobalIndex(lhs: &candidatePath, rhs: &candidatePath) -> i32 {
    if lhs.path.is_table_path()
        || rhs.path.is_table_path()
        || lhs.path.is_index_merge()
        || rhs.path.is_index_merge()
    {
        0
    } else {
        compareBool(
            lhs.path.index.as_ref().is_some_and(|i| i.global),
            rhs.path.index.as_ref().is_some_and(|i| i.global),
        )
    }
}
/// 比较访问路径的风险比例（行数估计上下界）。
pub fn compareRiskRatio(lhs: &candidatePath, rhs: &candidatePath) -> (i32, f64) {
    let risk = |p: &AccessPath| {
        if p.max_count_after_access > p.count_after_access && p.count_after_access > 0.0 {
            p.max_count_after_access / p.count_after_access
        } else {
            0.0
        }
    };
    let (l, r) = (risk(&lhs.path), risk(&rhs.path));
    let lhs_sum = lhs.path.count_after_access + lhs.path.max_count_after_access;
    let rhs_sum = rhs.path.count_after_access + rhs.path.max_count_after_access;
    if l < r
        && (lhs.path.count_after_access <= rhs.path.count_after_access
            || (lhs_sum < rhs_sum
                && lhs.path.min_count_after_access > 0.0
                && (lhs.path.min_count_after_access <= rhs.path.min_count_after_access
                    || lhs.path.count_after_index <= rhs.path.count_after_index)))
    {
        (1, l)
    } else if r < l
        && (rhs.path.count_after_access <= lhs.path.count_after_access
            || (rhs_sum < lhs_sum
                && rhs.path.min_count_after_access > 0.0
                && (rhs.path.min_count_after_access <= lhs.path.min_count_after_access
                    || rhs.path.count_after_index <= lhs.path.count_after_index)))
    {
        (-1, r)
    } else {
        (0, 0.0)
    }
}
/// 比较等值/IN 条件覆盖优劣。
pub fn compareEqOrIn(lhs: &candidatePath, rhs: &candidatePath) -> i32 {
    lhs.eqOrInCount.cmp(&rhs.eqOrInCount) as i32
}
/// 候选是否完全由索引覆盖（无需回表过滤）。
pub fn isFullIndexMatch(candidate: &candidatePath) -> bool {
    candidate
        .path
        .index
        .as_ref()
        .is_some_and(|index| candidate.eqOrInCount >= index.columns.len())
}
/// 伪统计下的候选比较。
pub fn comparePseudo(
    lhs_pseudo: bool,
    rhs_pseudo: bool,
    lhs_full: bool,
    rhs_full: bool,
    eq_cmp: i32,
    lhs_eq: usize,
    rhs_eq: usize,
    prefer_range: bool,
) -> i32 {
    if lhs_pseudo != rhs_pseudo {
        if lhs_pseudo && lhs_full && (!prefer_range || lhs_eq >= rhs_eq) {
            1
        } else if rhs_pseudo && rhs_full && (!prefer_range || rhs_eq >= lhs_eq) {
            -1
        } else {
            0
        }
    } else if eq_cmp != 0 {
        eq_cmp
    } else {
        compareBool(lhs_full, rhs_full)
    }
}

/// Skyline：多维比较两候选路径，决定剪枝关系。
pub fn compareCandidates(
    ds: &DataSource,
    prop: &PhysicalProperty,
    lhs: &candidatePath,
    rhs: &candidatePath,
    prefer_range: bool,
) -> (i32, bool) {
    if lhs.path.index.as_ref().is_some_and(|i| i.multi_valued)
        || rhs.path.index.as_ref().is_some_and(|i| i.multi_valued)
    {
        return (0, false);
    }
    let lhs_pseudo = lhs.path.index.as_ref().is_some_and(|i| {
        i.columns
            .first()
            .is_some_and(|id| ds.index_pseudo.contains(id))
    });
    let rhs_pseudo = rhs.path.index.as_ref().is_some_and(|i| {
        i.columns
            .first()
            .is_some_and(|id| ds.index_pseudo.contains(id))
    });
    let match_result = compareBool(lhs.matchPropResult.matched(), rhs.matchPropResult.matched());
    let global = compareGlobalIndex(lhs, rhs);
    let (access, access_comparable) = compare_maps(&lhs.accessCondsColMap, &rhs.accessCondsColMap);
    let (scan, scan_comparable) = compareIndexBack(lhs, rhs);
    let (risk, _) = compareRiskRatio(lhs, rhs);
    let equal = compareEqOrIn(lhs, rhs);
    let total = access + scan + match_result + global;
    if (lhs_pseudo || rhs_pseudo)
        && !ds.table_pseudo
        && (lhs.eqOrInCount > 0 || rhs.eqOrInCount > 0)
    {
        let result = comparePseudo(
            lhs_pseudo,
            rhs_pseudo,
            isFullIndexMatch(lhs),
            isFullIndexMatch(rhs),
            equal,
            lhs.eqOrInCount,
            rhs.eqOrInCount,
            prefer_range,
        );
        if result > 0 && total >= 0 {
            return (result, lhs_pseudo);
        }
        if result < 0 && total <= 0 {
            return (result, rhs_pseudo);
        }
    }
    if lhs.path.count_after_access > 100.0
        && rhs.path.count_after_access > 100.0
        && !lhs.path.is_index_merge()
        && !rhs.path.is_index_merge()
        && prop.expected_count.is_infinite()
    {
        if lhs.path.count_after_access * 1000.0 < rhs.path.count_after_access {
            return (1, lhs_pseudo);
        }
        if rhs.path.count_after_access * 1000.0 < lhs.path.count_after_access {
            return (-1, rhs_pseudo);
        }
    }
    let predicate = access + risk + equal;
    if access_comparable
        && scan_comparable
        && predicate >= 0
        && total >= 0
        && (predicate > 0 || total > 0)
    {
        (1, lhs_pseudo)
    } else if access_comparable
        && scan_comparable
        && predicate <= 0
        && total <= 0
        && (predicate < 0 || total < 0)
    {
        (-1, rhs_pseudo)
    } else {
        (0, false)
    }
}

/// 判断访问路径能否满足所需排序等物理属性。
pub fn matchProperty(
    _ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> PropMatchResult {
    if prop.is_sort_empty() {
        return PropMatchResult::NotMatched;
    }
    if path.force_no_keep_order {
        return PropMatchResult::NotMatched;
    }
    if prop
        .sort_items
        .windows(2)
        .any(|items| items[0].desc != items[1].desc)
    {
        return PropMatchResult::NotMatched;
    }
    let columns: Vec<usize> = if path.is_int_handle {
        vec![0]
    } else {
        path.index
            .as_ref()
            .map(|i| i.columns.clone())
            .unwrap_or_else(|| vec![0])
    };
    let equal_prefix = path
        .access_conditions
        .iter()
        .take_while(|e| e.name == "eq" || e.name == "in-single")
        .count();
    let mut pos = equal_prefix.min(columns.len());
    for item in &prop.sort_items {
        if columns.get(pos) != Some(&item.column) {
            return PropMatchResult::NotMatched;
        }
        pos += 1;
    }
    if path.index.as_ref().is_some_and(|i| {
        i.prefix_lengths
            .iter()
            .skip(equal_prefix)
            .take(prop.sort_items.len())
            .any(Option::is_some)
    }) {
        return PropMatchResult::NotMatched;
    }
    if path.grouped_ranges.is_empty() {
        PropMatchResult::Matched
    } else {
        PropMatchResult::MatchedNeedMergeSort
    }
}

/// 匹配部分有序属性，返回匹配前缀长度。
pub fn matchPartialOrderProperty(
    path: &AccessPath,
    partial: &PartialOrderInfo,
) -> PartialOrderMatchResult {
    let Some(index) = &path.index else {
        return PartialOrderMatchResult::default();
    };
    let mut matched = 0;
    for (idx, item) in partial.sort_items.iter().enumerate() {
        if index.columns.get(idx) == Some(&item.column)
            && !index.prefix_lengths.get(idx).is_some_and(Option::is_some)
        {
            matched += 1;
        } else {
            break;
        }
    }
    PartialOrderMatchResult {
        matched: matched >= partial.prefix_length.max(1),
        matched_length: matched,
    }
}

/// 按指定列下标对 range 分组。
pub fn GroupRangesByCols(ranges: &[Range], group_by: &[usize]) -> Result<Vec<Vec<Range>>, String> {
    let mut groups: Vec<(Vec<Datum>, Vec<Range>)> = Vec::new();
    for range in ranges {
        let mut key = Vec::new();
        for index in group_by {
            let Some(value) = range.low.get(*index) else {
                return Err(format!("range has no column {index}"));
            };
            key.push(value.clone());
        }
        if let Some((_, rows)) = groups.iter_mut().find(|(existing, _)| *existing == key) {
            rows.push(range.clone());
        } else {
            groups.push((key, vec![range.clone()]));
        }
    }
    groups.sort_by(|a, b| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)));
    Ok(groups.into_iter().map(|(_, rows)| rows).collect())
}

/// IndexMerge 各分支对物理属性的匹配。
pub fn matchPropForIndexMergeAlternatives(
    ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> (AccessPath, Vec<PropMatchResult>, bool, PropMatchResult) {
    let mut cloned = path.clone();
    let matches: Vec<_> = cloned
        .partial_index_paths
        .iter()
        .map(|p| matchProperty(ds, p, prop))
        .collect();
    let all = !matches.is_empty() && matches.iter().all(|m| m.matched());
    let advisory = prop.sort_items.is_empty() && !prop.sort_item_hints.is_empty();
    let result = if all
        && matches
            .iter()
            .any(|m| *m == PropMatchResult::MatchedNeedMergeSort)
    {
        PropMatchResult::MatchedNeedMergeSort
    } else if all {
        PropMatchResult::Matched
    } else {
        PropMatchResult::NotMatched
    };
    if result == PropMatchResult::MatchedNeedMergeSort {
        for partial in &mut cloned.partial_index_paths {
            if partial.grouped_ranges.is_empty() {
                partial.grouped_ranges =
                    GroupRangesByCols(&partial.ranges, &partial.group_by_col_idxs)
                        .unwrap_or_default();
            }
        }
    }
    (cloned, matches, advisory, result)
}

/// 表达式列表是否包含指定 hash 标识。
pub fn expressionContainsHash(exprs: &[Expression], hash: &[u8]) -> bool {
    exprs.iter().any(|e| e.name.as_bytes() == hash)
}
/// IndexMerge 部分路径是否覆盖某过滤。
pub fn indexMergePartialPathCoversFilter(partial: &AccessPath, filter: &Expression) -> bool {
    partial
        .access_conditions
        .iter()
        .chain(&partial.index_filters)
        .any(|e| e.name == filter.name && e.column == filter.column)
}
/// IndexMerge 顶层过滤是否已被覆盖。
pub fn indexMergeTopLevelFilterCovered(path: &AccessPath, filter: &Expression) -> bool {
    path.partial_index_paths
        .iter()
        .all(|partial| indexMergePartialPathCoversFilter(partial, filter))
}
/// 移除已被 IndexMerge 覆盖的顶层过滤。
pub fn removeCoveredIndexMergeTopLevelFilters(path: &mut AccessPath) {
    let filters = std::mem::take(&mut path.table_filters);
    path.table_filters = filters
        .into_iter()
        .filter(|f| !indexMergeTopLevelFilterCovered(path, f))
        .collect();
}
/// IndexMerge 路径是否整体匹配物理属性。
pub fn isMatchPropForIndexMerge(
    ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> (Vec<PropMatchResult>, PropMatchResult) {
    let (_, partials, _, result) = matchPropForIndexMergeAlternatives(ds, path, prop);
    (partials, result)
}

/// 统计访问条件中的等值谓词数量。
fn equal_predicate_count(path: &AccessPath) -> usize {
    path.access_conditions
        .iter()
        .take_while(|e| e.name == "eq" || e.name == "in-single")
        .count()
}
/// 候选路径比较键与访问器。
impl candidatePath {
    pub fn hasOnlyEqualPredicatesInDNF(&self) -> bool {
        self.path
            .partial_index_paths
            .iter()
            .flat_map(|p| &p.access_conditions)
            .all(|e| e.name == "eq" || e.name == "in-single")
    }
    pub fn equalPredicateCount(&self) -> usize {
        equal_predicate_count(&self.path)
    }
}
/// 是否存在 v0 新 collation 的字符串 handle 限制。
pub fn hasV0NewCollationStringHandle(ds: &DataSource) -> bool {
    ds.new_collation_enabled
        && ds.common_handle_version0
        && ds
            .schema
            .iter()
            .any(|t| matches!(t.code, crate::task::TypeCode::String))
}
/// 候选集是否均基于伪统计。
pub fn isCandidatesPseudo(
    lhs: &candidatePath,
    rhs: &candidatePath,
    ds: &DataSource,
) -> (bool, bool) {
    let pseudo = |c: &candidatePath| {
        c.path
            .index
            .as_ref()
            .and_then(|i| i.columns.first())
            .is_some_and(|id| ds.index_pseudo.contains(id))
    };
    (pseudo(lhs), pseudo(rhs))
}
/// 从表路径构造 skyline 候选。
pub fn getTableCandidate(
    ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> candidatePath {
    candidatePath {
        path: path.clone(),
        accessCondsColMap: expr_columns(&path.access_conditions),
        indexCondsColMap: expr_columns(&path.access_conditions),
        matchPropResult: matchProperty(ds, path, prop),
        partialOrderMatchResult: prop
            .partial_order
            .as_ref()
            .map(|p| matchPartialOrderProperty(path, p))
            .unwrap_or_default(),
        matchWithAdvisorySortItems: false,
        partialPathMatchResults: Vec::new(),
        indexJoinCols: 0,
        isFullRange: path.full_range(),
        eqOrInCount: equal_predicate_count(path),
    }
}
/// 从索引路径构造 skyline 候选。
pub fn getIndexCandidate(
    ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> candidatePath {
    let mut c = getTableCandidate(ds, path, prop);
    let mut conds = path.access_conditions.clone();
    conds.extend(path.index_filters.clone());
    c.indexCondsColMap = expr_columns(&conds);
    c
}
/// 为 Index Join 场景构造索引候选。
pub fn getIndexCandidateForIndexJoin(
    ds: &DataSource,
    path: &AccessPath,
    index_join_cols: usize,
) -> candidatePath {
    let mut c = getIndexCandidate(ds, path, &PhysicalProperty::default());
    c.indexJoinCols = index_join_cols;
    c
}
/// 构造 IndexMerge 候选。
pub fn getIndexMergeCandidate(
    ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> candidatePath {
    let (path, partials, advisory, result) = matchPropForIndexMergeAlternatives(ds, path, prop);
    let mut c = getIndexCandidate(ds, &path, prop);
    c.partialPathMatchResults = partials;
    c.matchWithAdvisorySortItems = advisory;
    c.matchPropResult = result;
    c
}
/// 收敛/合并 IndexMerge 候选。
pub fn convergeIndexMergeCandidate(
    ds: &DataSource,
    path: &AccessPath,
    prop: &PhysicalProperty,
) -> candidatePath {
    getIndexMergeCandidate(ds, path, prop)
}

/// 对 DataSource 访问路径做 skyline 剪枝，保留非支配候选。
pub fn skylinePruning(ds: &DataSource, prop: &PhysicalProperty) -> Vec<candidatePath> {
    let mut candidates = Vec::new();
    for path in &ds.paths {
        let candidate = if path.is_index_merge() {
            getIndexMergeCandidate(ds, path, prop)
        } else if path.is_table_path() {
            getTableCandidate(ds, path, prop)
        } else {
            getIndexCandidate(ds, path, prop)
        };
        let mut dominated = false;
        let mut remove = Vec::new();
        for (i, old) in candidates.iter().enumerate() {
            let (cmp, _) = compareCandidates(ds, prop, &candidate, old, !prop.is_sort_empty());
            if cmp < 0 {
                dominated = true;
                break;
            }
            if cmp > 0 {
                remove.push(i);
            }
        }
        if !dominated {
            for i in remove.into_iter().rev() {
                candidates.remove(i);
            }
            candidates.push(candidate);
        }
    }
    candidates
}

/// 生成剪枝结果的调试描述。
pub fn getPruningInfo(candidates: &[candidatePath]) -> String {
    candidates
        .iter()
        .map(|c| {
            if c.path.is_table_path() {
                "table".to_string()
            } else if c.path.is_index_merge() {
                "index_merge".to_string()
            } else {
                format!("index({:?})", c.path.index.as_ref().map(|i| &i.columns))
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
/// schema 是否可转为 PointGet。
pub fn isPointGetConvertableSchema(ds: &DataSource) -> bool {
    ds.schema
        .iter()
        .all(|t| !matches!(t.code, crate::task::TypeCode::Vector))
}
/// 是否需要探索强制（enforced）物理计划。
pub fn exploreEnforcedPlan(ds: &DataSource) -> bool {
    !ds.sample && !ds.memory_db
}

/// 路径是否可实现为 PointGet。
pub fn isPointGetPath(ds: &DataSource, path: &AccessPath) -> bool {
    if path.ranges.is_empty() {
        return false;
    }
    if !path.is_int_handle {
        let Some(index) = &path.index else {
            return false;
        };
        if !index.unique
            || index.prefix_lengths.iter().any(Option::is_some)
            || path
                .ranges
                .iter()
                .any(|r| r.low.len() != index.columns.len())
        {
            return false;
        }
    }
    isPointGetConvertableSchema(ds) && path.ranges.iter().all(Range::is_point_non_nullable)
}

/// 在扫描上包裹 Selection。
fn selection(plan: PlanNode, conditions: Vec<Expression>, stats: StatsInfo) -> PlanNode {
    if conditions.is_empty() {
        plan
    } else {
        let mut sel = PlanNode::new(PlanKind::Selection);
        sel.conditions = conditions;
        sel.stats = stats;
        sel.children = vec![plan];
        sel
    }
}
/// 拆分索引过滤与表过滤条件。
pub fn splitIndexFilterConditions(
    conditions: &[Expression],
    index_columns: &[usize],
) -> (Vec<Expression>, Vec<Expression>) {
    conditions
        .iter()
        .cloned()
        .partition(|c| c.column.is_some_and(|col| index_columns.contains(&col)))
}

/// 将路径转换为 PointGet 物理计划。
pub fn convertToPointGet(
    ds: &DataSource,
    prop: &PhysicalProperty,
    candidate: &candidatePath,
) -> Task {
    if (!prop.is_sort_empty() && !candidate.matchPropResult.matched())
        || ds.memory_db
        || !isPointGetPath(ds, &candidate.path)
    {
        return Task::invalid("point get requirements not satisfied");
    }
    let mut plan = PlanNode::new(PlanKind::PointGet);
    plan.stats = StatsInfo {
        row_count: candidate.path.count_after_access.min(1.0),
        ..ds.stats.clone()
    };
    plan.schema = ds.schema.clone();
    plan.ranges = 1;
    plan.flags.keep_order = !prop.is_sort_empty();
    let mut filters = candidate.path.index_filters.clone();
    filters.extend(candidate.path.table_filters.clone());
    Task::root(selection(plan, filters, ds.stats.clone()))
}
/// 将路径转换为 BatchPointGet。
pub fn convertToBatchPointGet(
    ds: &DataSource,
    prop: &PhysicalProperty,
    candidate: &candidatePath,
) -> Task {
    if (!prop.is_sort_empty() && candidate.matchPropResult != PropMatchResult::Matched)
        || !isPointGetPath(ds, &candidate.path)
    {
        return Task::invalid("batch point get requirements not satisfied");
    }
    let mut plan = PlanNode::new(PlanKind::BatchPointGet);
    plan.stats = StatsInfo {
        row_count: candidate
            .path
            .count_after_access
            .min(candidate.path.ranges.len() as f64),
        ..ds.stats.clone()
    };
    plan.schema = ds.schema.clone();
    plan.ranges = candidate.path.ranges.len();
    plan.flags.keep_order = !prop.is_sort_empty();
    plan.flags.desc = prop.sort_items.first().is_some_and(|i| i.desc);
    let mut filters = candidate.path.index_filters.clone();
    filters.extend(candidate.path.table_filters.clone());
    Task::root(selection(plan, filters, ds.stats.clone()))
}

/// 将索引路径转换为 IndexScan（可含回表）。
pub fn convertToIndexScan(
    ds: &DataSource,
    prop: &PhysicalProperty,
    candidate: &candidatePath,
) -> Task {
    if prop.task_type == TaskType::Mpp
        || candidate.path.index.is_none()
        || (!prop.is_sort_empty() && !candidate.matchPropResult.matched())
        || (prop.is_sort_empty() && candidate.path.force_keep_order)
        || (!prop.is_sort_empty() && candidate.path.force_no_keep_order)
    {
        return Task::invalid("index scan requirements not satisfied");
    }
    let mut scan = PlanNode::new(PlanKind::IndexScan);
    scan.stats = StatsInfo {
        row_count: candidate.path.count_after_access.min(prop.expected_count),
        ..ds.stats.clone()
    };
    scan.schema = ds.schema.clone();
    scan.store = StoreType::TiKv;
    scan.ranges = candidate.path.ranges.len();
    scan.conditions = candidate.path.access_conditions.clone();
    scan.flags.keep_order = candidate.matchPropResult.matched();
    scan.flags.desc = prop.sort_items.first().is_some_and(|i| i.desc);
    let (index_filters, table_filters) =
        splitIndexFilterConditions(&candidate.path.index_filters, &candidate.path.index_columns);
    let index_plan = selection(scan, index_filters, ds.stats.clone());
    let mut task = if candidate.path.is_single_scan {
        Task::Cop {
            index_plan: Some(selection(index_plan, table_filters, ds.stats.clone())),
            table_plan: None,
            index_finished: false,
            store: StoreType::TiKv,
            warnings: Default::default(),
        }
    } else {
        let mut table = PlanNode::new(PlanKind::TableScan);
        table.stats = ds.stats.clone();
        table.schema = ds.schema.clone();
        Task::Cop {
            index_plan: Some(index_plan),
            table_plan: Some(selection(table, table_filters, ds.stats.clone())),
            index_finished: true,
            store: StoreType::TiKv,
            warnings: Default::default(),
        }
    };
    if prop.task_type == TaskType::Root {
        task = task.into_root();
    }
    task
}

/// 转换为部分列 TableScan。
pub fn convertToPartialTableScan(
    ds: &DataSource,
    prop: &PhysicalProperty,
    path: &AccessPath,
    match_result: PropMatchResult,
) -> PlanNode {
    let mut scan = PlanNode::new(PlanKind::TableScan);
    scan.stats = StatsInfo {
        row_count: path.count_after_access.min(prop.expected_count),
        ..ds.stats.clone()
    };
    scan.schema = ds.schema.clone();
    scan.store = path.store.unwrap_or(StoreType::TiKv);
    scan.ranges = path.ranges.len();
    scan.conditions = path.access_conditions.clone();
    scan.flags.keep_order = match_result.matched();
    scan
}
/// 覆盖部分 TableScan 的输出 schema。
pub fn overwritePartialTableScanSchema(ds: &DataSource, scan: &mut PlanNode) {
    scan.schema = ds.schema.clone();
}

/// 转换为 IndexMerge 扫描。
pub fn convertToIndexMergeScan(
    ds: &DataSource,
    prop: &PhysicalProperty,
    candidate: &candidatePath,
) -> Task {
    if candidate.path.partial_index_paths.is_empty()
        || prop.task_type == TaskType::Mpp
        || (!prop.is_sort_empty() && !candidate.matchPropResult.matched())
    {
        return Task::invalid("index merge requirements not satisfied");
    }
    let mut partials = Vec::new();
    for (id, path) in candidate.path.partial_index_paths.iter().enumerate() {
        let matched = candidate
            .partialPathMatchResults
            .get(id)
            .copied()
            .unwrap_or_default();
        partials.push(if path.is_table_path() {
            convertToPartialTableScan(ds, prop, path, matched)
        } else {
            let mut scan = PlanNode::new(PlanKind::IndexScan);
            scan.stats.row_count = path.count_after_access;
            scan.schema = ds.schema.clone();
            scan.ranges = path.ranges.len();
            scan.flags.keep_order = matched.matched();
            scan
        });
    }
    let mut reader = PlanNode::new(PlanKind::IndexMergeReader);
    reader.children = partials;
    reader.stats = ds.stats.clone();
    reader.schema = ds.schema.clone();
    reader.flags.keep_order = candidate.matchPropResult.matched();
    reader.conditions = candidate.path.table_filters.clone();
    let task = Task::Cop {
        index_plan: None,
        table_plan: Some(reader),
        index_finished: true,
        store: StoreType::TiKv,
        warnings: Default::default(),
    };
    if prop.task_type == TaskType::Root {
        task.into_root()
    } else {
        task
    }
}

/// 转换为全表/范围 TableScan。
pub fn convertToTableScan(
    ds: &DataSource,
    prop: &PhysicalProperty,
    candidate: &candidatePath,
) -> Task {
    if prop.task_type == TaskType::CopMultiRead
        || (!prop.is_sort_empty() && !candidate.matchPropResult.matched())
        || (prop.is_sort_empty() && candidate.path.force_keep_order)
        || (!prop.is_sort_empty() && candidate.path.force_no_keep_order)
    {
        return Task::invalid("table scan requirements not satisfied");
    }
    let store = candidate.path.store.unwrap_or(StoreType::TiKv);
    let use_mpp = store == StoreType::TiFlash
        && ds.mpp_allowed
        && (prop.task_type == TaskType::Mpp
            || ((ds.disaggregated_tiflash || ds.tiflash_cop_banned)
                && prop.task_type == TaskType::Root));
    if store == StoreType::TiFlash
        && candidate.matchPropResult.matched()
        && (candidate.path.force_no_keep_order || ds.partitioned)
    {
        return Task::invalid("TiFlash table scan cannot keep required order");
    }
    if use_mpp && (!prop.mpp_partition_any || candidate.matchPropResult.matched()) {
        return Task::invalid("MPP scan cannot provide this property");
    }
    let mut scan = convertToPartialTableScan(ds, prop, &candidate.path, candidate.matchPropResult);
    scan.store = store;
    scan.flags.partitioned = ds.partitioned;
    let (pushdown, root) = candidate
        .path
        .table_filters
        .iter()
        .cloned()
        .partition::<Vec<_>, _>(|e| !e.virtual_column);
    let scan = selection(scan, pushdown, ds.stats.clone());
    let mut task = if use_mpp {
        Task::Mpp {
            plan: Some(scan),
            partition_keys: Vec::new(),
            warnings: Default::default(),
        }
    } else {
        if store == StoreType::TiFlash && (ds.disaggregated_tiflash || ds.tiflash_cop_banned) {
            return Task::invalid("TiFlash cop scan disabled");
        }
        Task::Cop {
            index_plan: None,
            table_plan: Some(scan),
            index_finished: true,
            store,
            warnings: Default::default(),
        }
    };
    if !root.is_empty() {
        task = attach2Task(
            selection(
                PlanNode::new(PlanKind::Other("Gather".into())),
                root,
                ds.stats.clone(),
            ),
            vec![task.into_root()],
        );
    } else if prop.task_type == TaskType::Root {
        task = task.into_root();
    }
    task
}

/// 转换为 TABLESAMPLE 扫描。
pub fn convertToSampleTable(
    ds: &DataSource,
    prop: &PhysicalProperty,
    candidate: &candidatePath,
) -> Task {
    if prop.task_type == TaskType::CopMultiRead
        || !prop.is_sort_empty()
        || candidate.matchPropResult.matched()
    {
        Task::invalid("sample table does not provide ordered/cop-multi task")
    } else {
        let mut plan = PlanNode::new(PlanKind::Other("TableSample".into()));
        plan.schema = ds.schema.clone();
        plan.stats = ds.stats.clone();
        Task::root(plan)
    }
}
/// 是否可构建仅含单个多值索引的 IndexMerge。
pub fn canBuildSingleMVIndexOnlyIndexMerge(ds: &DataSource, path: &AccessPath) -> bool {
    path.index.as_ref().is_some_and(|i| i.multi_valued)
        && path.is_single_scan
        && path
            .table_filters
            .iter()
            .all(|e| e.column.is_some_and(|c| ds.columns.contains(&c)))
}
/// 检查列下标是否均在 schema 内。
pub fn checkColinSchema(columns: &[usize], schema: &[FieldType]) -> bool {
    columns.iter().all(|c| *c < schema.len())
}
/// 校验 TABLESAMPLE 物理计划合法性。
pub fn validateTableSamplePlan(ds: &DataSource, task: &Task) -> Result<(), String> {
    if ds.sample
        && !task.is_invalid()
        && !task
            .plan()
            .is_some_and(|p| matches!(&p.kind, PlanKind::Other(name) if name == "TableSample"))
    {
        Err("plan not supported for TABLESAMPLE".into())
    } else {
        Ok(())
    }
}

/// DataSource 专用的 FindBestTask：skyline + 转换 + 代价择优。
pub fn findBestTask4LogicalDataSource(
    ds: &DataSource,
    prop: &PhysicalProperty,
    model_version: i32,
) -> Task {
    if let Some(dual) = tryToGetDualTask(ds) {
        return dual;
    }
    let candidates = skylinePruning(ds, prop);
    let mut best = Task::invalid("no access path");
    for candidate in &candidates {
        let task = if ds.sample || candidate.path.sample_path {
            convertToSampleTable(ds, prop, candidate)
        } else if candidate.path.is_index_merge() {
            convertToIndexMergeScan(ds, prop, candidate)
        } else if isPointGetPath(ds, &candidate.path) && candidate.path.ranges.len() == 1 {
            convertToPointGet(ds, prop, candidate)
        } else if isPointGetPath(ds, &candidate.path) {
            convertToBatchPointGet(ds, prop, candidate)
        } else if candidate.path.is_table_path() {
            convertToTableScan(ds, prop, candidate)
        } else {
            convertToIndexScan(ds, prop, candidate)
        };
        if compareTaskCost(&task, &best, model_version) {
            best = task;
        }
    }
    let _ = validateTableSamplePlan(ds, &best);
    best
}

/// 为 IndexScan 追加可下推的 Selection。
pub fn addPushedDownSelection4PhysicalIndexScan(
    scan: PlanNode,
    path: &AccessPath,
    stats: StatsInfo,
) -> (PlanNode, Vec<Expression>) {
    let (index, root) = path
        .index_filters
        .iter()
        .cloned()
        .partition::<Vec<_>, _>(|e| !e.virtual_column);
    (selection(scan, index, stats), root)
}
/// 为 TableScan 追加可下推的 Selection。
pub fn addPushedDownSelection4PhysicalTableScan(
    scan: PlanNode,
    path: &AccessPath,
    stats: StatsInfo,
) -> (PlanNode, Vec<Expression>) {
    let (pushed, root) = path
        .table_filters
        .iter()
        .cloned()
        .partition::<Vec<_>, _>(|e| !e.virtual_column);
    (selection(scan, pushed, stats), root)
}
/// 为 MPP TableScan 任务追加下推 Selection。
pub fn addPushedDownSelectionToMppTask4PhysicalTableScan(
    scan: PlanNode,
    path: &AccessPath,
    stats: StatsInfo,
) -> Task {
    let (plan, root) = addPushedDownSelection4PhysicalTableScan(scan, path, stats);
    let task = Task::mpp(plan);
    if root.is_empty() {
        task
    } else {
        attach2Task(
            selection(
                PlanNode::new(PlanKind::Other("Gather".into())),
                root,
                StatsInfo::default(),
            ),
            vec![task.into_root()],
        )
    }
}

#[derive(Clone, Debug, Default)]
/// FindBestTask 单测用的 mock 逻辑计划。
pub struct mockLogicalPlan4Test {
    pub hasHintForPlan2: bool,
    pub canGeneratePlan2: bool,
    pub costOverflow: bool,
    pub warning_count: u16,
}

/// mock 逻辑计划：Init / FindBestTask。
impl mockLogicalPlan4Test {
    pub fn Init() -> Self {
        Self::default()
    }

    fn getPhysicalPlan1(&self, _prop: &PhysicalProperty) -> PlanAlternative {
        let mut plan = PlanNode::new(PlanKind::Other("mockPhysicalPlan1".into()));
        plan.stats.row_count = 1.0;
        if self.costOverflow {
            plan.cost_v1 = Some(f64::MAX);
        }
        PlanAlternative {
            plan,
            child_properties: Vec::new(),
            hint: false,
            preferred: false,
        }
    }

    fn getPhysicalPlan2(&self, _prop: &PhysicalProperty) -> PlanAlternative {
        let mut plan = PlanNode::new(PlanKind::Other("mockPhysicalPlan2".into()));
        plan.stats.row_count = 1.0;
        if self.costOverflow {
            plan.cost_v1 = Some(f64::MAX);
        }
        PlanAlternative {
            plan,
            child_properties: Vec::new(),
            hint: self.hasHintForPlan2,
            preferred: false,
        }
    }

    pub fn FindBestTask(&mut self, prop: &PhysicalProperty, model_version: i32) -> Task {
        let alternatives = ExhaustPhysicalPlans4MockLogicalPlan(self, prop);
        let logical = LogicalPlan {
            alternatives: alternatives.clone(),
            ..LogicalPlan::default()
        };
        let (mut best, prefer) =
            enumeratePhysicalPlans4Task(&logical, &alternatives, prop, false, model_version);
        if !prefer && !prop.is_sort_empty() {
            let empty = prop.without_order();
            let alternatives = ExhaustPhysicalPlans4MockLogicalPlan(self, &empty);
            let logical = LogicalPlan {
                alternatives: alternatives.clone(),
                ..LogicalPlan::default()
            };
            let (relaxed, relaxed_prefer) =
                enumeratePhysicalPlans4Task(&logical, &alternatives, &empty, false, model_version);
            if prop.can_add_enforcer || relaxed_prefer {
                let enforced = enforce_property(prop, relaxed);
                if relaxed_prefer || compareTaskCost(&enforced, &best, model_version) {
                    best = enforced;
                }
            }
        }
        best
    }
}

/// mock 逻辑计划的物理计划穷举。
pub fn ExhaustPhysicalPlans4MockLogicalPlan(
    plan: &mut mockLogicalPlan4Test,
    prop: &PhysicalProperty,
) -> Vec<Vec<PlanAlternative>> {
    let mut physical_plan1 = Vec::new();
    let mut physical_plan2 = Vec::new();
    if prop.is_sort_empty() && plan.canGeneratePlan2 {
        physical_plan2.push(plan.getPhysicalPlan2(prop));
        if plan.hasHintForPlan2 {
            return vec![physical_plan2];
        }
    }
    let all_same_order = prop
        .sort_items
        .windows(2)
        .all(|items| items[0].desc == items[1].desc);
    if all_same_order {
        physical_plan1.push(plan.getPhysicalPlan1(prop));
    }
    if plan.hasHintForPlan2 {
        if prop.is_sort_empty() {
            plan.warning_count = plan.warning_count.saturating_add(1);
        }
        return vec![physical_plan1];
    }
    vec![physical_plan1, physical_plan2]
}
#[derive(Clone, Debug)]
/// FindBestTask 单测用的 mock 物理计划。
pub struct mockPhysicalPlan4Test {
    pub plan: PlanNode,
}
/// mock 物理计划代价与属性。
impl mockPhysicalPlan4Test {
    pub fn Init(plan: PlanNode) -> Self {
        Self { plan }
    }
    pub fn Attach2Task(&self, tasks: Vec<Task>) -> Task {
        attach2Task(self.plan.clone(), tasks)
    }
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of_val(self) as i64
            + self.plan.children.len() as i64 * std::mem::size_of::<PlanNode>() as i64
    }
}
