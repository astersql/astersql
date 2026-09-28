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

// 物理算子附着到 Task 的核心逻辑（对应 Go physical plan `attach2Task` 族）。
//
// Task 描述算子落点：Root（TiDB 层）、Cop（协处理器，推到 TiKV/TiFlash）、
// MPP（大规模并行处理，TiFlash 分布式）。本模块把物理计划节点挂到子 Task，
// 并处理 Limit/TopN 下推、Join 类型转换、重函数物化等优化路径。

use std::collections::{HashMap, HashSet};

/// 算子存储落点：TiDB / TiKV / TiFlash。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreType {
    TiDb,
    TiKv,
    TiFlash,
}

/// Task 种类：Root、单/多读 Cop、MPP。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskType {
    Root,
    CopSingleRead,
    CopMultiRead,
    Mpp,
}

/// Join 语义：内连接、外连接、半连接等。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    Semi,
    AntiSemi,
}

/// 字段类型编码（简化的类型系统占位）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeCode {
    Null,
    Int,
    UInt,
    Float,
    Decimal,
    String,
    Bytes,
    Vector,
}

/// 字段类型：类型码、长度、小数位、是否无符号。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldType {
    pub code: TypeCode,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned: bool,
}

/// 表达式占位：名称、列下标、函数计数、是否虚拟列、返回类型。
#[derive(Clone, Debug, Default)]
pub struct Expression {
    pub name: String,
    pub column: Option<usize>,
    pub function_count: usize,
    pub virtual_column: bool,
    pub return_type: Option<FieldType>,
}

/// 统计信息摘要：行数与平均行大小，供代价估算。
#[derive(Clone, Debug, Default)]
pub struct StatsInfo {
    pub row_count: f64,
    pub avg_row_size: f64,
    pub histogram_row_size: Option<f64>,
}

/// 物理/逻辑算子种类枚举。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanKind {
    Selection,
    Projection,
    UnionScan,
    Apply,
    IndexJoin,
    IndexHashJoin,
    IndexMergeJoin,
    HashJoin,
    MergeJoin,
    Limit,
    Sort,
    NominalSort,
    TopN,
    Expand,
    UnionAll,
    StreamAgg,
    HashAgg,
    Window,
    CteStorage,
    Sequence,
    TableScan,
    IndexScan,
    IndexReader,
    TableReader,
    IndexLookupReader,
    IndexMergeReader,
    PointGet,
    BatchPointGet,
    ExchangeSender,
    ExchangeReceiver,
    Cte,
    Other(String),
}

impl Default for PlanKind {
    fn default() -> Self {
        Self::Other("unknown".into())
    }
}

/// 计划节点布尔标志（保序、分页、MPP 强制、细粒度 Shuffle 等）。
#[derive(Clone, Debug, Default)]
pub struct PlanFlags {
    pub keep_order: bool,
    pub desc: bool,
    pub paging: bool,
    pub mpp_enforced: bool,
    pub temporary_table: bool,
    pub global_read: bool,
    pub partitioned: bool,
    pub use_cache: bool,
    pub from_data_source: bool,
    pub fine_grained_shuffle: bool,
    pub partial_order: bool,
}

/// 计划树节点：算子种类、子节点、统计、schema、表达式与各类参数。
#[derive(Clone, Debug)]
pub struct PlanNode {
    pub kind: PlanKind,
    pub children: Vec<PlanNode>,
    pub stats: StatsInfo,
    pub schema: Vec<FieldType>,
    pub expressions: Vec<Expression>,
    pub conditions: Vec<Expression>,
    pub by_items: Vec<Expression>,
    pub group_items: Vec<Expression>,
    pub agg_funcs: Vec<Expression>,
    pub join_keys: usize,
    pub inner_child: usize,
    pub concurrency: usize,
    pub offset: u64,
    pub count: u64,
    pub expected_count: f64,
    pub ranges: usize,
    pub partitions: usize,
    pub store: StoreType,
    pub join_type: JoinType,
    pub flags: PlanFlags,
    pub labels: HashMap<String, f64>,
    pub cost_v1: Option<f64>,
}

impl Default for PlanNode {
    fn default() -> Self {
        Self {
            kind: PlanKind::default(),
            children: Vec::new(),
            stats: StatsInfo::default(),
            schema: Vec::new(),
            expressions: Vec::new(),
            conditions: Vec::new(),
            by_items: Vec::new(),
            group_items: Vec::new(),
            agg_funcs: Vec::new(),
            join_keys: 0,
            inner_child: 1,
            concurrency: 1,
            offset: 0,
            count: 0,
            expected_count: f64::INFINITY,
            ranges: 1,
            partitions: 1,
            store: StoreType::TiDb,
            join_type: JoinType::Inner,
            flags: PlanFlags::default(),
            labels: HashMap::new(),
            cost_v1: None,
        }
    }
}

