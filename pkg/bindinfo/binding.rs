// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// SQL 绑定（SQL Binding）核心数据结构与匹配逻辑。
//
// SQL 绑定是一种执行计划管理机制：管理员可以为某条 SQL 语句绑定一条
// 带有优化器提示（hint，如 `/*+ USE_INDEX(t, idx) */`）的等价 SQL，
// 使优化器在执行该语句时强制采用绑定中指定的执行计划，从而在不修改
// 应用代码的前提下稳定或修正查询性能。
//
// 本模块主要包含：
// - 绑定的状态与来源常量（如 `enabled`/`disabled`、`manual` 等）；
// - `Binding`（一条绑定记录）、`Statement`（待匹配语句）、`TableName`
//   （表名三元组）等核心结构体；
// - 绑定匹配入口 `MatchSQLBinding`：按“会话级优先于全局级”的顺序，
//   基于 SQL 归一化摘要（digest）与表名列表查找可用绑定；
// - 跨库（cross-db）绑定匹配：绑定中省略库名的表可作为通配符匹配
//   任意库下的同名表；
// - SQL 归一化与摘要计算辅助函数：将字面量参数化、统一大小写与空白，
//   再计算 SHA-256 摘要，使同构 SQL 得到相同的键。

use crate::{BindError, Result};
use astersql_parser::{self as parser, ast};
use astersql_util_hint as hint;
use astersql_util_parser as utilparser;
use serde::{Deserialize, Serialize};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 绑定状态：已启用，优化器会应用该绑定。
pub const StatusEnabled: &str = "enabled";
/// 绑定状态：已禁用，绑定保留但不会被应用。
pub const StatusDisabled: &str = "disabled";
/// 绑定状态：使用中（历史遗留状态，语义上等同于已启用）。
pub const StatusUsing: &str = "using";
/// 绑定状态：已删除，作为墓碑记录用于同步删除操作。
pub const StatusDeleted: &str = "deleted";
/// 绑定状态：内建绑定，由系统自动创建。
pub const StatusBuiltin: &str = "builtin";
/// 绑定来源：由用户通过 `CREATE BINDING` 等语句手动创建。
pub const SourceManual: &str = "manual";
/// 绑定来源：从历史执行计划（statement history）创建。
pub const SourceHistory: &str = "history";
/// 绑定作用域：会话级，仅对当前会话生效，匹配时优先级高于全局级。
pub const SessionBindingScope: &str = "session";
/// 绑定作用域：全局级，对所有会话生效。
pub const GlobalBindingScope: &str = "global";

/// UTC timestamp in microseconds.  An integer is used deliberately: it keeps
/// ordering and persistence deterministic and avoids a process-local timezone.
///
/// 以微秒为单位的 UTC 时间戳。刻意采用整数表示：保证排序与持久化的
/// 确定性，并避免依赖进程本地时区。
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct BindingTime(pub i64);

impl BindingTime {
    /// 返回当前时刻的时间戳；若溢出 i64 则饱和为最大值。
    pub fn now() -> Self {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        Self(i64::try_from(micros).unwrap_or(i64::MAX))
    }

    /// 判断时间戳是否为零值（即未设置）。
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

/// 表名三元组：库名（Schema）、表名（Name）与别名（Alias）。
///
/// 当 `Schema` 为空时表示语句中省略了库名前缀，匹配时会退回到
/// 当前会话数据库，或在跨库绑定中作为通配符处理。
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TableName {
    pub Schema: String,
    pub Name: String,
    pub Alias: String,
}

impl TableName {
    /// 返回实际生效的库名：显式指定时用自身 Schema，否则回退到当前库。
    fn effective_schema<'a>(&'a self, current_db: &'a str) -> &'a str {
        if self.Schema.is_empty() {
            current_db
        } else {
            &self.Schema
        }
    }
}

/// 待匹配 SQL 的可序列化适配器：保留原文以供真实 parser 归一化，
/// 同时承载调用方从 AST 预收集的表名与参数标记。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub SQL: String,
    pub Tables: Vec<TableName>,
    pub HasParamMarker: bool,
}

/// 优化器提示（hint）集合，从绑定 SQL 中的 `/*+ ... */` 注释解析得到。
///
/// hint 是嵌在 SQL 注释里的优化器指令（如指定索引、连接顺序等），
/// 用于引导优化器生成期望的执行计划。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct HintSet {
    pub Hints: Vec<String>,
}

/// 一条 SQL 绑定记录：将原始 SQL（`OriginalSQL`）与带 hint 的绑定 SQL
/// （`BindSQL`）关联起来，并记录状态、来源、字符集、SQL/计划摘要
/// （digest，即归一化后 SQL 或执行计划的哈希标识）等元信息。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Binding {
    pub OriginalSQL: String,
    pub Db: String,
    pub BindSQL: String,
    pub Status: String,
    pub CreateTime: BindingTime,
    pub UpdateTime: BindingTime,
    pub Source: String,
    pub Charset: String,
    pub Collation: String,
    pub Hint: HintSet,
    pub ID: String,
    pub SQLDigest: String,
    pub PlanDigest: String,
    pub TableNames: Vec<TableName>,
    #[serde(skip)]
    pub UsageInfo: bindingInfoUsageInfo,
}

