// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 绑定计划生成模块（binding plan generation）。
//
// “绑定”（Binding / SQL Plan Binding）是数据库中把某条 SQL 语句与一个
// 固定的执行计划（execution plan，即优化器为 SQL 选择的具体执行方式，
// 如索引选择、连接顺序等）关联起来的机制，用于稳定查询性能。
//
// 本模块负责为一条给定的 SQL 自动探索出多个候选执行计划：
// - 通过调节优化器相关的“旋钮”（连接顺序提示 leading、索引提示 use_index、
//   子查询去关联开关 no_decorrelate、优化器系统变量、fix-control 修复开关）
//   构造不同的搜索状态（`state`）；
// - 以广度优先搜索（BFS）的方式遍历这些状态，在每个状态下生成执行计划，
//   并按计划摘要（plan digest，计划文本的哈希指纹）去重；
// - 最终把每个不同的计划包装为 `BindingPlanInfo`，其中包含带有
//   `/*+ ... */` 优化器提示（hint）注释的绑定 SQL，供用户挑选并创建绑定。

use crate::{
    BindError, Binding, BindingPlanInfo, PlanRuntime, Result, SourceHistory, StatusEnabled,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

/// 计划生成器接口：针对一条 SQL 生成一组候选的绑定计划信息。
///
/// 实现方需要根据默认库名、SQL 文本以及字符集/排序规则上下文，
/// 探索多个可行执行计划并返回对应的 `BindingPlanInfo` 列表。
pub trait PlanGenerator: Send + Sync {
    /// 为给定 SQL 生成候选绑定计划列表。
    ///
    /// - `defaultSchema`：默认数据库（schema）名，用于解析未限定库名的表；
    /// - `sql`：原始 SQL 文本；
    /// - `charset` / `collation`：连接使用的字符集与排序规则。
    fn Generate(
        &self,
        defaultSchema: &str,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> Result<Vec<BindingPlanInfo>>;
}

/// `PlanGenerator` 的默认实现，内部委托给 `PlanRuntime` 运行时完成
/// 具体的计划枚举与生成工作。
pub struct planGenerator {
    /// 计划生成所依赖的运行时环境（提供生成规格与在指定状态下产出计划的能力）。
    runtime: Arc<dyn PlanRuntime>,
}

impl planGenerator {
    /// 用给定的计划运行时构造生成器。
    pub fn new(runtime: Arc<dyn PlanRuntime>) -> Self {
        Self { runtime }
    }
}

impl PlanGenerator for planGenerator {
    fn Generate(
        &self,
        defaultSchema: &str,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> Result<Vec<BindingPlanInfo>> {
        let plans = generatePlanWithSCtx(
            self.runtime.as_ref(),
            defaultSchema,
            sql,
            charset,
            collation,
        )?;
        Ok(plans
            .into_iter()
            .map(|plan| {
                // 把计划对应的优化器提示（hint）拼进 SQL：
                // 在第一个关键字（如 SELECT）之后插入 `/*+ ... */` 注释，
                // 形成可直接用于创建绑定的 BindSQL。
                let binding_sql = if plan.planHints.is_empty() {
                    sql.to_owned()
                } else {
                    let split = sql.find(char::is_whitespace).unwrap_or(sql.len());
                    format!(
                        "{} /*+ {} */{}",
                        &sql[..split],
                        plan.planHints,
                        &sql[split..]
                    )
                };
                BindingPlanInfo {
                    Binding: Arc::new(Binding {
                        OriginalSQL: sql.to_owned(),
                        Db: defaultSchema.to_owned(),
                        BindSQL: binding_sql,
                        Status: StatusEnabled.to_owned(),
                        Source: SourceHistory.to_owned(),
                        Charset: charset.to_owned(),
                        Collation: collation.to_owned(),
                        PlanDigest: plan.planDigest.clone(),
                        ..Binding::default()
                    }),
                    Plan: plan.PlanText(),
                    ..BindingPlanInfo::default()
                }
            })
            .collect())
    }
}

/// 表名信息：库名（schema）、表名以及查询中使用的别名（alias）。
///
/// 在生成优化器提示时，若 SQL 中给表起了别名，则提示里必须使用别名
/// 才能正确匹配到该表。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct tableName {
    /// 所属数据库（schema）名。
    pub schema: String,
    /// 表的真实名称。
    pub name: String,
    /// SQL 中使用的别名；为空表示未使用别名。
    pub alias: String,
}

impl tableName {
    /// 返回在优化器提示中应使用的名称：有别名用别名，否则用表名。
    pub fn HintName(&self) -> &str {
        if self.alias.is_empty() {
            &self.name
        } else {
            &self.alias
        }
    }

    /// 返回 `schema.name`（或 `schema.alias`）形式的字符串表示。
    pub fn String(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for tableName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.HintName())
    }
}

/// 索引提示（index hint）：强制优化器在指定表上使用指定索引访问数据。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct indexHint {
    /// 应用该提示的目标表。
    pub table: tableName,
    /// 要求使用的索引名。
    pub index: String,
}