impl PlanNode {
    /// 按算子种类构造默认节点。
    pub fn new(kind: PlanKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }
    /// 估计行数（不小于 0）。
    pub fn rows(&self) -> f64 {
        self.stats.row_count.max(0.0)
    }
    /// 单行字节大小：优先直方图估计，否则用平均行大小。
    pub fn row_size(&self) -> f64 {
        self.stats
            .histogram_row_size
            .unwrap_or(self.stats.avg_row_size)
            .max(0.0)
    }
    /// 设置子节点并返回自身（建造者模式）。
    pub fn with_children(mut self, children: Vec<PlanNode>) -> Self {
        self.children = children;
        self
    }
}

/// Task 附着过程中的警告消息列表。
#[derive(Clone, Debug, Default)]
pub struct TaskWarnings(pub Vec<String>);

/// 执行任务：Root / Cop / MPP / Invalid。
#[derive(Clone, Debug)]
pub enum Task {
    Root {
        plan: Option<PlanNode>,
        warnings: TaskWarnings,
        index_join: bool,
    },
    Cop {
        index_plan: Option<PlanNode>,
        table_plan: Option<PlanNode>,
        index_finished: bool,
        store: StoreType,
        warnings: TaskWarnings,
    },
    Mpp {
        plan: Option<PlanNode>,
        partition_keys: Vec<FieldType>,
        warnings: TaskWarnings,
    },
    Invalid {
        reason: String,
    },
}

impl Task {
    /// 构造 Root Task。
    pub fn root(plan: PlanNode) -> Self {
        Self::Root {
            plan: Some(plan),
            warnings: TaskWarnings::default(),
            index_join: false,
        }
    }
    /// 构造 MPP Task。
    pub fn mpp(plan: PlanNode) -> Self {
        Self::Mpp {
            plan: Some(plan),
            partition_keys: Vec::new(),
            warnings: TaskWarnings::default(),
        }
    }
    /// 构造失败 Task，携带原因。
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::Invalid {
            reason: reason.into(),
        }
    }
    /// 是否为 Invalid 变体。
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }
    /// 取当前有效计划：Cop 在 index_finished 后切到 table_plan。
    pub fn plan(&self) -> Option<&PlanNode> {
        match self {
            Self::Root { plan, .. } | Self::Mpp { plan, .. } => plan.as_ref(),
            Self::Cop {
                index_plan,
                table_plan,
                index_finished,
                ..
            } => {
                if *index_finished {
                    table_plan.as_ref()
                } else {
                    index_plan.as_ref()
                }
            }
            Self::Invalid { .. } => None,
        }
    }
    /// 将 Cop/MPP Task 提升为 Root（保留警告）。
    pub fn into_root(self) -> Self {
        match self {
            Self::Root { .. } | Self::Invalid { .. } => self,
            Self::Mpp { plan, warnings, .. } => Self::Root {
                plan,
                warnings,
                index_join: false,
            },
            Self::Cop {
                index_plan,
                table_plan,
                index_finished,
                warnings,
                ..
            } => {
                let plan = if index_finished {
                    table_plan.or(index_plan)
                } else {
                    index_plan.or(table_plan)
                };
                Self::Root {
                    plan,
                    warnings,
                    index_join: false,
                }
            }
        }
    }
    /// 取出并清空当前计划槽位。
    fn take_plan(&mut self) -> Option<PlanNode> {
        match self {
            Self::Root { plan, .. } | Self::Mpp { plan, .. } => plan.take(),
            Self::Cop {
                index_plan,
                table_plan,
                index_finished,
                ..
            } => {
                if *index_finished {
                    table_plan.take()
                } else {
                    index_plan.take()
                }
            }
            Self::Invalid { .. } => None,
        }
    }
    /// 复制警告列表；Invalid 返回空。
    fn warnings(&self) -> TaskWarnings {
        match self {
            Self::Root { warnings, .. }
            | Self::Cop { warnings, .. }
            | Self::Mpp { warnings, .. } => warnings.clone(),
            Self::Invalid { .. } => TaskWarnings::default(),
        }
    }
}

/// 重函数名集合（向量距离、全文匹配等计算昂贵的表达式）。
pub fn HeavyFunctionNameMap() -> HashSet<&'static str> {
    [
        "vec_cosine_distance",
        "vec_l1_distance",
        "vec_l2_distance",
        "vec_negative_inner_product",
        "vec_dims",
        "vec_l2_norm",
        "fts_match_word",
    ]
    .into_iter()
    .collect()
}

/// 判断表达式是否为重函数。
pub fn ContainHeavyFunction(expr: &Expression) -> bool {
    HeavyFunctionNameMap().contains(expr.name.as_str())
}

/// 判断从 `tp` 转到公共类型 `rhs` 是否需要显式类型转换。
pub fn needConvert(tp: &FieldType, rhs: &FieldType) -> bool {
    if matches!(tp.code, TypeCode::String) && matches!(rhs.code, TypeCode::String) {
        return false;
    }
    if tp.code != rhs.code {
        return true;
    }
    if tp.code != TypeCode::Decimal {
        // Rust 将 Go 的无符号标志建模为独立 TypeCode；同为有符号整数时，
        // 公共类型扩宽仍需要物化转换，保证 MPP 分区键同步失效。
        return matches!(tp.code, TypeCode::Int | TypeCode::UInt) && tp.flen < rhs.flen;
    }
    if tp.decimal != rhs.decimal {
        return true;
    }
    match (decimal_bucket(tp.flen), decimal_bucket(rhs.flen)) {
        (Some(left), Some(right)) => left != right,
        _ => true,
    }
}