impl Binding {
    /// 判断绑定是否处于可用状态（`enabled` 或历史遗留的 `using`）。
    pub fn IsBindingEnabled(&self) -> bool {
        self.Status == StatusEnabled || self.Status == StatusUsing
    }

    /// 估算该绑定占用的内存大小（字节数），用于缓存容量统计。
    pub fn size(&self) -> f64 {
        (self.OriginalSQL.len()
            + self.Db.len()
            + self.BindSQL.len()
            + self.Status.len()
            + 2 * std::mem::size_of::<BindingTime>()
            + self.Charset.len()
            + self.Collation.len()
            + self.ID.len()) as f64
    }

    /// 将“最近使用时间”更新为当前时刻，在绑定被匹配命中时调用。
    pub fn UpdateLastUsedAt(&self) {
        *self
            .UsageInfo
            .LastUsedAt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(BindingTime::now());
    }

    /// 记录使用信息最近一次持久化（保存到存储）的时间。
    pub fn UpdateLastSavedAt(&self, ts: Option<BindingTime>) {
        *self
            .UsageInfo
            .LastSavedAt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ts;
    }
}

/// 绑定的使用统计信息：最近使用时间与最近保存时间。
///
/// 使用 `Arc<Mutex<...>>` 是为了在共享的 `Arc<Binding>` 上通过内部
/// 可变性并发地更新时间戳，且不参与序列化。
#[derive(Clone, Debug, Default)]
pub struct bindingInfoUsageInfo {
    pub LastUsedAt: Arc<Mutex<Option<BindingTime>>>,
    pub LastSavedAt: Arc<Mutex<Option<BindingTime>>>,
}