impl indexHint {
    /// 返回 `table:index` 形式的字符串表示。
    pub fn String(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for indexHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.table, self.index)
    }
}

/// 搜索过程中生成的单个执行计划。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct genedPlan {
    /// 计划摘要（plan digest）：由计划结构计算出的哈希指纹，用于去重。
    pub planDigest: String,
    /// 能复现该计划的优化器提示串（写入 `/*+ ... */` 注释的内容）。
    pub planHints: String,
    /// 计划的文本表示，按行、列组织（类似 EXPLAIN 输出的表格）。
    pub planText: Vec<Vec<String>>,
}

impl genedPlan {
    /// 把二维的计划表格拼成可读文本：列以制表符分隔，行以换行分隔。
    pub fn PlanText(&self) -> String {
        self.planText
            .iter()
            .map(|row| row.join("\t"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 优化器系统变量可能取的值类型（布尔、浮点、整数、字符串）。
///
/// 对应 Go 中的 `any` 动态值，这里用枚举显式表达。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnyValue {
    /// 布尔开关型变量。
    Bool(bool),
    /// 浮点数值型变量（如各类代价因子）。
    Float(f64),
    /// 整数型变量。
    Integer(i64),
    /// 字符串型变量。
    String(String),
}

impl fmt::Display for AnyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value:.4}"),
            Self::Integer(value) => write!(f, "{value}"),
            Self::String(value) => f.write_str(value),
        }
    }
}

/// 计划搜索状态：一组优化器“旋钮”的具体取值组合。
///
/// 每个状态唯一确定一次计划生成的上下文；广度优先搜索通过在当前状态上
/// 微调某一个旋钮来派生相邻状态。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct state {
    /// leading 提示的前两张表：强制优化器以这两张表作为连接（join）顺序的开头。
    pub leading2: [Option<tableName>; 2],
    /// 每张表对应的索引提示槽位；`None` 表示该表不施加索引提示。
    pub indexHints: Vec<Option<indexHint>>,
    /// 施加 no_decorrelate 提示（禁止子查询去关联优化）的查询块（query block）名。
    pub noDecorrelateQB: String,
    /// 参与调节的优化器系统变量名列表。
    pub varNames: Vec<String>,
    /// 与 `varNames` 一一对应的变量当前取值。
    pub varValues: Vec<AnyValue>,
    /// 参与调节的 fix-control 编号列表（fix-control 是按编号开关的优化器行为修正项）。
    pub fixIDs: Vec<u64>,
    /// 与 `fixIDs` 一一对应的 fix-control 当前取值。
    pub fixValues: Vec<String>,
}

impl state {
    /// 把状态编码为唯一字符串，用作访问去重的键（相同编码视为同一状态）。
    pub fn Encode(&self) -> String {
        let mut parts = Vec::new();
        parts.extend(self.leading2.iter().flatten().map(ToString::to_string));
        parts.extend(self.indexHints.iter().flatten().map(ToString::to_string));
        if !self.noDecorrelateQB.is_empty() {
            parts.push(format!("no_decorrelate@{}", self.noDecorrelateQB));
        }
        parts.extend(self.varValues.iter().map(ToString::to_string));
        parts.extend(self.fixValues.iter().cloned());
        parts.join(",")
    }
}