/// DECIMAL 按精度分桶，用于比较是否需要转换。
fn decimal_bucket(flen: i32) -> Option<u8> {
    match flen {
        0..=9 => Some(0),
        10..=18 => Some(1),
        19..=38 => Some(2),
        39..=65 => Some(3),
        _ => None,
    }
}

/// 协商两侧列的公共类型，并返回各自是否需要转换。
pub fn negotiateCommonType(left: &FieldType, right: &FieldType) -> (FieldType, bool, bool) {
    let code = match (&left.code, &right.code) {
        (TypeCode::String, _) | (_, TypeCode::String) => TypeCode::String,
        (TypeCode::Float, _) | (_, TypeCode::Float) => TypeCode::Float,
        (TypeCode::Decimal, _) | (_, TypeCode::Decimal) => TypeCode::Decimal,
        (TypeCode::Int, TypeCode::UInt) | (TypeCode::UInt, TypeCode::Int)
            if (left.code == TypeCode::UInt && left.flen >= 20)
                || (right.code == TypeCode::UInt && right.flen >= 20) =>
        {
            TypeCode::Decimal
        }
        (TypeCode::Int, TypeCode::UInt) | (TypeCode::UInt, TypeCode::Int) => TypeCode::Int,
        (TypeCode::UInt, TypeCode::UInt) => TypeCode::UInt,
        _ => TypeCode::Int,
    };
    let (flen, decimal) = if code == TypeCode::Decimal {
        let scale = left.decimal.max(right.decimal).max(0);
        let integer_digits = (left.flen - left.decimal.max(0))
            .max(right.flen - right.decimal.max(0))
            .max(0);
        ((integer_digits + scale).min(65), scale)
    } else if code == TypeCode::Int && left.code != right.code {
        (20, left.decimal.max(right.decimal))
    } else {
        (left.flen.max(right.flen), left.decimal.max(right.decimal))
    };
    let common = FieldType {
        code,
        flen,
        decimal,
        unsigned: left.unsigned && right.unsigned,
    };
    (
        common.clone(),
        needConvert(left, &common),
        needConvert(right, &common),
    )
}

/// 由统计或列类型估算平均行大小。
pub fn getAvgRowSize(stats: &StatsInfo, cols: &[FieldType]) -> f64 {
    stats
        .histogram_row_size
        .unwrap_or_else(|| {
            cols.iter()
                .map(|t| match t.code {
                    TypeCode::Null => 0.0,
                    TypeCode::Int | TypeCode::UInt | TypeCode::Float => 8.0,
                    TypeCode::Decimal => 16.0,
                    TypeCode::String | TypeCode::Bytes => t.flen.max(8) as f64,
                    TypeCode::Vector => t.flen.max(32) as f64,
                })
                .sum()
        })
        .max(0.0)
}

/// 将一元物理算子挂到 Task 当前计划之上。
pub fn attachPlan2Task(mut plan: PlanNode, mut task: Task) -> Task {
    if task.is_invalid() {
        return task;
    }
    // 取出原子计划作为唯一子节点，再写回对应槽位。
    if let Some(child) = task.take_plan() {
        plan.children = vec![child];
    }
    match &mut task {
        Task::Root { plan: slot, .. } | Task::Mpp { plan: slot, .. } => *slot = Some(plan),
        Task::Cop {
            index_plan,
            table_plan,
            index_finished,
            ..
        } => {
            if *index_finished {
                *table_plan = Some(plan)
            } else {
                *index_plan = Some(plan)
            }
        }
        Task::Invalid { .. } => {}
    }
    task
}

/// 二元算子：两侧先升为 Root，再组装子节点。
fn root_binary(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    if tasks.len() != 2 || tasks.iter().any(Task::is_invalid) {
        return Task::invalid("binary operator requires two valid tasks");
    }
    let mut warnings = TaskWarnings::default();
    let mut children = Vec::new();
    for task in tasks {
        let mut root = task.into_root();
        warnings.0.extend(root.warnings().0);
        if let Some(child) = root.take_plan() {
            children.push(child);
        }
    }
    plan.children = children;
    Task::Root {
        plan: Some(plan),
        warnings,
        index_join: false,
    }
}