impl bindingInfoUsageInfo {
    /// 读取最近一次被匹配使用的时间。
    pub fn last_used_at(&self) -> Option<BindingTime> {
        *self
            .LastUsedAt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 读取最近一次被持久化保存的时间。
    pub fn last_saved_at(&self) -> Option<BindingTime> {
        *self
            .LastSavedAt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 绑定匹配所需的预计算信息：去库名（no-db）的 SQL 摘要与表名列表。
///
/// 调用方可以提前算好并传入，避免在匹配路径上重复做 SQL 归一化。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BindingMatchInfo {
    pub NoDBDigest: String,
    pub TableNames: Vec<TableName>,
}

/// 单条语句的绑定匹配结果缓存项：命中的绑定、是否命中及其作用域。
#[derive(Clone, Debug, Default)]
pub struct BindingCacheItem {
    pub Binding: Option<Arc<Binding>>,
    pub Matched: bool,
    pub Scope: String,
}

/// Adapter supplied by a session.  It makes matching usable before the whole
/// SQL/session stack is ported while preserving session-before-global lookup.
///
/// 由会话提供的适配器 trait：在完整的 SQL/会话栈迁移完成之前，
/// 抽象出绑定匹配所需的最小能力（开关、当前库、结果缓存、
/// 会话级与全局级绑定查找），并保持“会话优先于全局”的查找顺序。
pub trait BindingMatchContext {
    /// 是否启用执行计划基线（plan baselines，即 SQL 绑定）功能。
    fn use_plan_baselines(&self) -> bool;
    /// 当前会话使用的默认数据库名。
    fn current_db(&self) -> &str;
    /// 按语句键查询已缓存的匹配结果。
    fn cached_match(&self, statement_key: &str) -> Option<BindingCacheItem>;
    /// 将匹配结果写入语句级缓存。
    fn cache_match(&mut self, statement_key: String, item: BindingCacheItem);
    /// 是否允许 schema `*` 匹配任意数据库。
    fn enable_fuzzy_binding(&self) -> bool {
        false
    }
    /// 会话进入语句匹配前同步跨库匹配开关。
    fn set_enable_fuzzy_binding(&mut self, _enabled: bool) {}
    /// 全局绑定命中时是否记录最近使用时间。
    fn enable_binding_usage(&self) -> bool {
        true
    }
    /// 会话进入语句匹配前同步使用信息开关。
    fn set_enable_binding_usage(&mut self, _enabled: bool) {}
    /// 测试模式下绕过缓存并校验缓存结果。
    fn in_test_mode(&self) -> bool {
        false
    }
    /// 记录一次真实绑定查找的耗时（缓存命中不记录）。
    fn record_binding_match_duration(&self, _duration: Duration) {}
    /// 在会话级绑定中按摘要与表名查找可用绑定。
    fn match_session_binding(
        &self,
        no_db_digest: &str,
        tables: &[TableName],
    ) -> Option<Arc<Binding>>;
    /// 按当前 fuzzy 开关查找会话绑定；旧实现默认委托原方法。
    fn match_session_binding_with_fuzzy(
        &self,
        no_db_digest: &str,
        tables: &[TableName],
        _enable_fuzzy_binding: bool,
    ) -> Option<Arc<Binding>> {
        self.match_session_binding(no_db_digest, tables)
    }
    /// 在全局级绑定中按摘要与表名查找可用绑定。
    fn match_global_binding(
        &self,
        no_db_digest: &str,
        tables: &[TableName],
    ) -> Option<Arc<Binding>>;
    /// 按当前 fuzzy 开关查找全局绑定；旧实现默认委托原方法。
    fn match_global_binding_with_fuzzy(
        &self,
        no_db_digest: &str,
        tables: &[TableName],
        _enable_fuzzy_binding: bool,
    ) -> Option<Arc<Binding>> {
        self.match_global_binding(no_db_digest, tables)
    }
}

/// 绑定匹配入口：为给定语句查找可用的 SQL 绑定。
///
/// 返回三元组：（命中的绑定、是否命中、绑定作用域）。
pub fn MatchSQLBinding(
    sctx: &mut dyn BindingMatchContext,
    stmtNode: &Statement,
) -> (Option<Arc<Binding>>, bool, String) {
    MatchSQLBindingWithCache(sctx, stmtNode, None)
}

/// 带缓存的绑定匹配：先查语句级缓存，未命中时执行核心匹配逻辑并回填缓存。
///
/// `info` 可传入预计算的匹配信息（摘要与表名），避免重复归一化。
pub fn MatchSQLBindingWithCache(
    sctx: &mut dyn BindingMatchContext,
    stmtNode: &Statement,
    info: Option<&mut BindingMatchInfo>,
) -> (Option<Arc<Binding>>, bool, String) {
    // 功能未开启或 SQL 为空时直接返回未命中。
    if !sctx.use_plan_baselines() || stmtNode.SQL.is_empty() {
        return (None, false, String::new());
    }
    // 优先复用缓存的匹配结果，避免重复归一化与查找。
    let key = statement_cache_key(stmtNode);
    let cache = getMatchSQLBindingCache(sctx, &key);
    if sctx.in_test_mode() {
        let started = Instant::now();
        let result = matchSQLBindingCore(sctx, stmtNode, None);
        sctx.record_binding_match_duration(started.elapsed());
        assert!(assertMatchSQLBinding(
            cache.as_ref(),
            result.1,
            result.0.as_ref(),
            &result.2,
        ));
        setMatchSQLBindingCache(sctx, key, result.1, result.0.clone(), result.2.clone());
        return result;
    }
    if let Some(cache) = cache {
        return (cache.Binding, cache.Matched, cache.Scope);
    }
    let started = Instant::now();
    let result = matchSQLBindingCore(sctx, stmtNode, info);
    setMatchSQLBindingCache(sctx, key, result.1, result.0.clone(), result.2.clone());
    sctx.record_binding_match_duration(started.elapsed());
    result
}

/// 生成语句级缓存键：由 SQL 文本与表数量组合而成。
fn statement_cache_key(stmt: &Statement) -> String {
    let mut key = format!("{}:{}:{}", stmt.SQL.len(), stmt.SQL, stmt.HasParamMarker);
    for table in &stmt.Tables {
        key.push_str(&format!(
            ":{}:{}:{}:{}:{}:{}",
            table.Schema.len(),
            table.Schema,
            table.Name.len(),
            table.Name,
            table.Alias.len(),
            table.Alias,
        ));
    }
    key
}

/// 从会话上下文读取缓存的匹配结果。
fn getMatchSQLBindingCache(
    sctx: &dyn BindingMatchContext,
    stmtNode: &str,
) -> Option<BindingCacheItem> {
    sctx.cached_match(stmtNode)
}

/// 将匹配结果写入会话上下文的缓存。
fn setMatchSQLBindingCache(
    sctx: &mut dyn BindingMatchContext,
    stmtNode: String,
    matched: bool,
    binding: Option<Arc<Binding>>,
    scope: String,
) {
    sctx.cache_match(
        stmtNode,
        BindingCacheItem {
            Binding: binding,
            Matched: matched,
            Scope: scope,
        },
    );
}

/// 核心匹配逻辑：先按会话级、再按全局级查找绑定，命中即更新使用时间。
fn matchSQLBindingCore(
    sctx: &dyn BindingMatchContext,
    stmtNode: &Statement,
    mut info: Option<&mut BindingMatchInfo>,
) -> (Option<Arc<Binding>>, bool, String) {
    let complete = info
        .as_deref()
        .is_some_and(|info| !info.NoDBDigest.is_empty() && !info.TableNames.is_empty());
    let (no_db_digest, table_names) = if complete {
        let info = info.as_deref().expect("complete match info exists");
        (info.NoDBDigest.clone(), info.TableNames.clone())
    } else {
        let (_, digest) = NormalizeStmtForBinding(stmtNode, "", true);
        let tables = CollectTableNames(stmtNode);
        if let Some(info) = info.as_deref_mut() {
            info.NoDBDigest = digest.clone();
            info.TableNames = tables.clone();
        }
        (digest, tables)
    };
    let enable_fuzzy_binding = sctx.enable_fuzzy_binding();
    // 会话级绑定优先于全局级绑定。
    if let Some(binding) =
        sctx.match_session_binding_with_fuzzy(&no_db_digest, &table_names, enable_fuzzy_binding)
    {
        return (Some(binding), true, SessionBindingScope.to_owned());
    }
    if let Some(binding) =
        sctx.match_global_binding_with_fuzzy(&no_db_digest, &table_names, enable_fuzzy_binding)
    {
        if sctx.enable_binding_usage() {
            binding.UpdateLastUsedAt();
        }
        return (Some(binding), true, GlobalBindingScope.to_owned());
    }
    (None, false, String::new())
}

/// 校验缓存的匹配结果与本次实际匹配结果是否一致，用于断言/自检。
pub fn assertMatchSQLBinding(
    cache: Option<&BindingCacheItem>,
    hit: bool,
    binding: Option<&Arc<Binding>>,
    scope: &str,
) -> bool {
    let Some(cached) = cache else {
        return true;
    };
    if !hit {
        return !cached.Matched;
    }
    cached.Matched
        && cached.Scope == scope
        && matches!((&cached.Binding, binding), (Some(a), Some(b)) if Arc::ptr_eq(a, b))
}

/// 计算绑定对应的去库名（no-db）SQL 摘要。
///
/// 去库名摘要在归一化时剥掉 `db.` 前缀，使跨库场景下同构的 SQL
/// 得到相同的摘要键。原始 SQL 为空时返回错误。
pub fn noDBDigestFromBinding(binding: &Binding) -> Result<String> {
    let mut parser = parser::Parser::default();
    let statement = parser
        .ParseOneStmt(&binding.BindSQL, &binding.Charset, &binding.Collation)
        .map_err(|error| BindError(error.to_string()))?;
    Ok(normalize_parsed_statement(statement.as_ref(), &binding.BindSQL, "", true).1)
}

/// 在候选绑定中做跨库匹配，返回最优绑定及是否命中。
///
/// 优选规则：通配符（省略库名的表）数量越少越精确、越优先；
/// 数量相同时取更新时间（UpdateTime）更晚的绑定。
pub fn crossDBMatchBindings(
    currentDB: &str,
    tableNames: &[TableName],
    bindings: &[Arc<Binding>],
) -> (Option<Arc<Binding>>, bool) {
    crossDBMatchBindingsWithFuzzy(currentDB, tableNames, bindings, true)
}

/// 按会话 fuzzy 开关执行跨库绑定匹配。
pub fn crossDBMatchBindingsWithFuzzy(
    currentDB: &str,
    tableNames: &[TableName],
    bindings: &[Arc<Binding>],
    enable_fuzzy_binding: bool,
) -> (Option<Arc<Binding>>, bool) {
    let mut least_wildcards = tableNames.len() + 1;
    let mut matched_binding = None;
    // 只考虑处于启用状态的绑定。
    for binding in bindings.iter().filter(|binding| binding.IsBindingEnabled()) {
        let (wildcards, matched) =
            crossDBMatchBindingTableName(currentDB, tableNames, &binding.TableNames);
        if !matched || (wildcards > 0 && !enable_fuzzy_binding) {
            continue;
        }
        if wildcards < least_wildcards {
            least_wildcards = wildcards;
            matched_binding = Some(Arc::clone(binding));
        }
    }
    let matched = matched_binding.is_some();
    (matched_binding, matched)
}

/// 逐个比较语句与绑定的表名列表是否跨库匹配。
///
/// 返回（通配符匹配次数、是否匹配）。表名必须逐位置一致（忽略
/// 大小写）；绑定中库名为空的表视为通配符，可匹配任意库。
pub fn crossDBMatchBindingTableName(
    currentDB: &str,
    stmtTableNames: &[TableName],
    bindingTableNames: &[TableName],
) -> (usize, bool) {
    if stmtTableNames.len() != bindingTableNames.len() {
        return (0, false);
    }
    let mut wildcards = 0;
    for (stmt, binding) in stmtTableNames.iter().zip(bindingTableNames) {
        if !stmt.Name.eq_ignore_ascii_case(&binding.Name) {
            return (0, false);
        }
        if binding.Schema == "*" {
            wildcards += 1;
            continue;
        }
        if binding.Schema.eq_ignore_ascii_case(&stmt.Schema)
            || (stmt.Schema.is_empty() && binding.Schema.eq_ignore_ascii_case(currentDB))
        {
            continue;
        }
        return (0, false);
    }
    (wildcards, true)
}

/// 判断语句是否是跨库绑定候选：只要存在省略库名的表即成立。
pub fn isCrossDBBinding(stmt: &Statement) -> bool {
    stmt.Tables.iter().any(|table| table.Schema == "*")
}

#[derive(Default)]
struct AstFacts {
    tables: Vec<TableName>,
    has_param: bool,
}

fn parse_sql(sql: &str, charset: &str, collation: &str) -> Result<Box<dyn ast::Node>> {
    parser::Parser::default()
        .ParseOneStmt(sql, charset, collation)
        .map_err(|error| BindError(error.to_string()))
}

fn record_ast_table(facts: &mut AstFacts, table: &ast::TableName, alias: &str) {
    facts.tables.push(TableName {
        Schema: table.Schema.L.clone(),
        Name: table.Name.L.clone(),
        Alias: alias.to_owned(),
    });
}

fn walk_ast_result_set(node: &ast::ResultSetNode, facts: &mut AstFacts) {
    match node {
        ast::ResultSetNode::TableSource(source) => {
            if let Some(query) = &source.QuerySource {
                let _ = query.with_node(|node| walk_ast_node(node, facts));
            } else {
                record_ast_table(facts, &source.Source, &source.AsName.L);
            }
        }
        ast::ResultSetNode::Join(join) => walk_ast_join(join, facts),
    }
}

fn walk_ast_join(join: &ast::Join, facts: &mut AstFacts) {
    if let Some(left) = join.Left.as_deref() {
        walk_ast_result_set(left, facts);
    }
    if let Some(right) = join.Right.as_deref() {
        walk_ast_result_set(right, facts);
    }
    if let Some(on) = &join.On {
        walk_ast_expr(on, facts);
    }
}

fn walk_ast_by_items(items: &[ast::ByItem], facts: &mut AstFacts) {
    for item in items {
        walk_ast_expr(&item.Expr, facts);
    }
}

fn walk_ast_limit(limit: Option<&ast::Limit>, facts: &mut AstFacts) {
    let Some(limit) = limit else { return };
    if let Some(count) = &limit.Count {
        walk_ast_expr(count, facts);
    }
    if let Some(offset) = &limit.Offset {
        walk_ast_expr(offset, facts);
    }
}

fn walk_ast_window_spec(spec: &ast::WindowSpec, facts: &mut AstFacts) {
    walk_ast_by_items(&spec.PartitionBy, facts);
    walk_ast_by_items(&spec.OrderBy, facts);
    if let Some(frame) = &spec.Frame {
        if let Some(expr) = &frame.Extent.Start.Expr {
            walk_ast_expr(expr, facts);
        }
        if let Some(expr) = &frame.Extent.End.Expr {
            walk_ast_expr(expr, facts);
        }
    }
}

fn walk_ast_expr(expr: &ast::ExprNode, facts: &mut AstFacts) {
    use ast::ExprKind;
    match &expr.Kind {
        ExprKind::TableName(table) => record_ast_table(facts, table, ""),
        ExprKind::ParamMarker { .. } => facts.has_param = true,
        ExprKind::Variable { Value, .. } => {
            if let Some(value) = Value {
                walk_ast_expr(value, facts);
            }
        }
        ExprKind::Function { Args, .. } | ExprKind::Row(Args) => {
            for arg in Args {
                walk_ast_expr(arg, facts);
            }
        }
        ExprKind::AggregateFunction { Args, Order, .. } => {
            for arg in Args {
                walk_ast_expr(arg, facts);
            }
            walk_ast_by_items(Order, facts);
        }
        ExprKind::WindowFunction { Args, Spec, .. } => {
            for arg in Args {
                walk_ast_expr(arg, facts);
            }
            walk_ast_window_spec(Spec, facts);
        }
        ExprKind::Binary { L, R, .. } | ExprKind::CompareSubquery { L, R, .. } => {
            walk_ast_expr(L, facts);
            walk_ast_expr(R, facts);
        }
        ExprKind::Unary { V, .. }
        | ExprKind::Parentheses(V)
        | ExprKind::ExistsSubquery { Sel: V, .. }
        | ExprKind::IsTruth { Expr: V, .. }
        | ExprKind::IsNull { Expr: V, .. }
        | ExprKind::Collate { Expr: V, .. }
        | ExprKind::Cast { Expr: V, .. }
        | ExprKind::JSONSumCrc32 { Expr: V, .. } => walk_ast_expr(V, facts),
        ExprKind::InList { Expr, List, .. } => {
            walk_ast_expr(Expr, facts);
            for item in List {
                walk_ast_expr(item, facts);
            }
        }
        ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            walk_ast_expr(Expr, facts);
            walk_ast_expr(Left, facts);
            walk_ast_expr(Right, facts);
        }
        ExprKind::Like { Expr, Pattern, .. } | ExprKind::Regexp { Expr, Pattern, .. } => {
            walk_ast_expr(Expr, facts);
            walk_ast_expr(Pattern, facts);
        }
        ExprKind::MatchAgainst { Against, .. } => walk_ast_expr(Against, facts),
        ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            if let Some(value) = Value {
                walk_ast_expr(value, facts);
            }
            for clause in WhenClauses {
                walk_ast_expr(&clause.Expr, facts);
                walk_ast_expr(&clause.Result, facts);
            }
            if let Some(value) = ElseClause {
                walk_ast_expr(value, facts);
            }
        }
        ExprKind::Subquery { Query, .. } => {
            let _ = Query.with_node(|node| walk_ast_node(node, facts));
        }
        ExprKind::InSubquery { Expr, Sel, .. } => {
            walk_ast_expr(Expr, facts);
            walk_ast_expr(Sel, facts);
        }
        ExprKind::Value(_)
        | ExprKind::IntroducedValue { .. }
        | ExprKind::Column(_)
        | ExprKind::NamedDefault(_)
        | ExprKind::MaxValue
        | ExprKind::TimeUnit(_)
        | ExprKind::GetFormatSelector(_)
        | ExprKind::TrimDirection(_)
        | ExprKind::DefaultValue => {}
    }
}

