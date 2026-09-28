// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

#![allow(non_snake_case)]

// 运行时 PlanBuilder：基于真实 AST 构建逻辑计划。
//
// 相对轻量的 `planbuilder.rs` 桩结构，本模块对接 parser AST、InfoSchema、
// hint 与逻辑/物理算子，提供结果集构建、DML（INSERT/UPDATE/DELETE）
// 与 EXPLAIN 等完整构建路径，并可注入 DataSourceProvider。

use crate::{ast, context};
use base_dependency as base;
use base_dependency::Plan as _;
use expression_dependency as expression;
use expression_dependency::Expression as _;
use hint_dependency as hint;
use infoschema_dependency as infoschema;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use physicalop_dependency as physicalop;
use plannererrors_dependency as plannererrors;
use resolve_dependency as resolve;
use rule_dependency as rule;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

/// 共享可变的 CTE 构建状态引用。
pub type CteInfoRef = std::rc::Rc<RefCell<CteInfo>>;

#[derive(Default)]
/// 运行时 CTE 构建标志。
pub struct CteInfo {
    /// 是否正在构建该 CTE。
    pub isBuilding: bool,
    /// 是否已进入子查询。
    pub enterSubquery: bool,
}

/// 运行时权限访问信息（含 expression::Error）。
pub struct VisitInfo {
    /// 数据库名。
    pub db: String,
    /// 表名。
    pub table: String,
    /// 列名。
    pub column: String,
    /// 关联错误信息。
    pub error: expression::Error,
    /// 是否允许写类 ALTER 权限语义。
    pub alterWritable: bool,
    /// 动态权限名列表。
    pub dynamicPrivs: Vec<String>,
    /// 动态权限是否带 GRANT OPTION。
    pub dynamicWithGrant: bool,
}

impl VisitInfo {
    /// 构造仅含动态权限的 VisitInfo。
    pub fn dynamic(privileges: Vec<String>, with_grant: bool, error: expression::Error) -> Self {
        Self {
            db: String::new(),
            table: String::new(),
            column: String::new(),
            error,
            alterWritable: false,
            dynamicPrivs: privileges,
            dynamicWithGrant: with_grant,
        }
    }
}

/// The subquery path only needs to discard the handle scope created by its
/// result-set builder. Full DML builders can store their maps in the same stack
/// without exposing the concrete handle-column representation here.
#[derive(Default)]
pub struct HandleColHelper {
    scopes: Vec<usize>,
}

impl HandleColHelper {
    /// 压入一层 handle 列作用域映射。
    pub fn pushMap(&mut self) {
        self.scopes.push(0);
    }

    /// 弹出当前 handle 列作用域映射。
    pub fn popMap(&mut self) {
        self.scopes
            .pop()
            .expect("result-set builder must push one handle scope");
    }

    /// 清空状态以便 PlanBuilder 对象池复用。
    pub fn resetForReuse(&mut self) {
        self.scopes.clear();
    }
}

/// 结果集节点构建回调函数类型。
pub type ResultSetBuildFn = fn(
    &mut PlanBuilder,
    &dyn context::Context,
    &ast::NodeRef,
    bool,
) -> Result<logicalop::LogicalPlanRef, expression::Error>;

/// 运行时构建结果：逻辑计划或非逻辑计划。
pub enum BuiltRuntimePlan {
    Logical(logicalop::LogicalPlanRef),
    NonLogical(Box<dyn base::Plan>),
}

/// EMPTY_EXPLAIN_STATS：计划构建相关符号（对齐 Go 同名定义）。
static EMPTY_EXPLAIN_STATS: std::sync::LazyLock<property_dependency::StatsInfo> =
    std::sync::LazyLock::new(property_dependency::StatsInfo::default);

/// Non-logical EXPLAIN wrapper.  Its target is fully optimized during plan
/// building, matching Go's `buildExplain` boundary.
pub struct RuntimeExplain {
    /// 简单 Schema 生产者基座。
    pub SimpleSchemaProducer: physicalop::SimpleSchemaProducer,
    /// EXPLAIN 目标物理计划。
    pub TargetPlan: Box<dyn base::PhysicalPlan>,
    /// EXPLAIN 输出格式。
    pub Format: String,
    /// 是否 EXPLAIN ANALYZE。
    pub Analyze: bool,
}

impl RuntimeExplain {
    /// 构造 RuntimeExplain 并初始化空 Schema。
    fn New(
        ctx: base::ContextRef,
        target: Box<dyn base::PhysicalPlan>,
        format: String,
        analyze: bool,
    ) -> Self {
        let mut producer = physicalop::SimpleSchemaProducer::New(ctx, "Explain", 0);
        producer.SetSchema(expression::NewSchema(Vec::new()));
        Self {
            SimpleSchemaProducer: producer,
            TargetPlan: target,
            Format: format,
            Analyze: analyze,
        }
    }
}

impl base::Plan for RuntimeExplain {
    /// as_any：计划构建相关符号（对齐 Go 同名定义）。
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    /// as_any_mut：可变计划类型边界。
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    /// schema：计划构建相关符号（对齐 Go 同名定义）。
    fn schema(&self) -> &expression::Schema {
        self.SimpleSchemaProducer
            .SchemaRef()
            .expect("Explain initializes schema")
    }
    /// id：计划构建相关符号（对齐 Go 同名定义）。
    fn id(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.ID()
    }
    /// 设置_id（对应同名 Go 逻辑）。
    fn set_id(&mut self, id: i32) {
        self.SimpleSchemaProducer.Plan.SetID(id)
    }
    /// tp：计划构建相关符号（对齐 Go 同名定义）。
    fn tp(&self, flags: &[bool]) -> String {
        self.SimpleSchemaProducer.Plan.TP(flags)
    }
    /// explain_id：计划构建相关符号（对齐 Go 同名定义）。
    fn explain_id(&self, flags: &[bool]) -> Box<dyn std::fmt::Display + '_> {
        self.SimpleSchemaProducer.Plan.ExplainID(flags)
    }
    /// explain_info：计划构建相关符号（对齐 Go 同名定义）。
    fn explain_info(&self) -> String {
        self.SimpleSchemaProducer.Plan.ExplainInfo()
    }
    /// replace_expr_columns：计划构建相关符号（对齐 Go 同名定义）。
    fn replace_expr_columns(&mut self, replace: &HashMap<String, expression::Column>) {
        self.SimpleSchemaProducer.Plan.ReplaceExprColumns(replace)
    }
    /// s_ctx：计划构建相关符号（对齐 Go 同名定义）。
    fn s_ctx(&self) -> &base::ContextRef {
        self.SimpleSchemaProducer.Plan.SCtx()
    }
    /// stats_info：计划构建相关符号（对齐 Go 同名定义）。
    fn stats_info(&self) -> &property_dependency::StatsInfo {
        &EMPTY_EXPLAIN_STATS
    }
    /// output_names：计划构建相关符号（对齐 Go 同名定义）。
    fn output_names(&self) -> base::types::NameSlice {
        self.SimpleSchemaProducer.OutputNames()
    }
    /// 设置_output_names（对应同名 Go 逻辑）。
    fn set_output_names(&mut self, names: base::types::NameSlice) {
        self.SimpleSchemaProducer.SetOutputNames(names)
    }
    /// query_block_offset：计划构建相关符号（对齐 Go 同名定义）。
    fn query_block_offset(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.QueryBlockOffset()
    }
    /// 克隆_for_plan_cache（对应同名 Go 逻辑）。
    fn clone_for_plan_cache(
        &self,
        new_ctx: base::ContextRef,
    ) -> (Option<Box<dyn base::Plan>>, bool) {
        let Ok(target) = self.TargetPlan.clone_physical(new_ctx.clone()) else {
            return (None, false);
        };
        (
            Some(Box::new(Self {
                SimpleSchemaProducer: self.SimpleSchemaProducer.CloneSelfForPlanCache(new_ctx),
                TargetPlan: target,
                Format: self.Format.clone(),
                Analyze: self.Analyze,
            })),
            true,
        )
    }
    /// 设置_noncacheable_reason（对应同名 Go 逻辑）。
    fn set_noncacheable_reason(&mut self, reason: String) {
        self.SimpleSchemaProducer.Plan.SetNoncacheableReason(reason)
    }
    /// 获取_noncacheable_reason（对应同名 Go 逻辑）。
    fn get_noncacheable_reason(&self) -> String {
        self.SimpleSchemaProducer.Plan.GetNoncacheableReason()
    }
}