/// UnionScan 附着：必要时把 Projection 提到 UnionScan 之上以保持投影语义。
pub fn attach2Task4PhysicalUnionScan(plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    let Some(task) = tasks.pop() else {
        return Task::invalid("union scan requires one child");
    };
    let mut root = task.into_root();
    let Some(mut child) = root.take_plan() else {
        return Task::invalid("union scan child is empty");
    };
    let (warnings, index_join) = match root {
        Task::Root {
            warnings,
            index_join,
            ..
        } => (warnings, index_join),
        _ => unreachable!("into_root must produce a root task for a valid child"),
    };
    // Sel(Proj(x)) → Proj(UnionScan(Sel(x)))，保持投影在最外层。
    if child.kind == PlanKind::Selection
        && child
            .children
            .first()
            .is_some_and(|p| p.kind == PlanKind::Projection)
    {
        let mut projection = child.children.remove(0);
        let grand = std::mem::take(&mut projection.children);
        child.children = grand;
        let mut union = plan;
        union.stats = child.stats.clone();
        union.children = vec![child];
        projection.children = vec![union];
        return Task::Root {
            plan: Some(projection),
            warnings,
            index_join,
        };
    }
    // 根已是 Projection：UnionScan 插入其子树下方。
    if child.kind == PlanKind::Projection {
        let grand = std::mem::take(&mut child.children);
        let mut union = plan;
        union.stats = child.stats.clone();
        union.children = grand;
        child.children = vec![union];
        Task::Root {
            plan: Some(child),
            warnings,
            index_join,
        }
    } else {
        let mut union = plan;
        union.stats = child.stats.clone();
        union.children = vec![child];
        Task::Root {
            plan: Some(union),
            warnings,
            index_join,
        }
    }
}

/// Apply（相关子查询嵌套循环）附着为 Root 二元算子。
pub fn attach2Task4PhysicalApply(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    plan.kind = PlanKind::Apply;
    root_binary(plan, tasks)
}
/// IndexJoin 附着，并标记 index_join 以便代价/执行侧识别。
pub fn attach2Task4PhysicalIndexJoin(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    plan.kind = PlanKind::IndexJoin;
    let mut t = root_binary(plan, tasks);
    if let Task::Root { index_join, .. } = &mut t {
        *index_join = true;
    }
    t
}
/// IndexHashJoin：复用 IndexJoin 附着路径。
pub fn attach2Task4PhysicalIndexHashJoin(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    plan.kind = PlanKind::IndexHashJoin;
    attach2Task4PhysicalIndexJoin(plan, tasks)
}
/// IndexMergeJoin：复用 IndexJoin 附着路径。
pub fn attach2Task4PhysicalIndexMergeJoin(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    plan.kind = PlanKind::IndexMergeJoin;
    attach2Task4PhysicalIndexJoin(plan, tasks)
}
/// HashJoin：TiFlash + 两侧皆 MPP 时走 MPP 附着，否则 Root。
pub fn attach2Task4PhysicalHashJoin(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    if plan.store == StoreType::TiFlash && tasks.iter().all(|t| matches!(t, Task::Mpp { .. })) {
        return attach2TaskForMpp4PhysicalHashJoin(plan, tasks);
    }
    plan.kind = PlanKind::HashJoin;
    root_binary(plan, tasks)
}
/// TiFlash HashJoin：两侧均为 MPP 则保持 MPP，否则退回 Root。
pub fn attach2TaskForTiFlash4PhysicalHashJoin(plan: PlanNode, tasks: Vec<Task>) -> Task {
    if tasks.iter().all(|t| matches!(t, Task::Mpp { .. })) {
        attach2TaskForMpp4PhysicalHashJoin(plan, tasks)
    } else {
        root_binary(plan, tasks)
    }
}
/// MPP HashJoin：要求两侧均为 MPP Task。
pub fn attach2TaskForMpp4PhysicalHashJoin(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    if tasks.len() != 2 || tasks.iter().any(|t| !matches!(t, Task::Mpp { .. })) {
        return Task::invalid("MPP hash join requires two MPP tasks");
    }
    let mut children = Vec::new();
    let mut warnings = TaskWarnings::default();
    for mut task in tasks.drain(..) {
        warnings.0.extend(task.warnings().0);
        if let Some(child) = task.take_plan() {
            children.push(child);
        }
    }
    plan.children = children;
    Task::Mpp {
        plan: Some(plan),
        partition_keys: Vec::new(),
        warnings,
    }
}
/// MergeJoin 附着为 Root 二元算子。
pub fn attach2Task4PhysicalMergeJoin(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    plan.kind = PlanKind::MergeJoin;
    root_binary(plan, tasks)
}

/// MPP HashJoin 分区键类型对齐：必要时协商公共类型并关闭细粒度 Shuffle。
pub fn convertPartitionKeysIfNeed4PhysicalHashJoin(left: &mut Task, right: &mut Task) {
    let (
        Task::Mpp {
            partition_keys: left_keys,
            plan: left_plan,
            ..
        },
        Task::Mpp {
            partition_keys: right_keys,
            plan: right_plan,
            ..
        },
    ) = (left, right)
    else {
        return;
    };
    for i in 0..left_keys.len().min(right_keys.len()) {
        let (common, convert_left, convert_right) =
            negotiateCommonType(&left_keys[i], &right_keys[i]);
        if convert_left {
            left_keys[i] = common.clone();
            if let Some(plan) = left_plan {
                plan.flags.fine_grained_shuffle = false;
            }
        }
        if convert_right {
            right_keys[i] = common.clone();
            if let Some(plan) = right_plan {
                plan.flags.fine_grained_shuffle = false;
            }
        }
    }
}