/// 计划生成规格：由运行时对 SQL 分析后得到的搜索空间描述。
#[derive(Clone, Debug, Default)]
pub struct GenerationSpec {
    /// 默认数据库（schema）名。
    pub default_schema: String,
    /// 待生成计划的原始 SQL。
    pub sql: String,
    /// SQL 中涉及的表，用于枚举 leading 连接顺序提示。
    pub tables: Vec<tableName>,
    /// 每张表可选的索引提示集合（外层按表索引对齐）。
    pub index_hint_options: Vec<Vec<Option<indexHint>>>,
    /// 可施加 no_decorrelate 提示的查询块名列表。
    pub no_decorrelate_qbs: Vec<String>,
    /// 参与调节的优化器变量及其默认值。
    pub variable_defaults: Vec<(String, AnyValue)>,
    /// 参与调节的 fix-control 编号及其默认值。
    pub fix_defaults: Vec<(u64, String)>,
    /// 最多收集的不同计划数；0 表示使用默认上限（30）。
    pub max_plans: usize,
    /// 最多探索的状态数；0 表示使用默认上限（10000）。
    pub max_explore_states: usize,
}

/// 基于旧状态派生新状态：替换 leading 提示的前两张表。
pub fn newStateWithLeading2(old: &state, leading2: [Option<tableName>; 2]) -> state {
    state {
        leading2,
        ..old.clone()
    }
}

/// 基于旧状态派生新状态：给第 `tableIdx` 张表设置（或清除）索引提示。
pub fn newStateWithIndexHint(old: &state, tableIdx: usize, hint: Option<indexHint>) -> state {
    let mut next = old.clone();
    if let Some(slot) = next.indexHints.get_mut(tableIdx) {
        *slot = hint;
    }
    next
}

/// 基于旧状态派生新状态：对指定查询块施加 no_decorrelate 提示。
pub fn newStateWithNoDecorrelateQB(old: &state, qbName: String) -> state {
    state {
        noDecorrelateQB: qbName,
        ..old.clone()
    }
}

/// 基于旧状态派生新状态：把名为 `varName` 的优化器变量改为新值；
/// 若变量不存在则原样返回克隆。
pub fn newStateWithNewVar(old: &state, varName: &str, varVal: AnyValue) -> state {
    let mut next = old.clone();
    if let Some(index) = next.varNames.iter().position(|name| name == varName) {
        next.varValues[index] = varVal;
    }
    next
}

/// 基于旧状态派生新状态：把编号为 `fixID` 的 fix-control 改为新值；
/// 若编号不存在则原样返回克隆。
pub fn newStateWithNewFix(old: &state, fixID: u64, fixVal: String) -> state {
    let mut next = old.clone();
    if let Some(index) = next.fixIDs.iter().position(|id| *id == fixID) {
        next.fixValues[index] = fixVal;
    }
    next
}

/// 在给定运行时上下文中为 SQL 生成候选计划集合：
/// 先向运行时索取生成规格（搜索空间），再执行广度优先搜索。
pub fn generatePlanWithSCtx(
    runtime: &dyn PlanRuntime,
    defaultSchema: &str,
    sql: &str,
    charset: &str,
    collation: &str,
) -> Result<Vec<genedPlan>> {
    let spec = runtime.generation_spec(defaultSchema, sql, charset, collation)?;
    breadthFirstPlanSearch(runtime, &spec)
}