#[derive(Default)]
/// 收集 INSERT ON DUPLICATE 中引用的列名。
struct InsertOnDuplicateColumnCollector {
    columns: Vec<ast::ColumnName>,
    has_aggregate: bool,
}

impl ast::ExprNodeVisitor for InsertOnDuplicateColumnCollector {
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        match &input.Kind {
            ast::ExprKind::Column(column) => self.columns.push(column.clone()),
            ast::ExprKind::AggregateFunction { .. } => self.has_aggregate = true,
            _ => {}
        }
        (input.clone(), false)
    }

    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        (input.clone(), true)
    }
}

/// 克隆去掉 CTE 定义的 SELECT，供内部重写。
fn clone_select_without_ctes(select: &ast::SelectStmt) -> Option<ast::SelectStmt> {
    if select.With.is_some() || !select.children.is_empty() {
        return None;
    }
    Some(ast::SelectStmt {
        node_text: Default::default(),
        Kind: select.Kind,
        SelectStmtOpts: select.SelectStmtOpts.clone(),
        Distinct: select.Distinct,
        From: select.From.clone(),
        Where: select.Where.clone(),
        Fields: select.Fields.clone(),
        GroupBy: select.GroupBy.clone(),
        GroupByRollup: select.GroupByRollup,
        Having: select.Having.clone(),
        OrderBy: select.OrderBy.clone(),
        Limit: select.Limit.clone(),
        TableHints: select.TableHints.clone(),
        IsInBraces: select.IsInBraces,
        QueryBlockOffset: select.QueryBlockOffset,
        With: None,
        WithBeforeBraces: select.WithBeforeBraces,
        lock_info: select.lock_info.clone(),
        children: Vec::new(),
        Lists: select.Lists.clone(),
        WindowSpecs: select.WindowSpecs.clone(),
        SelectIntoOpt: select.SelectIntoOpt.clone(),
    })
}

/// Storage/statistics boundary for a resolved logical table.  The default AST
/// builder creates a complete pseudo-statistics table path first, then lets an
/// installed provider replace it with catalog-backed statistics/access paths.
/// 已解析逻辑表的存储/统计边界：默认先建伪统计路径，再由 provider 替换。
pub trait DataSourceProvider: Send + Sync {
    /// 填充 DataSource 的统计与访问路径信息。
    fn Populate(
        &self,
        ctx: &dyn context::Context,
        plan_ctx: &base::ContextRef,
        info_schema: &dyn infoschema::InfoSchema,
        table_name: &ast::TableName,
        source: &mut logicalop::DataSource,
    ) -> Result<(), expression::Error>;
}

/// DataSourceProvider 的共享引用。
pub type DataSourceProviderRef = Arc<dyn DataSourceProvider>;

/// Initialized, reusable planner state. It deliberately does not contain an
/// expression-rewriter pool: each rewrite owns a fully reset local rewriter.
/// 已初始化、可复用的计划器状态（不含表达式重写器池）。
pub struct PlanBuilder {
    /// 计划上下文（会话、表达式环境等）。
    pub ctx: base::ContextRef,
    /// 当前 InfoSchema（信息系统）快照。
    pub is: Arc<dyn infoschema::InfoSchema>,
    /// 外层查询 Schema 栈（相关子查询用）。
    pub outerSchemas: Vec<expression::Schema>,
    /// 外层输出列名栈。
    pub outerNames: Vec<expression::types::NameSlice>,
    /// 外层可见 CTE 信息栈。
    pub outerCTEs: Vec<CteInfoRef>,
    /// 当前结果集构建回调可继承的运行时 CTE 环境。
    pub(crate) runtimeCTEs: Option<crate::logical_plan_builder_runtime::CteEnvironment>,
    /// 外层 BLOCK EXPAND 算子栈。
    pub outerBlockExpand: Vec<Option<Box<logicalop::LogicalExpand>>>,
    /// 当前查询块的 LogicalExpand。
    pub currentBlockExpand: Option<Box<logicalop::LogicalExpand>>,
    /// 列下标映射。
    pub colMapper: HashMap<usize, usize>,
    /// 权限访问信息列表。
    pub visitInfo: Vec<VisitInfo>,
    /// 表级提示信息栈。
    pub tableHintInfo: Vec<hint::PlanHints>,
    /// 优化器标志位集合。
    pub optFlag: u64,
    /// 当前所处 SQL 子句编码。
    pub curClause: usize,
    /// 窗口定义名到规格的映射。
    pub windowSpecs: HashMap<String, ast::WindowSpec>,
    /// 是否处于 UPDATE 语句构建中。
    pub inUpdateStmt: bool,
    /// 是否处于 DELETE 语句构建中。
    pub inDeleteStmt: bool,
    /// 是否为 FOR UPDATE 读路径。
    pub isForUpdateRead: bool,
    /// 当前查询块是否要求 STRAIGHT_JOIN 顺序。
    pub(crate) inStraightJoin: bool,
    /// handle 列作用域辅助结构。
    pub handleHelper: HandleColHelper,
    /// 查询块（query block）偏移栈。
    pub qbOffset: Vec<i32>,
    /// 累积的输出列名切片。
    pub allNames: Vec<expression::types::NameSlice>,
    /// 相关聚合表达式映射。
    pub correlatedAggMapper: HashMap<String, expression::ExprBox>,
    /// 子查询上下文标志。
    pub subQueryCtx: u64,
    /// 子查询相关提示标志。
    pub subQueryHintFlags: u64,
    /// 是否禁用子查询预处理。
    pub disableSubQueryPreprocessing: bool,
    /// 是否允许构建 CAST ARRAY。
    pub allowBuildCastArray: bool,
    /// 是否启用半连接改写。
    pub enableSemiJoinRewrite: bool,
    /// 是否禁止解相关。
    pub noDecorrelate: bool,
    /// 是否见过不可行的全文匹配。
    nonViableFTSMatch: bool,
    /// 是否见过谓词匹配。
    predicateMatchSeen: bool,
    /// 查询块提示处理器。
    hintProcessor: hint::QBHintHandler,
    /// 提示构建状态。
    hintState: hint::QBHintBuildState,
    /// 结果集构建回调。
    resultSetBuilder: Option<ResultSetBuildFn>,
    /// 可选的 DataSource 填充器。
    dataSourceProvider: Option<DataSourceProviderRef>,
}