fn walk_ast_with(with: Option<&ast::WithClauseRef>, facts: &mut AstFacts) {
    let Some(with) = with else { return };
    let with = with.borrow();
    for cte in &with.CTEs {
        walk_ast_node(cte.Query.as_ref(), facts);
    }
}

fn walk_ast_fields(fields: &[ast::SelectField], facts: &mut AstFacts) {
    for field in fields {
        if let Some(expr) = &field.Expr {
            walk_ast_expr(expr, facts);
        }
    }
}

fn walk_ast_select(select: &ast::SelectStmt, facts: &mut AstFacts) {
    walk_ast_with(select.With.as_ref(), facts);
    if let Some(from) = &select.From {
        walk_ast_join(&from.TableRefs, facts);
    }
    walk_ast_fields(&select.Fields.Fields, facts);
    if let Some(expr) = &select.Where {
        walk_ast_expr(expr, facts);
    }
    walk_ast_by_items(&select.GroupBy, facts);
    if let Some(expr) = &select.Having {
        walk_ast_expr(expr, facts);
    }
    walk_ast_by_items(&select.OrderBy, facts);
    walk_ast_limit(select.Limit.as_ref(), facts);
    for row in &select.Lists {
        for value in &row.Values {
            walk_ast_expr(value, facts);
        }
    }
    for spec in &select.WindowSpecs {
        walk_ast_window_spec(spec, facts);
    }
    for child in &select.children {
        walk_ast_node(child.as_ref(), facts);
    }
}