/// 分区键列数不符预期时，强制插入 ExchangeSender 重新分区。
pub fn enforceExchangerByBackup4PhysicalHashJoin(mut task: Task, expected_cols: usize) -> Task {
    if let Task::Mpp {
        plan,
        partition_keys,
        ..
    } = &mut task
    {
        if partition_keys.len() != expected_cols {
            partition_keys.clear();
            if let Some(child) = plan.take() {
                *plan = Some(PlanNode::new(PlanKind::ExchangeSender).with_children(vec![child]));
            }
        }
    }
    task
}

/// Limit/TopN 取 min(统计行数, offset+count)，否则取统计行数。
pub fn extractRows(plan: &PlanNode) -> f64 {
    if matches!(plan.kind, PlanKind::Limit | PlanKind::TopN) {
        plan.rows()
            .min(plan.offset.saturating_add(plan.count) as f64)
    } else {
        plan.rows()
    }
}
/// 分页（paging）代价：按选择率与 range 数估算索引侧开销。
pub fn calcPagingCost(index_plan: &PlanNode, expect: u64) -> f64 {
    let selectivity = expect as f64 / index_plan.rows().max(1.0);
    index_plan.rows() * selectivity.clamp(0.0, 1.0) * (1.0 + index_plan.ranges.max(1) as f64).ln()
}

/// Limit 附着：Cop 侧下推 offset+count；MPP 直接挂接；其余升 Root。
pub fn attach2Task4PhysicalLimit(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    let Some(task) = tasks.pop() else {
        return Task::invalid("limit requires one child");
    };
    match task {
        Task::Cop {
            index_plan,
            table_plan,
            index_finished,
            store,
            warnings,
        } => {
            // Cop 下推：局部 Limit 的 count = offset+count，offset 置 0。
            let pushed = PlanNode {
                kind: PlanKind::Limit,
                offset: 0,
                count: plan.offset.saturating_add(plan.count),
                children: vec![],
                ..plan.clone()
            };
            let (index_plan, table_plan) = if index_finished {
                (
                    index_plan,
                    table_plan.map(|child| pushed.clone().with_children(vec![child])),
                )
            } else {
                (
                    index_plan.map(|child| pushed.clone().with_children(vec![child])),
                    table_plan,
                )
            };
            plan.children.clear();
            Task::Cop {
                index_plan,
                table_plan,
                index_finished,
                store,
                warnings,
            }
        }
        Task::Mpp {
            plan: child,
            partition_keys,
            warnings,
        } => {
            plan.children = child.into_iter().collect();
            Task::Mpp {
                plan: Some(plan),
                partition_keys,
                warnings,
            }
        }
        other => attachPlan2Task(plan, other.into_root()),
    }
}
/// 将 Limit 沉入 IndexLookUp 的 table/index 计划槽。
pub fn sinkIntoIndexLookUp(limit: &PlanNode, task: &mut Task) -> bool {
    let Task::Cop {
        table_plan,
        index_plan,
        ..
    } = task
    else {
        return false;
    };
    let Some(child) = table_plan.take().or_else(|| index_plan.take()) else {
        return false;
    };
    let pushed = PlanNode {
        kind: PlanKind::Limit,
        offset: 0,
        count: limit.offset.saturating_add(limit.count),
        children: vec![child],
        ..limit.clone()
    };
    *table_plan = Some(pushed);
    true
}
/// 将 Limit 沉入 IndexMerge 的各部分计划。
pub fn sinkIntoIndexMerge(limit: &PlanNode, task: &mut Task) -> bool {
    let Task::Cop {
        index_plan,
        table_plan,
        ..
    } = task
    else {
        return false;
    };
    let slots = [index_plan, table_plan];
    let mut changed = false;
    for slot in slots {
        if let Some(child) = slot.take() {
            *slot = Some(PlanNode {
                kind: PlanKind::Limit,
                offset: 0,
                count: limit.offset.saturating_add(limit.count),
                children: vec![child],
                ..limit.clone()
            });
            changed = true;
        }
    }
    changed
}

/// Sort 附着：升 Root 后挂接。
pub fn attach2Task4PhysicalSort(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|t| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::Sort;
                    plan
                },
                t.into_root(),
            )
        })
        .unwrap_or_else(|| Task::invalid("sort requires one child"))
}
/// 名义排序：仅部分有序时物化为真正 Sort，否则透传子 Task。
pub fn attach2Task4NominalSort(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    let Some(task) = tasks.pop() else {
        return Task::invalid("nominal sort requires one child");
    };
    if plan.flags.partial_order {
        plan.kind = PlanKind::Sort;
        attachPlan2Task(plan, task)
    } else {
        task
    }
}