/// 广度优先（BFS）计划搜索：从默认状态出发，逐步微调优化器旋钮派生
/// 相邻状态，在每个状态下生成计划并按计划摘要去重，直到达到计划数
/// 或状态数上限。返回按计划摘要排序的去重计划列表。
pub fn breadthFirstPlanSearch(
    runtime: &dyn PlanRuntime,
    spec: &GenerationSpec,
) -> Result<Vec<genedPlan>> {
    let start = getStartState(
        &spec.variable_defaults,
        &spec.fix_defaults,
        spec.index_hint_options.len(),
    )?;
    // visited_states 按状态编码去重，避免重复探索同一状态；
    // visited_plans 按计划摘要去重，收集互不相同的计划。
    let mut visited_states = HashSet::from([start.Encode()]);
    let mut visited_plans = HashMap::<String, genedPlan>::new();
    let mut queue = VecDeque::from([start]);
    // 上限为 0 时采用默认值：最多 30 个计划、最多探索 10000 个状态。
    let max_plans = if spec.max_plans == 0 {
        30
    } else {
        spec.max_plans
    };
    let max_states = if spec.max_explore_states == 0 {
        10_000
    } else {
        spec.max_explore_states
    };
    // 标准 BFS 主循环：出队一个状态，生成其计划，再把未访问过的相邻状态入队。
    while visited_plans.len() < max_plans && visited_states.len() < max_states && !queue.is_empty()
    {
        let current = queue.pop_front().expect("queue checked non-empty");
        let plan = genPlanUnderState(runtime, spec, &current)?;
        visited_plans.insert(plan.planDigest.clone(), plan);
        for candidate in neighboring_states(spec, &current)? {
            if visited_states.insert(candidate.Encode()) {
                queue.push_back(candidate);
            }
        }
    }
    // 按计划摘要排序，保证输出顺序确定。
    let mut plans: Vec<_> = visited_plans.into_values().collect();
    plans.sort_by(|a, b| a.planDigest.cmp(&b.planDigest));
    Ok(plans)
}

/// 枚举当前状态的所有相邻状态：每个相邻状态只在一个旋钮上与当前状态不同。
/// 依次尝试 no_decorrelate 提示、leading 两表连接顺序、各表索引提示、
/// 优化器变量微调、fix-control 取值翻转。
fn neighboring_states(spec: &GenerationSpec, current: &state) -> Result<Vec<state>> {
    let mut states = Vec::new();
    states.extend(
        spec.no_decorrelate_qbs
            .iter()
            .cloned()
            .map(|qb| newStateWithNoDecorrelateQB(current, qb)),
    );
    // 枚举所有有序表对作为 leading 提示的开头两表（排除同表组合）。
    for first in &spec.tables {
        for second in &spec.tables {
            if first != second {
                states.push(newStateWithLeading2(
                    current,
                    [Some(first.clone()), Some(second.clone())],
                ));
            }
        }
    }
    for (table_index, options) in spec.index_hint_options.iter().enumerate() {
        states.extend(
            options
                .iter()
                .cloned()
                .map(|hint| newStateWithIndexHint(current, table_index, hint)),
        );
    }
    for (index, name) in current.varNames.iter().enumerate() {
        states.push(newStateWithNewVar(
            current,
            name,
            adjustVar(name, current.varValues[index].clone())?,
        ));
    }
    for (index, fix_id) in current.fixIDs.iter().copied().enumerate() {
        states.push(newStateWithNewFix(
            current,
            fix_id,
            adjustFix(fix_id, &current.fixValues[index])?,
        ));
    }
    Ok(states)
}

/// 在指定搜索状态下生成执行计划，具体工作委托给运行时。
pub fn genPlanUnderState(
    runtime: &dyn PlanRuntime,
    spec: &GenerationSpec,
    search_state: &state,
) -> Result<genedPlan> {
    runtime.plan_under_state(spec, search_state)
}