fn walk_ast_node(node: &dyn ast::Node, facts: &mut AstFacts) {
    if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
        walk_ast_select(select, facts);
    } else if let Some(set) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
        walk_ast_with(set.With.as_ref(), facts);
        walk_ast_with(set.select_list.With.as_ref(), facts);
        for select in &set.select_list.selects {
            walk_ast_node(select.as_ref(), facts);
        }
        walk_ast_by_items(&set.select_list.OrderBy, facts);
        walk_ast_limit(set.select_list.Limit.as_ref(), facts);
        walk_ast_by_items(&set.OrderBy, facts);
        walk_ast_limit(set.Limit.as_ref(), facts);
    } else if let Some(insert) = node.as_any().downcast_ref::<ast::InsertStmt>() {
        if let Some(table) = &insert.Table {
            walk_ast_join(&table.TableRefs, facts);
        }
        for row in &insert.Lists {
            for value in row {
                walk_ast_expr(value, facts);
            }
        }
        for assignment in &insert.OnDuplicate {
            walk_ast_expr(&assignment.Expr, facts);
        }
        if let Some(select) = &insert.Select {
            walk_ast_node(select.as_ref(), facts);
        }
        walk_ast_fields(&insert.Returning, facts);
    } else if let Some(update) = node.as_any().downcast_ref::<ast::UpdateStmt>() {
        walk_ast_with(update.With.as_ref(), facts);
        if let Some(table) = &update.TableRefs {
            walk_ast_join(&table.TableRefs, facts);
        }
        for assignment in &update.List {
            walk_ast_expr(&assignment.Expr, facts);
        }
        if let Some(expr) = &update.Where {
            walk_ast_expr(expr, facts);
        }
        walk_ast_by_items(&update.Order, facts);
        walk_ast_limit(update.Limit.as_ref(), facts);
        walk_ast_fields(&update.Returning, facts);
    } else if let Some(delete) = node.as_any().downcast_ref::<ast::DeleteStmt>() {
        walk_ast_with(delete.With.as_ref(), facts);
        if let Some(table) = &delete.TableRefs {
            walk_ast_join(&table.TableRefs, facts);
        }
        for table in &delete.Tables {
            record_ast_table(facts, table, "");
        }
        if let Some(expr) = &delete.Where {
            walk_ast_expr(expr, facts);
        }
        walk_ast_by_items(&delete.Order, facts);
        walk_ast_limit(delete.Limit.as_ref(), facts);
        walk_ast_fields(&delete.Returning, facts);
    } else if let Some(explain) = node.as_any().downcast_ref::<ast::ExplainStmt>() {
        if let Some(statement) = &explain.stmt {
            walk_ast_node(statement.as_ref(), facts);
        }
    }
}