/// 排序键是否可下推为 protobuf 表达式（排除虚拟列与 TiFlash 不支持项）。
pub fn canExpressionConvertedToPB(plan: &PlanNode, store: StoreType) -> bool {
    plan.by_items.iter().all(|e| {
        !e.virtual_column && !(store == StoreType::TiFlash && e.name.starts_with("tikv_only"))
    })
}
/// by_items 中是否含虚拟列。
pub fn containVirtualColumn(plan: &PlanNode) -> bool {
    plan.by_items.iter().any(|e| e.virtual_column)
}
/// TopN/Limit 等是否可下推到 TiKV Cop。
pub fn canPushDownToTiKV(plan: &PlanNode, task: &Task) -> bool {
    matches!(
        task,
        Task::Cop {
            store: StoreType::TiKv,
            ..
        }
    ) && canExpressionConvertedToPB(plan, StoreType::TiKv)
        && !containVirtualColumn(plan)
}
/// 是否可下推到 TiFlash（MPP 或 TiFlash Cop）。
pub fn canPushDownToTiFlash(plan: &PlanNode, task: &Task) -> bool {
    matches!(
        task,
        Task::Mpp { .. }
            | Task::Cop {
                store: StoreType::TiFlash,
                ..
            }
    ) && canExpressionConvertedToPB(plan, StoreType::TiFlash)
        && !containVirtualColumn(plan)
}
/// 标记部分有序后附着 TopN。
pub fn handlePartialOrderTopN(mut plan: PlanNode, task: Task) -> Task {
    plan.flags.partial_order = true;
    attachPlan2Task(plan, task)
}
/// 部分有序估计上限（固定 1<<20）。
pub fn estimateMaxXForPartialOrder() -> u64 {
    1 << 20
}
/// 排序键列下标是否都落在索引计划 schema 内。
pub fn canPushToIndexPlan(index_plan: &PlanNode, by_items: &[Expression]) -> bool {
    by_items
        .iter()
        .all(|item| item.column.is_some_and(|idx| idx < index_plan.schema.len()))
}
/// 构造下推的 Local TopN；若有 offset 则保留 Global TopN。
pub fn getPushedDownTopN(
    plan: &PlanNode,
    child: &PlanNode,
    store: StoreType,
) -> (Option<PlanNode>, Option<PlanNode>) {
    if !canExpressionConvertedToPB(plan, store) || containVirtualColumn(plan) {
        return (None, Some(plan.clone()));
    }
    let mut local = PlanNode {
        offset: 0,
        count: plan.offset.saturating_add(plan.count),
        children: vec![child.clone()],
        ..plan.clone()
    };
    let has_heavy_function = plan.by_items.iter().any(ContainHeavyFunction);
    let mut global = (plan.offset != 0 || has_heavy_function).then(|| plan.clone());
    if has_heavy_function {
        if let Some(global_top_n) = global.as_mut() {
            tryReturnDistanceFromIndex(&mut local, global_top_n, child);
        }
    }
    (Some(local), global)
}
/// 在 Local TopN 前插入 Projection，把重函数（如向量距离）物化为列。
pub fn tryReturnDistanceFromIndex(
    local: &mut PlanNode,
    global: &mut PlanNode,
    child: &PlanNode,
) -> bool {
    // 找出返回 Float 的重函数排序项。
    let heavy_items = local
        .by_items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            ContainHeavyFunction(item)
                && matches!(
                    item.return_type.as_ref().map(|ty| &ty.code),
                    Some(TypeCode::Float)
                )
        })
        .map(|(index, item)| (index, item.clone()))
        .collect::<Vec<_>>();
    if heavy_items.is_empty() {
        return false;
    }
    // 投影先透传子 schema，再追加重函数列，并把 by_items 改为列引用。
    let mut projection = PlanNode::new(PlanKind::Projection);
    projection.expressions = child
        .schema
        .iter()
        .enumerate()
        .map(|(idx, ty)| Expression {
            column: Some(idx),
            return_type: Some(ty.clone()),
            ..Expression::default()
        })
        .collect();
    projection.schema = child.schema.clone();
    projection.children = vec![child.clone()];
    for (item_index, item) in heavy_items {
        let projection_index = projection.schema.len();
        projection.expressions.push(item.clone());
        if let Some(ty) = &item.return_type {
            projection.schema.push(ty.clone());
        }
        local.by_items[item_index] = Expression {
            column: Some(projection_index),
            return_type: item.return_type,
            ..Expression::default()
        };
    }
    global.by_items = local.by_items.clone();
    local.children = vec![projection];
    true
}
/// 若 Task 为 TiDB Cop，则下推 Limit（offset+count）。
pub fn pushLimitDownToTiDBCop(plan: &PlanNode, task: Task) -> (Task, bool) {
    if matches!(
        task,
        Task::Cop {
            store: StoreType::TiDb,
            ..
        }
    ) {
        (
            attachPlan2Task(
                PlanNode {
                    kind: PlanKind::Limit,
                    offset: 0,
                    count: plan.offset + plan.count,
                    ..plan.clone()
                },
                task,
            ),
            true,
        )
    } else {
        (task, false)
    }
}
/// IndexMerge 场景下剔除非列引用的建议排序项。
pub fn handleAdvisorySortItemsForIndexMerge(plan: &mut PlanNode, task: &Task) {
    if matches!(task, Task::Cop { .. }) && plan.by_items.len() > 1 {
        plan.by_items.retain(|item| item.column.is_some());
    }
}