#[derive(Default)]
/// 尚未 Init 的 PlanBuilder 建造器。
pub struct PendingPlanBuilder {
    /// 待应用：禁用子查询预处理。
    disable_subquery_preprocessing: bool,
    /// 待应用：允许 CAST ARRAY。
    allow_build_cast_array: bool,
    /// 待注入的结果集构建函数。
    result_set_builder: Option<ResultSetBuildFn>,
    /// 待注入的 DataSourceProvider。
    data_source_provider: Option<DataSourceProviderRef>,
}

impl PendingPlanBuilder {
    /// 链式选项：禁用子查询预处理（对应 noExecution 选项）。
    pub fn noExecution(mut self) -> Self {
        self.disable_subquery_preprocessing = true;
        self
    }

    /// 链式选项：允许 CAST ARRAY。
    pub fn allowCastArray(mut self) -> Self {
        self.allow_build_cast_array = true;
        self
    }

    /// 注入自定义结果集构建函数。
    pub fn withResultSetBuilder(mut self, builder: ResultSetBuildFn) -> Self {
        self.result_set_builder = Some(builder);
        self
    }

    /// 注入 DataSource 填充提供者。
    pub fn withDataSourceProvider(mut self, provider: DataSourceProviderRef) -> Self {
        self.data_source_provider = Some(provider);
        self
    }

    /// 完成 PlanBuilder 初始化并返回自身。
    pub fn Init(
        self,
        ctx: base::ContextRef,
        info_schema: Arc<dyn infoschema::InfoSchema>,
        hint_processor: hint::QBHintHandler,
    ) -> (PlanBuilder, Vec<ast::HintTable>) {
        let saved_block_names = ctx
            .GetSessionVars()
            .PlannerSelectBlockAsName
            .Load()
            .map_or_else(Vec::new, |names| names.as_ref().clone());
        ctx.GetSessionVars()
            .PlannerSelectBlockAsName
            .Store(Some(vec![
                ast::HintTable::default();
                hint_processor.MaxSelectStmtOffset() as usize + 1
            ]));
        let enable_semi_join_rewrite = ctx.GetSessionVars().EnableSemiJoinRewrite
            || ctx
                .GetSessionVars()
                .GetSystemVar(vardef_dependency::TiDBOptEnableSemiJoinRewrite)
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                });
        let no_decorrelate = ctx.GetSessionVars().EnableNoDecorrelateInSelect;
        let hint_state = hint_processor.NewBuildState();
        (
            PlanBuilder {
                ctx,
                is: info_schema,
                outerSchemas: Vec::new(),
                outerNames: Vec::new(),
                outerCTEs: Vec::new(),
                runtimeCTEs: None,
                outerBlockExpand: Vec::new(),
                currentBlockExpand: None,
                colMapper: HashMap::new(),
                visitInfo: Vec::new(),
                tableHintInfo: Vec::new(),
                optFlag: 0,
                curClause: 0,
                windowSpecs: HashMap::new(),
                inUpdateStmt: false,
                inDeleteStmt: false,
                isForUpdateRead: false,
                inStraightJoin: false,
                handleHelper: HandleColHelper::default(),
                qbOffset: Vec::new(),
                allNames: Vec::new(),
                correlatedAggMapper: HashMap::new(),
                subQueryCtx: 0,
                subQueryHintFlags: 0,
                disableSubQueryPreprocessing: self.disable_subquery_preprocessing,
                allowBuildCastArray: self.allow_build_cast_array,
                enableSemiJoinRewrite: enable_semi_join_rewrite,
                noDecorrelate: no_decorrelate,
                nonViableFTSMatch: false,
                predicateMatchSeen: false,
                hintProcessor: hint_processor,
                hintState: hint_state,
                resultSetBuilder: self.result_set_builder,
                dataSourceProvider: self.data_source_provider,
            },
            saved_block_names,
        )
    }
}

/// 使用选项列表构造 PlanBuilder。
pub fn NewPlanBuilder() -> PendingPlanBuilder {
    PendingPlanBuilder::default()
        .withResultSetBuilder(crate::logical_plan_builder_runtime::BuildResultSetNode)
}

impl PlanBuilder {
    /// 是否标记了不可行的全文匹配。
    pub fn HasNonViableFTSMatch(&self) -> bool {
        self.nonViableFTSMatch
    }

    /// 标记存在不可行的全文匹配。
    pub fn MarkNonViableFTSMatch(&mut self) {
        self.nonViableFTSMatch = true;
    }

    /// 是否检测到谓词匹配提示命中。
    pub fn HasPredicateMatch(&self) -> bool {
        self.predicateMatchSeen
    }

    /// 标记谓词匹配已发生。
    pub fn MarkPredicateMatch(&mut self) {
        self.predicateMatchSeen = true;
    }

    /// 取出权限访问信息列表。
    pub fn GetVisitInfo(&self) -> &[VisitInfo] {
        &self.visitInfo
    }

    /// 取出优化器提示构建状态。
    pub fn GetHintState(&self) -> &hint::QBHintBuildState {
        &self.hintState
    }

    /// 取出优化标志位。
    pub fn GetOptFlag(&self) -> u64 {
        self.optFlag
    }

    /// 是否为 FOR UPDATE 读路径。
    pub fn GetIsForUpdateRead(&self) -> bool {
        self.isForUpdateRead
    }

    /// 当前 SELECT 查询块偏移。
    pub fn getSelectOffset(&self) -> i32 {
        self.qbOffset.last().copied().unwrap_or(-1)
    }

    /// 压入查询块偏移。
    pub fn pushSelectOffset(&mut self, offset: i32) {
        self.qbOffset.push(offset);
    }

    /// 弹出查询块偏移。
    pub fn popSelectOffset(&mut self) {
        self.qbOffset
            .pop()
            .expect("query-block offset stack must not be empty");
    }

    /// 当前查询块的表级提示。
    pub fn TableHints(&self) -> Option<&hint::PlanHints> {
        self.tableHintInfo.last()
    }

    /// joinHintPreference：计划构建相关符号（对齐 Go 同名定义）。
    pub(crate) fn joinHintPreference(&self) -> (u64, bool) {
        let Some(hints) = self.TableHints() else {
            return (0, false);
        };
        let mut prefer = 0;
        if !hints.HashJoin.is_empty() {
            prefer |= 1;
        }
        if !hints.SortMergeJoin.is_empty() {
            prefer |= 1 << 1;
        }
        if !hints.IndexJoin.INLJTables.is_empty()
            || !hints.IndexJoin.INLHJTables.is_empty()
            || !hints.IndexJoin.INLMJTables.is_empty()
        {
            prefer |= 1 << 2;
        }
        if !hints.NoIndexJoin.INLJTables.is_empty() {
            prefer |= u64::from(hint::PreferNoIndexJoin);
        }
        if !hints.NoIndexJoin.INLHJTables.is_empty() {
            prefer |= u64::from(hint::PreferNoIndexHashJoin);
        }
        if !hints.NoIndexJoin.INLMJTables.is_empty() {
            prefer |= u64::from(hint::PreferNoIndexMergeJoin);
        }
        (
            prefer,
            hints.StraightJoinOrder || !hints.LeadingJoinOrder.is_empty(),
        )
    }