fn collect_ast_facts(node: &dyn ast::Node) -> AstFacts {
    let mut facts = AstFacts::default();
    walk_ast_node(node, &mut facts);
    facts
}

/// 收集语句中引用的全部表名。
pub fn CollectTableNames(input: &Statement) -> Vec<TableName> {
    if !input.Tables.is_empty() {
        return input.Tables.clone();
    }
    match parse_sql(&input.SQL, "", "") {
        Ok(statement) => {
            let tables = collect_ast_facts(statement.as_ref()).tables;
            if tables.is_empty() {
                input.Tables.clone()
            } else {
                tables
            }
        }
        Err(_) => input.Tables.clone(),
    }
}

/// 表名收集器：以访问者（visitor）风格逐个收集遍历到的表名。
pub struct tableNameCollector {
    tables: Vec<TableName>,
}

/// 创建一个空的表名收集器。
pub fn newCollectTableName() -> tableNameCollector {
    tableNameCollector { tables: Vec::new() }
}

impl tableNameCollector {
    /// 访问者进入节点时记录一个表名。
    pub fn Enter(&mut self, table: &TableName) {
        self.tables.push(table.clone());
    }

    /// 结束遍历，返回收集到的表名列表。
    pub fn Leave(self) -> Vec<TableName> {
        self.tables
    }
}