/// TopN 附着：可下推时先挂 Local，再在 Root 保留 Global。
pub fn attach2Task4PhysicalTopN(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    let Some(task) = tasks.pop() else {
        return Task::invalid("topn requires one child");
    };
    let pushable = canPushDownToTiKV(&plan, &task) || canPushDownToTiFlash(&plan, &task);
    if pushable {
        // Local：offset 归零、count 扩为 offset+count；Global 留在 Root。
        let local = PlanNode {
            offset: 0,
            count: plan.offset.saturating_add(plan.count),
            ..plan.clone()
        };
        let pushed = attachPlan2Task(local, task);
        plan.children = pushed.plan().cloned().into_iter().collect();
        Task::root(plan)
    } else {
        attachPlan2Task(plan, task.into_root())
    }
}

/// Projection 附着。
pub fn attach2Task4PhysicalProjection(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|task| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::Projection;
                    plan
                },
                task,
            )
        })
        .unwrap_or_else(|| Task::invalid("projection requires one child"))
}
/// Expand（GROUPING SETS 展开）附着。
pub fn attach2Task4PhysicalExpand(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|task| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::Expand;
                    plan
                },
                task,
            )
        })
        .unwrap_or_else(|| Task::invalid("expand requires one child"))
}

/// UnionAll：全 MPP 则保持 MPP，否则全部升 Root。
pub fn attach2Task4PhysicalUnionAll(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    if tasks.is_empty() {
        return Task::invalid("union all requires children");
    }
    // 全 MPP：保持分布式；否则统一升 Root。
    if tasks.iter().all(|t| matches!(t, Task::Mpp { .. })) {
        let mut children = Vec::new();
        let mut warnings = TaskWarnings::default();
        for mut t in tasks {
            warnings.0.extend(t.warnings().0);
            if let Some(p) = t.take_plan() {
                children.push(p);
            }
        }
        plan.children = children;
        Task::Mpp {
            plan: Some(plan),
            partition_keys: Vec::new(),
            warnings,
        }
    } else {
        let mut children = Vec::new();
        let mut warnings = TaskWarnings::default();
        for t in tasks {
            let mut t = t.into_root();
            warnings.0.extend(t.warnings().0);
            if let Some(p) = t.take_plan() {
                children.push(p);
            }
        }
        plan.children = children;
        Task::Root {
            plan: Some(plan),
            warnings,
            index_join: false,
        }
    }
}
/// 强制要求全部子 Task 为 MPP 的 UnionAll。
pub fn attach2MppTasks4PhysicalUnionAll(plan: PlanNode, tasks: Vec<Task>) -> Task {
    if tasks.iter().all(|t| matches!(t, Task::Mpp { .. })) {
        attach2Task4PhysicalUnionAll(plan, tasks)
    } else {
        Task::invalid("MPP union all received a non-MPP task")
    }
}