/// 对优化器变量做一次“微调”，得到下一个候选取值：
/// - 布尔变量直接取反；
/// - 比例类变量（如选择率因子）每次增加 0.1，上限为 1.0；
/// - 代价因子类变量每次放大 5 倍，上限为 1e6；
/// - 其他变量不支持，返回错误。
pub fn adjustVar(varName: &str, varVal: AnyValue) -> Result<AnyValue> {
    match (varName, varVal) {
        (name, AnyValue::Bool(value)) if is_bool_variable(name) => Ok(AnyValue::Bool(!value)),
        (name, AnyValue::Float(value)) if is_ratio_variable(name) => {
            if value <= 0.0 {
                Ok(AnyValue::Float(0.1))
            } else if value + 0.1 > 1.0 {
                Ok(AnyValue::Float(value))
            } else {
                Ok(AnyValue::Float(value + 0.1))
            }
        }
        (name, AnyValue::Float(value)) if is_cost_variable(name) => {
            Ok(AnyValue::Float(if value >= 1e6 {
                value
            } else {
                value * 5.0
            }))
        }
        _ => Err(BindError(format!(
            "unsupported variable {varName} in plan generation"
        ))),
    }
}

/// 判断变量是否属于“比例”类（名称含 ratio 或 selectivity_factor，
/// selectivity 即选择率：谓词过滤后保留行数的比例估计）。
fn is_ratio_variable(name: &str) -> bool {
    matches!(
        name,
        "tidb_opt_ordering_index_selectivity_ratio"
            | "tidb_opt_risk_eq_skew_ratio"
            | "tidb_opt_risk_range_skew_ratio"
            | "tidb_opt_group_ndv_skew_ratio"
            | "tidb_opt_selectivity_factor"
    )
}

/// 判断变量是否属于“代价因子”类（名称含 cost_factor，
/// 代价因子用于调节优化器代价模型中各算子的相对开销权重）。
fn is_cost_variable(name: &str) -> bool {
    matches!(
        name,
        "tidb_opt_index_scan_cost_factor"
            | "tidb_opt_index_reader_cost_factor"
            | "tidb_opt_table_reader_cost_factor"
            | "tidb_opt_table_full_scan_cost_factor"
            | "tidb_opt_table_range_scan_cost_factor"
            | "tidb_opt_table_rowid_scan_cost_factor"
            | "tidb_opt_table_tiflash_scan_cost_factor"
            | "tidb_opt_index_lookup_cost_factor"
            | "tidb_opt_index_merge_cost_factor"
            | "tidb_opt_sort_cost_factor"
            | "tidb_opt_topn_cost_factor"
            | "tidb_opt_limit_cost_factor"
            | "tidb_opt_stream_agg_cost_factor"
            | "tidb_opt_hash_agg_cost_factor"
            | "tidb_opt_merge_join_cost_factor"
            | "tidb_opt_hash_join_cost_factor"
            | "tidb_opt_index_join_cost_factor"
    )
}

fn is_bool_variable(name: &str) -> bool {
    matches!(
        name,
        "tidb_opt_prefer_range_scan"
            | "tidb_opt_enable_no_decorrelate_in_select"
            | "tidb_opt_always_keep_join_key"
            | "tidb_opt_enable_semi_join_rewrite"
            | "tidb_opt_enable_alternative_logical_plans"
    )
}

/// 对 fix-control（按编号开关的优化器行为修正项）做一次取值调整：
/// - 44855 / 52869：布尔开关，在 ON/OFF 之间翻转；
/// - 45132：整数阈值，大于 10 时减半，否则保持不变；
/// - 其他编号不支持，返回错误。
pub fn adjustFix(fixID: u64, fixVal: &str) -> Result<String> {
    match fixID {
        44855 | 52869 => Ok(if fixVal.trim().eq_ignore_ascii_case("OFF") {
            "ON".to_owned()
        } else {
            "OFF".to_owned()
        }),
        45132 => {
            let value: i64 = fixVal
                .parse()
                .map_err(|error| BindError(format!("invalid fix 45132 value: {error}")))?;
            if value <= 10 {
                Ok(fixVal.to_owned())
            } else {
                Ok((value / 2).to_string())
            }
        }
        _ => Err(BindError(format!(
            "unsupported fix-control {fixID} in plan generation"
        ))),
    }
}