/// 绑定 SQL 合法性校验器：由调用方（会话）提供具体校验实现。
pub trait BindingValidator {
    fn validate_binding_sql(&self, sql: &str) -> Result<()>;
    /// 返回当前计划基线开关；无会话状态的校验器可保持默认。
    fn use_plan_baselines(&self) -> Option<bool> {
        None
    }
    /// 临时切换计划基线开关，与 [`Self::use_plan_baselines`] 配对。
    fn set_use_plan_baselines(&self, _enabled: bool) {}
}

/// 校验绑定 SQL 并从其中的 `/*+ ... */` 注释块解析 hint 列表，
/// 结果写入 `binding.Hint`；找不到注释块时得到空列表。
pub fn prepareHints(sctx: &dyn BindingValidator, binding: &mut Binding) -> Result<()> {
    match catch_unwind(AssertUnwindSafe(|| prepare_hints_inner(sctx, binding))) {
        Ok(result) => result,
        Err(payload) => {
            let panic = payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(BindError(format!(
                "panic when preparing hints for binding {}, panic: {}",
                binding.BindSQL, panic
            )))
        }
    }
}

fn prepare_hints_inner(sctx: &dyn BindingValidator, binding: &mut Binding) -> Result<()> {
    if (!binding.Hint.Hints.is_empty() && !binding.ID.is_empty()) || binding.Status == StatusDeleted
    {
        return Ok(());
    }

    let parsed_statement = parse_sql(&binding.BindSQL, &binding.Charset, &binding.Collation)?;
    let parsed_facts = collect_ast_facts(parsed_statement.as_ref());
    let cross_db = parsed_facts.tables.iter().any(|table| table.Schema == "*");
    let db_name = if cross_db { "*" } else { &binding.Db };
    let mut parser = parser::Parser::default();
    let (hints_set, statement, warnings) = hint::ParseHintsSet(
        &mut parser,
        &binding.BindSQL,
        &binding.Charset,
        &binding.Collation,
        db_name,
    )
    .map_err(|error| BindError(error.to_string()))?;
    let facts = collect_ast_facts(statement.as_ref());
    if !cross_db && !facts.has_param {
        checkBindingValidation(sctx, &binding.BindSQL)?;
    }

    let hints_string = hints_set
        .Restore()
        .map_err(|error| BindError(error.to_string()))?;
    if hints_string.is_empty() {
        if let Some(warning) = warnings.first() {
            return Err(BindError(warning.to_string()));
        }
    }
    let mut parsed_hints = Vec::new();
    for table_hint in hints_set.tableHints.iter().flatten() {
        parsed_hints.push(hint::RestoreTableOptimizerHint(table_hint));
    }
    for index_hint in hints_set.indexHints.iter().flatten() {
        parsed_hints.push(
            hint::RestoreIndexHint(index_hint).map_err(|error| BindError(error.to_string()))?,
        );
    }
    binding.Hint.Hints = parsed_hints;
    binding.ID = hints_string;
    binding.TableNames = facts.tables;
    Ok(())
}