/// Selection（过滤）附着。
pub fn attach2Task4PhysicalSelection(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|task| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::Selection;
                    plan
                },
                task,
            )
        })
        .unwrap_or_else(|| Task::invalid("selection requires one child"))
}
/// StreamAgg（流式聚合）附着。
pub fn attach2Task4PhysicalStreamAgg(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|task| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::StreamAgg;
                    plan
                },
                task,
            )
        })
        .unwrap_or_else(|| Task::invalid("stream agg requires one child"))
}
/// IndexJoin 内侧继承底部 Task 的统计信息。
pub fn inheritStatsFromBottomTaskForIndexJoinInner(plan: &mut PlanNode, task: &Task) {
    if let Some(bottom) = task.plan() {
        plan.stats = bottom.stats.clone();
    }
}
/// 按 GROUPING SETS 个数放大行数估计。
pub fn scaleStats4GroupingSets(stats: &StatsInfo, grouping_sets: usize) -> StatsInfo {
    StatsInfo {
        row_count: stats.row_count * grouping_sets.max(1) as f64,
        ..stats.clone()
    }
}
/// 三阶段聚合可用性：至多一个 DISTINCT，且 grouping sets ≤ 1。
pub fn adjust3StagePhaseAgg(can_use: bool, distinct_funcs: usize, grouping_sets: usize) -> bool {
    can_use && distinct_funcs <= 1 && grouping_sets <= 1
}
/// MPP 一阶段聚合附着，并标记 agg_phase=1。
pub fn attach2TaskForMpp1Phase(mut plan: PlanNode, task: Task) -> Task {
    plan.labels.insert("agg_phase".into(), 1.0);
    attachPlan2Task(plan, task)
}
/// MPP 二/三阶段聚合附着。
pub fn attach2TaskForMpp(mut plan: PlanNode, task: Task, use_three_stage: bool) -> Task {
    plan.labels
        .insert("agg_phase".into(), if use_three_stage { 3.0 } else { 2.0 });
    attachPlan2Task(plan, task)
}
/// HashAgg：MPP 保持 MPP，否则升 Root。
pub fn attach2Task4PhysicalHashAgg(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    let Some(task) = tasks.pop() else {
        return Task::invalid("hash agg requires one child");
    };
    plan.kind = PlanKind::HashAgg;
    if matches!(task, Task::Mpp { .. }) {
        attachPlan2Task(plan, task)
    } else {
        attachPlan2Task(plan, task.into_root())
    }
}
/// Window（窗口函数）附着。
pub fn attach2Task4PhysicalWindow(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|task| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::Window;
                    plan
                },
                task,
            )
        })
        .unwrap_or_else(|| Task::invalid("window requires one child"))
}
/// MPP Window：要求 MPP Task，并开启细粒度 Shuffle。
pub fn attach2TaskForMPP4PhysicalWindow(mut plan: PlanNode, task: Task) -> Task {
    if !matches!(task, Task::Mpp { .. }) {
        return Task::invalid("MPP window requires an MPP task");
    }
    plan.flags.fine_grained_shuffle = true;
    attachPlan2Task(plan, task)
}
/// CTE 物化存储附着（升 Root）。
pub fn attach2Task4PhysicalCTEStorage(mut plan: PlanNode, mut tasks: Vec<Task>) -> Task {
    tasks
        .pop()
        .map(|task| {
            attachPlan2Task(
                {
                    plan.kind = PlanKind::CteStorage;
                    plan
                },
                task.into_root(),
            )
        })
        .unwrap_or_else(|| Task::invalid("CTE storage requires one child"))
}
/// Sequence：顺序执行多个子计划，全部升 Root 后组装。
pub fn attach2Task4PhysicalSequence(mut plan: PlanNode, tasks: Vec<Task>) -> Task {
    let mut children = Vec::new();
    for task in tasks {
        let mut root = task.into_root();
        if let Some(p) = root.take_plan() {
            children.push(p);
        }
    }
    plan.children = children;
    Task::root(plan)
}

/// 递归累加 MPP 计划树各节点行大小。
pub fn collectRowSizeFromMPPPlan(plan: &PlanNode) -> f64 {
    plan.row_size()
        + plan
            .children
            .iter()
            .map(collectRowSizeFromMPPPlan)
            .sum::<f64>()
}
/// 累加 MPP ExchangeSender 的网络 seek 代价（按 range 数）。
pub fn accumulateNetSeekCost4MPP(plan: &PlanNode) -> f64 {
    let here = if matches!(plan.kind, PlanKind::ExchangeSender) {
        plan.ranges.max(1) as f64
    } else {
        0.0
    };
    here + plan
        .children
        .iter()
        .map(accumulateNetSeekCost4MPP)
        .sum::<f64>()
}

/// 按算子种类分发到对应的 attach2Task* 实现。
pub fn attach2Task(plan: PlanNode, tasks: Vec<Task>) -> Task {
    match plan.kind {
        PlanKind::UnionScan => attach2Task4PhysicalUnionScan(plan, tasks),
        PlanKind::Apply => attach2Task4PhysicalApply(plan, tasks),
        PlanKind::IndexJoin => attach2Task4PhysicalIndexJoin(plan, tasks),
        PlanKind::IndexHashJoin => attach2Task4PhysicalIndexHashJoin(plan, tasks),
        PlanKind::IndexMergeJoin => attach2Task4PhysicalIndexMergeJoin(plan, tasks),
        PlanKind::HashJoin => attach2Task4PhysicalHashJoin(plan, tasks),
        PlanKind::MergeJoin => attach2Task4PhysicalMergeJoin(plan, tasks),
        PlanKind::Limit => attach2Task4PhysicalLimit(plan, tasks),
        PlanKind::Sort => attach2Task4PhysicalSort(plan, tasks),
        PlanKind::NominalSort => attach2Task4NominalSort(plan, tasks),
        PlanKind::TopN => attach2Task4PhysicalTopN(plan, tasks),
        PlanKind::Projection => attach2Task4PhysicalProjection(plan, tasks),
        PlanKind::Expand => attach2Task4PhysicalExpand(plan, tasks),
        PlanKind::UnionAll => attach2Task4PhysicalUnionAll(plan, tasks),
        PlanKind::Selection => attach2Task4PhysicalSelection(plan, tasks),
        PlanKind::StreamAgg => attach2Task4PhysicalStreamAgg(plan, tasks),
        PlanKind::HashAgg => attach2Task4PhysicalHashAgg(plan, tasks),
        PlanKind::Window => attach2Task4PhysicalWindow(plan, tasks),
        PlanKind::CteStorage => attach2Task4PhysicalCTEStorage(plan, tasks),
        PlanKind::Sequence => attach2Task4PhysicalSequence(plan, tasks),
        _ => match tasks.into_iter().next() {
            Some(task) => attachPlan2Task(plan, task),
            None => Task::root(plan),
        },
    }
}