    /// Resolve method hints against this join's visible table aliases.
    pub(crate) fn joinHintPreferenceFor(
        &mut self,
        left: &dyn logicalop::LogicalPlan,
        right: &dyn logicalop::LogicalPlan,
    ) -> (u64, bool) {
        let (left_index, right_index) = self.joinHintSidePreference(left, right);
        let Some(hints) = self.TableHints() else {
            return (0, false);
        };
        fn matches_visible_table(plan: &dyn logicalop::LogicalPlan, table: &str) -> bool {
            if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
                let visible = source
                    .TableAsName
                    .as_ref()
                    .unwrap_or(&source.TableInfo.Name);
                return visible.L == table;
            }
            plan.Children()
                .iter()
                .any(|child| matches_visible_table(child.as_ref(), table))
        }
        let both_match = |tables: &[hint::HintedTable]| {
            tables
                .iter()
                .any(|table| matches_visible_table(left, &table.TblName.L))
                && tables
                    .iter()
                    .any(|table| matches_visible_table(right, &table.TblName.L))
        };
        let mut prefer = 0;
        if both_match(&hints.HashJoin) {
            prefer |= 1;
        }
        if both_match(&hints.SortMergeJoin) {
            prefer |= 1 << 1;
        }
        if left_index != 0 || right_index != 0 {
            prefer |= 1 << 2;
        }
        if both_match(&hints.NoIndexJoin.INLJTables) {
            prefer |= u64::from(hint::PreferNoIndexJoin);
        }
        if both_match(&hints.NoIndexJoin.INLHJTables) {
            prefer |= u64::from(hint::PreferNoIndexHashJoin);
        }
        if both_match(&hints.NoIndexJoin.INLMJTables) {
            prefer |= u64::from(hint::PreferNoIndexMergeJoin);
        }
        (
            prefer,
            hints.StraightJoinOrder || !hints.LeadingJoinOrder.is_empty(),
        )
    }

    /// joinHintSidePreference：计划构建相关符号（对齐 Go 同名定义）。
    pub(crate) fn joinHintSidePreference(
        &mut self,
        left: &dyn logicalop::LogicalPlan,
        right: &dyn logicalop::LogicalPlan,
    ) -> (u64, u64) {
        let Some(hints) = self.tableHintInfo.last_mut() else {
            return (0, 0);
        };
        fn matches_visible_table(plan: &dyn logicalop::LogicalPlan, table: &str) -> bool {
            if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
                let visible = source
                    .TableAsName
                    .as_ref()
                    .unwrap_or(&source.TableInfo.Name);
                return visible.L == table;
            }
            plan.Children()
                .iter()
                .any(|child| matches_visible_table(child.as_ref(), table))
        }
        let mut left_match = false;
        let mut right_match = false;
        for hinted in hints
            .IndexJoin
            .INLJTables
            .iter_mut()
            .chain(&mut hints.IndexJoin.INLHJTables)
            .chain(&mut hints.IndexJoin.INLMJTables)
        {
            let matches_left = matches_visible_table(left, &hinted.TblName.L);
            let matches_right = matches_visible_table(right, &hinted.TblName.L);
            hinted.Matched |= matches_left || matches_right;
            left_match |= matches_left;
            right_match |= matches_right;
        }
        (u64::from(left_match) << 2, u64::from(right_match) << 2)
    }

    /// 为子查询准备 CTE 检查快照。
    pub fn prepareCTECheckForSubQuery(&mut self) -> Vec<CteInfoRef> {
        let mut modified = Vec::new();
        for cte in &self.outerCTEs {
            let should_mark = {
                let cte = cte.borrow();
                cte.isBuilding && !cte.enterSubquery
            };
            if should_mark {
                cte.borrow_mut().enterSubquery = true;
                modified.push(cte.clone());
            }
        }
        modified
    }

    /// 由 AST 结果集节点构建逻辑计划。
    pub fn buildResultSetNode(
        &mut self,
        ctx: &dyn context::Context,
        node: &ast::NodeRef,
        is_cte: bool,
    ) -> Result<logicalop::LogicalPlanRef, expression::Error> {
        let builder = self
            .resultSetBuilder
            .ok_or_else(|| expression::errors::New("result-set plan builder is not installed"))?;
        builder(self, ctx, node, is_cte)
    }

    /// 根据语句分派并构建对应计划。
    pub fn Build(
        &mut self,
        ctx: &dyn context::Context,
        node: &resolve::NodeW,
    ) -> Result<BuiltRuntimePlan, expression::Error> {
        self.BuildNodeRef(ctx, &node.node)
    }

    /// Canonical AST entry shared by the resolver-backed optimizer and direct
    /// parser callers.  Subquery callers intentionally keep using
    /// `buildResultSetNode`, whose contract is logical-result-set-only.
    pub fn BuildNodeRef(
        &mut self,
        ctx: &dyn context::Context,
        node: &ast::NodeRef,
    ) -> Result<BuiltRuntimePlan, expression::Error> {
        node.with_node(|statement| {
            if let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() {
                return self
                    .buildInsert(ctx, insert)
                    .map(BuiltRuntimePlan::NonLogical);
            }
            if let Some(delete) = statement.as_any().downcast_ref::<ast::DeleteStmt>() {
                return self
                    .buildDelete(ctx, delete)
                    .map(BuiltRuntimePlan::NonLogical);
            }
            if let Some(update) = statement.as_any().downcast_ref::<ast::UpdateStmt>() {
                return self
                    .buildUpdate(ctx, update)
                    .map(BuiltRuntimePlan::NonLogical);
            }
            if let Some(explain) = statement.as_any().downcast_ref::<ast::ExplainStmt>() {
                return self
                    .buildExplain(ctx, explain)
                    .map(BuiltRuntimePlan::NonLogical);
            }
            crate::logical_plan_builder_runtime::BuildBorrowedResultSetNode(self, ctx, statement)
                .map(BuiltRuntimePlan::Logical)
        })
        .ok_or_else(|| expression::errors::New("statement AST node has already been consumed"))?
    }

    /// 构建 EXPLAIN（含 analyze/format）计划。
    fn buildExplain(
        &mut self,
        ctx: &dyn context::Context,
        statement: &ast::ExplainStmt,
    ) -> Result<Box<dyn base::Plan>, expression::Error> {
        let target = statement
            .stmt
            .as_deref()
            .ok_or_else(|| expression::errors::New("EXPLAIN target is missing"))?;
        let mut logical =
            crate::logical_plan_builder_runtime::BuildBorrowedResultSetNode(self, ctx, target)?;
        let (physical, _) = crate::DoOptimize(ctx, &self.ctx, self.optFlag, &mut logical)?;
        Ok(Box::new(RuntimeExplain::New(
            self.ctx.clone(),
            physical,
            statement.Format.clone(),
            statement.analyze,
        )))
    }

    /// Builds Go's non-logical UPDATE wrapper around the optimized row source.
    /// Assignment expressions are rewritten in statement order so later
    /// assignments observe the same schema (including subquery Apply nodes)
    /// produced by earlier rewrites.
    fn buildUpdate(
        &mut self,
        ctx: &dyn context::Context,
        statement: &ast::UpdateStmt,
    ) -> Result<Box<dyn base::Plan>, expression::Error> {
        if statement.With.is_some() {
            return Err(expression::errors::New(
                "UPDATE with CTE requires the CTE builder before the row plan",
            ));
        }
        let select = ast::SelectStmt {
            From: statement.TableRefs.clone(),
            Where: statement.Where.clone(),
            Fields: ast::FieldList {
                Fields: vec![ast::SelectField {
                    WildCard: Some(ast::WildCardField::default()),
                    ..Default::default()
                }],
            },
            OrderBy: statement.Order.clone(),
            Limit: statement.Limit.clone(),
            TableHints: statement.TableHints.clone(),
            ..Default::default()
        };
        let previous_update = self.inUpdateStmt;
        let previous_update_read = self.isForUpdateRead;
        self.inUpdateStmt = true;
        self.isForUpdateRead = true;
        let result =
            crate::logical_plan_builder_runtime::BuildBorrowedResultSetNode(self, ctx, &select);
        self.inUpdateStmt = previous_update;
        self.isForUpdateRead = previous_update_read;
        let mut logical = result?;
        // Go inserts this projection after WHERE/ORDER/LIMIT to freeze the
        // update input's column order before assignment rewriting.
        let frozen_columns = logical.Schema().Columns.clone();
        for column in &frozen_columns {
            self.ctx
                .GetSessionVars()
                .StmtCtx
                .InsertLogicalPlanColumnReference(column.UniqueID as i32);
        }
        let frozen_names = logical.OutputNames().Shallow();
        let frozen_expressions = expression::Column2Exprs(&frozen_columns);
        let mut projection = logicalop::LogicalProjection {
            Exprs: frozen_expressions,
            ..Default::default()
        }
        .Init(self.ctx.clone(), 0);
        projection.SetSchema(expression::NewSchema(frozen_columns));
        projection.SetOutputNames(frozen_names);
        projection.SetChildren(vec![logical]);
        logical = Box::new(projection);
        let mut ordered_list = Vec::with_capacity(statement.List.len());
        let mut all_assignments_are_constant = true;
        for assignment in &statement.List {
            let target = expression::FindFieldName(logical.OutputNames(), &assignment.Column)?
                .ok_or_else(|| {
                    plannererrors::ErrUnknownColumn.GenWithStackByArgs(&[
                        assignment.Column.Name.O.clone().into(),
                        "field list".into(),
                    ])
                })?;
            let column = logical.Schema().Columns[target].Clone();
            let (rewritten, rewritten_plan) = crate::expression_rewriter::rewrite(
                self,
                crate::context::TODOArc(),
                &assignment.Expr,
                logical,
                crate::expression_rewriter::AggregateMapper::default(),
                true,
            )?;
            logical = rewritten_plan;
            let expression = rewritten.ok_or_else(|| {
                expression::errors::New("UPDATE assignment produced no scalar value")
            })?;
            for referenced in expression::ExtractColumns(expression.as_ref()) {
                self.ctx
                    .GetSessionVars()
                    .StmtCtx
                    .InsertLogicalPlanColumnReference(referenced.UniqueID as i32);
            }
            all_assignments_are_constant &= expression.ConstLevel() != expression::ConstNone;
            ordered_list.push(expression::Assignment {
                Col: column,
                ColName: assignment.Column.Name.clone(),
                Expr: expression,
                LazyErr: None,
            });
        }
        let output_names = logical.OutputNames().Shallow();
        // Go keeps the freeze projection through UPDATE subplan optimization;
        // ordered-list columns are resolved before physical post-optimization.
        let update_flags = self.optFlag & !rule::FLAG_ELIMINATE_PROJECTION;
        let (physical, _) =
            crate::optimizer_runtime::DoOptimizeForUpdate(&self.ctx, update_flags, &mut logical)?;
        let mut update = physicalop::Update::New(self.ctx.clone(), physical);
        update.OrderedList = ordered_list;
        update.AllAssignmentsAreConstant = all_assignments_are_constant;
        update.VirtualAssignmentsOffset = statement.List.len();
        update.IgnoreError = statement.IgnoreErr;
        update.set_output_names(output_names);
        update.ResolveIndices()?;
        Ok(Box::new(update))
    }

    /// Builds Go's non-logical DELETE wrapper around an optimized row-producing
    /// plan, including the pruned mixed-row layout consumed by the executor.
    fn buildDelete(
        &mut self,
        ctx: &dyn context::Context,
        statement: &ast::DeleteStmt,
    ) -> Result<Box<dyn base::Plan>, expression::Error> {
        if statement.With.is_some() {
            return Err(expression::errors::New(
                "DELETE with CTE requires the CTE builder before the row plan",
            ));
        }
        for item in &statement.Order {
            let mut windows = Vec::new();
            crate::logical_plan_builder_runtime::collect_window_expressions(
                &item.Expr,
                &mut windows,
            );
            if let Some(ast::ExprNode {
                Kind: ast::ExprKind::WindowFunction { Name, .. },
                ..
            }) = windows.first()
            {
                return Err(plannererrors::ErrWindowInvalidWindowFuncUse
                    .GenWithStackByArgs(&[Name.to_ascii_lowercase().into()])
                    .into());
            }
        }
        let select = ast::SelectStmt {
            From: statement.TableRefs.clone(),
            Where: statement.Where.clone(),
            Fields: ast::FieldList {
                Fields: vec![ast::SelectField {
                    WildCard: Some(ast::WildCardField::default()),
                    ..Default::default()
                }],
            },
            OrderBy: statement.Order.clone(),
            Limit: statement.Limit.clone(),
            TableHints: statement.TableHints.clone(),
            ..Default::default()
        };
        let previous_delete = self.inDeleteStmt;
        let previous_update_read = self.isForUpdateRead;
        self.inDeleteStmt = true;
        self.isForUpdateRead = true;
        let result =
            crate::logical_plan_builder_runtime::BuildBorrowedResultSetNode(self, ctx, &select);
        self.inDeleteStmt = previous_delete;
        self.isForUpdateRead = previous_update_read;
        let mut logical = result?;

        #[derive(Clone)]
        /// DeleteSource：计划构建相关符号（对齐 Go 同名定义）。
        struct DeleteSource {
            info: expression::model::TableInfo,
            alias: String,
            columns: Vec<expression::Column>,
            handle: Vec<expression::Column>,
        }
        /// 收集_sources（对应同名 Go 逻辑）。
        fn collect_sources(plan: &dyn logicalop::LogicalPlan, out: &mut Vec<DeleteSource>) {
            if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
                let handle = source.HandleCols.as_ref().map_or_else(Vec::new, |handles| {
                    (0..handles.NumCols())
                        .filter_map(|index| handles.GetCol(index).map(expression::Column::Clone))
                        .collect()
                });
                out.push(DeleteSource {
                    info: source.TableInfo.Clone(),
                    alias: source
                        .TableAsName
                        .as_ref()
                        .map_or_else(|| source.TableInfo.Name.L.clone(), |name| name.L.clone()),
                    columns: source.Schema().Columns.clone(),
                    handle,
                });
            }
            for child in plan.Children() {
                collect_sources(child.as_ref(), out);
            }
        }

        let mut sources = Vec::new();
        collect_sources(logical.as_ref(), &mut sources);
        let explicit_targets = statement
            .Tables
            .iter()
            .map(|table| table.Name.L.clone())
            .collect::<HashSet<_>>();
        if statement.IsMultiTable {
            sources.retain(|source| {
                explicit_targets.contains(&source.alias)
                    || explicit_targets.contains(&source.info.Name.L)
            });
        }
        if sources.is_empty() {
            return Err(expression::errors::New(
                "DELETE has no updatable base table",
            ));
        }

        let mut retained_ids = HashSet::new();
        let mut layouts = Vec::new();
        let mut output_offset = 0usize;
        for source in &sources {
            let partitioned = source.info.Partition.is_some();
            let mut local_offsets = std::collections::BTreeSet::new();
            if partitioned {
                local_offsets.extend(0..source.info.Columns.len());
            } else {
                for handle in &source.handle {
                    if let Some(offset) = source
                        .columns
                        .iter()
                        .position(|column| column.UniqueID == handle.UniqueID)
                    {
                        local_offsets.insert(offset);
                    }
                }
                for index in &source.info.Indices {
                    for column in &index.Columns {
                        if column.Offset >= 0 {
                            local_offsets.insert(column.Offset as usize);
                        }
                    }
                }
            }
            let mut local_to_pruned = HashMap::new();
            for offset in &local_offsets {
                local_to_pruned.insert(*offset, local_to_pruned.len());
                if let Some(column) = source.columns.get(*offset) {
                    retained_ids.insert(column.UniqueID);
                }
            }
            // `_tidb_rowid` is appended after public columns and is the handle
            // for tables without a clustered primary key.
            for handle in &source.handle {
                retained_ids.insert(handle.UniqueID);
            }
            let public_count = local_offsets
                .iter()
                .filter(|offset| **offset < source.info.Columns.len())
                .count();
            let mut handle_columns = source.handle.clone();
            for handle in &mut handle_columns {
                let local = source
                    .columns
                    .iter()
                    .position(|column| column.UniqueID == handle.UniqueID)
                    .unwrap_or(source.info.Columns.len());
                handle.Index = (output_offset
                    + local_to_pruned.get(&local).copied().unwrap_or(public_count))
                    as isize;
            }
            let index_layouts = (!partitioned).then(|| {
                let layouts = source
                    .info
                    .Indices
                    .iter()
                    .map(|index| physicalop::DeleteIndexLayout {
                        ID: index.ID,
                        Name: index.Name.O.clone(),
                        Columns: index
                            .Columns
                            .iter()
                            .map(|column| column.Name.O.clone())
                            .collect(),
                        Offsets: index
                            .Columns
                            .iter()
                            .filter_map(|column| {
                                local_to_pruned.get(&(column.Offset as usize)).copied()
                            })
                            .collect(),
                    })
                    .collect::<Vec<_>>();
                physicalop::DeleteIndexRowLayout::New(layouts)
            });
            layouts.push(physicalop::TblColPosInfo {
                TblID: source.info.ID,
                Start: output_offset,
                End: output_offset + public_count,
                HandleCols: handle_columns,
                IndexesRowLayout: index_layouts,
            });
            output_offset += public_count
                + usize::from(source.handle.iter().any(|handle| {
                    !source
                        .info
                        .Columns
                        .iter()
                        .any(|column| column.ID == handle.ID)
                }));
        }

        let mut retained_columns = Vec::new();
        let mut retained_names = Vec::new();
        for (index, column) in logical.Schema().Columns.iter().enumerate() {
            if retained_ids.contains(&column.UniqueID) {
                retained_columns.push(column.Clone());
                retained_names.push(logical.OutputNames().0.get(index).cloned().flatten());
            }
        }
        let expressions = retained_columns
            .iter()
            .map(|column| Box::new(column.Clone()) as expression::ExprBox)
            .collect();
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(self.ctx.clone(), 0);
        projection.SetSchema(expression::NewSchema(retained_columns));
        projection.SetOutputNames(expression::types::NameSlice(retained_names));
        projection.SetChildren(vec![logical]);
        logical = Box::new(projection);
        self.optFlag |= rule::FLAG_PRUNE_COLUMNS | rule::FLAG_ELIMINATE_PROJECTION;
        let output_names = logical.OutputNames().Shallow();
        let delete_schema = logical.Schema().Clone();
        let (mut physical, _) = crate::DoOptimize(ctx, &self.ctx, self.optFlag, &mut logical)?;
        let handle_column_name = sources.first().and_then(|source| {
            let handle = source.handle.first()?;
            let offset = source
                .columns
                .iter()
                .position(|column| column.UniqueID == handle.UniqueID)?;
            source
                .info
                .Columns
                .get(offset)
                .map(|column| column.Name.L.clone())
        });
        let fast_handles = statement.Where.as_ref().and_then(|predicate| {
            let value = |node: &ast::ExprNode| match &node.Kind {
                ast::ExprKind::Value(ast::ValueExpr {
                    Datum: ast::ValueDatum::Int64(value),
                    ..
                }) => Some(*value),
                ast::ExprKind::Value(ast::ValueExpr {
                    Datum: ast::ValueDatum::Uint64(value),
                    ..
                }) => i64::try_from(*value).ok(),
                _ => None,
            };
            let is_handle = |node: &ast::ExprNode| {
                matches!(&node.Kind, ast::ExprKind::Column(column)
                    if handle_column_name.as_ref().is_some_and(|handle| column.Name.L == *handle))
            };
            match &predicate.Kind {
                ast::ExprKind::Binary { Op, L, R }
                    if Op == "=" || Op.eq_ignore_ascii_case("eq") =>
                {
                    if is_handle(L) {
                        value(R).map(|value| vec![value])
                    } else if is_handle(R) {
                        value(L).map(|value| vec![value])
                    } else {
                        None
                    }
                }
                ast::ExprKind::InList {
                    Expr, List, Not, ..
                } if !*Not && is_handle(Expr) => List.iter().map(value).collect::<Option<Vec<_>>>(),
                _ => None,
            }
        });
        if !statement.IsMultiTable
            && sources.len() == 1
            && sources[0].info.PKIsHandle
            && let Some(handles) = fast_handles
        {
            if handles.len() == 1 {
                let mut point = physicalop::PointGetPlan::New(self.ctx.clone());
                point.DBName = self.ctx.GetSessionVars().CurrentDB();
                point.TblInfo = Some(sources[0].info.Clone());
                point.Handle = handles.first().copied();
                point.Columns = sources[0].info.Columns.clone();
                point.AccessColumns = sources[0].handle.clone();
                point.CostValue = 1.0;
                point.SetSchema(delete_schema.Clone());
                point.set_output_names(output_names.Shallow());
                physical = Box::new(point);
            } else if !handles.is_empty() {
                let mut batch = physicalop::BatchPointGetPlan::New(self.ctx.clone());
                batch.PointGetPlan.DBName = self.ctx.GetSessionVars().CurrentDB();
                batch.PointGetPlan.TblInfo = Some(sources[0].info.Clone());
                batch.PointGetPlan.Columns = sources[0].info.Columns.clone();
                batch.PointGetPlan.AccessColumns = sources[0].handle.clone();
                batch.PointGetPlan.CostValue = 1.0;
                batch.PointGetPlan.SetSchema(delete_schema.Clone());
                batch.Handles = handles;
                batch.set_output_names(output_names.Shallow());
                physical = Box::new(batch);
            }
        }
        let mut delete = physicalop::Delete::New(self.ctx.clone(), physical);
        delete.IsMultiTable = statement.IsMultiTable;
        delete.IgnoreErr = statement.IgnoreErr;
        delete.TblColPosInfos = layouts;
        delete.set_output_names(output_names);
        Ok(Box::new(delete))
    }

    /// 构建 INSERT/REPLACE 计划。
    fn buildInsert(
        &mut self,
        ctx: &dyn context::Context,
        statement: &ast::InsertStmt,
    ) -> Result<Box<dyn base::Plan>, expression::Error> {
        let table_source = statement
            .Table
            .as_ref()
            .and_then(|clause| clause.TableRefs.Left.as_deref())
            .and_then(|node| match node {
                ast::ResultSetNode::TableSource(source) => Some(source),
                ast::ResultSetNode::Join(_) => None,
            })
            .ok_or_else(|| expression::errors::New("insert target table does not exist"))?;
        if table_source.QuerySource.is_some() {
            return Err(expression::errors::New(
                "insert target must be a base table",
            ));
        }
        let schema_name = if table_source.Source.Schema.O.is_empty() {
            self.ctx.GetSessionVars().CurrentDB()
        } else {
            table_source.Source.Schema.O.clone()
        };
        if schema_name.is_empty() {
            return Err(expression::errors::New("No database selected"));
        }
        let schema_key = infoschema::infoschema::CiString::from(schema_name.as_str());
        let table_key = infoschema::infoschema::CiString::from(table_source.Source.Name.O.as_str());
        let table_info = self
            .is
            .ModelTableInfoByName(&schema_key, &table_key)
            .map_err(|error| expression::errors::New(error.to_string()))?;
        if table_info.IsView() {
            return Err(expression::errors::New(format!(
                "insert into view {} is not supported now",
                table_info.Name.O
            )));
        }
        if table_info.IsSequence() {
            return Err(expression::errors::New(format!(
                "insert into sequence {} is not supported now",
                table_info.Name.O
            )));
        }

        let (table_schema, table_names) = expression::TableInfo2SchemaAndNames(
            self.ctx.GetExprCtx(),
            ast::NewCIStr(&schema_name),
            table_info.as_ref(),
        )?;
        let target_table =
            physicalop::NewInsertTargetTable(table_info.as_ref(), &statement.PartitionNames)?;
        let mut seen = HashSet::new();
        let affected_columns = if statement.Columns.is_empty() {
            table_info
                .Columns
                .iter()
                .filter(|column| !column.Hidden)
                .collect::<Vec<_>>()
        } else {
            statement
                .Columns
                .iter()
                .map(|column| {
                    if !seen.insert(column.Name.L.clone()) {
                        return Err(expression::errors::New(format!(
                            "Column '{}' specified twice",
                            column.Name.O
                        )));
                    }
                    table_info
                        .Columns
                        .iter()
                        .find(|candidate| candidate.Name.L == column.Name.L && !candidate.Hidden)
                        .ok_or_else(|| {
                            expression::errors::New(format!(
                                "Unknown column '{}' in 'field list'",
                                column.Name.O
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
        };

        let select = statement.Select.as_deref().ok_or_else(|| {
            expression::errors::New("this runtime segment requires INSERT ... SELECT")
        })?;
        let mut augmented_select = None;
        let mut actual_column_count = None;
        if !statement.OnDuplicate.is_empty()
            && let Some(select_statement) = select.as_any().downcast_ref::<ast::SelectStmt>()
            && select_statement.GroupBy.is_empty()
            && !select_statement
                .Fields
                .Fields
                .iter()
                .any(|field| field.WildCard.is_some())
        {
            let mut collector = InsertOnDuplicateColumnCollector::default();
            for assignment in &statement.OnDuplicate {
                let _ = assignment.Expr.Accept(&mut collector);
            }
            if !collector.has_aggregate
                && let Some(mut cloned) = clone_select_without_ctes(select_statement)
            {
                actual_column_count = Some(cloned.Fields.Fields.len());
                for column in collector.columns {
                    if table_names.FindAstColName(&column)
                        || cloned.Fields.Fields.iter().any(|field| {
                            field.Expr.as_ref().is_some_and(|expression| {
                                matches!(
                                    &expression.Kind,
                                    ast::ExprKind::Column(existing)
                                        if (column.Schema.L.is_empty()
                                            || existing.Schema.L.is_empty()
                                            || column.Schema.L == existing.Schema.L)
                                            && (column.Table.L.is_empty()
                                                || existing.Table.L.is_empty()
                                                || column.Table.L == existing.Table.L)
                                            && column.Name.L == existing.Name.L
                                )
                            })
                        })
                    {
                        continue;
                    }
                    let offset = cloned.Fields.Fields.len();
                    cloned.Fields.Fields.push(ast::SelectField {
                        Offset: offset,
                        Expr: Some(ast::ExprNode::Column(column)),
                        ..Default::default()
                    });
                }
                augmented_select = Some(cloned);
            }
        }
        let select_for_build: &dyn ast::Node = augmented_select
            .as_ref()
            .map_or(select, |select| select as &dyn ast::Node);
        self.isForUpdateRead = true;
        let mut select_logical = crate::logical_plan_builder_runtime::BuildBorrowedResultSetNode(
            self,
            ctx,
            select_for_build,
        )?;
        let row_len = actual_column_count.unwrap_or_else(|| select_logical.Schema().Len());
        if row_len != affected_columns.len() {
            return Err(expression::errors::New(
                "Column count doesn't match value count at row 1",
            ));
        }
        if let Some(column) = affected_columns.iter().find(|column| column.IsGenerated()) {
            return Err(expression::errors::New(format!(
                "The value specified for generated column '{}' in table '{}' is not allowed",
                column.Name.O, table_info.Name.O
            )));
        }
        let select_names = select_logical.OutputNames().Shallow();
        let (select_plan, _) =
            crate::DoOptimize(ctx, &self.ctx, self.optFlag, &mut select_logical)?;

        // Go exposes both the old target row and the newly projected row while
        // rewriting ON DUPLICATE expressions.  Preserve that layout even when
        // no ON DUPLICATE clause is present because executor construction uses
        // the same schema contract.
        let mut new_row_columns = table_info
            .Columns
            .iter()
            .map(|_| {
                let mut column = expression::Column::default();
                column.UniqueID = self.ctx.GetSessionVars().AllocPlanColumnID();
                column
            })
            .collect::<Vec<_>>();
        let mut new_row_names =
            vec![Some(expression::types::EmptyName.clone()); table_info.Columns.len()];
        for (select_offset, affected) in affected_columns.iter().enumerate() {
            let mut projected = select_plan.schema().Columns[select_offset].Clone();
            projected.RetType = Some(affected.FieldType.clone());
            let target_offset = affected.Offset as usize;
            new_row_columns[target_offset] = projected;
            new_row_names[target_offset] = select_names
                .0
                .get(select_offset)
                .cloned()
                .unwrap_or_else(|| Some(expression::types::EmptyName.clone()));
        }
        let mut on_duplicate_schema = table_schema.Clone();
        on_duplicate_schema
            .Columns
            .extend(select_plan.schema().Columns[row_len..].iter().cloned());
        on_duplicate_schema.Columns.extend(new_row_columns);
        let mut on_duplicate_names = table_names.Shallow();
        on_duplicate_names
            .0
            .extend(select_names.0[row_len..].iter().cloned());
        on_duplicate_names.0.extend(new_row_names);

        let mut insert = physicalop::Insert::New(self.ctx.clone());
        insert.Table = Some(target_table);
        insert.TableSchema = Some(table_schema.Clone());
        insert.TableColNames = table_names.Shallow();
        insert.Columns = statement.Columns.iter().cloned().map(Box::new).collect();
        insert.SelectPlan = Some(select_plan);
        insert.IsReplace = statement.IsReplace;
        insert.IgnoreErr = statement.IgnoreErr;
        insert.RowLen = row_len as isize;
        insert.Schema4OnDuplicate = Some(on_duplicate_schema);
        insert.Names4OnDuplicate = on_duplicate_names;
        insert
            .SimpleSchemaProducer
            .SetOutputNames(expression::types::NameSlice(Vec::new()));
        let mut updated_columns = HashSet::new();
        for assignment in &statement.OnDuplicate {
            let target_offset = table_info
                .Columns
                .iter()
                .position(|column| column.Name.L == assignment.Column.Name.L && !column.Hidden)
                .ok_or_else(|| {
                    expression::errors::New(format!(
                        "Unknown column '{}' in 'field list'",
                        assignment.Column.Name.O
                    ))
                })?;
            if table_info.Columns[target_offset].IsGenerated() {
                let is_default_for_target = match assignment.Expr.DefaultName() {
                    None => assignment.Expr.IsDefaultExpr(),
                    Some(default_name) => {
                        default_name.Name.L == assignment.Column.Name.L
                            && (default_name.Table.L.is_empty()
                                || default_name.Table.L == assignment.Column.Table.L)
                    }
                };
                if is_default_for_target {
                    continue;
                }
                return Err(expression::errors::New(format!(
                    "The value specified for generated column '{}' in table '{}' is not allowed",
                    assignment.Column.Name.O, table_info.Name.O
                )));
            }
            updated_columns.insert(table_info.Columns[target_offset].Name.L.clone());
            let mut rewrite_expr = assignment.Expr.clone();
            if matches!(rewrite_expr.Kind, ast::ExprKind::DefaultValue) {
                rewrite_expr.Kind = ast::ExprKind::NamedDefault(assignment.Column.clone());
            }
            let mut mock = logicalop::LogicalTableDual::default()
                .Init(self.ctx.clone(), self.getSelectOffset());
            mock.LogicalSchemaProducer.SetSchema(
                insert
                    .Schema4OnDuplicate
                    .as_ref()
                    .expect("INSERT initializes ON DUPLICATE schema")
                    .Clone(),
            );
            mock.LogicalSchemaProducer
                .SetOutputNames(insert.Names4OnDuplicate.Shallow());
            let rewritten = crate::expression_rewriter::rewriteInsertOnDuplicateUpdate(
                self,
                crate::context::TODOArc(),
                &rewrite_expr,
                Box::new(mock),
                &insert,
            )?;
            insert.OnDuplicate.push(Box::new(expression::Assignment {
                Col: insert
                    .TableSchema
                    .as_ref()
                    .expect("INSERT initializes target schema")
                    .Columns[target_offset]
                    .Clone(),
                ColName: assignment.Column.Name.clone(),
                Expr: rewritten,
                LazyErr: None,
            }));
        }
        for column in table_info.Columns.iter().filter(|column| {
            !column.IsGenerated() && expression::mysql::HasOnUpdateNowFlag(column.GetFlag())
        }) {
            updated_columns.insert(column.Name.L.clone());
        }
        for column in table_info
            .Columns
            .iter()
            .filter(|column| column.IsGenerated())
        {
            let parsed = expression::generatedexpr::ParseExpression(&column.GeneratedExprString)
                .map_err(|error| expression::errors::New(error.to_string()))?;
            let resolved =
                expression::generatedexpr::SimpleResolveName(parsed, table_info.as_ref())
                    .map_err(|error| expression::errors::New(error.to_string()))?;
            let generated = expression::BuildSimpleExpr(
                self.ctx.GetExprCtx(),
                &resolved,
                vec![
                    expression::WithInputSchemaAndNames(
                        insert
                            .TableSchema
                            .as_ref()
                            .expect("INSERT initializes target schema"),
                        insert.TableColNames.Shallow(),
                        Some(table_info.as_ref()),
                    ),
                    expression::WithAllowCastArray(true),
                ],
            )?;
            insert.GenCols.Exprs.push(generated.CloneExpr());
            if column
                .Dependences
                .keys()
                .any(|dependency| updated_columns.contains(dependency))
            {
                insert
                    .GenCols
                    .OnDuplicates
                    .push(Box::new(expression::Assignment {
                        Col: insert
                            .TableSchema
                            .as_ref()
                            .expect("INSERT initializes target schema")
                            .Columns[column.Offset as usize]
                            .Clone(),
                        ColName: column.Name.clone(),
                        Expr: generated,
                        LazyErr: None,
                    }));
                updated_columns.insert(column.Name.L.clone());
            }
        }
        insert.ResolveIndices()?;
        Ok(Box::new(insert))
    }

    /// 弹出ulateDataSource（对应同名 Go 逻辑）。
    pub(crate) fn populateDataSource(
        &self,
        ctx: &dyn context::Context,
        table_name: &ast::TableName,
        source: &mut logicalop::DataSource,
    ) -> Result<(), expression::Error> {
        if let Some(provider) = &self.dataSourceProvider {
            provider.Populate(ctx, &self.ctx, self.is.as_ref(), table_name, source)?;
        }
        Ok(())
    }

    /// 重置ForReuse（对应同名 Go 逻辑）。
    pub fn ResetForReuse(mut self) -> PendingPlanBuilder {
        self.handleHelper.resetForReuse();
        PendingPlanBuilder {
            disable_subquery_preprocessing: self.disableSubQueryPreprocessing,
            allow_build_cast_array: self.allowBuildCastArray,
            result_set_builder: self.resultSetBuilder,
            data_source_provider: self.dataSourceProvider.clone(),
        }
    }
}