/// 构造搜索的起始状态：不施加任何提示，变量与 fix-control 均取默认值。
/// 若存在名字为空的变量则返回错误。
pub fn getStartState(
    vars: &[(String, AnyValue)],
    fixes: &[(u64, String)],
    indexHintCount: usize,
) -> Result<state> {
    if vars.iter().any(|(name, _)| name.is_empty()) {
        return Err(BindError("optimizer variable name is empty".to_owned()));
    }
    Ok(state {
        indexHints: vec![None; indexHintCount],
        varNames: vars.iter().map(|(name, _)| name.clone()).collect(),
        varValues: vars.iter().map(|(_, value)| value.clone()).collect(),
        fixIDs: fixes.iter().map(|(id, _)| *id).collect(),
        fixValues: fixes.iter().map(|(_, value)| value.clone()).collect(),
        ..state::default()
    })
}

/// 表名提取器：遍历语法树（AST）收集 SQL 中出现的所有表名。
pub struct tableNameExtractor {
    /// 默认数据库名，用于补全未限定库名的表。
    pub defaultSchema: String,
    /// 收集到的表名，键为表的标识字符串。
    pub tableNames: HashMap<String, tableName>,
}

/// SELECT 偏移分配器：为查询中的各个 SELECT 块分配递增编号
/// （编号即查询块 offset，用于在提示中定位子查询）。
pub struct selectOffsetAssigner {
    /// 当前已分配到的偏移值。
    pub offset: usize,
}

/// 子查询偏移提取器：收集相关子查询所在查询块的偏移集合，
/// 用于生成 no_decorrelate 提示的候选目标。
pub struct subqueryOffsetExtractor {
    /// 收集到的查询块偏移集合。
    pub offsets: HashSet<usize>,
}

/// 把子查询偏移的迭代器收集为去重集合。
pub fn collectSubqueryOffsets(offsets: impl IntoIterator<Item = usize>) -> HashSet<usize> {
    offsets.into_iter().collect()
}

/// 从 SELECT 列表中的子查询收集偏移集合（语义同 `collectSubqueryOffsets`）。
pub fn collectSubqueryOffsetsFromSelectList(
    offsets: impl IntoIterator<Item = usize>,
) -> HashSet<usize> {
    collectSubqueryOffsets(offsets)
}

/// 提取 SELECT 语句涉及的表名列表（当前直接取自生成规格）。
pub fn extractSelectTableNames(_defaultSchema: &str, spec: &GenerationSpec) -> Vec<tableName> {
    spec.tables.clone()
}

/// 提取可施加 no_decorrelate 提示的查询块名列表（当前直接取自生成规格）。
pub fn extractNoDecorrelateQBs(spec: &GenerationSpec) -> Vec<String> {
    spec.no_decorrelate_qbs.clone()
}

/// 谓词列提取器：按表收集出现在过滤/连接条件中的列，
/// 用于挑选可能受益于索引提示的列。
pub struct predicateColumnExtractor {
    /// 表名到其谓词列集合的映射。
    pub columnsByTable: HashMap<String, HashSet<String>>,
}

/// 判断某个 `schema.table` 引用是否指向目标表：
/// 库名为空视为通配，表名可匹配真实名或别名，比较均不区分大小写。
pub fn matchesColumnTable(target: &tableName, schema: &str, table: &str) -> bool {
    (schema.is_empty() || target.schema.eq_ignore_ascii_case(schema))
        && (target.name.eq_ignore_ascii_case(table) || target.alias.eq_ignore_ascii_case(table))
}

/// 提取带别名信息的表名列表（当前直接取自生成规格）。
pub fn extractSelectTableNamesWithAlias(spec: &GenerationSpec) -> Vec<tableName> {
    spec.tables.clone()
}

/// 把连接（join）谓词中的 (表, 列) 对累加进谓词列提取器。
pub fn collectJoinPredicates(
    predicates: impl IntoIterator<Item = (String, String)>,
    extractor: &mut predicateColumnExtractor,
) {
    for (table, column) in predicates {
        extractor
            .columnsByTable
            .entry(table)
            .or_default()
            .insert(column);
    }
}

/// 提取每张表候选的索引提示集合（当前直接取自生成规格）。
pub fn extractSelectIndexHints(spec: &GenerationSpec) -> Vec<Vec<Option<indexHint>>> {
    spec.index_hint_options.clone()
}