/// 在缓存绑定与来自存储的绑定之间选出最新的有效版本。
///
/// 以（更新时间、创建时间）为序取最新记录；若最新记录状态为
/// `deleted`（墓碑），则视为该绑定已被删除，返回 `None`。
pub fn pickCachedBinding(
    cachedBinding: Option<Arc<Binding>>,
    bindingsFromStorage: impl IntoIterator<Item = Arc<Binding>>,
) -> Option<Arc<Binding>> {
    let mut bindings = Vec::new();
    if let Some(binding) = cachedBinding {
        bindings.push(binding);
    }
    bindings.extend(bindingsFromStorage);
    let max_update_time = bindings.iter().map(|binding| binding.UpdateTime).max()?;
    bindings
        .into_iter()
        .find(|binding| binding.UpdateTime == max_update_time && binding.Status != StatusDeleted)
}

/// 为语句中省略库名的表补上默认库前缀（`db.table`），返回改写后的 SQL。
pub fn RestoreDBForBinding(node: &Statement, defaultDB: &str) -> String {
    let Ok(statement) = parse_sql(&node.SQL, "", "") else {
        return String::new();
    };
    utilparser::RestoreWithDefaultDB(statement.as_ref(), defaultDB, &node.SQL)
}

/// 将语句归一化并计算摘要，返回（归一化 SQL、摘要）。
///
/// `noDB` 为 true 时生成去库名形式（归一化时剥掉 `db.` 前缀）；
/// 否则先用 `specifiedDB` 补全省略的库名再归一化。
pub fn NormalizeStmtForBinding(
    stmtNode: &Statement,
    specifiedDB: &str,
    noDB: bool,
) -> (String, String) {
    if stmtNode.SQL.is_empty() {
        return (String::new(), String::new());
    }
    let Ok(statement) = parse_sql(&stmtNode.SQL, "", "") else {
        return (String::new(), String::new());
    };
    normalize_parsed_statement(statement.as_ref(), &stmtNode.SQL, specifiedDB, noDB)
}

/// 判断语句是否属于 SQL binding 支持的 DML 类型。
fn is_bindable_statement(node: &dyn ast::Node) -> bool {
    node.as_any().is::<ast::SelectStmt>()
        || node.as_any().is::<ast::SetOprStmt>()
        || node.as_any().is::<ast::DeleteStmt>()
        || node.as_any().is::<ast::UpdateStmt>()
        || node.as_any().is::<ast::InsertStmt>()
}

fn normalize_parsed_statement(
    node: &dyn ast::Node,
    original_sql: &str,
    specified_db: &str,
    no_db: bool,
) -> (String, String) {
    let node = if let Some(explain) = node.as_any().downcast_ref::<ast::ExplainStmt>() {
        let Some(statement) = explain.stmt.as_deref() else {
            return (String::new(), String::new());
        };
        if !is_bindable_statement(statement) {
            return (String::new(), String::new());
        }
        statement
    } else if is_bindable_statement(node) {
        node
    } else {
        return (String::new(), String::new());
    };

    let restored = if no_db {
        utilparser::RestoreWithoutDB(node)
    } else {
        utilparser::RestoreWithDefaultDB(node, specified_db, original_sql)
    };
    if restored.is_empty() {
        return (String::new(), String::new());
    }
    let (normalized, digest) = parser::NormalizeDigestForBinding(&restored);
    (normalized, digest.String().to_owned())
}

/// 若语句最后一个字符是分号，仅删除该分号。
pub fn eraseLastSemicolon(stmt: &mut Statement) {
    if stmt.SQL.ends_with(';') {
        stmt.SQL.pop();
    }
}

/// 参数占位符检查器：以访问者风格检查语句是否包含 `?` 占位符。
pub struct paramChecker {
    has_param: bool,
}

impl paramChecker {
    /// 访问语句节点，累积是否发现参数占位符。
    pub fn Enter(&mut self, statement: &Statement) {
        self.has_param |= hasParam(statement);
    }

    /// 结束检查，返回是否包含参数占位符。
    pub fn Leave(self) -> bool {
        self.has_param
    }
}

/// 判断语句是否包含参数占位符（`?`）；含占位符的语句不能创建绑定。
pub fn hasParam(stmt: &Statement) -> bool {
    if stmt.HasParamMarker {
        return true;
    }
    parse_sql(&stmt.SQL, "", "")
        .map(|statement| collect_ast_facts(statement.as_ref()).has_param)
        .unwrap_or(false)
}

/// 校验绑定 SQL 的合法性：非空检查后交由校验器做具体校验。
pub fn checkBindingValidation(sctx: &dyn BindingValidator, bindingSQL: &str) -> Result<()> {
    if bindingSQL.trim().is_empty() {
        return Err(BindError("binding SQL is empty".to_owned()));
    }
    let original = sctx.use_plan_baselines();
    if original.is_some() {
        sctx.set_use_plan_baselines(false);
    }
    let result = sctx.validate_binding_sql(bindingSQL);
    if let Some(original) = original {
        sctx.set_use_plan_baselines(original);
    }
    result
}
