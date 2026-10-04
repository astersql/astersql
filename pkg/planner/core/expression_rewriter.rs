// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 表达式重写器：将 AST 表达式改写为可执行的 expression 树。
//
// 在 PlanBuilder 上下文中遍历 AST（Enter/Leave），处理子查询（EXISTS/IN/比较/标量）、
// 聚合与窗口映射、列解析、DEFAULT、用户/系统变量、MATCH AGAINST（全文检索）等；
// 必要时插入 Apply/SemiJoin。clauseCode 标识表达式所处 SQL 子句以便报错定位。

// 规划器内存中的结构与控制流。
// Go 的接口、指针、nil、类型断言、切片/映射、多返回值和外部依赖按原顺序保留，待后续 Rust 模块接线。

// import (
// 	"context"
// 	"fmt"
// 	"slices"
// 	"strconv"
// 	"strings"
// 	"github.com/pingcap/errors"
// 	"github.com/pingcap/tidb/pkg/expression"
// 	"github.com/pingcap/tidb/pkg/expression/aggregation"
// 	"github.com/pingcap/tidb/pkg/expression/exprctx"
// 	"github.com/pingcap/tidb/pkg/expression/expropt"
// 	"github.com/pingcap/tidb/pkg/infoschema"
// 	"github.com/pingcap/tidb/pkg/meta/model"
// 	"github.com/pingcap/tidb/pkg/parser/ast"
// 	"github.com/pingcap/tidb/pkg/parser/charset"
// 	"github.com/pingcap/tidb/pkg/parser/mysql"
// 	"github.com/pingcap/tidb/pkg/parser/opcode"
// 	"github.com/pingcap/tidb/pkg/planner/core/base"
// 	"github.com/pingcap/tidb/pkg/planner/core/operator/logicalop"
// 	"github.com/pingcap/tidb/pkg/planner/core/operator/physicalop"
// 	"github.com/pingcap/tidb/pkg/planner/core/rule"
// 	"github.com/pingcap/tidb/pkg/planner/util/coreusage"
// 	"github.com/pingcap/tidb/pkg/sessionctx/vardef"
// 	"github.com/pingcap/tidb/pkg/sessionctx/variable"
// 	"github.com/pingcap/tidb/pkg/table"
// 	"github.com/pingcap/tidb/pkg/types"
// 	driver "github.com/pingcap/tidb/pkg/types/parser_driver"
// 	"github.com/pingcap/tidb/pkg/util/chunk"
// 	"github.com/pingcap/tidb/pkg/util/collate"
// 	"github.com/pingcap/tidb/pkg/util/dbterror/plannererrors"
// 	"github.com/pingcap/tidb/pkg/util/hint"
// 	"github.com/pingcap/tidb/pkg/util/intest"
// 	sem "github.com/pingcap/tidb/pkg/util/sem/compat"
// 	"github.com/pingcap/tidb/pkg/util/stringutil"
// )

use crate::{
    DoOptimize, NewPlanBuilder, PlanBuilder, appendDynamicVisitInfo, ast, context,
    resetCTECheckForSubQuery,
};
use aggregation_dependency as aggregation;
use base_dependency as base;
use coreusage_dependency as coreusage;
use expression::{charset, chunk, collate, errors, exprctx, model, mysql, opcode, types};
use expression_dependency as expression;
use expression_dependency::{CollationInfo as _, Expression as _};
use expression_expropt_dependency as expropt;
use hint_dependency as hint;
use infoschema_dependency as infoschema;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use physicalop_dependency as physicalop;
use plannererrors_dependency as plannererrors;
use rule_dependency as rule;
use sem_dependency as sem;
use stringutil_dependency::string_util as stringutil;
use table_dependency as table;
use vardef_dependency as vardef;
use variable_dependency as variable;

use crate::scalar_subq_expression::{
    ScalarSubQueryExpr, ScalarSubqueryEvalCtx, eval_subquery_first_row,
};

fn explain_non_eval_scalar_subquery(vars: &variable::session::SessionVars) -> bool {
    vars.ExplainNonEvaledSubQuery
        || vars
            .GetSystemVar(vardef::TiDBOptExplainNoEvaledSubQuery)
            .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "on" | "1" | "true"))
}

/// FieldType 深拷贝，避免与 AST 共享可变类型状态。
trait FieldTypeDeepCopy {
    type Output;
    fn DeepCopy(&self) -> Self::Output;
}

impl FieldTypeDeepCopy for types::FieldType {
    type Output = types::FieldType;

    fn DeepCopy(&self) -> Self::Output {
        types::FieldType::DeepCopy(Some(self)).expect("non-null field type")
    }
}

impl FieldTypeDeepCopy for Option<types::FieldType> {
    type Output = Option<types::FieldType>;

    fn DeepCopy(&self) -> Self::Output {
        self.as_ref().map(FieldTypeDeepCopy::DeepCopy)
    }
}

/// 模拟 Go 可选值的 Clone 行为。
trait GoOptionClone<T> {
    fn Clone(&self) -> Option<T>;
}

impl<T: Clone> GoOptionClone<T> for Option<T> {
    fn Clone(&self) -> Option<T> {
        self.clone()
    }
}

/// 将表达式列表用 AND 合成 CNF。
fn composeCNF(
    ctx: &dyn expression::BuildContext,
    conditions: Vec<expression::ExprBox>,
) -> expression::ExprBox {
    expression::ComposeCNFCondition(ctx, &conditions)
        .expect("planner only composes a non-empty CNF")
}

/// 将表达式列表用 OR 合成 DNF。
fn composeDNF(
    ctx: &dyn expression::BuildContext,
    conditions: Vec<expression::ExprBox>,
) -> expression::ExprBox {
    expression::ComposeDNFCondition(ctx, &conditions)
        .expect("planner only composes a non-empty DNF")
}

/// SQL 子句编码：标识当前表达式所处子句，用于错误信息。
pub type clauseCode = usize;
/// 未知/未指定子句。
pub const unknowClause: clauseCode = 0;
/// SELECT 字段列表子句。
pub const fieldList: clauseCode = 1;
/// HAVING 子句。
pub const havingClause: clauseCode = 2;
/// JOIN ON 条件子句。
pub const onClause: clauseCode = 3;
/// ORDER BY 子句。
pub const orderByClause: clauseCode = 4;
/// WHERE 子句。
pub const whereClause: clauseCode = 5;
/// GROUP BY 子句。
pub const groupByClause: clauseCode = 6;
/// SHOW 语句上下文。
pub const showStatement: clauseCode = 7;
/// 全局 ORDER BY（如 UNION 外层排序）。
pub const globalOrderByClause: clauseCode = 8;
/// 一般表达式上下文。
pub const expressionClause: clauseCode = 9;
/// 窗口函数 ORDER BY。
pub const windowOrderByClause: clauseCode = 10;
/// 窗口函数 PARTITION BY。
pub const partitionByClause: clauseCode = 11;

/// 子句编码到英文提示文案的映射表。
pub static clauseMsg: [&str; 12] = [
    "",
    "field list",
    "having clause",
    "on clause",
    "order clause",
    "where clause",
    "group statement",
    "show statement",
    "global ORDER clause",
    "expression",
    "window order by",
    "window partition by",
];

/// 当前正在处理的子查询种类位标志。
pub type subQueryCtx = u64;
/// 未处于子查询处理中。
pub const notHandlingSubquery: subQueryCtx = 0;
/// 正在处理 EXISTS 子查询。
pub const handlingExistsSubquery: subQueryCtx = 1;
/// 正在处理比较子查询（=ANY/>ALL 等）。
pub const handlingCompareSubquery: subQueryCtx = 2;
/// 正在处理 IN 子查询。
pub const handlingInSubquery: subQueryCtx = 3;
/// 正在处理标量子查询。
pub const handlingScalarSubquery: subQueryCtx = 4;

/// 追加单个名称到诊断字符串。
fn append_name(
    mut names: types::NameSlice,
    name: std::sync::Arc<types::FieldName>,
) -> types::NameSlice {
    names.0.push(Some(name));
    names
}

/// 追加多个名称到诊断字符串。
fn append_names(
    mut names: types::NameSlice,
    appended: Vec<std::sync::Arc<types::FieldName>>,
) -> types::NameSlice {
    names.0.extend(appended.into_iter().map(Some));
    names
}

/// 求值子查询首行（用于常量折叠等）。
fn callEvalSubqueryFirstRow(
    ctx: &dyn context::Context,
    plan: &dyn base::PhysicalPlan,
    info_schema: &dyn infoschema::InfoSchema,
    plan_context: &dyn base::PlanContext,
) -> Result<Option<Vec<types::Datum>>, errors::Error> {
    eval_subquery_first_row(ctx, plan, info_schema, plan_context)
}

// evalAstExprWithPlanCtx evaluates ast expression with plan context.
// Different with expression.EvalSimpleAst, it uses planner context and is more powerful to build some special expressions
// like subquery, window function, etc.
/// 在规划器上下文中求值 AST 表达式（可含子查询/窗口等）。
pub fn evalAstExprWithPlanCtx(
    sctx: &base::ContextRef,
    info_schema: std::sync::Arc<dyn infoschema::InfoSchema>,
    expr: &ast::ExprNode,
) -> Result<types::Datum, errors::Error> {
    if let ast::ExprKind::Value(value) = &expr.Kind {
        return datumFromAstValue(&value.Datum);
    }
    let new_expr = rewriteAstExprWithPlanCtx(sctx, info_schema, expr, None, None, false)?;
    new_expr.Eval(sctx.GetExprCtx().GetEvalCtx(), chunk::Row::default())
}

// evalAstExpr evaluates ast expression directly.
/// 直接求值简单 AST 表达式（无完整规划器能力）。
pub fn evalAstExpr(
    ctx: &dyn expression::BuildContext,
    expr: &ast::ExprNode,
) -> Result<types::Datum, errors::Error> {
    if let ast::ExprKind::Value(value) = &expr.Kind {
        return datumFromAstValue(&value.Datum);
    }
    let new_expr = buildSimpleExpr(ctx, Some(expr), Vec::new())?;
    new_expr.Eval(ctx.GetEvalCtx(), chunk::Row::default())
}

// rewriteAstExprWithPlanCtx rewrites ast expression directly.
// Different with expression.BuildSimpleExpr, it uses planner context and is more powerful to build some special expressions
// like subquery, window function, etc.
/// 在规划器上下文中将 AST 改写为 expression。
pub fn rewriteAstExprWithPlanCtx(
    sctx: &base::ContextRef,
    info_schema: std::sync::Arc<dyn infoschema::InfoSchema>,
    expr: &ast::ExprNode,
    schema: Option<&expression::Schema>,
    names: Option<&types::NameSlice>,
    allow_cast_array: bool,
) -> Result<expression::ExprBox, errors::Error> {
    let (mut builder, saved_block_names) =
        NewPlanBuilder().Init(sctx.clone(), info_schema, hint::NewQBHintHandler(None));
    builder.allowBuildCastArray = allow_cast_array;

    let mut fake_plan = logicalop::LogicalTableDual::default().Init(sctx.clone(), 0);
    if let Some(schema) = schema {
        fake_plan.SetSchema(schema.Clone());
        fake_plan.SetOutputNames(
            names
                .map(types::NameSlice::Shallow)
                .unwrap_or_else(|| types::NameSlice(Vec::new())),
        );
    }
    builder.curClause = expressionClause;
    let rewritten = rewrite(
        &mut builder,
        context::TODOArc(),
        expr,
        Box::new(fake_plan),
        AggregateMapper::default(),
        true,
    );
    sctx.GetSessionVars()
        .PlannerSelectBlockAsName
        .Store(Some(saved_block_names));
    let (new_expr, _) = rewritten?;
    let new_expr =
        new_expr.ok_or_else(|| errors::New("scalar expression rewrite returned no expression"))?;
    Ok(new_expr)
}

/// 按 BuildOption 构建简单表达式（可带输入 schema/表源）。
pub fn buildSimpleExpr<'a>(
    ctx: &dyn expression::BuildContext,
    node: Option<&ast::ExprNode>,
    opts: Vec<expression::BuildOption<'a>>,
) -> Result<expression::ExprBox, errors::Error> {
    let node = node.ok_or_else(|| errors::New("expression node should be present"))?;
    let mut options = expression::BuildOptions {
        UseNewCollate: ctx.NewCollationEnabled(),
        ..expression::BuildOptions::default()
    };
    for option in opts {
        option(&mut options);
    }

    if options.InputSchema.is_none() && !options.InputNames.0.is_empty() {
        return Err(errors::New(
            "InputSchema and InputNames should be specified at the same time",
        ));
    }
    if let Some(input_schema) = options.InputSchema {
        if input_schema.Len() != options.InputNames.0.len() {
            return Err(errors::New(
                "InputSchema and InputNames should be the same length",
            ));
        }
    }

    // Go guards the single-database invariant with `intest.AssertFunc`, so it
    // is intentionally inactive in normal builds. Runtime plans can contain
    // mixed names after joins/applies and derived columns with an empty DB.

    let mut rewriter = expressionRewriter {
        ctxStack: Vec::new(),
        ctxNameStk: Vec::new(),
        ctx: context::TODOArc(),
        sctx: ctx,
        schema: options.InputSchema.map(expression::Schema::Clone),
        names: options.InputNames,
        err: None,
        sourceTable: options.SourceTable,
        allowBuildCastArray: options.AllowCastArray,
        asScalar: true,
        preprocess: None,
        disableFoldCounter: 0,
        tryFoldCounter: 0,
        nextParamOrder: 0,
        astNodeStack: Vec::new(),
        planCtx: None,
        useNewCollate: options.UseNewCollate,
    };

    if let Some(table) = options.SourceTable.filter(|_| rewriter.schema.is_none()) {
        let public_columns = table
            .Cols()
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let (columns, names) = expression::ColumnInfos2ColumnsAndNames(
            ctx,
            options.SourceTableDB,
            table.Name.clone(),
            &public_columns,
            table,
        )?;
        assert_eq!(columns.len(), names.0.len());
        rewriter.schema = Some(expression::NewSchema(columns));
        rewriter.names = names;
    }
    if rewriter.schema.is_none() {
        rewriter.schema = Some(expression::NewSchema(Vec::new()));
    }

    let mut expression = rewriteSimpleExprNode(&mut rewriter, node)?;
    if let Some(field_type) = options.TargetFieldType {
        expression = expression::BuildCastFunction(ctx, &expression, field_type);
    }
    Ok(expression)
}

/// 改写 INSERT ... ON DUPLICATE KEY UPDATE 中的赋值表达式。
pub fn rewriteInsertOnDuplicateUpdate(
    builder: &mut PlanBuilder,
    ctx: std::sync::Arc<dyn context::Context>,
    expr_node: &ast::ExprNode,
    mock_plan: logicalop::LogicalPlanRef,
    insert_plan: &physicalop::Insert,
) -> Result<expression::ExprBox, errors::Error> {
    builder.curClause = fieldList;
    let allow_build_cast_array = builder.allowBuildCastArray;
    let mut rewriter = getExpressionRewriter(builder, ctx, mock_plan);
    if let Some(error) = rewriter.err.take() {
        return Err(error);
    }
    let plan_context = rewriter
        .planCtx
        .as_mut()
        .expect("planner rewrite always installs plan context");
    plan_context.insertPlan = Some(insert_plan);
    rewriter.asScalar = true;
    rewriter.allowBuildCastArray = allow_build_cast_array;
    rewriteExprNode(&mut rewriter, expr_node, true).and_then(|result| {
        result
            .0
            .ok_or_else(|| errors::New("scalar expression rewrite returned no expression"))
    })
}

// rewrite function rewrites ast expr to expression.Expression.
// aggMapper maps ast.AggregateFuncExpr to the columns offset in p's output schema.
// asScalar means whether this expression must be treated as a scalar expression.
// And this function returns a result expression, a new plan that may have apply or semi-join.
/// 将 AST 表达式改写为 expression，并可能扩展为含 Apply 的计划。
pub fn rewrite(
    builder: &mut PlanBuilder,
    ctx: std::sync::Arc<dyn context::Context>,
    expr_node: &ast::ExprNode,
    plan: logicalop::LogicalPlanRef,
    aggregate_mapper: AggregateMapper,
    as_scalar: bool,
) -> Result<(Option<expression::ExprBox>, logicalop::LogicalPlanRef), errors::Error> {
    rewriteWithPreprocess(
        builder,
        ctx,
        expr_node,
        plan,
        aggregate_mapper,
        None,
        as_scalar,
        None,
    )
}

// rewriteWithPreprocess is for handling the situation that we need to adjust the input ast tree
// before really using its node in `expressionRewriter.Leave`. In that case, we first call
// er.preprocess(expr), which returns a new expr. Then we use the new expr in `Leave`.
// rewriteWithPreprocess 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 带预处理回调的改写：Leave 前可调整 AST 节点。
pub fn rewriteWithPreprocess(
    builder: &mut PlanBuilder,
    ctx: std::sync::Arc<dyn context::Context>,
    expr_node: &ast::ExprNode,
    plan: logicalop::LogicalPlanRef,
    aggregate_mapper: AggregateMapper,
    window_mapper: Option<WindowMapper>,
    as_scalar: bool,
    preprocess: Option<PreprocessFn>,
) -> Result<(Option<expression::ExprBox>, logicalop::LogicalPlanRef), errors::Error> {
    let allow_build_cast_array = builder.allowBuildCastArray;
    let mut rewriter = getExpressionRewriter(builder, ctx, plan);
    if let Some(error) = rewriter.err.take() {
        return Err(error);
    }
    let plan_context = rewriter
        .planCtx
        .as_mut()
        .expect("planner rewrite always installs plan context");
    plan_context.aggrMap = aggregate_mapper;
    plan_context.windowMap = window_mapper.unwrap_or_default();
    rewriter.asScalar = as_scalar;
    rewriter.allowBuildCastArray = allow_build_cast_array;
    rewriter.preprocess = preprocess;
    rewriteExprNode(&mut rewriter, expr_node, as_scalar)
}

/// 构造绑定当前计划 schema/names 的 expressionRewriter。
pub fn getExpressionRewriter<'a>(
    builder: &'a mut PlanBuilder,
    ctx: std::sync::Arc<dyn context::Context>,
    plan: logicalop::LogicalPlanRef,
) -> expressionRewriter<'a> {
    let mut rewriter = expressionRewriter::new(ctx, plan, builder);
    if let Some(plan_context) = rewriter.planCtx.as_ref() {
        if let Some(join) = plan_context
            .plan
            .as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
        {
            if let Some(schema) = &join.FullSchema {
                rewriter.schema = Some(schema.Clone());
                rewriter.names = join.FullNames.Shallow();
                return rewriter;
            }
        }
        rewriter.schema = Some(plan_context.plan.Schema().Clone());
        rewriter.names = plan_context.plan.OutputNames().Shallow();
    }
    rewriter
}

/// 驱动 AST 访问并对栈顶结果做标量化收尾。
pub fn rewriteExprNode(
    rewriter: &mut expressionRewriter<'_>,
    expr_node: &ast::ExprNode,
    as_scalar: bool,
) -> Result<(Option<expression::ExprBox>, logicalop::LogicalPlanRef), errors::Error> {
    let plan_context = rewriter.planCtx.as_mut();
    // sourceTable is only used to build simple expression with one table
    // when planCtx is present, sourceTable should be nil.
    assert!(plan_context.is_none() || rewriter.sourceTable.is_none());
    let original_column_len = plan_context
        .as_ref()
        .map(|context| context.plan.Schema().Len());
    expr_node.Accept(rewriter);
    if let Some(error) = rewriter.err.take() {
        return Err(errors::Trace(error));
    }
    if let (Some(context), Some(column_len)) = (rewriter.planCtx.as_mut(), original_column_len) {
        let mut names = context.plan.OutputNames().Shallow();
        names.0.truncate(column_len);
        while names.0.len() < context.plan.Schema().Len() {
            names.0.push(Some(types::EmptyName.clone()));
        }
        // After rewriting finished, only old columns are visible.
        // e.g. select * from t where t.a in (select t1.a from t1);
        // The output columns before we enter the subquery are the columns from t.
        // But when we leave the subquery `t.a in (select t1.a from t1)`, we got a Apply operator
        // and the output columns become [t.*, t1.*]. But t1.* is used only inside the subquery. If there's another filter
        // which is also a subquery where t1 is involved. The name resolving will fail if we still expose the column from
        // the previous subquery.
        // So here we just reset the names to empty to avoid this situation.
        // TODO: implement ScalarSubQuery and resolve it during optimizing. In building phase, we will not change the plan's structure.
        context.plan.SetOutputNames(names);
    }

    let plan = rewriter
        .planCtx
        .as_mut()
        .ok_or_else(|| errors::New("simple expression has no logical plan"))?
        .plan
        .take();
    if !as_scalar && rewriter.ctxStack.is_empty() {
        return Ok((None, plan));
    }
    if rewriter.ctxStack.len() != 1 {
        return Err(errors::New(format!(
            "context len {} is invalid",
            rewriter.ctxStack.len()
        )));
    }
    expression::CheckArgsNotMultiColumnRow(&rewriter.ctxStack)?;
    Ok((Some(rewriter.ctxStack.remove(0)), plan))
}

/// Runs the same visitor as planner rewriting without manufacturing a logical
/// plan. Go's `buildSimpleExpr` deliberately constructs `expressionRewriter`
/// with a nil `planCtx`; requiring a plan at the return boundary changed that
/// contract and made every otherwise-valid constant/column/function fail.
fn rewriteSimpleExprNode(
    rewriter: &mut expressionRewriter<'_>,
    expr_node: &ast::ExprNode,
) -> Result<expression::ExprBox, errors::Error> {
    assert!(rewriter.planCtx.is_none());
    expr_node.Accept(rewriter);
    if let Some(error) = rewriter.err.take() {
        return Err(errors::Trace(error));
    }
    if rewriter.ctxStack.len() != 1 {
        return Err(errors::New(format!(
            "context len {} is invalid",
            rewriter.ctxStack.len()
        )));
    }
    expression::CheckArgsNotMultiColumnRow(&rewriter.ctxStack)?;
    Ok(rewriter.ctxStack.remove(0))
}

// 该结构体保持 Go 字段顺序；引用关系和所有权在后续模块接线时确定。
/// 聚合函数 AST 节点到输出列偏移的映射。
pub type AggregateMapper = std::collections::HashMap<String, isize>;

/// Rust AST visitor clones child nodes, so use a structural key to preserve
/// Go's aggregate lookup semantics across the cloned traversal nodes.
pub fn AggregateMapperKey(node: &ast::ExprNode) -> String {
    struct NormalizeLookupKey;
    impl ast::ExprNodeVisitor for NormalizeLookupKey {
        fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            let mut node = input.clone();
            node.OriginTextPosition = 0;
            node.Flag = Default::default();
            match &mut node.Kind {
                ast::ExprKind::Column(column) => {
                    column.Schema.O = column.Schema.L.clone();
                    column.Table.O = column.Table.L.clone();
                    column.Name.O = column.Name.L.clone();
                }
                ast::ExprKind::Function { FnName, .. } => FnName.O = FnName.L.clone(),
                ast::ExprKind::AggregateFunction { Name, .. }
                | ast::ExprKind::WindowFunction { Name, .. } => *Name = Name.to_ascii_lowercase(),
                _ => {}
            }
            (node, false)
        }
        fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }
    let (normalized, _) = node.Accept(&mut NormalizeLookupKey);
    // Retain the full key, not only its hash: unrelated ASTs must never
    // alias merely because a hash collides or an identifier contains dots.
    format!("{:?}", normalized.Kind)
}
/// 窗口函数 AST 节点到输出列偏移的映射。
pub type WindowMapper = std::collections::HashMap<String, isize>;
/// 改写前对 AST 节点的预处理函数类型。
pub type PreprocessFn = fn(&ast::ExprNode) -> ast::ExprNode;

/// 非空 PlanBuilder 裸指针包装（对齐 Go 指针语义）。
struct PlanBuilderPtr(std::ptr::NonNull<PlanBuilder>);

impl PlanBuilderPtr {
    fn new(builder: &mut PlanBuilder) -> Self {
        Self(std::ptr::NonNull::from(builder))
    }
}

impl std::ops::Deref for PlanBuilderPtr {
    type Target = PlanBuilder;

    fn deref(&self) -> &Self::Target {
        // The pointer is created from the caller's live `&mut PlanBuilder` and
        // never escapes the synchronous expression rewrite invocation.
        unsafe { self.0.as_ref() }
    }
}

impl std::ops::DerefMut for PlanBuilderPtr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Each rewriter is local to one call, so there is exactly one mutable
        // builder access path while this wrapper is in use.
        unsafe { self.0.as_mut() }
    }
}

/// Gives Go-style move-out-and-replace semantics to the current logical plan.
/// A plan is absent only between `take` and the immediately following replace.
struct MovableLogicalPlan(Option<logicalop::LogicalPlanRef>);

impl MovableLogicalPlan {
    fn new(plan: logicalop::LogicalPlanRef) -> Self {
        Self(Some(plan))
    }

    fn take(&mut self) -> logicalop::LogicalPlanRef {
        self.0.take().expect("logical plan was already moved")
    }

    fn replace(&mut self, plan: logicalop::LogicalPlanRef) {
        self.0 = Some(plan);
    }

    fn as_ref(&self) -> &dyn logicalop::LogicalPlan {
        self.0
            .as_deref()
            .expect("logical plan is temporarily absent")
    }
}

impl std::ops::Deref for MovableLogicalPlan {
    type Target = dyn logicalop::LogicalPlan;

    fn deref(&self) -> &Self::Target {
        self.0
            .as_deref()
            .expect("logical plan is temporarily absent")
    }
}

impl std::ops::DerefMut for MovableLogicalPlan {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .as_deref_mut()
            .expect("logical plan is temporarily absent")
    }
}

/// 改写器持有的规划侧上下文：当前计划、聚合/窗口映射等。
pub struct exprRewriterPlanCtx<'a> {
    plan: MovableLogicalPlan,
    builder: PlanBuilderPtr,

    // curClause tracks which part of the query is being processed
    curClause: clauseCode,

    aggrMap: AggregateMapper,
    windowMap: WindowMapper,

    // insertPlan is only used to rewrite the expressions inside the assignment
    // of the "INSERT" statement.
    insertPlan: Option<&'a physicalop::Insert>,

    rollExpand: Option<&'a logicalop::LogicalExpand>,
}

// 该结构体保持 Go 字段顺序；引用关系和所有权在后续模块接线时确定。
/// AST→expression 访问器：维护表达式栈、schema 与错误状态。
pub struct expressionRewriter<'a> {
    ctxStack: Vec<expression::ExprBox>,
    ctxNameStk: Vec<std::sync::Arc<types::FieldName>>,
    schema: Option<expression::Schema>,
    names: types::NameSlice,
    err: Option<errors::Error>,

    sctx: &'a dyn expression::BuildContext,
    ctx: std::sync::Arc<dyn context::Context>,

    // asScalar 为 true 时改写结果必须是标量（非子查询计划侧效果）。
    // asScalar indicates the return value must be a scalar value.
    // NOTE: This value can be changed during expression rewritten.
    asScalar: bool,
    // allowBuildCastArray indicates whether allow cast(... as ... array).
    allowBuildCastArray: bool,
    // sourceTable is only used to build simple expression without all columns from a single table
    sourceTable: Option<&'a model::TableInfo>,

    // preprocess is called for every ast.Node in Leave.
    preprocess: Option<PreprocessFn>,

    // 大于 0 时禁用常量折叠；Enter 进入作用域 +1，Leave 退出 -1。
    // disableFoldCounter controls fold-disabled scope. If > 0, rewriter will NOT do constant folding.
    // Typically, during visiting AST, while entering the scope(disable), the counter will +1; while
    // leaving the scope(enable again), the counter will -1.
    // NOTE: This value can be changed during expression rewritten.
    disableFoldCounter: isize,
    tryFoldCounter: isize,

    // Go ParamMarker 的 Offset 是 SQL token 偏移；求值按从左到右的 Order 索引参数。
    // Go's ParamMarkerExpr.Offset is a SQL token offset, while expression
    // evaluation indexes parameters by the left-to-right marker Order.
    nextParamOrder: usize,

    astNodeStack: Vec<ast::ExprNode>,

    planCtx: Option<Box<exprRewriterPlanCtx<'a>>>,
    useNewCollate: bool,
}

impl<'a> expressionRewriter<'a> {
    fn new(
        ctx: std::sync::Arc<dyn context::Context>,
        plan: logicalop::LogicalPlanRef,
        builder: &'a mut PlanBuilder,
    ) -> Self {
        let builder_ptr = PlanBuilderPtr::new(builder);
        // Obtain the expression context through the stable planner context
        // object. It outlives this synchronous rewrite call.
        let sctx = unsafe { builder_ptr.0.as_ref() }.ctx.GetExprCtx();
        let cur_clause = unsafe { builder_ptr.0.as_ref() }.curClause;
        Self {
            ctxStack: Vec::new(),
            ctxNameStk: Vec::new(),
            schema: None,
            names: types::NameSlice(Vec::new()),
            err: None,
            sctx,
            ctx,
            asScalar: false,
            allowBuildCastArray: false,
            sourceTable: None,
            preprocess: None,
            disableFoldCounter: 0,
            tryFoldCounter: 0,
            nextParamOrder: 0,
            astNodeStack: Vec::new(),
            planCtx: Some(Box::new(exprRewriterPlanCtx {
                plan: MovableLogicalPlan::new(plan),
                builder: builder_ptr,
                curClause: cur_clause,
                aggrMap: AggregateMapper::default(),
                windowMap: WindowMapper::default(),
                insertPlan: None,
                rollExpand: None,
            })),
            useNewCollate: collate::NewCollationEnabled(),
        }
    }
}

impl ast::ExprNodeVisitor for expressionRewriter<'_> {
    fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        Enter(self, input)
    }

    fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        Leave(self, input)
    }
}

// ctxStackLen 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 返回表达式求值栈当前深度。
pub fn ctxStackLen(rewriter: &expressionRewriter<'_>) -> usize {
    rewriter.ctxStack.len()
}

// ctxStackPop 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 弹出栈顶若干表达式。
pub fn ctxStackPop(rewriter: &mut expressionRewriter<'_>, count: usize) {
    let new_len = rewriter.ctxStack.len() - count;
    rewriter.ctxStack.truncate(new_len);
    rewriter.ctxNameStk.truncate(new_len);
}

// ctxStackAppend 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 向栈压入表达式及其输出列名。
pub fn ctxStackAppend(
    rewriter: &mut expressionRewriter<'_>,
    expression: expression::ExprBox,
    name: std::sync::Arc<types::FieldName>,
) {
    rewriter.ctxStack.push(expression);
    rewriter.ctxNameStk.push(name);
}

// constructBinaryOpFunction converts binary operator functions
/*
    The algorithm is as follows:
    1. If the length of the two sides of the expression is 1, return l op r directly.
    2. If the length of the two sides of the expression is not equal, return an error.
    3. If the operator is EQ, NE, or NullEQ, converts (a0,a1,a2) op (b0,b1,b2) to (a0 op b0) and (a1 op b1) and (a2 op b2)
    4. If the operator is not EQ, NE, or NullEQ,
            converts (a0,a1,a2) op (b0,b1,b2) to (a0 > b0) or (a0 = b0 and a1 > b1) or (a0 = b0 and a1 = b1 and a2 op b2)
       Especially, op is GE or LE, the prefix element will be converted to > or <.
            converts (a0,a1,a2) >= (b0,b1,b2) to (a0 > b0) or (a0 = b0 and a1 > b1) or (a0 = b0 and a1 = b1 and a2 >= b2)
       The only different between >= and > is that >= additional include the (x,y,z) = (a,b,c).
*/
/// 由左右操作数构造二元运算标量函数。
pub fn constructBinaryOpFunction(
    rewriter: &mut expressionRewriter<'_>,
    left: expression::ExprBox,
    right: expression::ExprBox,
    operator: &str,
) -> Result<expression::ExprBox, errors::Error> {
    let left_len = expression::GetRowLen(left.as_ref());
    let right_len = expression::GetRowLen(right.as_ref());
    if left_len == 1 && right_len == 1 {
        return newFunction(
            rewriter,
            operator,
            *types::NewFieldType(mysql::TypeTiny),
            vec![left, right],
        );
    }
    if right_len != left_len {
        return Err(expression::ErrOperandColumns.GenWithStackByArgs(left_len));
    }

    if matches!(operator, ast::EQ | ast::NE | ast::NullEQ) {
        let mut functions = Vec::with_capacity(left_len);
        for index in 0..left_len {
            functions.push(constructBinaryOpFunction(
                rewriter,
                expression::GetFuncArg(left.as_ref(), index),
                expression::GetFuncArg(right.as_ref(), index),
                operator,
            )?);
        }
        return Ok(if operator == ast::NE {
            composeDNF(rewriter.sctx, functions)
        } else {
            composeCNF(rewriter.sctx, functions)
        });
    }

    // Lexicographic row comparison: every disjunct contains equality for the
    // preceding fields followed by the current comparison. GE/LE use GT/LT on
    // every non-final field, exactly as the Go implementation.
    let mut disjunction = Vec::with_capacity(left_len);
    for index in 0..left_len {
        let mut conjunction = Vec::with_capacity(index + 1);
        for prefix in 0..index {
            conjunction.push(constructBinaryOpFunction(
                rewriter,
                expression::GetFuncArg(left.as_ref(), prefix),
                expression::GetFuncArg(right.as_ref(), prefix),
                ast::EQ,
            )?);
        }
        let current_operator = if index < left_len - 1 {
            match operator {
                ast::GE => ast::GT,
                ast::LE => ast::LT,
                _ => operator,
            }
        } else {
            operator
        };
        conjunction.push(constructBinaryOpFunction(
            rewriter,
            expression::GetFuncArg(left.as_ref(), index),
            expression::GetFuncArg(right.as_ref(), index),
            current_operator,
        )?);
        disjunction.push(composeCNF(rewriter.sctx, conjunction));
    }
    Ok(composeDNF(rewriter.sctx, disjunction))
}

// buildSubquery translates the subquery ast to plan.
// Subquery related hints are returned through hintFlags. Please see comments around HintFlagSemiJoinRewrite and PlanBuilder.subQueryHintFlags for details.
// buildSubquery 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 构建并优化子查询逻辑计划。
pub fn buildSubquery(
    rewriter: &mut expressionRewriter<'_>,
    ctx: &dyn context::Context,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    query: &ast::NodeRef,
    subquery_context: subQueryCtx,
) -> Result<(logicalop::LogicalPlanRef, u64), errors::Error> {
    let builder = &mut plan_context.builder;
    let pushed_outer_scope = if let Some(schema) = &rewriter.schema {
        builder.outerSchemas.push(schema.Clone());
        builder.outerNames.push(rewriter.names.Shallow());
        let current_block_expand = builder.currentBlockExpand.take();
        builder.outerBlockExpand.push(current_block_expand);
        true
    } else {
        false
    };

    let old_subquery_context = std::mem::replace(&mut builder.subQueryCtx, subquery_context);
    let old_hint_flags = std::mem::take(&mut builder.subQueryHintFlags);
    let outer_window_specs = std::mem::take(&mut builder.windowSpecs);
    let result = (|| {
        let plan = builder.buildResultSetNode(ctx, query, false)?;
        let hint_flags = builder.subQueryHintFlags;
        builder.handleHelper.popMap();
        Ok((plan, hint_flags))
    })();
    builder.windowSpecs = outer_window_specs;
    builder.subQueryCtx = old_subquery_context;
    builder.subQueryHintFlags = old_hint_flags;
    if pushed_outer_scope {
        builder.outerSchemas.pop();
        builder.outerNames.pop();
        builder.currentBlockExpand = builder.outerBlockExpand.pop().flatten();
    }
    result
}

// requirePlanCtx 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 返回 AST 节点对应的 Go 类型名（诊断用）。
fn astExprGoType(input_node: &ast::ExprNode) -> &'static str {
    match &input_node.Kind {
        ast::ExprKind::AggregateFunction { .. } => "*ast.AggregateFuncExpr",
        ast::ExprKind::Column(_) => "*ast.ColumnNameExpr",
        ast::ExprKind::CompareSubquery { .. } => "*ast.CompareSubqueryExpr",
        ast::ExprKind::ExistsSubquery { .. } => "*ast.ExistsSubqueryExpr",
        ast::ExprKind::InSubquery { .. } | ast::ExprKind::InList { .. } => "*ast.PatternInExpr",
        ast::ExprKind::Subquery { .. } => "*ast.SubqueryExpr",
        ast::ExprKind::Variable { .. } => "*ast.VariableExpr",
        ast::ExprKind::WindowFunction { .. } => "*ast.WindowFuncExpr",
        _ => "ast.Node",
    }
}

/// 缺少 PlanCtx 时构造错误。
fn missingPlanCtxError(input_node: &ast::ExprNode, detail: &str) -> errors::Error {
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(", {detail}")
    };
    errors::New(format!(
        "planCtx is required when rewriting node: '{}'{}",
        astExprGoType(input_node),
        suffix
    ))
}

/// 要求存在 planCtx，否则返回规划器错误。
pub fn requirePlanCtx<'r, 'ctx>(
    rewriter: &'r mut expressionRewriter<'ctx>,
    input_node: &ast::ExprNode,
    detail: &str,
) -> Result<&'r mut exprRewriterPlanCtx<'ctx>, errors::Error> {
    rewriter
        .planCtx
        .as_deref_mut()
        .ok_or_else(|| missingPlanCtxError(input_node, detail))
}

/// AST 访问 Enter：处理子查询展开、禁用常量折叠等进入逻辑。
pub fn Enter(
    rewriter: &mut expressionRewriter<'_>,
    input_node: &ast::ExprNode,
) -> (ast::ExprNode, bool) {
    rewriter.astNodeStack.push(input_node.clone());
    let mut output_node = input_node.clone();
    let mut skip_children = false;
    match &input_node.Kind {
        ast::ExprKind::AggregateFunction { .. } => {
            // ExprNode::Accept walks cloned children, so pointer identity is
            // not stable below the root. Use the structural key installed by
            // the aggregation builder, matching equivalent cloned AST nodes.
            let key = AggregateMapperKey(input_node);
            match rewriter
                .planCtx
                .as_mut()
                .and_then(|context| context.aggrMap.get(&key).copied())
            {
                Some(index) if index < 0 => {
                    ctxStackAppend(
                        rewriter,
                        Box::new(expression::NewNull()),
                        types::EmptyName.clone(),
                    );
                }
                Some(index) => {
                    let column = rewriter.schema.as_ref().unwrap().Columns[index as usize].Clone();
                    let name = rewriter.names.0[index as usize].clone().unwrap();
                    ctxStackAppend(rewriter, Box::new(column), name);
                }
                None => {
                    let correlated = rewriter
                        .planCtx
                        .as_mut()
                        .and_then(|context| context.builder.correlatedAggMapper.get(&key).cloned());
                    if let Some(column) = correlated {
                        ctxStackAppend(rewriter, column, types::EmptyName.clone());
                    } else {
                        rewriter.err = Some(
                            plannererrors::ErrInvalidGroupFuncUse
                                .GenWithStackByArgs(&[])
                                .into(),
                        );
                    }
                }
            }
            skip_children = true;
        }
        ast::ExprKind::Column(_) => {
            let key = input_node as *const ast::ExprNode as usize;
            let mapped = rewriter
                .planCtx
                .as_mut()
                .and_then(|context| context.builder.colMapper.get(&key).copied());
            if let Some(index) = mapped {
                let column = rewriter.schema.as_ref().unwrap().Columns[index].Clone();
                let name = rewriter.names.0[index].clone().unwrap();
                ctxStackAppend(rewriter, Box::new(column), name);
                skip_children = true;
            }
        }
        ast::ExprKind::CompareSubquery { .. } => {
            if let Some(mut context) = rewriter.planCtx.take() {
                let rewrite_ctx = rewriter.ctx.clone();
                skip_children =
                    handleCompareSubquery(rewriter, rewrite_ctx.as_ref(), &mut context, input_node);
                rewriter.planCtx = Some(context);
            } else {
                rewriter.err = Some(missingPlanCtxError(input_node, ""));
                skip_children = true;
            }
        }
        ast::ExprKind::ExistsSubquery { .. } => {
            if let Some(mut context) = rewriter.planCtx.take() {
                let rewrite_ctx = rewriter.ctx.clone();
                skip_children =
                    handleExistSubquery(rewriter, rewrite_ctx.as_ref(), &mut context, input_node);
                rewriter.planCtx = Some(context);
            } else {
                rewriter.err = Some(missingPlanCtxError(input_node, ""));
                skip_children = true;
            }
        }
        ast::ExprKind::InSubquery { .. } => {
            if let Some(mut context) = rewriter.planCtx.take() {
                let rewrite_ctx = rewriter.ctx.clone();
                skip_children =
                    handleInSubquery(rewriter, rewrite_ctx.as_ref(), &mut context, input_node);
                rewriter.planCtx = Some(context);
            } else {
                rewriter.err = Some(missingPlanCtxError(input_node, ""));
                skip_children = true;
            }
        }
        ast::ExprKind::InList {
            Expr, List, Not, ..
        } => {
            if List.len() == 1 {
                let mut nested = &List[0];
                while let ast::ExprKind::Parentheses(inner) = &nested.Kind {
                    nested = inner;
                }
                if matches!(nested.Kind, ast::ExprKind::Subquery { .. }) {
                    let mut normalized = input_node.clone();
                    normalized.Kind = ast::ExprKind::InSubquery {
                        Expr: Expr.clone(),
                        Sel: Box::new(nested.clone()),
                        Not: *Not,
                    };
                    output_node = normalized.clone();
                    if let Some(mut context) = rewriter.planCtx.take() {
                        let rewrite_ctx = rewriter.ctx.clone();
                        let _ = handleInSubquery(
                            rewriter,
                            rewrite_ctx.as_ref(),
                            &mut context,
                            &normalized,
                        );
                        rewriter.planCtx = Some(context);
                    } else {
                        rewriter.err = Some(missingPlanCtxError(input_node, ""));
                    }
                    skip_children = true;
                } else {
                    rewriter.asScalar = true;
                }
            } else {
                rewriter.asScalar = true;
            }
        }
        ast::ExprKind::Subquery { .. } => {
            if let Some(mut context) = rewriter.planCtx.take() {
                let rewrite_ctx = rewriter.ctx.clone();
                skip_children =
                    handleScalarSubquery(rewriter, rewrite_ctx.as_ref(), &mut context, input_node);
                rewriter.planCtx = Some(context);
            } else {
                rewriter.err = Some(missingPlanCtxError(input_node, ""));
                skip_children = true;
            }
        }
        ast::ExprKind::WindowFunction { Name, .. } => {
            let key = AggregateMapperKey(input_node);
            let index = rewriter
                .planCtx
                .as_ref()
                .and_then(|context| context.windowMap.get(&key).copied());
            if let Some(index) = index {
                let column = rewriter.schema.as_ref().unwrap().Columns[index as usize].Clone();
                let name = rewriter.names.0[index as usize].clone().unwrap();
                ctxStackAppend(rewriter, Box::new(column), name);
            } else {
                rewriter.err = Some(
                    plannererrors::ErrWindowInvalidWindowFuncUse
                        .GenWithStackByArgs(&[Name.to_lowercase().into()])
                        .into(),
                );
            }
            skip_children = true;
        }
        ast::ExprKind::Function { FnName, .. } => {
            rewriter.asScalar = true;
            if expression::DisableFoldFunctions.contains_key(FnName.L.as_str()) {
                rewriter.disableFoldCounter += 1;
            }
            if expression::TryFoldFunctions.contains_key(FnName.L.as_str()) {
                rewriter.tryFoldCounter += 1;
            }
        }
        ast::ExprKind::Case { .. } => {
            rewriter.asScalar = true;
            if expression::DisableFoldFunctions.contains_key("case") {
                rewriter.disableFoldCounter += 1;
            }
            if expression::TryFoldFunctions.contains_key("case") {
                rewriter.tryFoldCounter += 1;
            }
        }
        ast::ExprKind::Binary { Op, .. } => {
            rewriter.asScalar = true;
            if Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or") {
                rewriter.tryFoldCounter += 1;
            }
        }
        ast::ExprKind::Parentheses(_) | ast::ExprKind::Collate { .. } => {}
        _ => rewriter.asScalar = true,
    }
    (output_node, skip_children)
}

// canTreatInSubqueryAsExistsForFilter reports whether the IN subquery is in a WHERE/HAVING boolean chain
// composed only of AND/OR and parentheses, so it can be treated like EXISTS for filter context.
// canTreatInSubqueryAsExistsForFilter 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 过滤语境下可否将 IN 子查询当作 EXISTS 处理。
pub fn canTreatInSubqueryAsExistsForFilter(
    rewriter: &expressionRewriter<'_>,
    plan_context: Option<&exprRewriterPlanCtx<'_>>,
) -> bool {
    let Some(plan_context) = plan_context else {
        return false;
    };
    if !matches!(plan_context.curClause, whereClause | havingClause) {
        return false;
    }
    rewriter
        .astNodeStack
        .iter()
        .rev()
        .skip(1)
        .all(|node| match &node.Kind {
            ast::ExprKind::Parentheses(_) => true,
            ast::ExprKind::Binary { Op, .. } => {
                Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or")
            }
            _ => false,
        })
}

// inDirectMatchBooleanContext reports whether the MATCH...AGAINST currently
// being rewritten sits in a position where its boolean (0/1) result is
// directly consumed as a predicate — i.e. every ancestor up to the WHERE /
// HAVING / JOIN ON root is one of: parentheses, AND, OR, or NOT.
// Any other ancestor (comparison `= 0` / `> 0.5`, `IS NULL`, CASE, arithmetic,
// XOR, scalar function, etc.) means MATCH is being used as a scalar relevance
// score, where the LIKE rewrite's 0/1 output would diverge from the native
// float score and silently produce wrong rows. In those positions the
// rewriter must fall through to the native FTSMysqlMatchAgainst builtin,
// which preserves the relevance-score semantics (and errors at execution if
// no FTS index is available — the same behavior the user would see with
// alternative logical plans disabled).
// inDirectMatchBooleanContext 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 当前是否处于可直接匹配的布尔语境。
pub fn inDirectMatchBooleanContext(rewriter: &expressionRewriter<'_>) -> bool {
    let Some(plan_context) = rewriter.planCtx.as_ref() else {
        return false;
    };
    if !matches!(
        plan_context.builder.curClause,
        whereClause | havingClause | onClause
    ) {
        return false;
    }
    rewriter
        .astNodeStack
        .iter()
        .rev()
        .skip(1)
        .all(|node| match &node.Kind {
            ast::ExprKind::Parentheses(_) => true,
            ast::ExprKind::Binary { Op, .. } => {
                Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or")
            }
            ast::ExprKind::Unary { Op, .. } => Op.eq_ignore_ascii_case("not") || Op == "!",
            _ => false,
        })
}

// matchHasLikeFallbackRescue reports whether matchAgainstToBuiltin is being
// invoked in a position where the alt-rounds driver will discard the produced
// plan and rebuild via the fts-like-fallback round. It is used by the modifier
// guard in matchAgainstToBuiltin to allow native emission of a non-default
// modifier when round 1's plan is destined for discard anyway. The rescue
// conditions mirror the ones in matchAgainstToExpression that trigger
// MarkNonViableFTSMatch — alternative logical plans enabled AND a direct
// boolean predicate context.
// matchHasLikeFallbackRescue 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// MATCH AGAINST 是否启用 LIKE 回退救援路径。
pub fn matchHasLikeFallbackRescue(rewriter: &expressionRewriter<'_>) -> bool {
    rewriter.planCtx.as_ref().is_some_and(|context| {
        context
            .builder
            .ctx
            .GetSessionVars()
            .EnableAlternativeLogicalPlans
            && inDirectMatchBooleanContext(rewriter)
    })
}

// buildSemiApplyFromEqualSubq 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 由等值子查询构造 Semi Apply 计划。
pub fn buildSemiApplyFromEqualSubq(
    rewriter: &mut expressionRewriter<'_>,
    subquery_plan: logicalop::LogicalPlanRef,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    mut left: expression::ExprBox,
    mut right: expression::ExprBox,
    negated: bool,
    mark_no_decorrelate: bool,
) -> Result<(), errors::Error> {
    if rewriter.asScalar || negated {
        if expression::GetRowLen(right.as_ref()) == 1 {
            let right_column = right
                .as_any()
                .downcast_ref::<expression::Column>()
                .expect("subquery output is a column");
            if !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), left.as_ref())
                || !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), right_column)
            {
                let mut copy = right_column.CloneColumn();
                copy.InOperand = true;
                right = Box::new(copy);
                left = expression::SetExprColumnInOperand(left);
            }
        } else {
            let row_function = right
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
                .expect("multi-column subquery output is ROW");
            let mut arguments = Vec::with_capacity(row_function.GetArgs().len());
            let mut modified = false;
            for (index, right_argument) in row_function.GetArgs().iter().enumerate() {
                let left_argument = expression::GetFuncArg(left.as_ref(), index);
                if !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), left_argument.as_ref())
                    || !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), right_argument.as_ref())
                {
                    let mut column = right_argument
                        .as_any()
                        .downcast_ref::<expression::Column>()
                        .expect("subquery ROW argument is a column")
                        .CloneColumn();
                    column.InOperand = true;
                    arguments.push(Box::new(column) as expression::ExprBox);
                    modified = true;
                } else {
                    arguments.push(right_argument.CloneExpr());
                }
            }
            if modified {
                let return_type = arguments[0].GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
                right = newFunction(rewriter, ast::RowFunc, return_type, arguments)?;
                left = expression::SetExprColumnInOperand(left);
            }
        }
    }
    let condition = constructBinaryOpFunction(rewriter, left, right, ast::EQ)?;
    let new_plan = plan_context.builder.buildSemiApply(
        plan_context.plan.take(),
        subquery_plan,
        vec![condition],
        rewriter.asScalar,
        negated,
        false,
        mark_no_decorrelate,
    )?;
    plan_context.plan.replace(new_plan);
    Ok(())
}

// handleCompareSubquery 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理比较类子查询（quantified comparison）。
pub fn handleCompareSubquery(
    rewriter: &mut expressionRewriter<'_>,
    ctx: &dyn context::Context,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) -> bool {
    let ast::ExprKind::CompareSubquery { Op, L, R, All } = &node.Kind else {
        rewriter.err = Some(errors::New(
            "compare-subquery handler received another node",
        ));
        return true;
    };
    let cte_check = plan_context.builder.prepareCTECheckForSubQuery();
    let result = (|| -> Result<(), errors::Error> {
        let saved_as_scalar = rewriter.asScalar;
        rewriter.asScalar = true;
        L.Accept(rewriter);
        rewriter.asScalar = saved_as_scalar;
        let left = rewriter
            .ctxStack
            .last()
            .ok_or_else(|| errors::New("compare-subquery left expression is empty"))?
            .CloneExpr();

        let ast::ExprKind::Subquery { Query, .. } = &R.Kind else {
            return Err(errors::New(format!("Unknown compare type {:?}", R.Kind)));
        };
        let (subquery_plan, hint_flags) =
            buildSubquery(rewriter, ctx, plan_context, Query, handlingCompareSubquery)?;
        let correlated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(
            subquery_plan.as_ref(),
            plan_context.plan.Schema(),
        );
        let no_decorrelate = isNoDecorrelate(
            plan_context,
            &correlated,
            hint_flags,
            handlingCompareSubquery,
        );

        let can_multi_column = (!All && Op == "=") || (*All && Op == "!=");
        let left_len = expression::GetRowLen(left.as_ref());
        let right_len = subquery_plan.Schema().Len();
        if !can_multi_column && (left_len != 1 || right_len != 1) {
            return Err(expression::ErrOperandColumns.GenWithStackByArgs(1));
        }
        if left_len != right_len {
            return Err(expression::ErrOperandColumns.GenWithStackByArgs(left_len));
        }
        let right = if right_len == 1 {
            Box::new(subquery_plan.Schema().Columns[0].Clone()) as expression::ExprBox
        } else {
            let arguments = subquery_plan
                .Schema()
                .Columns
                .iter()
                .cloned()
                .map(|column| Box::new(column) as expression::ExprBox)
                .collect::<Vec<_>>();
            let return_type = arguments[0].GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
            newFunction(rewriter, ast::RowFunc, return_type, arguments)?
        };
        expression::CheckAndDeriveCollationFromExprs(
            rewriter.sctx,
            Op,
            types::ETInt,
            &[left.as_ref(), right.as_ref()],
        )?;

        match Op.as_str() {
            "=" if *All => handleEQAll(
                rewriter,
                plan_context,
                left,
                right,
                subquery_plan,
                no_decorrelate,
            )?,
            "=" => {
                rewriter.asScalar = true;
                buildSemiApplyFromEqualSubq(
                    rewriter,
                    subquery_plan,
                    plan_context,
                    left,
                    right,
                    false,
                    no_decorrelate,
                )?;
            }
            "!=" if *All => {
                rewriter.asScalar = true;
                buildSemiApplyFromEqualSubq(
                    rewriter,
                    subquery_plan,
                    plan_context,
                    left,
                    right,
                    true,
                    no_decorrelate,
                )?;
            }
            "!=" => handleNEAny(
                rewriter,
                plan_context,
                left,
                right,
                subquery_plan,
                no_decorrelate,
            )?,
            "<=>" => return Err(errors::New("We don't support <=> all or <=> any now")),
            _ => {
                let use_min =
                    ((Op == "<" || Op == "<=") && *All) || ((Op == ">" || Op == ">=") && !All);
                handleOtherComparableSubq(
                    rewriter,
                    plan_context,
                    left,
                    right,
                    subquery_plan,
                    use_min,
                    Op,
                    *All,
                    no_decorrelate,
                )?;
            }
        }
        ctxStackPop(rewriter, 1);
        if rewriter.asScalar {
            let last = plan_context.plan.Schema().Len() - 1;
            let column = plan_context.plan.Schema().Columns[last].Clone();
            let name = plan_context.plan.OutputNames().0[last].clone().unwrap();
            ctxStackAppend(rewriter, Box::new(column), name);
        }
        Ok(())
    })();
    resetCTECheckForSubQuery(cte_check);
    if let Err(error) = result {
        rewriter.err = Some(error);
    }
    true
}

// handleOtherComparableSubq handles the queries like < any, < max, etc. For example, if the query is t.id < any (select s.id from s),
// it will be rewrote to t.id < (select max(s.id) from s).
// handleOtherComparableSubq 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理其余可比较形式的子查询。
pub fn handleOtherComparableSubq(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    left: expression::ExprBox,
    right: expression::ExprBox,
    subquery_plan: logicalop::LogicalPlanRef,
    use_min: bool,
    compare_function: &str,
    all: bool,
    mark_no_decorrelate: bool,
) -> Result<(), errors::Error> {
    let mut aggregate = logicalop::LogicalAggregation::default().Init(
        plan_context.builder.ctx.clone(),
        plan_context.builder.getSelectOffset(),
    );
    if let Some(hints) = plan_context.builder.TableHints() {
        aggregate.PreferAggType = hints.PreferAggType as u64;
        aggregate.PreferAggToCop = hints.PreferAggToCop;
    }
    aggregate.SetChildren(vec![subquery_plan]);

    // Create the MAX/MIN aggregate exactly as the Go rewriter does.
    let function_name = if use_min {
        ast::AggFuncMin
    } else {
        ast::AggFuncMax
    };
    let max_or_min = aggregation::NewAggFuncDesc(
        plan_context.builder.ctx.GetExprCtx(),
        function_name,
        vec![right.CloneExpr()],
        false,
    )?;
    let mut aggregate_column = expression::Column::new(
        max_or_min
            .RetTp
            .Clone()
            .expect("MAX/MIN aggregate return type is initialized"),
        0,
        plan_context.builder.ctx.GetExprCtx().AllocPlanColumnID(),
        0,
    );
    aggregate_column.SetCoercibility(right.Coercibility());
    let aggregate_column = Box::new(aggregate_column);
    aggregate.SetOutputNames(append_name(
        aggregate.OutputNames().Shallow(),
        types::EmptyName.clone(),
    ));
    aggregate.SetSchema(expression::NewSchema(vec![aggregate_column.CloneColumn()]));
    aggregate.AggFuncs = vec![max_or_min];

    let condition = expression::NewFunction(
        rewriter.sctx,
        compare_function,
        *types::NewFieldType(mysql::TypeTiny),
        vec![left.CloneExpr(), aggregate_column],
    )?;
    buildQuantifierPlan(
        rewriter,
        plan_context,
        Box::new(aggregate),
        condition,
        left,
        right,
        all,
        mark_no_decorrelate,
    )
}

// buildQuantifierPlan adds extra condition for any / all subquery.
// buildQuantifierPlan 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 构造量词（ANY/ALL）比较对应的计划形态。
pub fn buildQuantifierPlan(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    mut aggregate: Box<logicalop::LogicalAggregation>,
    mut condition: expression::ExprBox,
    left: expression::ExprBox,
    right: expression::ExprBox,
    all: bool,
    mark_no_decorrelate: bool,
) -> Result<(), errors::Error> {
    let inner_is_null = expression::NewFunction(
        rewriter.sctx,
        ast::IsNull,
        *types::NewFieldType(mysql::TypeTiny),
        vec![right.CloneExpr()],
    )?;
    let outer_is_null = expression::NewFunction(
        rewriter.sctx,
        ast::IsNull,
        *types::NewFieldType(mysql::TypeTiny),
        vec![left],
    )?;
    let expression_context = plan_context.builder.ctx.GetExprCtx();
    let sum = aggregation::NewAggFuncDesc(
        expression_context,
        ast::AggFuncSum,
        vec![inner_is_null],
        false,
    )?;
    let builder_context = plan_context.builder.ctx.clone();
    let sum_column = Box::new(expression::Column::new(
        sum.RetTp
            .Clone()
            .expect("SUM aggregate return type is initialized"),
        0,
        builder_context.GetExprCtx().AllocPlanColumnID(),
        0,
    ));
    aggregate.AggFuncs.push(sum);
    let mut aggregate_schema = aggregate.Schema().Clone();
    aggregate_schema.Append([sum_column.CloneColumn()]);
    aggregate.SetSchema(aggregate_schema);
    let inner_has_null = expression::NewFunction(
        rewriter.sctx,
        ast::NE,
        *types::NewFieldType(mysql::TypeTiny),
        vec![sum_column, Box::new(expression::NewZero())],
    )?;

    // COUNT(1) distinguishes an empty subquery from one whose aggregate is NULL.
    let count = aggregation::NewAggFuncDesc(
        expression_context,
        ast::AggFuncCount,
        vec![Box::new(expression::NewOne())],
        false,
    )?;
    let count_column = Box::new(expression::Column::new(
        count
            .RetTp
            .Clone()
            .expect("COUNT aggregate return type is initialized"),
        0,
        builder_context.GetExprCtx().AllocPlanColumnID(),
        0,
    ));
    aggregate.AggFuncs.push(count);
    let mut aggregate_schema = aggregate.Schema().Clone();
    aggregate_schema.Append([count_column.CloneColumn()]);
    aggregate.SetSchema(aggregate_schema);

    if all {
        let inner_null_checker = expression::NewFunction(
            rewriter.sctx,
            ast::If,
            *types::NewFieldType(mysql::TypeTiny),
            vec![
                inner_has_null,
                Box::new(expression::NewNull()),
                Box::new(expression::NewOne()),
            ],
        )?;
        condition = composeCNF(rewriter.sctx, vec![condition, inner_null_checker]);
        let empty_checker = expression::NewFunction(
            rewriter.sctx,
            ast::EQ,
            *types::NewFieldType(mysql::TypeTiny),
            vec![count_column.CloneExpr(), Box::new(expression::NewZero())],
        )?;
        let outer_null_checker = expression::NewFunction(
            rewriter.sctx,
            ast::If,
            *types::NewFieldType(mysql::TypeTiny),
            vec![
                outer_is_null,
                Box::new(expression::NewNull()),
                Box::new(expression::NewZero()),
            ],
        )?;
        condition = composeDNF(
            rewriter.sctx,
            vec![condition, empty_checker, outer_null_checker],
        );
    } else {
        let inner_null_checker = expression::NewFunction(
            rewriter.sctx,
            ast::If,
            *types::NewFieldType(mysql::TypeTiny),
            vec![
                inner_has_null,
                Box::new(expression::NewNull()),
                Box::new(expression::NewZero()),
            ],
        )?;
        condition = composeDNF(rewriter.sctx, vec![condition, inner_null_checker]);
        let empty_checker = expression::NewFunction(
            rewriter.sctx,
            ast::NE,
            *types::NewFieldType(mysql::TypeTiny),
            vec![count_column.CloneExpr(), Box::new(expression::NewZero())],
        )?;
        let outer_null_checker = expression::NewFunction(
            rewriter.sctx,
            ast::If,
            *types::NewFieldType(mysql::TypeTiny),
            vec![
                outer_is_null,
                Box::new(expression::NewNull()),
                Box::new(expression::NewOne()),
            ],
        )?;
        condition = composeCNF(
            rewriter.sctx,
            vec![condition, empty_checker, outer_null_checker],
        );
    }

    if !rewriter.asScalar {
        let new_plan = plan_context.builder.buildSemiApply(
            plan_context.plan.take(),
            aggregate,
            vec![condition],
            false,
            false,
            false,
            mark_no_decorrelate,
        )?;
        plan_context.plan.replace(new_plan);
        return Ok(());
    }

    let outer_schema_length = plan_context.plan.Schema().Len();
    let new_plan = plan_context.builder.buildApplyWithJoinType(
        plan_context.plan.take(),
        aggregate,
        base::JoinType::InnerJoin,
        mark_no_decorrelate,
    );
    plan_context.plan.replace(new_plan);
    let join_schema = plan_context.plan.Schema().Clone();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expression::Column2Exprs(&join_schema.Columns[..outer_schema_length]),
        ..Default::default()
    }
    .Init(
        plan_context.builder.ctx.clone(),
        plan_context.builder.getSelectOffset(),
    );
    projection.SetOutputNames(types::NameSlice(Vec::with_capacity(
        outer_schema_length + 1,
    )));
    projection.SetOutputNames(plan_context.plan.OutputNames().Shallow());
    projection.SetSchema(expression::NewSchema(
        join_schema.Columns[..outer_schema_length].to_vec(),
    ));
    let condition_type = condition.GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
    projection.Exprs.push(condition);
    let mut projection_schema = projection.Schema().Clone();
    projection_schema.Append([expression::Column::new(
        condition_type,
        0,
        builder_context.GetExprCtx().AllocPlanColumnID(),
        0,
    )]);
    projection.SetSchema(projection_schema);
    projection.SetOutputNames(append_name(
        projection.OutputNames().Shallow(),
        types::EmptyName.clone(),
    ));
    projection.SetChildren(vec![plan_context.plan.take()]);
    plan_context.plan.replace(Box::new(projection));
    Ok(())
}

// handleNEAny handles the case of != any. For example, if the query is t.id != any (select s.id from s), it will be rewrote to
// t.id != s.id or count(distinct s.id) > 1 or [any checker]. If there are two different values in s.id
// there must exist a s.id that doesn't equal to t.id.
// handleNEAny 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理 `<> ANY` 量词比较。
pub fn handleNEAny(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    left: expression::ExprBox,
    right: expression::ExprBox,
    subquery_plan: logicalop::LogicalPlanRef,
    mark_no_decorrelate: bool,
) -> Result<(), errors::Error> {
    let session_context = plan_context.builder.ctx.clone();
    let expression_context = session_context.GetExprCtx();
    // If there is NULL in s.id column, s.id should be the value that isn't null in condition t.id != s.id.
    // So use function max to filter NULL.
    let max_function = aggregation::NewAggFuncDesc(
        expression_context,
        ast::AggFuncMax,
        vec![right.CloneExpr()],
        false,
    )?;
    let count_function = aggregation::NewAggFuncDesc(
        expression_context,
        ast::AggFuncCount,
        vec![right.CloneExpr()],
        true,
    )?;
    let mut aggregate = logicalop::LogicalAggregation {
        AggFuncs: vec![max_function.Clone(), count_function.Clone()],
        ..Default::default()
    }
    .Init(
        session_context.clone(),
        plan_context.builder.getSelectOffset(),
    );
    if let Some(hints) = plan_context.builder.TableHints() {
        aggregate.PreferAggType = hints.PreferAggType as u64;
        aggregate.PreferAggToCop = hints.PreferAggToCop;
    }
    aggregate.SetChildren(vec![subquery_plan]);
    let mut max_result_column = expression::Column::new(
        max_function
            .RetTp
            .Clone()
            .expect("MAX aggregate return type is initialized"),
        0,
        session_context.GetExprCtx().AllocPlanColumnID(),
        0,
    );
    max_result_column.SetCoercibility(right.Coercibility());
    let max_result_column = Box::new(max_result_column);
    let count_column = Box::new(expression::Column::new(
        count_function
            .RetTp
            .Clone()
            .expect("COUNT aggregate return type is initialized"),
        0,
        session_context.GetExprCtx().AllocPlanColumnID(),
        0,
    ));
    aggregate.SetOutputNames(append_names(
        aggregate.OutputNames().Shallow(),
        vec![types::EmptyName.clone(), types::EmptyName.clone()],
    ));
    aggregate.SetSchema(expression::NewSchema(vec![
        max_result_column.CloneColumn(),
        count_column.CloneColumn(),
    ]));
    let greater_than = expression::NewFunction(
        rewriter.sctx,
        ast::GT,
        *types::NewFieldType(mysql::TypeTiny),
        vec![count_column, Box::new(expression::NewOne())],
    )?;
    let not_equal = expression::NewFunction(
        rewriter.sctx,
        ast::NE,
        *types::NewFieldType(mysql::TypeTiny),
        vec![left.CloneExpr(), max_result_column],
    )?;
    let condition = composeDNF(rewriter.sctx, vec![greater_than, not_equal]);
    buildQuantifierPlan(
        rewriter,
        plan_context,
        Box::new(aggregate),
        condition,
        left,
        right,
        false,
        mark_no_decorrelate,
    )
}

// handleEQAll handles the case of = all. For example, if the query is t.id = all (select s.id from s), it will be rewrote to
// t.id = (select s.id from s having count(distinct s.id) <= 1 and [all checker]).
// handleEQAll 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理 `= ALL` 量词比较。
pub fn handleEQAll(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    left: expression::ExprBox,
    right: expression::ExprBox,
    subquery_plan: logicalop::LogicalPlanRef,
    mark_no_decorrelate: bool,
) -> Result<(), errors::Error> {
    let session_context = plan_context.builder.ctx.clone();
    let expression_context = session_context.GetExprCtx();
    // If there is NULL in s.id column, s.id should be the value that isn't null in condition t.id == s.id.
    // So use function max to filter NULL.
    let max_function = aggregation::NewAggFuncDesc(
        expression_context,
        ast::AggFuncMax,
        vec![right.CloneExpr()],
        false,
    )?;
    let count_function = aggregation::NewAggFuncDesc(
        expression_context,
        ast::AggFuncCount,
        vec![right.CloneExpr()],
        true,
    )?;
    let mut aggregate = logicalop::LogicalAggregation {
        AggFuncs: vec![max_function.Clone(), count_function.Clone()],
        ..Default::default()
    }
    .Init(
        session_context.clone(),
        plan_context.builder.getSelectOffset(),
    );
    if let Some(hints) = plan_context.builder.TableHints() {
        aggregate.PreferAggType = hints.PreferAggType as u64;
        aggregate.PreferAggToCop = hints.PreferAggToCop;
    }
    aggregate.SetChildren(vec![subquery_plan]);
    aggregate.SetOutputNames(append_name(
        aggregate.OutputNames().Shallow(),
        types::EmptyName.clone(),
    ));
    let mut max_result_column = expression::Column::new(
        max_function
            .RetTp
            .Clone()
            .expect("MAX aggregate return type is initialized"),
        0,
        session_context.GetExprCtx().AllocPlanColumnID(),
        0,
    );
    max_result_column.SetCoercibility(right.Coercibility());
    let max_result_column = Box::new(max_result_column);
    aggregate.SetOutputNames(append_name(
        aggregate.OutputNames().Shallow(),
        types::EmptyName.clone(),
    ));
    let count_column = Box::new(expression::Column::new(
        count_function
            .RetTp
            .Clone()
            .expect("COUNT aggregate return type is initialized"),
        0,
        session_context.GetExprCtx().AllocPlanColumnID(),
        0,
    ));
    aggregate.SetSchema(expression::NewSchema(vec![
        max_result_column.CloneColumn(),
        count_column.CloneColumn(),
    ]));
    let less_or_equal = expression::NewFunction(
        rewriter.sctx,
        ast::LE,
        *types::NewFieldType(mysql::TypeTiny),
        vec![count_column, Box::new(expression::NewOne())],
    )?;
    let equal = expression::NewFunction(
        rewriter.sctx,
        ast::EQ,
        *types::NewFieldType(mysql::TypeTiny),
        vec![left.CloneExpr(), max_result_column],
    )?;
    let condition = composeCNF(rewriter.sctx, vec![less_or_equal, equal]);
    buildQuantifierPlan(
        rewriter,
        plan_context,
        Box::new(aggregate),
        condition,
        left,
        right,
        true,
        mark_no_decorrelate,
    )
}

// handleExistSubquery 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理 EXISTS/NOT EXISTS 子查询。
pub fn handleExistSubquery(
    rewriter: &mut expressionRewriter<'_>,
    ctx: &dyn context::Context,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) -> bool {
    let ast::ExprKind::ExistsSubquery { Sel, Not } = &node.Kind else {
        rewriter.err = Some(errors::New("exists-subquery handler received another node"));
        return true;
    };
    let negated = *Not;
    let as_scalar = rewriter.asScalar;
    let cte_check = plan_context.builder.prepareCTECheckForSubQuery();
    let result = (|| -> Result<(), errors::Error> {
        let ast::ExprKind::Subquery { Query, .. } = &Sel.Kind else {
            return Err(errors::New(format!("Unknown exists type {:?}", Sel.Kind)));
        };
        let (mut subquery_plan, hint_flags) =
            buildSubquery(rewriter, ctx, plan_context, Query, handlingExistsSubquery)?;
        let correlated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(
            subquery_plan.as_ref(),
            plan_context.plan.Schema(),
        );
        let semi_join_rewrite_hint = hint_flags & hint::HintFlagSemiJoinRewrite != 0;
        if semi_join_rewrite_hint {
            plan_context.builder.enableSemiJoinRewrite = true;
        }
        let mut no_decorrelate = isNoDecorrelate(
            plan_context,
            &correlated,
            hint_flags,
            handlingExistsSubquery,
        );
        if !no_decorrelate && !correlated.is_empty() && !semi_join_rewrite_hint {
            plan_context
                .builder
                .ctx
                .GetSessionVars()
                .RecordRelevantOptVar(vardef::TiDBOptEnableAlternativeLogicalPlans);
            if plan_context
                .builder
                .ctx
                .GetSessionVars()
                .EnableCorrelateSubquery
            {
                no_decorrelate = true;
            }
        }
        // EXISTS only depends on row existence. Remove wildcard/projection
        // layers before adding the NO_DECORRELATE LIMIT so the limit does not
        // make an otherwise useless projection opaque to popExistsSubPlan.
        subquery_plan = popExistsSubPlan(rewriter, plan_context, subquery_plan);
        if no_decorrelate && !hasLimit(subquery_plan.as_ref()) {
            let limit = ast::Limit {
                Count: Some(ast::NewValueExpr(1, "", "")),
                ..Default::default()
            };
            let block_offset = subquery_plan.QueryBlockOffset();
            subquery_plan = plan_context
                .builder
                .buildLimit(subquery_plan, &limit, block_offset)?;
        }
        let mut semi_join_rewrite = semi_join_rewrite_hint;
        if semi_join_rewrite && hint_flags & hint::HintFlagNoDecorrelate != 0 {
            plan_context.builder.ctx.GetSessionVars().StmtCtx.SetHintWarning(
                "NO_DECORRELATE() and SEMI_JOIN_REWRITE() are in conflict. Both will be ineffective.",
            );
            no_decorrelate = false;
            semi_join_rewrite = false;
        }

        // popExistsSubPlan reduces a scalar aggregation without GROUP BY to a
        // one-row Dual. Its existence is already known at plan time, so Go
        // returns the boolean constant without optimizing or executing an
        // independent subquery.
        if let Some(dual) = subquery_plan
            .as_any()
            .downcast_ref::<logicalop::LogicalTableDual>()
        {
            let exists = dual.RowCount > 0;
            let value = if exists != negated {
                expression::NewSignedOne()
            } else {
                expression::NewSignedZero()
            };
            ctxStackAppend(rewriter, Box::new(value), types::EmptyName.clone());
            return Ok(());
        }

        let builder = &mut *plan_context.builder;
        let requires_apply = builder.disableSubQueryPreprocessing
            || !coreusage::ExtractCorrelatedCols4LogicalPlan(subquery_plan.as_ref()).is_empty()
            || hasCTEConsumerInSubPlan(subquery_plan.as_ref());
        if requires_apply {
            let new_plan = builder.buildSemiApply(
                plan_context.plan.take(),
                subquery_plan,
                Vec::new(),
                as_scalar,
                negated,
                semi_join_rewrite,
                no_decorrelate,
            )?;
            plan_context.plan.replace(new_plan);
            if as_scalar {
                let last = plan_context.plan.Schema().Len() - 1;
                ctxStackAppend(
                    rewriter,
                    Box::new(plan_context.plan.Schema().Columns[last].Clone()),
                    plan_context.plan.OutputNames().0[last].clone().unwrap(),
                );
            } else if rewriter.asScalar {
                // In a WHERE/HAVING boolean chain the semi join already
                // enforces EXISTS. Keep a TRUE stack operand so the parent
                // AND/OR expression consumes the correct arity without
                // leaking an unrelated projected column.
                ctxStackAppend(
                    rewriter,
                    Box::new(expression::NewSignedOne()),
                    types::EmptyName.clone(),
                );
            }
            return Ok(());
        }

        // Disable nth_plan only around independent subquery optimization.
        let nth_plan_backup = builder
            .ctx
            .GetSessionVars()
            .StmtCtx
            .StmtHints
            .SwapForceNthPlan(-1);
        let optimized = DoOptimize(ctx, &builder.ctx, builder.optFlag, &mut subquery_plan);
        builder
            .ctx
            .GetSessionVars()
            .StmtCtx
            .StmtHints
            .StoreForceNthPlan(nth_plan_backup);
        let mut physical_plan = optimized?.0;
        let scalar_mpp_frontier =
            physicalop::AlignScalarMppAggregationPlanIDs(physical_plan.as_mut())?;
        if let Some(frontier) = scalar_mpp_frontier {
            builder.ctx.restore_plan_id_checkpoint(frontier);
            let column_frontier = builder
                .ctx
                .GetSessionVars()
                .PlanColumnID
                .load(std::sync::atomic::Ordering::SeqCst);
            builder
                .ctx
                .GetSessionVars()
                .PlanColumnID
                .store(column_frontier + 5, std::sync::atomic::Ordering::SeqCst);
        }
        let physical_plan: std::sync::Arc<dyn base::PhysicalPlan> = physical_plan.into();

        if builder.ctx.GetBuildPBCtx().InExplainStmt
            && !builder
                .ctx
                .GetSessionVars()
                .StmtCtx
                .IsInExplainAnalyzeStmt()
            && explain_non_eval_scalar_subquery(builder.ctx.GetSessionVars())
        {
            let new_column_id = builder.ctx.GetExprCtx().AllocPlanColumnID();
            let mut subquery_context = ScalarSubqueryEvalCtx::New(
                builder.ctx.clone(),
                subquery_plan.QueryBlockOffset(),
                physical_plan.clone(),
                rewriter.ctx.clone(),
                builder.is.clone(),
            );
            subquery_context.output_col_ids = vec![new_column_id];
            let mut scalar_subquery =
                ScalarSubQueryExpr::new(new_column_id, subquery_context.clone());
            scalar_subquery.RetType = Some(
                subquery_plan.Schema().Columns[0]
                    .GetType(rewriter.sctx.GetEvalCtx())
                    .DeepCopy(),
            );
            scalar_subquery.SetCoercibility(subquery_plan.Schema().Columns[0].Coercibility());
            builder
                .ctx
                .GetSessionVars()
                .RegisterScalarSubQ(subquery_context);
            if scalar_mpp_frontier.is_some() {
                builder.ctx.alloc_plan_id();
            }
            let scalar_subquery = Box::new(scalar_subquery) as expression::ExprBox;
            if negated {
                let wrapped = expression::NewFunction(
                    builder.ctx.GetExprCtx(),
                    ast::UnaryNot,
                    *types::NewFieldType(mysql::TypeTiny),
                    vec![scalar_subquery],
                )?;
                ctxStackAppend(rewriter, wrapped, types::EmptyName.clone());
            } else {
                ctxStackAppend(rewriter, scalar_subquery, types::EmptyName.clone());
            }
            return Ok(());
        }

        let mut subquery_context = ScalarSubqueryEvalCtx::New(
            builder.ctx.clone(),
            subquery_plan.QueryBlockOffset(),
            physical_plan.clone(),
            rewriter.ctx.clone(),
            builder.is.clone(),
        );
        subquery_context.output_col_ids = subquery_plan
            .Schema()
            .Columns
            .iter()
            .map(|_| builder.ctx.GetExprCtx().AllocPlanColumnID())
            .collect();
        builder
            .ctx
            .GetSessionVars()
            .RegisterScalarSubQ(subquery_context);
        let row = callEvalSubqueryFirstRow(
            ctx,
            physical_plan.as_ref(),
            builder.is.as_ref(),
            builder.ctx.as_ref(),
        )?;
        if (row.is_some() && !negated) || (row.is_none() && negated) {
            ctxStackAppend(
                rewriter,
                Box::new(expression::NewSignedOne()),
                types::EmptyName.clone(),
            );
        } else {
            ctxStackAppend(
                rewriter,
                Box::new(expression::NewSignedZero()),
                types::EmptyName.clone(),
            );
        }
        Ok(())
    })();
    resetCTECheckForSubQuery(cte_check);
    if let Err(error) = result {
        rewriter.err = Some(error);
    }
    true
}

// popExistsSubPlan will remove the useless plan in exist's child.
// See comments inside the method for more details.
// popExistsSubPlan 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 弹出 EXISTS 改写产生的子计划结果。
pub fn popExistsSubPlan(
    _rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    mut plan: logicalop::LogicalPlanRef,
) -> logicalop::LogicalPlanRef {
    loop {
        // Projection and Sort never affect whether the child has a row.
        if plan.as_any().is::<logicalop::LogicalProjection>()
            || plan.as_any().is::<logicalop::LogicalSort>()
        {
            plan = plan.TakeChildren().remove(0);
            continue;
        }
        if let Some(aggregation) = plan
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
        {
            if aggregation.GroupByItems.is_empty() {
                return Box::new(
                    logicalop::LogicalTableDual {
                        RowCount: 1,
                        ..Default::default()
                    }
                    .Init(
                        plan_context.builder.ctx.clone(),
                        plan_context.builder.getSelectOffset(),
                    ),
                );
            }
            plan = plan.TakeChildren().remove(0);
            continue;
        }
        return plan;
    }
}

// handleInSubquery 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理 IN/NOT IN 子查询。
pub fn handleInSubquery(
    rewriter: &mut expressionRewriter<'_>,
    ctx: &dyn context::Context,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) -> bool {
    let ast::ExprKind::InSubquery { Expr, Sel, Not } = &node.Kind else {
        rewriter.err = Some(errors::New("IN-subquery handler received another node"));
        return true;
    };
    let cte_check = plan_context.builder.prepareCTECheckForSubQuery();
    let result = (|| -> Result<(), errors::Error> {
        let was_scalar = rewriter.asScalar;
        rewriter.asScalar = true;
        Expr.Accept(rewriter);
        if let Some(error) = rewriter.err.take() {
            return Err(error);
        }
        let mut left = rewriter
            .ctxStack
            .last()
            .ok_or_else(|| errors::New("IN-subquery left expression is empty"))?
            .CloneExpr();
        let ast::ExprKind::Subquery { Query, .. } = &Sel.Kind else {
            return Err(errors::New(format!("Unknown compare type {:?}", Sel.Kind)));
        };
        let (subquery_plan, hint_flags) =
            buildSubquery(rewriter, ctx, plan_context, Query, handlingInSubquery)?;
        let left_length = expression::GetRowLen(left.as_ref());
        if left_length != subquery_plan.Schema().Len() {
            return Err(expression::ErrOperandColumns.GenWithStackByArgs(left_length));
        }
        let mark_in_operand = *Not
            || (was_scalar && !canTreatInSubqueryAsExistsForFilter(rewriter, Some(plan_context)));
        let right = if subquery_plan.Schema().Len() == 1 {
            let mut column = subquery_plan.Schema().Columns[0].Clone();
            if mark_in_operand
                && (!expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), left.as_ref())
                    || !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), &column))
            {
                column.InOperand = true;
                left = expression::SetExprColumnInOperand(left);
            }
            Box::new(column) as expression::ExprBox
        } else {
            let mut arguments = Vec::with_capacity(subquery_plan.Schema().Len());
            for (index, source_column) in subquery_plan.Schema().Columns.iter().enumerate() {
                let mut column = source_column.Clone();
                if mark_in_operand {
                    let left_argument = expression::GetFuncArg(left.as_ref(), index);
                    if !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), left_argument.as_ref())
                        || !expression::ExprNotNull(rewriter.sctx.GetEvalCtx(), &column)
                    {
                        column.InOperand = true;
                        let scalar = left
                            .as_any_mut()
                            .downcast_mut::<expression::ScalarFunction>()
                            .expect("multi-column IN left operand is ROW");
                        scalar.GetArgsMut()[index] =
                            expression::SetExprColumnInOperand(left_argument);
                    }
                }
                arguments.push(Box::new(column) as expression::ExprBox);
            }
            let return_type = arguments[0].GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
            newFunction(rewriter, ast::RowFunc, return_type, arguments)?
        };
        let mut check_condition =
            constructBinaryOpFunction(rewriter, left.CloneExpr(), right.CloneExpr(), ast::EQ)?;

        // If the leftKey and the rightKey have different collations, don't convert the sub-query to an inner-join
        // since when converting we will add a distinct-agg upon the right child and this distinct-agg doesn't have the right collation.
        // To keep it simple, we forbid this converting if they have different collations.
        // tested by TestCollateSubQuery.
        let left_type = left.GetType(rewriter.sctx.GetEvalCtx());
        let right_type = right.GetType(rewriter.sctx.GetEvalCtx());
        let collations_compatible =
            collate::CompatibleCollate(left_type.GetCollate(), right_type.GetCollate());
        let correlated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(
            subquery_plan.as_ref(),
            plan_context.plan.Schema(),
        );
        let mut no_decorrelate =
            isNoDecorrelate(plan_context, &correlated, hint_flags, handlingInSubquery);
        // When EnableCorrelateSubquery is ON (set by the correlate alternative round),
        // prevent decorrelation of correlated IN subqueries so they stay as Apply with index lookups.
        if !no_decorrelate && !correlated.is_empty() && !*Not {
            let session_variables = plan_context.builder.ctx.GetSessionVars();
            session_variables.RecordRelevantOptVar(vardef::TiDBOptEnableAlternativeLogicalPlans);
            if session_variables.EnableAlternativeLogicalPlans {
                session_variables
                    .StmtCtx
                    .MarkAlternativeLogicalPlanPreferCorrelate();
            }
            if session_variables.EnableCorrelateSubquery {
                no_decorrelate = true;
            }
        }

        // If it's not the form of `not in (SUBQUERY)`,
        // and has no correlated column from the current level plan(if the correlated column is from upper level,
        // we can treat it as constant, because the upper LogicalApply cannot be eliminated since current node is a join node),
        // and don't need to append a scalar value, we can rewrite it to inner join.
        // When EnableCorrelateSubquery is ON (set by the correlate alternative round), skip the
        // InnerJoin+Agg rewrite so that a SemiJoin is built instead; the CorrelateSolver rule can
        // then convert it to a correlated Apply with index lookups.
        let can_rewrite_to_join_aggregate = plan_context
            .builder
            .ctx
            .GetSessionVars()
            .GetAllowInSubqToJoinAndAgg()
            && !*Not
            && !was_scalar
            && correlated.is_empty()
            && collations_compatible;
        if can_rewrite_to_join_aggregate {
            // Record that the alternative logical plans variable is relevant — toggling it
            // changes whether we take the InnerJoin+Agg path or the SemiApply path.
            plan_context
                .builder
                .ctx
                .GetSessionVars()
                .RecordRelevantOptVar(vardef::TiDBOptEnableAlternativeLogicalPlans);
            // Signal that a correlate alternative round is worth attempting.
            if plan_context
                .builder
                .ctx
                .GetSessionVars()
                .EnableAlternativeLogicalPlans
            {
                plan_context
                    .builder
                    .ctx
                    .GetSessionVars()
                    .StmtCtx
                    .MarkAlternativeLogicalPlanPreferCorrelate();
            }
        }
        if can_rewrite_to_join_aggregate
            && !plan_context
                .builder
                .ctx
                .GetSessionVars()
                .EnableCorrelateSubquery
        {
            // The IN-to-inner-join path must retain all optimizer flags used by Go.
            plan_context.builder.optFlag |= rule::FLAG_ELIMINATE_AGG;
            plan_context.builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION;
            plan_context.builder.optFlag |= rule::FLAG_JOIN_REORDER;
            plan_context.builder.optFlag |= rule::FLAG_EMPTY_SELECTION_ELIMINATOR;
            let mut distinct_length = subquery_plan.Schema().Len();
            let mut distinct_child = subquery_plan;
            let mut join_condition = check_condition;
            // IN-subquery rewrite turns:
            //   outer_col IN (SELECT inner_col ...)
            // into:
            //   outer JOIN DISTINCT(inner_col) ON outer_col = inner_col
            // DISTINCT must be applied on the same comparison domain as the join predicate.
            // If "=" injects an implicit cast on the RHS (e.g. blob/string -> number), deduplicating
            // raw inner values is insufficient: different raw values may become equal after cast and
            // multiply outer rows in the rewritten inner join.
            // To keep semantics equivalent to IN, we project the RHS comparison expression first,
            // then distinct on that projected key.
            if left_length == 1 {
                if let Some(equal_condition) = join_condition
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .filter(|function| function.FuncName.L == ast::EQ)
                {
                    let lhs = equal_condition.GetArgs()[0].CloneExpr();
                    let rhs = equal_condition.GetArgs()[1].CloneExpr();
                    if expression::ExprFromSchema(lhs.as_ref(), plan_context.plan.Schema())
                        && !lhs.as_any().is::<expression::Column>()
                    {
                        let old_schema = plan_context.plan.Schema().Clone();
                        let mut projection_expressions =
                            expression::Column2Exprs(&old_schema.Columns);
                        projection_expressions.push(lhs.CloneExpr());
                        let mut projection_schema = old_schema;
                        let projection_column = expression::Column::new(
                            lhs.GetType(rewriter.sctx.GetEvalCtx()).DeepCopy(),
                            0,
                            plan_context.builder.ctx.GetExprCtx().AllocPlanColumnID(),
                            projection_expressions.len() as isize - 1,
                        );
                        projection_schema.Append([projection_column.Clone()]);
                        let mut output_names = plan_context.plan.OutputNames().Shallow();
                        output_names.0.push(Some(types::EmptyName.clone()));
                        let mut projection = logicalop::LogicalProjection {
                            Exprs: projection_expressions,
                            ..Default::default()
                        }
                        .Init(
                            plan_context.builder.ctx.clone(),
                            plan_context.builder.getSelectOffset(),
                        );
                        projection.SetSchema(projection_schema);
                        projection.SetOutputNames(output_names);
                        projection.SetChildren(vec![plan_context.plan.take()]);
                        plan_context.plan.replace(Box::new(projection));
                        join_condition = constructBinaryOpFunction(
                            rewriter,
                            Box::new(projection_column),
                            rhs,
                            ast::EQ,
                        )?;
                    }
                }
                if let Some(equal_condition) = join_condition
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .filter(|function| function.FuncName.L == ast::EQ)
                {
                    let rhs = equal_condition.GetArgs()[1].CloneExpr();
                    if expression::ExprFromSchema(rhs.as_ref(), distinct_child.Schema())
                        && !rhs.as_any().is::<expression::Column>()
                    {
                        // rhs is computed from inner columns (typically with an implicit cast generated
                        // by type coercion rules). Materialize it as an inner projection column so both
                        // DISTINCT and JOIN use exactly this coerced key.
                        let mut projection = logicalop::LogicalProjection {
                            Exprs: vec![rhs.CloneExpr()],
                            ..Default::default()
                        }
                        .Init(
                            plan_context.builder.ctx.clone(),
                            plan_context.builder.getSelectOffset(),
                        );
                        let projection_column = expression::Column::new(
                            rhs.GetType(rewriter.sctx.GetEvalCtx()).DeepCopy(),
                            0,
                            plan_context
                                .builder
                                .ctx
                                .GetSessionVars()
                                .AllocPlanColumnID(),
                            0,
                        );
                        projection.SetChildren(vec![distinct_child]);
                        projection
                            .SetSchema(expression::NewSchema(vec![projection_column.Clone()]));
                        projection
                            .SetOutputNames(types::NameSlice(vec![Some(types::EmptyName.clone())]));
                        distinct_child = Box::new(projection);
                        distinct_length = 1;
                        // Rebuild join condition against the projected key; this preserves
                        // the same coercion behavior while preventing duplicate matches.
                        join_condition = constructBinaryOpFunction(
                            rewriter,
                            left.CloneExpr(),
                            Box::new(projection_column),
                            ast::EQ,
                        )?;
                    }
                }
            }
            // Build distinct for the inner query.
            let distinct = plan_context
                .builder
                .buildDistinct(distinct_child, distinct_length)?;
            // Build inner join above the aggregation.
            let mut join = logicalop::LogicalJoin {
                JoinType: base::JoinType::InnerJoin,
                // 该 InnerJoin 来自 IN 半连接语义，不能再与外层普通
                // InnerJoin 扁平化重排，否则 DISTINCT 只约束内侧的边界会丢失。
                FromSemiJoinRewrite: true,
                FromInSubqueryRewrite: true,
                ..Default::default()
            }
            .Init(
                plan_context.builder.ctx.clone(),
                plan_context.builder.getSelectOffset(),
            );
            join.SetSchema(
                expression::MergeSchema(Some(plan_context.plan.Schema()), Some(distinct.Schema()))
                    .expect("an inner join always has both input schemas"),
            );
            let mut output_names = plan_context.plan.OutputNames().Shallow();
            output_names
                .0
                .extend(distinct.OutputNames().0.iter().cloned());
            join.SetOutputNames(output_names);
            join.SetChildren(vec![plan_context.plan.take(), distinct]);
            join.AttachOnConds(expression::SplitCNFItems(join_condition.as_ref()));
            // set FullSchema and FullNames for this join
            let inherited_full_schema = {
                let children = join.Children();
                children[0]
                    .as_any()
                    .downcast_ref::<logicalop::LogicalJoin>()
                    .and_then(|left_join| {
                        left_join
                            .FullSchema
                            .as_ref()
                            .map(|schema| (schema.Clone(), left_join.FullNames.Shallow()))
                    })
            };
            if let Some((schema, names)) = inherited_full_schema {
                join.FullSchema = Some(schema);
                join.FullNames = names;
            }
            // Set join hint for this join.
            let (prefer_join_type, prefer_join_order) = plan_context.builder.joinHintPreference();
            join.SetPreferredJoinTypeAndOrder(prefer_join_type, prefer_join_order);
            plan_context.plan.replace(Box::new(join));
        } else {
            let semi_rewrite = hint_flags & hint::HintFlagSemiJoinRewrite != 0;
            let new_plan = plan_context.builder.buildSemiApply(
                plan_context.plan.take(),
                subquery_plan,
                expression::SplitCNFItems(check_condition.as_ref()),
                was_scalar,
                *Not,
                semi_rewrite,
                no_decorrelate,
            )?;
            plan_context.plan.replace(new_plan);
            // When EnableCorrelateSubquery is ON (set by the correlate alternative round)
            // and the subquery is non-correlated, mark the join so that CorrelateSolver
            // converts it to a correlated Apply.
            if correlated.is_empty() && !*Not {
                let session_variables = plan_context.builder.ctx.GetSessionVars();
                session_variables
                    .RecordRelevantOptVar(vardef::TiDBOptEnableAlternativeLogicalPlans);
                if session_variables.EnableCorrelateSubquery {
                    if let Some(apply) = plan_context
                        .plan
                        .as_any_mut()
                        .downcast_mut::<logicalop::LogicalApply>()
                    {
                        apply.LogicalJoin.PreferCorrelate = true;
                    }
                }
            }
        }

        ctxStackPop(rewriter, 1);
        if was_scalar {
            let last = plan_context.plan.Schema().Len() - 1;
            ctxStackAppend(
                rewriter,
                Box::new(plan_context.plan.Schema().Columns[last].Clone()),
                plan_context.plan.OutputNames().0[last].clone().unwrap(),
            );
        }
        Ok(())
    })();
    resetCTECheckForSubQuery(cte_check);
    if let Err(error) = result {
        rewriter.err = Some(error);
    }
    true
}

// isNoDecorrelate 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 是否禁用解相关（no_decorrelate）Hint。
pub fn isNoDecorrelate(
    plan_context: &mut exprRewriterPlanCtx<'_>,
    correlated_columns: &[expression::CorrelatedColumn],
    hint_flags: u64,
    subquery_context: subQueryCtx,
) -> bool {
    let mut no_decorrelate = hint_flags & hint::HintFlagNoDecorrelate != 0;

    if correlated_columns.is_empty() {
        if no_decorrelate {
            plan_context
                .builder
                .ctx
                .GetSessionVars()
                .StmtCtx
                .SetHintWarning(
                    "NO_DECORRELATE() is inapplicable because there are no correlated columns.",
                );
            no_decorrelate = false;
        }
    } else {
        let semi_join_rewrite = hint_flags & hint::HintFlagSemiJoinRewrite != 0;
        // We can't override noDecorrelate via the variable for EXISTS subqueries with semi join rewrite
        // as this will cause a conflict that will result in both being disabled in later code
        // SemiJoinRewrite does not check the variable TiDBOptEnableSemiJoinRewrite.
        // If that variable is enabled - we can still choose NOT to decorrelate here.
        if !(semi_join_rewrite && subquery_context == handlingExistsSubquery) {
            // Only support scalar and exists subqueries
            let valid_subquery_type = subquery_context == handlingScalarSubquery
                || subquery_context == handlingExistsSubquery;
            if valid_subquery_type && plan_context.curClause == fieldList {
                plan_context
                    .builder
                    .ctx
                    .GetSessionVars()
                    .RecordRelevantOptVar(vardef::TiDBOptEnableNoDecorrelateInSelect);
                // If it isn't already enabled via hint, and variable is set, then enable it
                if !no_decorrelate
                    && plan_context
                        .builder
                        .ctx
                        .GetSessionVars()
                        .EnableNoDecorrelateInSelect
                {
                    no_decorrelate = true;
                }
            }
        }
    }
    no_decorrelate
}

// handleScalarSubquery 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 处理标量子查询，必要时生成 Apply。
pub fn handleScalarSubquery(
    rewriter: &mut expressionRewriter<'_>,
    ctx: &dyn context::Context,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) -> bool {
    let ast::ExprKind::Subquery { Query, .. } = &node.Kind else {
        rewriter.err = Some(errors::New("scalar-subquery handler received another node"));
        return true;
    };
    let cte_check = plan_context.builder.prepareCTECheckForSubQuery();
    let result = (|| -> Result<(), errors::Error> {
        let (subquery_plan, hint_flags) =
            buildSubquery(rewriter, ctx, plan_context, Query, handlingScalarSubquery)?;
        let mut subquery_plan = plan_context.builder.buildMaxOneRow(subquery_plan);
        let correlated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(
            subquery_plan.as_ref(),
            plan_context.plan.Schema(),
        );
        let no_decorrelate = isNoDecorrelate(
            plan_context,
            &correlated,
            hint_flags,
            handlingScalarSubquery,
        );
        let requires_apply = plan_context.builder.disableSubQueryPreprocessing
            || !coreusage::ExtractCorrelatedCols4LogicalPlan(subquery_plan.as_ref()).is_empty()
            || hasCTEConsumerInSubPlan(subquery_plan.as_ref());
        if requires_apply {
            let subquery_columns = subquery_plan.Schema().Columns.clone();
            let new_plan = plan_context.builder.buildApplyWithJoinType(
                plan_context.plan.take(),
                subquery_plan,
                base::JoinType::LeftOuterJoin,
                no_decorrelate,
            );
            plan_context.plan.replace(new_plan);
            if subquery_columns.len() > 1 {
                let arguments = subquery_columns
                    .into_iter()
                    .map(|column| Box::new(column) as expression::ExprBox)
                    .collect::<Vec<_>>();
                let return_type = arguments[0].GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
                let row = newFunction(rewriter, ast::RowFunc, return_type, arguments)?;
                ctxStackAppend(rewriter, row, types::EmptyName.clone());
            } else {
                let last = plan_context.plan.Schema().Len() - 1;
                ctxStackAppend(
                    rewriter,
                    Box::new(plan_context.plan.Schema().Columns[last].Clone()),
                    plan_context.plan.OutputNames().0[last].clone().unwrap(),
                );
            }
            return Ok(());
        }

        let nth_plan_backup = plan_context
            .builder
            .ctx
            .GetSessionVars()
            .StmtCtx
            .StmtHints
            .SwapForceNthPlan(-1);
        let optimized = DoOptimize(
            ctx,
            &plan_context.builder.ctx,
            plan_context.builder.optFlag,
            &mut subquery_plan,
        );
        plan_context
            .builder
            .ctx
            .GetSessionVars()
            .StmtCtx
            .StmtHints
            .StoreForceNthPlan(nth_plan_backup);
        let mut physical_plan = optimized?.0;
        let scalar_mpp_frontier =
            physicalop::AlignScalarMppAggregationPlanIDs(physical_plan.as_mut())?;
        if let Some(frontier) = scalar_mpp_frontier {
            plan_context
                .builder
                .ctx
                .restore_plan_id_checkpoint(frontier);
            let column_frontier = plan_context
                .builder
                .ctx
                .GetSessionVars()
                .PlanColumnID
                .load(std::sync::atomic::Ordering::SeqCst);
            plan_context
                .builder
                .ctx
                .GetSessionVars()
                .PlanColumnID
                .store(column_frontier + 5, std::sync::atomic::Ordering::SeqCst);
        }
        let physical_plan: std::sync::Arc<dyn base::PhysicalPlan> = physical_plan.into();
        let block_offset = subquery_plan.QueryBlockOffset();

        if plan_context.builder.ctx.GetBuildPBCtx().InExplainStmt
            && !plan_context
                .builder
                .ctx
                .GetSessionVars()
                .StmtCtx
                .InExplainAnalyzeStmt
            && explain_non_eval_scalar_subquery(plan_context.builder.ctx.GetSessionVars())
        {
            let mut subquery_context = ScalarSubqueryEvalCtx::New(
                plan_context.builder.ctx.clone(),
                block_offset,
                physical_plan.clone(),
                rewriter.ctx.clone(),
                plan_context.builder.is.clone(),
            );
            let mut column_ids = Vec::with_capacity(subquery_plan.Schema().Len());
            let mut scalar_expressions = Vec::with_capacity(subquery_plan.Schema().Len());
            for column in &subquery_plan.Schema().Columns {
                let column_id = plan_context.builder.ctx.GetExprCtx().AllocPlanColumnID();
                let mut scalar_subquery =
                    ScalarSubQueryExpr::new(column_id, subquery_context.clone());
                scalar_subquery.RetType = column.RetType.DeepCopy();
                scalar_subquery.SetCoercibility(column.Coercibility());
                column_ids.push(column_id);
                scalar_expressions.push(Box::new(scalar_subquery) as expression::ExprBox);
            }
            subquery_context.output_col_ids = column_ids;
            plan_context
                .builder
                .ctx
                .GetSessionVars()
                .RegisterScalarSubQ(subquery_context);
            if scalar_mpp_frontier.is_some() {
                plan_context.builder.ctx.alloc_plan_id();
            }
            if scalar_expressions.len() == 1 {
                ctxStackAppend(
                    rewriter,
                    scalar_expressions.remove(0),
                    types::EmptyName.clone(),
                );
            } else {
                let return_type = scalar_expressions[0]
                    .GetType(rewriter.sctx.GetEvalCtx())
                    .DeepCopy();
                let row = newFunction(rewriter, ast::RowFunc, return_type, scalar_expressions)?;
                ctxStackAppend(rewriter, row, types::EmptyName.clone());
            }
            return Ok(());
        }

        let mut subquery_context = ScalarSubqueryEvalCtx::New(
            plan_context.builder.ctx.clone(),
            block_offset,
            physical_plan.clone(),
            rewriter.ctx.clone(),
            plan_context.builder.is.clone(),
        );
        let column_ids = subquery_plan
            .Schema()
            .Columns
            .iter()
            .map(|_| plan_context.builder.ctx.GetExprCtx().AllocPlanColumnID())
            .collect::<Vec<_>>();
        subquery_context.output_col_ids = column_ids.clone();
        plan_context
            .builder
            .ctx
            .GetSessionVars()
            .RegisterScalarSubQ(subquery_context);
        let row = callEvalSubqueryFirstRow(
            ctx,
            physical_plan.as_ref(),
            plan_context.builder.is.as_ref(),
            plan_context.builder.ctx.as_ref(),
        )?;
        let row = row.unwrap_or_else(|| {
            subquery_plan
                .Schema()
                .Columns
                .iter()
                .map(|_| {
                    let mut datum = types::Datum::default();
                    datum.SetNull();
                    datum
                })
                .collect()
        });
        if row.len() != subquery_plan.Schema().Len() {
            return Err(errors::New(format!(
                "scalar subquery returned {} columns, expected {}",
                row.len(),
                subquery_plan.Schema().Len()
            )));
        }
        let mut constants = Vec::with_capacity(subquery_plan.Schema().Len());
        for (index, datum) in row.into_iter().enumerate() {
            let mut constant = expression::Constant::with_subquery(
                datum,
                subquery_plan.Schema().Columns[index]
                    .GetType(rewriter.sctx.GetEvalCtx())
                    .DeepCopy(),
                column_ids[index],
            );
            constant.SetCoercibility(subquery_plan.Schema().Columns[index].Coercibility());
            constants.push(Box::new(constant) as expression::ExprBox);
        }
        if constants.len() > 1 {
            let return_type = constants[0].GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
            let row = newFunction(rewriter, ast::RowFunc, return_type, constants)?;
            ctxStackAppend(rewriter, row, types::EmptyName.clone());
        } else {
            ctxStackAppend(rewriter, constants.remove(0), types::EmptyName.clone());
        }
        Ok(())
    })();
    resetCTECheckForSubQuery(cte_check);
    if let Err(error) = result {
        rewriter.err = Some(error);
    }
    true
}

// hasCTEConsumerInSubPlan 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 子计划中是否包含 CTE Consumer。
pub fn hasCTEConsumerInSubPlan(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalCTE>()
        || plan
            .Children()
            .iter()
            .any(|child| hasCTEConsumerInSubPlan(child.as_ref()))
}

// initConstantRepertoire 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 初始化可用于常量折叠的函数白名单。
pub fn initConstantRepertoire(
    evaluation_context: &dyn expression::EvalContext,
    constant: &mut expression::Constant,
) {
    constant.SetRepertoire(expression::ASCII);
    if constant
        .RetType
        .as_ref()
        .expect("a constant always has a return type")
        .EvalType()
        == types::ETString
        && constant.Value.GetBytes().iter().any(|byte| *byte >= 0x80)
    {
        constant.SetRepertoire(expression::UNICODE);
    }
}

/// 时间单位枚举转名称。
fn timeUnitName(unit: ast::TimeUnitType) -> &'static str {
    match unit {
        ast::TimeUnitType::Invalid => "INVALID",
        ast::TimeUnitType::Microsecond => "MICROSECOND",
        ast::TimeUnitType::Second => "SECOND",
        ast::TimeUnitType::Minute => "MINUTE",
        ast::TimeUnitType::Hour => "HOUR",
        ast::TimeUnitType::Day => "DAY",
        ast::TimeUnitType::Week => "WEEK",
        ast::TimeUnitType::Month => "MONTH",
        ast::TimeUnitType::Quarter => "QUARTER",
        ast::TimeUnitType::Year => "YEAR",
        ast::TimeUnitType::SecondMicrosecond => "SECOND_MICROSECOND",
        ast::TimeUnitType::MinuteMicrosecond => "MINUTE_MICROSECOND",
        ast::TimeUnitType::MinuteSecond => "MINUTE_SECOND",
        ast::TimeUnitType::HourMicrosecond => "HOUR_MICROSECOND",
        ast::TimeUnitType::HourSecond => "HOUR_SECOND",
        ast::TimeUnitType::HourMinute => "HOUR_MINUTE",
        ast::TimeUnitType::DayMicrosecond => "DAY_MICROSECOND",
        ast::TimeUnitType::DaySecond => "DAY_SECOND",
        ast::TimeUnitType::DayMinute => "DAY_MINUTE",
        ast::TimeUnitType::DayHour => "DAY_HOUR",
        ast::TimeUnitType::YearMonth => "YEAR_MONTH",
    }
}

/// GET_FORMAT 选择器转名称。
fn getFormatSelectorName(selector: ast::GetFormatSelectorType) -> &'static str {
    match selector {
        ast::GetFormatSelectorType::Date => "DATE",
        ast::GetFormatSelectorType::Datetime => "DATETIME",
        ast::GetFormatSelectorType::Time => "TIME",
    }
}

// adjustUTF8MB4Collation 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 按需调整 utf8mb4 相关排序规则。
pub fn adjustUTF8MB4Collation(
    rewriter: &expressionRewriter<'_>,
    field_type: &mut types::FieldType,
) {
    if field_type.GetFlag() & mysql::UnderScoreCharsetFlag != 0
        && charset::CharsetUTF8MB4 == field_type.GetCharset()
    {
        field_type.SetCollate(rewriter.sctx.GetDefaultCollationForUTF8MB4());
    }
}

/// AST ValueDatum 转为 types::Datum。
fn datumFromAstValue(value: &ast::ValueDatum) -> Result<types::Datum, errors::Error> {
    Ok(match value {
        ast::ValueDatum::Null => {
            let mut datum = types::Datum::default();
            datum.SetNull();
            datum
        }
        ast::ValueDatum::Bool(value) => types::NewIntDatum(i64::from(*value)),
        ast::ValueDatum::Int64(value) => types::NewIntDatum(*value),
        ast::ValueDatum::Uint64(value) => types::NewUintDatum(*value),
        ast::ValueDatum::Float32(bits) => types::NewFloat32Datum(f32::from_bits(*bits)),
        ast::ValueDatum::Float64(bits) => types::NewFloat64Datum(f64::from_bits(*bits)),
        ast::ValueDatum::Decimal(value) => {
            let mut decimal = types::MyDecimal::default();
            decimal
                .FromString(value.as_bytes())
                .map_err(|error| errors::New(error.to_string()))?;
            types::NewDecimalDatum(decimal)
        }
        ast::ValueDatum::String(value) => types::NewStringDatum(value.clone()),
        ast::ValueDatum::Bytes(value) => types::NewBytesDatum(value.clone()),
        ast::ValueDatum::BitLiteral(value) | ast::ValueDatum::HexLiteral(value) => {
            types::NewBinaryLiteralDatum(types::BinaryLiteral(value.clone()))
        }
    })
}

// Leave implements Visitor interface.
// Leave 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// AST 访问 Leave：按节点类型落到具体 ToExpression 逻辑。
pub fn Leave(
    rewriter: &mut expressionRewriter<'_>,
    origin_node: &ast::ExprNode,
) -> (ast::ExprNode, bool) {
    let input_node = rewriter
        .preprocess
        .map_or_else(|| origin_node.clone(), |preprocess| preprocess(origin_node));
    let result = (|| -> Result<(), errors::Error> {
        if let Some(error) = rewriter.err.take() {
            return Err(error);
        }
        match &input_node.Kind {
            ast::ExprKind::AggregateFunction { .. }
            | ast::ExprKind::Parentheses(_)
            | ast::ExprKind::Subquery { .. }
            | ast::ExprKind::ExistsSubquery { .. }
            | ast::ExprKind::CompareSubquery { .. }
            | ast::ExprKind::InSubquery { .. }
            | ast::ExprKind::WindowFunction { .. } => {}
            ast::ExprKind::Value(value) => {
                let mut return_type = types::FieldType::DeepCopy(Some(&value.Type))
                    .expect("AST value field type is always present");
                if matches!(value.Datum, ast::ValueDatum::Null) {
                    return_type.DelFlag(mysql::NotNullFlag);
                } else {
                    return_type.AddFlag(mysql::NotNullFlag);
                }
                let mut constant =
                    expression::Constant::with_type(datumFromAstValue(&value.Datum)?, return_type);
                initConstantRepertoire(rewriter.sctx.GetEvalCtx(), &mut constant);
                adjustUTF8MB4Collation(
                    rewriter,
                    constant
                        .RetType
                        .as_mut()
                        .expect("AST constant return type is always present"),
                );
                ctxStackAppend(rewriter, Box::new(constant), types::EmptyName.clone());
            }
            ast::ExprKind::IntroducedValue {
                Value,
                Charset,
                Collation,
                Binary,
            } => {
                let mut node = ast::ExprNode::StringValue(Value.clone(), Charset, Collation);
                let ast::ExprKind::Value(value) = &mut node.Kind else {
                    unreachable!()
                };
                value.Type.AddFlag(mysql::UnderScoreCharsetFlag);
                if *Binary {
                    value.Type.AddFlag(mysql::BinaryFlag);
                }
                let mut constant = expression::Constant::with_type(
                    datumFromAstValue(&value.Datum)?,
                    value.Type.DeepCopy(),
                );
                initConstantRepertoire(rewriter.sctx.GetEvalCtx(), &mut constant);
                ctxStackAppend(rewriter, Box::new(constant), types::EmptyName.clone());
            }
            ast::ExprKind::ParamMarker { .. } => {
                let order = rewriter.nextParamOrder;
                rewriter.nextParamOrder += 1;
                toParamMarker(rewriter, order);
            }
            ast::ExprKind::Variable { IsSystem, .. } => {
                if *IsSystem {
                    let mut plan_context = rewriter.planCtx.take().ok_or_else(|| {
                        missingPlanCtxError(
                            &input_node,
                            "accessing system variable requires plan context",
                        )
                    })?;
                    rewriteSystemVariable(rewriter, &mut plan_context, &input_node);
                    rewriter.planCtx = Some(plan_context);
                } else {
                    rewriteUserVariable(rewriter, &input_node);
                }
            }
            ast::ExprKind::Function { FnName, .. } => {
                if FnName.L == ast::Grouping {
                    let mut plan_context = rewriter
                        .planCtx
                        .take()
                        .ok_or_else(|| errors::New("grouping function requires plan context"))?;
                    funcCallToExpressionWithPlanCtx(rewriter, &mut plan_context, &input_node);
                    rewriter.planCtx = Some(plan_context);
                } else {
                    if expression::TryFoldFunctions.contains_key(FnName.L.as_str()) {
                        rewriter.tryFoldCounter -= 1;
                    }
                    funcCallToExpression(rewriter, &input_node);
                    if expression::DisableFoldFunctions.contains_key(FnName.L.as_str()) {
                        rewriter.disableFoldCounter -= 1;
                    }
                }
            }
            ast::ExprKind::TableName(_) => toTable(rewriter, &input_node),
            ast::ExprKind::Column(_) => toColumn(rewriter, &input_node),
            ast::ExprKind::Unary { .. } => unaryOpToExpression(rewriter, &input_node),
            ast::ExprKind::Binary { Op, .. } => {
                if Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or") {
                    rewriter.tryFoldCounter -= 1;
                }
                binaryOpToExpression(rewriter, &input_node);
            }
            ast::ExprKind::Between { .. } => betweenToExpression(rewriter, &input_node),
            ast::ExprKind::Case { .. } => {
                if expression::TryFoldFunctions.contains_key("case") {
                    rewriter.tryFoldCounter -= 1;
                }
                caseToExpression(rewriter, &input_node);
                if expression::DisableFoldFunctions.contains_key("case") {
                    rewriter.disableFoldCounter -= 1;
                }
            }
            ast::ExprKind::Cast {
                Tp,
                ExplicitCharSet,
                ..
            } => {
                if Tp.IsArray() && !rewriter.allowBuildCastArray {
                    return Err(expression::ErrNotSupportedYet.GenWithStackByArgs(
                        "Use of CAST( .. AS .. ARRAY) outside of functional index in CREATE(non-SELECT)/ALTER TABLE or in general expressions",
                    ));
                }
                let argument = rewriter.ctxStack.last().unwrap().CloneExpr();
                expression::CheckArgsNotMultiColumnRow(argument.as_ref())?;
                checkTimePrecision(rewriter, Tp)?;
                let mut cast = expression::BuildCastFunctionWithCheck(
                    rewriter.sctx,
                    argument,
                    Tp.DeepCopy(),
                    false,
                    *ExplicitCharSet,
                )?;
                if Tp.EvalType() == types::ETString {
                    cast.SetCoercibility(expression::CoercibilityImplicit);
                    cast.SetRepertoire(if Tp.GetCharset() == charset::CharsetASCII {
                        expression::ASCII
                    } else {
                        expression::UNICODE
                    });
                } else {
                    cast.SetCoercibility(expression::CoercibilityNumeric);
                    cast.SetRepertoire(expression::ASCII);
                }
                let last = rewriter.ctxStack.len() - 1;
                rewriter.ctxStack[last] = cast;
                rewriter.ctxNameStk[last] = types::EmptyName.clone();
            }
            ast::ExprKind::JSONSumCrc32 { Tp, .. } => {
                let argument = rewriter.ctxStack.last().unwrap().CloneExpr();
                let mut function = expression::BuildJSONSumCrc32FunctionWithCheck(
                    rewriter.sctx,
                    argument,
                    Tp.DeepCopy(),
                )?;
                function.SetCoercibility(expression::CoercibilityNumeric);
                function.SetRepertoire(expression::ASCII);
                let last = rewriter.ctxStack.len() - 1;
                rewriter.ctxStack[last] = function;
                rewriter.ctxNameStk[last] = types::EmptyName.clone();
            }
            ast::ExprKind::Like { .. } => patternLikeOrIlikeToExpression(rewriter, &input_node),
            ast::ExprKind::Regexp { .. } => regexpToScalarFunc(rewriter, &input_node),
            ast::ExprKind::Row(_) => rowToScalarFunc(rewriter, &input_node),
            ast::ExprKind::InList {
                List, Not, Type, ..
            } => {
                inToExpression(rewriter, List.len(), *Not, Type.DeepCopy());
            }
            ast::ExprKind::IsNull { .. } => isNullToExpression(rewriter, &input_node),
            ast::ExprKind::IsTruth { .. } => isTrueToScalarFunc(rewriter, &input_node),
            ast::ExprKind::DefaultValue | ast::ExprKind::NamedDefault(_) => {
                if let Some(mut plan_context) = rewriter.planCtx.take() {
                    evalDefaultExprWithPlanCtx(rewriter, &mut plan_context, &input_node);
                    rewriter.planCtx = Some(plan_context);
                } else if let Some(table) = rewriter.sourceTable {
                    evalDefaultExprForTable(rewriter, &input_node, table);
                } else {
                    return Err(errors::New(
                        "Unsupported expr *ast.DefaultExpr when source table not provided",
                    ));
                }
            }
            ast::ExprKind::TrimDirection(direction) => ctxStackAppend(
                rewriter,
                Box::new(expression::Constant::with_type(
                    types::NewIntDatum(*direction as i64),
                    *types::NewFieldType(mysql::TypeTiny),
                )),
                types::EmptyName.clone(),
            ),
            ast::ExprKind::TimeUnit(unit) => ctxStackAppend(
                rewriter,
                Box::new(expression::Constant::with_type(
                    types::NewStringDatum(timeUnitName(*unit).to_owned()),
                    *types::NewFieldType(mysql::TypeVarchar),
                )),
                types::EmptyName.clone(),
            ),
            ast::ExprKind::GetFormatSelector(selector) => ctxStackAppend(
                rewriter,
                Box::new(expression::Constant::with_type(
                    types::NewStringDatum(getFormatSelectorName(*selector).to_owned()),
                    *types::NewFieldType(mysql::TypeVarchar),
                )),
                types::EmptyName.clone(),
            ),
            ast::ExprKind::Collate { Collation, .. } => {
                let index = rewriter.ctxStack.len() - 1;
                let argument = rewriter.ctxStack[index].CloneExpr();
                rewriter.ctxStack[index] = expression::SetCollationToExpression(
                    rewriter.sctx,
                    argument,
                    Collation,
                    rewriter.useNewCollate,
                )?;
            }
            ast::ExprKind::MatchAgainst { .. } => matchAgainstToExpression(rewriter, &input_node),
            ast::ExprKind::MaxValue => {
                return Err(errors::New("MAXVALUE is not a scalar expression"));
            }
        }
        rewriter.err.take().map_or(Ok(()), Err)
    })();
    rewriter.astNodeStack.pop();
    if let Err(error) = result {
        rewriter.err = Some(error);
        return (origin_node.clone(), false);
    }
    (origin_node.clone(), true)
}

// newFunctionWithInit chooses which expression.NewFunctionImpl() will be used.
// newFunctionWithInit 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 创建带初始化参数的标量函数表达式。
pub fn newFunctionWithInit(
    rewriter: &mut expressionRewriter<'_>,
    function_name: &str,
    return_type: types::FieldType,
    initializer: Option<expression::ScalarFunctionCallBack>,
    arguments: Vec<expression::ExprBox>,
) -> Result<expression::ExprBox, errors::Error> {
    let result = if let Some(initializer) = initializer {
        expression::NewFunctionWithInit(
            rewriter.sctx,
            function_name,
            return_type,
            initializer,
            arguments,
        )?
    } else if rewriter.disableFoldCounter > 0 {
        expression::NewFunctionBase(rewriter.sctx, function_name, return_type, arguments)?
    } else if rewriter.tryFoldCounter > 0 {
        expression::NewFunctionTryFold(rewriter.sctx, function_name, return_type, arguments)?
    } else {
        expression::NewFunction(rewriter.sctx, function_name, return_type, arguments)?
    };
    if let Some(scalar_function) = result.as_any().downcast_ref::<expression::ScalarFunction>() {
        if let Some(plan_context) = rewriter.planCtx.as_ref() {
            plan_context
                .builder
                .ctx
                .BuiltinFunctionUsageInc(&scalar_function.Function.PbCode().to_string());
        }
    }
    Ok(result)
}

// newFunction is being redirected to newFunctionWithInit.
// newFunction 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 创建普通标量函数表达式。
pub fn newFunction(
    rewriter: &mut expressionRewriter<'_>,
    function_name: &str,
    return_type: types::FieldType,
    arguments: Vec<expression::ExprBox>,
) -> Result<expression::ExprBox, errors::Error> {
    newFunctionWithInit(rewriter, function_name, return_type, None, arguments)
}

// checkTimePrecision 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 校验时间类型精度是否合法。
pub fn checkTimePrecision(
    _rewriter: &expressionRewriter<'_>,
    field_type: &types::FieldType,
) -> Result<(), errors::Error> {
    if field_type.EvalType() == types::ETDuration
        && field_type.GetDecimal() > types::MaxFsp as isize
    {
        return Err(plannererrors::ErrTooBigPrecision
            .GenWithStackByArgs(&[
                field_type.GetDecimal().into(),
                "CAST".into(),
                types::MaxFsp.into(),
            ])
            .into());
    }
    Ok(())
}

// useCache 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 当前改写是否允许使用计划缓存相关路径。
pub fn useCache(rewriter: &expressionRewriter<'_>) -> bool {
    rewriter.sctx.IsUseCache()
}

// rewriteUserVariable 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 改写用户变量读写表达式。
pub fn rewriteUserVariable(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Variable { Name, Value, .. } = &node.Kind else {
        rewriter.err = Some(errors::New("user-variable rewriter received another node"));
        return;
    };
    let result = (|| -> Result<(), errors::Error> {
        let stack_length = rewriter.ctxStack.len();
        let name = Name.to_lowercase();
        let evaluation_context = rewriter.sctx.GetEvalCtx();
        if Value.is_some() {
            if !evaluation_context
                .GetOptionalPropSet()
                .Contains(exprctx::OptPropSessionVars)
            {
                return Err(errors::New(format!(
                    "rewriting user variable requires '{}' in evalCtx",
                    exprctx::OptPropSessionVars
                )));
            }
            let session_variables = expropt::SessionVarsPropReader
                .get_session_vars(evaluation_context)
                .map_err(|error| errors::New(error.to_string()))?;
            let return_type = rewriter.ctxStack[stack_length - 1]
                .GetType(evaluation_context)
                .DeepCopy();
            let name_constant = expression::DatumToConstant(
                types::NewStringDatum(name.clone()),
                mysql::TypeString,
                0,
            );
            let value = rewriter.ctxStack[stack_length - 1].CloneExpr();
            rewriter.ctxStack[stack_length - 1] = newFunction(
                rewriter,
                ast::SetVar,
                return_type.DeepCopy(),
                vec![name_constant, value],
            )?;
            rewriter.ctxNameStk[stack_length - 1] = types::EmptyName.clone();
            session_variables.SetUserVarType(name, return_type);
            return Ok(());
        }
        let mut return_type = evaluation_context
            .GetUserVarsReader()
            .GetUserVarType(&name)
            .unwrap_or_else(|| {
                let mut field_type = *types::NewFieldType(mysql::TypeVarString);
                field_type.SetFlen(mysql::MaxFieldVarCharLength as isize);
                field_type
            });
        let name_constant =
            expression::DatumToConstant(types::NewStringDatum(name), mysql::TypeString, 0);
        let mut function = newFunction(
            rewriter,
            ast::GetVar,
            return_type.DeepCopy(),
            vec![name_constant],
        )?;
        function.SetCoercibility(expression::CoercibilityImplicit);
        ctxStackAppend(rewriter, function, types::EmptyName.clone());
        Ok(())
    })();
    if let Err(error) = result {
        rewriter.err = Some(error);
    }
}

// rewriteSystemVariable 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 改写系统变量引用。
pub fn rewriteSystemVariable(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) {
    let ast::ExprKind::Variable {
        Name,
        IsGlobal,
        IsInstance,
        IsSystem: true,
        ExplicitScope,
        ..
    } = &node.Kind
    else {
        rewriter.err = Some(errors::New(
            "system-variable rewriter received another node",
        ));
        return;
    };
    let result = (|| -> Result<(), errors::Error> {
        let name = Name.to_lowercase();
        let system_variable = variable::GetSysVar(&name).ok_or_else(|| {
            variable::CheckSysVarIsRemoved(&name)
                .err()
                .map(errors::New)
                .unwrap_or_else(|| {
                    errors::New(variable::ErrUnknownSystemVar.format(&[name.as_str()]))
                })
        })?;
        if system_variable.IsNoop && !vardef::EnableNoopVariables.Load() {
            plan_context
                .builder
                .ctx
                .GetSessionVars()
                .StmtCtx
                .AppendWarning(
                    plannererrors::ErrGettingNoopVariable
                        .FastGenByArgs(&[system_variable.Name.clone().into()]),
                );
        }
        if sem::IsEnabled() && sem::IsInvisibleSysVar(&system_variable.Name) {
            let error = plannererrors::ErrSpecificAccessDenied
                .GenWithStackByArgs(&["RESTRICTED_VARIABLES_ADMIN".into()]);
            plan_context.builder.visitInfo = appendDynamicVisitInfo(
                std::mem::take(&mut plan_context.builder.visitInfo),
                vec!["RESTRICTED_VARIABLES_ADMIN".to_owned()],
                false,
                error.into(),
            );
        }
        if *ExplicitScope && !system_variable.HasNoneScope() {
            if *IsGlobal
                && !(system_variable.HasGlobalScope() || system_variable.HasInstanceScope())
            {
                return Err(errors::New(
                    variable::ErrIncorrectScope.format(&[name.as_str(), "SESSION"]),
                ));
            }
            if *IsInstance && !system_variable.HasInstanceScope() {
                return Err(errors::New(
                    variable::ErrIncorrectScope.format(&[name.as_str(), "SESSION or GLOBAL"]),
                ));
            }
            if !*IsGlobal && !*IsInstance {
                if !system_variable.HasSessionScope() {
                    return Err(errors::New(
                        variable::ErrIncorrectScope.format(&[name.as_str(), "GLOBAL"]),
                    ));
                }
                if system_variable.InternalSessionVariable {
                    return Err(errors::New(
                        variable::ErrUnknownSystemVar.format(&[name.as_str()]),
                    ));
                }
            }
        }
        let session_variables = plan_context.builder.ctx.GetSessionVars();
        let value = if system_variable.HasNoneScope() {
            system_variable.Value.clone()
        } else if *IsGlobal || *IsInstance {
            session_variables
                .GetGlobalSystemVar(rewriter.ctx.clone(), &name)
                .map_err(errors::New)?
        } else {
            session_variables
                .GetSessionOrGlobalSystemVar(rewriter.ctx.clone(), &name)
                .map_err(errors::New)?
        };
        let (native_value, native_type, native_flag) = system_variable.GetNativeValType(&value);
        let native_value = match native_value {
            variable::Datum::Int(value) => types::NewIntDatum(value),
            variable::Datum::Uint(value) => types::NewUintDatum(value),
            variable::Datum::String(value) => types::NewStringDatum(value),
        };
        let mut expression =
            expression::DatumToConstant(native_value, native_type, native_flag.into());
        match native_type {
            mysql::TypeVarString => {
                let connection_charset = session_variables
                    .GetSystemVar(vardef::CharacterSetConnection)
                    .ok_or_else(|| errors::New("character_set_connection is not initialized"))?;
                let connection_collation = session_variables
                    .GetSystemVar(vardef::CollationConnection)
                    .ok_or_else(|| errors::New("collation_connection is not initialized"))?;
                expression.GetTypeMut().SetCharset(connection_charset);
                expression.GetTypeMut().SetCollate(connection_collation);
            }
            mysql::TypeLong | mysql::TypeLonglong => {
                expression
                    .GetTypeMut()
                    .SetCharset(charset::CharsetBin.to_owned());
                expression
                    .GetTypeMut()
                    .SetCollate(charset::CollationBin.to_owned());
            }
            unsupported => {
                return Err(errors::New(format!(
                    "Not supported type({unsupported:x}) in GetNativeValType() function"
                )));
            }
        }
        ctxStackAppend(rewriter, expression, types::EmptyName.clone());
        Ok(())
    })();
    if let Err(error) = result {
        rewriter.err = Some(error);
    }
}

// unaryOpToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 一元运算符 AST → expression。
pub fn unaryOpToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Unary { Op, .. } = &node.Kind else {
        rewriter.err = Some(errors::New("unary rewriter received another node"));
        return;
    };
    if Op == "+" {
        return;
    }
    let function_name = match Op.as_str() {
        "-" => ast::UnaryMinus,
        "~" => ast::BitNeg,
        "!" | "NOT" | "not" => ast::UnaryNot,
        _ => {
            rewriter.err = Some(errors::New(format!("Unknown Unary Op {Op}")));
            return;
        }
    };
    let stack_length = rewriter.ctxStack.len();
    let argument = rewriter.ctxStack[stack_length - 1].CloneExpr();
    if expression::GetRowLen(argument.as_ref()) != 1 {
        rewriter.err = Some(expression::ErrOperandColumns.GenWithStackByArgs(1));
        return;
    }
    let return_type = argument.GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
    match newFunction(rewriter, function_name, return_type, vec![argument]) {
        Ok(function) => {
            rewriter.ctxStack[stack_length - 1] = function;
            rewriter.ctxNameStk[stack_length - 1] = types::EmptyName.clone();
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// binaryOpToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 二元运算符 AST → expression。
pub fn binaryOpToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Binary { Op, .. } = &node.Kind else {
        rewriter.err = Some(errors::New("binary rewriter received another node"));
        return;
    };
    let stack_length = rewriter.ctxStack.len();
    let left = rewriter.ctxStack[stack_length - 2].CloneExpr();
    let right = rewriter.ctxStack[stack_length - 1].CloneExpr();
    let function_name = match Op.to_ascii_lowercase().as_str() {
        "=" => ast::EQ,
        "!=" | "<>" => ast::NE,
        "<=>" => ast::NullEQ,
        ">" => ast::GT,
        ">=" => ast::GE,
        "<" => ast::LT,
        "<=" => ast::LE,
        "and" | "&&" => "and",
        "or" | "||" => "or",
        "xor" => "xor",
        "+" => "plus",
        "-" => "minus",
        "*" => "mul",
        "/" => "div",
        "div" => "intdiv",
        "%" | "mod" => "mod",
        "&" => "bitand",
        "|" => "bitor",
        "^" => "bitxor",
        _ => Op.as_str(),
    };
    let result = if matches!(
        function_name,
        ast::EQ | ast::NE | ast::NullEQ | ast::GT | ast::GE | ast::LT | ast::LE
    ) {
        constructBinaryOpFunction(rewriter, left, right, function_name)
    } else if expression::GetRowLen(left.as_ref()) != 1
        || expression::GetRowLen(right.as_ref()) != 1
    {
        Err(expression::ErrOperandColumns.GenWithStackByArgs(1))
    } else {
        newFunction(
            rewriter,
            function_name,
            *types::NewFieldType(mysql::TypeUnspecified),
            vec![left, right],
        )
    };
    match result {
        Ok(function) => {
            ctxStackPop(rewriter, 2);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// notToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// NOT 谓词 → expression。
pub fn notToExpression(
    rewriter: &mut expressionRewriter<'_>,
    has_not: bool,
    operator: &str,
    return_type: types::FieldType,
    arguments: Vec<expression::ExprBox>,
) -> Result<expression::ExprBox, errors::Error> {
    let function = newFunction(rewriter, operator, return_type.DeepCopy(), arguments)?;
    if has_not {
        newFunction(rewriter, ast::UnaryNot, return_type, vec![function])
    } else {
        Ok(function)
    }
}

// isNullToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// IS NULL / IS NOT NULL → expression。
pub fn isNullToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::IsNull { Not, .. } = &node.Kind else {
        return;
    };
    let argument = rewriter.ctxStack.last().unwrap().CloneExpr();
    if expression::GetRowLen(argument.as_ref()) != 1 {
        rewriter.err = Some(expression::ErrOperandColumns.GenWithStackByArgs(1));
        return;
    }
    match notToExpression(
        rewriter,
        *Not,
        ast::IsNull,
        ast::ExprNode::PredicateType(),
        vec![argument],
    ) {
        Ok(function) => {
            ctxStackPop(rewriter, 1);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// positionToScalarFunc 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// POSITION 函数 → 标量函数。
pub fn positionToScalarFunc(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    mut position: i64,
    parameterized: bool,
) {
    let mut display = position.to_string();
    if parameterized {
        display = "?".to_owned();
        let value = rewriter.ctxStack.last().unwrap();
        match expression::GetIntFromConstant(rewriter.sctx.GetEvalCtx(), value.as_ref()) {
            Ok((_, true)) => return,
            Ok((number, false)) => {
                position = number;
                ctxStackPop(rewriter, 1);
            }
            Err(error) => {
                rewriter.err = Some(error);
                return;
            }
        }
    }
    let valid = position > 0
        && position as usize <= rewriter.schema.as_ref().unwrap().Len()
        && !rewriter.schema.as_ref().unwrap().Columns[position as usize - 1].IsHidden;
    if valid {
        ctxStackAppend(
            rewriter,
            Box::new(rewriter.schema.as_ref().unwrap().Columns[position as usize - 1].Clone()),
            rewriter.names.0[position as usize - 1].clone().unwrap(),
        );
    } else {
        rewriter.err = Some(
            plannererrors::ErrUnknownColumn
                .GenWithStackByArgs(&[
                    display.into(),
                    clauseMsg[plan_context.builder.curClause].into(),
                ])
                .into(),
        );
    }
}

// isTrueToScalarFunc 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// IS TRUE / IS FALSE → 标量函数。
pub fn isTrueToScalarFunc(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::IsTruth { Not, True, .. } = &node.Kind else {
        return;
    };
    let argument = rewriter.ctxStack.last().unwrap().CloneExpr();
    if expression::GetRowLen(argument.as_ref()) != 1 {
        rewriter.err = Some(expression::ErrOperandColumns.GenWithStackByArgs(1));
        return;
    }
    let operator = if *True {
        ast::IsTruthWithoutNull
    } else {
        ast::IsFalsity
    };
    match notToExpression(
        rewriter,
        *Not,
        operator,
        ast::ExprNode::PredicateType(),
        vec![argument],
    ) {
        Ok(function) => {
            ctxStackPop(rewriter, 1);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// inToExpression converts in expression to a scalar function. The argument lLen means the length of in list.
// The argument not means if the expression is not in. The tp stands for the expression type, which is always bool.
// a in (b, c, d) will be rewritten as `(a = b) or (a = c) or (a = d)`.
// inToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// IN 列表谓词 → expression。
pub fn inToExpression(
    rewriter: &mut expressionRewriter<'_>,
    list_length: usize,
    negated: bool,
    return_type: types::FieldType,
) {
    let result = (|| -> Result<expression::ExprBox, errors::Error> {
        let stack_length = rewriter.ctxStack.len();
        let left_index = stack_length - list_length - 1;
        let row_length = expression::GetRowLen(rewriter.ctxStack[left_index].as_ref());
        for item in &rewriter.ctxStack[stack_length - list_length..] {
            if expression::GetRowLen(item.as_ref()) != row_length {
                return Err(expression::ErrOperandColumns.GenWithStackByArgs(row_length));
            }
        }
        let left_type = rewriter.ctxStack[left_index]
            .GetType(rewriter.sctx.GetEvalCtx())
            .DeepCopy();
        let left_eval_type = left_type.EvalType();
        if left_type.GetType() == mysql::TypeNull {
            return Ok(Box::new(expression::NewNull()));
        }

        if left_eval_type == types::ETInt {
            for index in stack_length - list_length..stack_length {
                let Some(mut constant) = rewriter.ctxStack[index]
                    .as_any()
                    .downcast_ref::<expression::Constant>()
                    .cloned()
                else {
                    continue;
                };
                if expression::MaybeOverOptimized4PlanCache(rewriter.sctx, &constant) {
                    if constant
                        .RetType
                        .as_ref()
                        .expect("a constant always has a return type")
                        .EvalType()
                        == types::ETInt
                    {
                        continue;
                    }
                    let reason = format!(
                        "'{}' may be converted to INT",
                        constant
                            .StringWithCtx(rewriter.sctx.GetEvalCtx(), errors::RedactLogDisable,)
                    );
                    rewriter.sctx.SetSkipPlanCache(&reason);
                    let mut mutable_constant = Box::new(constant.clone()) as expression::ExprBox;
                    expression::RemoveMutableConst(
                        rewriter.sctx,
                        std::slice::from_mut(&mut mutable_constant),
                    )?;
                    constant = mutable_constant
                        .as_any()
                        .downcast_ref::<expression::Constant>()
                        .cloned()
                        .expect("removing mutable state preserves constant type");
                }
                let (refined, exceptional) = expression::RefineComparedConstant(
                    rewriter.sctx,
                    left_type.DeepCopy(),
                    &constant,
                    opcode::Op::EQ,
                );
                if !exceptional {
                    rewriter.ctxStack[index] = refined;
                }
            }
        }
        let left = rewriter.ctxStack[left_index].CloneExpr();
        let all_same_type =
            rewriter.ctxStack[stack_length - list_length..]
                .iter()
                .all(|argument| {
                    argument.GetType(rewriter.sctx.GetEvalCtx()).GetType() == mysql::TypeNull
                        || expression::GetAccurateCmpType(
                            rewriter.sctx.GetEvalCtx(),
                            left.as_ref(),
                            argument.as_ref(),
                        ) == left_eval_type
                });
        if all_same_type && row_length == 1 && list_length > 1 {
            let arguments = rewriter.ctxStack[left_index..]
                .iter()
                .map(|argument| argument.CloneExpr())
                .collect();
            return notToExpression(rewriter, negated, ast::In, return_type, arguments);
        }
        let arguments = rewriter.ctxStack[left_index..]
            .iter()
            .map(|argument| argument.CloneExpr())
            .collect::<Vec<_>>();
        let collation = deriveCollationForIn(rewriter, row_length, list_length, &arguments)?;
        castCollationForIn(
            rewriter,
            row_length,
            list_length,
            stack_length,
            collation.as_ref(),
        )?;
        let mut equalities = Vec::with_capacity(list_length);
        for index in stack_length - list_length..stack_length {
            equalities.push(constructBinaryOpFunction(
                rewriter,
                left.CloneExpr(),
                rewriter.ctxStack[index].CloneExpr(),
                ast::EQ,
            )?);
        }
        let function = composeDNF(rewriter.sctx, equalities);
        if negated {
            newFunction(rewriter, ast::UnaryNot, return_type, vec![function])
        } else {
            Ok(function)
        }
    })();
    match result {
        Ok(function) => {
            ctxStackPop(rewriter, list_length + 1);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// deriveCollationForIn derives collation for in expression.
// We don't handle the cases if the element is a tuple, such as (a, b, c) in ((x1, y1, z1), (x2, y2, z2)).
// deriveCollationForIn 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 推导 IN 比较的排序规则。
pub fn deriveCollationForIn(
    rewriter: &mut expressionRewriter<'_>,
    column_length: usize,
    _element_count: usize,
    arguments: &[expression::ExprBox],
) -> Result<Option<expression::ExprCollation>, errors::Error> {
    if column_length == 1 {
        expression::CheckAndDeriveCollationFromExprs(
            rewriter.sctx,
            "IN",
            types::ETInt,
            &arguments
                .iter()
                .map(|argument| argument.as_ref())
                .collect::<Vec<_>>(),
        )
        .map(Some)
    } else {
        Ok(None)
    }
}

// castCollationForIn casts collation info for arguments in the `in clause` to make sure the used collation is correct after we
// rewrite it to equal expression.
// castCollationForIn 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 为 IN 两侧操作数施加排序规则转换。
pub fn castCollationForIn(
    rewriter: &mut expressionRewriter<'_>,
    column_length: usize,
    element_count: usize,
    stack_length: usize,
    collation: Option<&expression::ExprCollation>,
) -> Result<(), errors::Error> {
    let Some(collation) = collation else {
        return Ok(());
    };
    if column_length != 1 || !rewriter.useNewCollate {
        return Ok(());
    }
    let left_index = stack_length - element_count - 1;
    for index in stack_length - element_count..stack_length {
        let expression_type = rewriter.ctxStack[index]
            .GetType(rewriter.sctx.GetEvalCtx())
            .DeepCopy();
        if expression_type.EvalType() != types::ETString {
            continue;
        }
        if rewriter.ctxStack[index]
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
            .is_some_and(|function| function.FuncName.L.as_str() == ast::RowFunc)
            || expression_type.GetCollate() == collation.Collation
        {
            continue;
        }
        let mut target_type = expression_type.DeepCopy();
        if expression_type.Hybrid() {
            if expression::GetAccurateCmpType(
                rewriter.sctx.GetEvalCtx(),
                rewriter.ctxStack[left_index].as_ref(),
                rewriter.ctxStack[index].as_ref(),
            ) != types::ETString
            {
                continue;
            }
            target_type = *types::NewFieldType(mysql::TypeVarString);
        } else if collation.Charset == charset::CharsetBin {
            target_type.SetType(mysql::TypeVarString);
        }
        target_type.SetCharset(collation.Charset.clone());
        target_type.SetCollate(collation.Collation.clone());
        let source = rewriter.ctxStack[index].CloneExpr();
        let mut cast = expression::BuildCastFunction(rewriter.sctx, &source, &target_type);
        cast.SetCoercibility(expression::CoercibilityExplicit);
        rewriter.ctxStack[index] = cast;
    }
    Ok(())
}

// caseToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// CASE WHEN → expression。
pub fn caseToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Case {
        Value,
        WhenClauses,
        ElseClause,
    } = &node.Kind
    else {
        return;
    };
    let result = (|| -> Result<(expression::ExprBox, usize), errors::Error> {
        let stack_length = rewriter.ctxStack.len();
        let mut argument_length = 2 * WhenClauses.len() + usize::from(ElseClause.is_some());
        expression::CheckArgsNotMultiColumnRow(
            &rewriter.ctxStack[stack_length - argument_length..],
        )?;
        let arguments = if Value.is_some() {
            let value = rewriter.ctxStack[stack_length - argument_length - 1].CloneExpr();
            let mut arguments = Vec::with_capacity(argument_length);
            let pair_end = stack_length - usize::from(ElseClause.is_some());
            for index in (stack_length - argument_length..pair_end).step_by(2) {
                arguments.push(newFunction(
                    rewriter,
                    ast::EQ,
                    *types::NewFieldType(mysql::TypeTiny),
                    vec![value.CloneExpr(), rewriter.ctxStack[index].CloneExpr()],
                )?);
                arguments.push(rewriter.ctxStack[index + 1].CloneExpr());
            }
            if ElseClause.is_some() {
                arguments.push(rewriter.ctxStack[stack_length - 1].CloneExpr());
            }
            argument_length += 1;
            arguments
        } else {
            rewriter.ctxStack[stack_length - argument_length..]
                .iter()
                .map(|argument| argument.CloneExpr())
                .collect()
        };
        let function = newFunction(
            rewriter,
            ast::Case,
            *types::NewFieldType(mysql::TypeUnspecified),
            arguments,
        )?;
        Ok((function, argument_length))
    })();
    match result {
        Ok((function, argument_length)) => {
            ctxStackPop(rewriter, argument_length);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// patternLikeOrIlikeToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// LIKE / ILIKE 模式匹配 → expression。
pub fn patternLikeOrIlikeToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Like {
        Not,
        Escape,
        Explicit,
        IsLike,
        Type,
        ..
    } = &node.Kind
    else {
        return;
    };
    let result = (|| -> Result<expression::ExprBox, errors::Error> {
        let length = rewriter.ctxStack.len();
        expression::CheckArgsNotMultiColumnRow(&vec![
            rewriter.ctxStack[length - 2].CloneExpr(),
            rewriter.ctxStack[length - 1].CloneExpr(),
        ])?;
        let mut escape = Escape.chars().next().unwrap_or('\\') as i64;
        let evaluation_context = rewriter.sctx.GetEvalCtx();
        if !*Explicit
            && evaluation_context.SQLMode().HasNoBackslashEscapesMode()
            && evaluation_context
                .GetOptionalPropSet()
                .Contains(exprctx::OptPropSessionVars)
            && expropt::SessionVarsPropReader
                .get_session_vars(evaluation_context)
                .map_err(|error| errors::New(error.to_string()))?
                .EnableNoBackslashEscapesInLike
        {
            escape = 0;
        }
        if !rewriter.useNewCollate {
            if let Some(pattern) = rewriter.ctxStack[length - 1]
                .as_any()
                .downcast_ref::<expression::Constant>()
            {
                let (pattern, is_null) =
                    pattern.EvalString(evaluation_context, chunk::Row::default())?;
                if !is_null {
                    let (compiled, kinds) = stringutil::CompilePattern(&pattern, escape as u8);
                    if stringutil::IsExactMatch(&kinds)
                        && rewriter.ctxStack[length - 2]
                            .GetType(evaluation_context)
                            .EvalType()
                            == types::ETString
                    {
                        let compiled = compiled.into_iter().collect::<String>();
                        let (connection_charset, connection_collation) =
                            rewriter.sctx.GetCharsetInfo();
                        let mut field_type = types::FieldType::default();
                        types_dependency::field::DefaultTypeForValue(
                            Some(&compiled as &dyn std::any::Any),
                            &mut field_type,
                            &connection_charset,
                            &connection_collation,
                        );
                        let constant = Box::new(expression::Constant::with_type(
                            types::NewStringDatum(compiled),
                            field_type,
                        ));
                        return constructBinaryOpFunction(
                            rewriter,
                            rewriter.ctxStack[length - 2].CloneExpr(),
                            constant,
                            if *Not { ast::NE } else { ast::EQ },
                        );
                    }
                }
            }
        }
        notToExpression(
            rewriter,
            *Not,
            if *IsLike { ast::Like } else { ast::Ilike },
            Type.DeepCopy(),
            vec![
                rewriter.ctxStack[length - 2].CloneExpr(),
                rewriter.ctxStack[length - 1].CloneExpr(),
                Box::new(expression::Constant::with_type(
                    types::NewIntDatum(escape),
                    *types::NewFieldType(mysql::TypeLonglong),
                )),
            ],
        )
    })();
    match result {
        Ok(function) => {
            ctxStackPop(rewriter, 2);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// matchAgainstToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// MATCH AGAINST 全文检索 → expression。
pub fn matchAgainstToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::MatchAgainst {
        ColumnNames,
        Modifier,
        ..
    } = &node.Kind
    else {
        return;
    };
    // Both the column expressions and Against expression have been visited
    // and pushed onto the ctxStack. The stack layout is:
    // [..., col1, col2, ..., colN, against]
    let column_count = ColumnNames.len();
    let stack_length = rewriter.ctxStack.len();
    if stack_length < column_count + 1 {
        rewriter.err = Some(errors::New(format!(
            "Unexpected stack length for MatchAgainst: {stack_length}"
        )));
        return;
    }

    // Default behavior (Alt-disabled or Alt-enabled round 1) is to emit the
    // native FTSMysqlMatchAgainst builtin. The alternative-rounds driver flips
    // AlternativeLogicalPlanFTSLikeFallback to true and re-runs the build
    // only when round 1 reported a direct-boolean-context MATCH that the
    // native builtin cannot serve (no FTS index on a TiFlash replica /
    // modifier not pushdown-supported). In that second pass the rewriter
    // emits ILIKE for direct-boolean-context MATCH only — scoring contexts
    // (SELECT field list / ORDER BY) and scalar predicate positions
    // (IS NULL, comparisons, CASE, arithmetic) need the float relevance
    // score, so they keep using the native builtin and will error at
    // execution if no FTS index exists there.
    // "Direct boolean context" requires that every ancestor up to the
    // WHERE/HAVING/ON root is AND/OR/NOT/parens — see inDirectMatchBooleanContext.
    // Limiting the LIKE rewrite to that subset preserves the 0/1-vs-float
    // distinction: in scalar positions, `MATCH(...) IS NULL`, `MATCH(...) > 0.5`,
    // etc. would silently produce wrong rows if the LIKE rewrite's integer
    // result were substituted for the native float score.
    // Round 1 also has to record viability before committing to native: if
    // any boolean-context MATCH is non-viable, the resulting plan would
    // fail at execution. The rewriter records that on the planBuilder so the
    // round driver can invalidate the plan and trigger the fallback round.
    // Round 1 additionally records that a direct-boolean-context MATCH was
    // seen so the driver runs the LIKE round for cost competition even when
    // round 1's native plan is executable.
    let mut use_like_fallback = false;
    if inDirectMatchBooleanContext(rewriter) {
        let (fallback_round, alternative_round) = rewriter
            .planCtx
            .as_ref()
            .map(|plan_context| {
                let session_variables = plan_context.builder.ctx.GetSessionVars();
                (
                    session_variables.StmtCtx.AlternativeFTSLikeFallback(),
                    session_variables.EnableAlternativeLogicalPlans,
                )
            })
            .unwrap_or((false, false));
        use_like_fallback = fallback_round;
        let native_viable = fallback_round
            || !alternative_round
            || ftsNativeViable(rewriter, *Modifier, column_count, stack_length);
        if alternative_round && !fallback_round {
            let plan_context = rewriter
                .planCtx
                .as_mut()
                .expect("alternative MATCH rewrite requires plan context");
            plan_context.builder.MarkPredicateMatch();
            if !native_viable {
                plan_context.builder.MarkNonViableFTSMatch();
            }
        }
    }

    if use_like_fallback {
        matchAgainstToLike(rewriter, node, column_count, stack_length);
    } else {
        matchAgainstToBuiltin(rewriter, node, column_count, stack_length);
    }
}

// ftsNativeViable reports whether the MATCH(...) currently being rewritten
// can be served on TiFlash by the native FTSMysqlMatchAgainst builtin. It
// walks the resolved column FieldNames sitting on ctxNameStk (stack layout is
// [..., col1, ..., colN, against]) and requires for each column:
//   - the originating table has an available TiFlash replica;
//   - the column is covered by a public FULLTEXT index on that table.
// In addition, the modifier must be the default natural-language mode. Boolean
// mode and WITH QUERY EXPANSION are not encoded in the tipb pushdown today
// (only ScalarFuncSig_FTSMatchExpression is emitted regardless of modifier),
// so a native plan that wins on cost would execute on TiFlash with the modifier
// silently dropped. Until the modifier is carried in the pushdown protocol, we
// treat those modifiers as non-viable for native pushdown.
// ftsNativeViable 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 判断原生全文检索（FTS）下推是否可行。
pub fn ftsNativeViable(
    rewriter: &expressionRewriter<'_>,
    modifier: u8,
    column_count: usize,
    stack_length: usize,
) -> bool {
    if column_count == 0 || !ftsModifierAllowsNativePushdown(modifier) {
        return false;
    }
    let Some(plan_context) = rewriter.planCtx.as_ref() else {
        return false;
    };
    let builder = &plan_context.builder;
    let session_variables = builder.ctx.GetSessionVars();
    let name_start = stack_length - column_count - 1;
    for index in 0..column_count {
        let name = &rewriter.ctxNameStk[name_start + index];
        let mut table_name = name.OrigTblName.clone();
        if table_name.L.is_empty() {
            table_name = name.TblName.clone();
        }
        if table_name.L.is_empty() {
            return false;
        }
        let mut database_name = name.DBName.clone();
        if database_name.L.is_empty() {
            database_name = ast::NewCIStr(&session_variables.CurrentDB());
        }
        let database_name = infoschema::CiString::new(database_name.O.clone());
        let table_name = infoschema::CiString::new(table_name.O.clone());
        let Ok(table_info) = builder.is.ModelTableInfoByName(&database_name, &table_name) else {
            return false;
        };
        let Some(replica) = table_info.TiFlashReplica.as_ref() else {
            return false;
        };
        if !replica.Available || replica.Count == 0 {
            return false;
        }
        let mut column_name = name.OrigColName.clone();
        if column_name.L.is_empty() {
            column_name = name.ColName.clone();
        }
        if !tableHasPublicFTSIndexOnColumn(&table_info, &column_name.L) {
            return false;
        }
    }
    true
}

// ftsModifierAllowsNativePushdown reports whether an FTS modifier can be
// safely served by the native FTSMysqlMatchAgainst builtin pushed to TiFlash.
// Today the tipb pushdown encodes only ScalarFuncSig_FTSMatchExpression and
// drops the modifier, so any non-default modifier would be executed by TiFlash
// as natural-language mode, silently producing wrong results. Only the default
// (natural-language, no query expansion) modifier is currently safe.
// ftsModifierAllowsNativePushdown 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// FTS modifier 是否允许原生下推。
pub fn ftsModifierAllowsNativePushdown(modifier: u8) -> bool {
    const MODE_MASK: u8 = 0x0f;
    const BOOLEAN_MODE: u8 = 1;
    const WITH_QUERY_EXPANSION: u8 = 1 << 4;

    modifier & MODE_MASK != BOOLEAN_MODE && modifier & WITH_QUERY_EXPANSION == 0
}

// tableHasPublicFTSIndexOnColumn reports whether tblInfo has a public FULLTEXT
// index covering the given column. TiDB's FULLTEXT index is single-column, so
// each column in MATCH(...) needs its own FTS index for the native path to be
// viable.
// tableHasPublicFTSIndexOnColumn 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 表在指定列上是否有公开的全文索引。
pub fn tableHasPublicFTSIndexOnColumn(table_info: &model::TableInfo, column_name: &str) -> bool {
    table_info.Indices.iter().any(|index| {
        index.FullTextInfo.is_some()
            && index.IsPublic()
            && index.FindColumnByName(column_name).is_some()
    })
}

// matchAgainstToBuiltin converts MATCH...AGAINST to the FTSMysqlMatchAgainst
// builtin scalar function which can be pushed down to TiFlash for execution
// against a fulltext index.
// matchAgainstToBuiltin 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 将 MATCH AGAINST 转为内置 FTS 函数。
pub fn matchAgainstToBuiltin(
    rewriter: &mut expressionRewriter<'_>,
    node: &ast::ExprNode,
    column_count: usize,
    stack_length: usize,
) {
    let ast::ExprKind::MatchAgainst { Modifier, .. } = &node.Kind else {
        return;
    };
    // Reject non-default modifiers when native is the final plan. The tipb
    // pushdown protocol (see expression/distsql_builtin.go for the explicit
    // note) does not serialize the FTS modifier, so TiFlash would silently
    // execute Boolean-mode / query-expansion searches as natural-language
    // mode. Until the modifier rides through pushdown, refuse to emit
    // native here unless the alt-rounds driver is expected to discard this
    // emission and rebuild via the fts-like-fallback round (which handles
    // Boolean mode correctly via ILIKE; query expansion still errors there
    // with a specific message).
    if !ftsModifierAllowsNativePushdown(*Modifier) && !matchHasLikeFallbackRescue(rewriter) {
        rewriter.err = Some(expression::ErrNotSupportedYet.GenWithStackByArgs(
			"MATCH...AGAINST with this modifier on the native FTS path (modifier is not carried through pushdown to TiFlash)",
		));
        return;
    }

    let mut arguments = Vec::with_capacity(1 + column_count);
    arguments.push(rewriter.ctxStack[stack_length - 1].CloneExpr());
    arguments.extend(
        rewriter.ctxStack[stack_length - column_count - 1..stack_length - 1]
            .iter()
            .map(|column| column.CloneExpr()),
    );
    ctxStackPop(rewriter, column_count + 1);
    let result = (|| -> Result<expression::ExprBox, errors::Error> {
        let function = newFunction(
            rewriter,
            ast::FTSMysqlMatchAgainst,
            *types::NewFieldType(mysql::TypeDouble),
            arguments,
        )?;
        let scalar = function
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
            .ok_or_else(|| errors::New("unexpected expression type for FTS match"))?;
        expression::SetFTSMysqlMatchAgainstModifier(scalar, *Modifier)?;
        Ok(function)
    })();
    match result {
        Ok(function) => ctxStackAppend(rewriter, function, types::EmptyName.clone()),
        Err(error) => rewriter.err = Some(error),
    }
}

// matchAgainstToLike converts MATCH...AGAINST to LIKE predicates as a
// fallback when the native FTS pushdown path is not viable.
// matchAgainstToLike 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 将 MATCH AGAINST 回退为 LIKE 形式。
pub fn matchAgainstToLike(
    rewriter: &mut expressionRewriter<'_>,
    node: &ast::ExprNode,
    column_count: usize,
    stack_length: usize,
) {
    let ast::ExprKind::MatchAgainst { Modifier, .. } = &node.Kind else {
        return;
    };
    let Some(constant) = rewriter.ctxStack[stack_length - 1]
        .as_any()
        .downcast_ref::<expression::Constant>()
    else {
        rewriter.err = Some(
            expression::ErrNotSupportedYet
                .GenWithStackByArgs("MATCH...AGAINST with non-constant search string"),
        );
        return;
    };

    // The LIKE fallback bakes the search value into the produced plan — either
    // as ILIKE pattern constants (non-NULL case) or as a Constant(NULL)
    // short-circuit. A cached plan would reuse the first execution's baked
    // value for later executions, producing wrong results whenever the AGAINST
    // argument is mutable: a `?` parameter marker, a user variable, or another
    // deferred expression. In particular, a NULL first bind would bake a
    // Constant(NULL) plan and reuse it for a later non-NULL bind. Mark the
    // plan non-cacheable here, before the NULL fast-path and before Eval, so
    // the skip applies uniformly across all branches below.
    if expression::MaybeOverOptimized4PlanCache(rewriter.sctx, constant) {
        rewriter.sctx.SetSkipPlanCache(
            "MATCH...AGAINST LIKE fallback bakes a mutable search string into plan constants",
        );
    }

    // Reject non-string matched columns before any value-based branch so the
    // column-type error always wins. In current architecture round 1's
    // matchAgainstToBuiltin → getFunction (builtin_fts.go) already rejects
    // non-string columns before round 2 (this function) can run, but keep
    // the check here too as defense in depth: the LIKE fallback's own NULL
    // fast-path and strict-subset validator below should never accept a
    // non-string column, regardless of any future code path that might
    // reach this function around round 1.
    let mut columns = Vec::with_capacity(column_count);
    for index in 0..column_count {
        let column = &rewriter.ctxStack[stack_length - column_count - 1 + index];
        if column.GetType(rewriter.sctx.GetEvalCtx()).EvalType() != types::ETString {
            rewriter.err = Some(expression::ErrNotSupportedYet.GenWithStackByArgs(
                "Doesn't support match search on a non-string column without fulltext index",
            ));
            return;
        }
        columns.push(column.CloneExpr());
    }

    let search_text = match constant.Eval(rewriter.sctx.GetEvalCtx(), chunk::Row::default()) {
        Ok(value) => value,
        Err(error) => {
            rewriter.err = Some(error);
            return;
        }
    };

    if search_text.IsNull() {
        // NULL search yields NULL in MySQL FTS semantics
        // (builtin_fts.go evalReal returns isNull=true for NULL args), so we
        // emit Constant(NULL) rather than Constant(0). This preserves
        // three-valued logic under NOT — NOT NULL = NULL filters the row —
        // and under IS NULL / IS NOT NULL. A literal Constant(0) would make
        // NOT(MATCH...) admit every row when the search is NULL, diverging
        // from native semantics.
        ctxStackPop(rewriter, column_count + 1);
        ctxStackAppend(
            rewriter,
            Box::new(expression::Constant::with_type(
                types::Datum::default(),
                *types::NewFieldType(mysql::TypeTiny),
            )),
            types::EmptyName.clone(),
        );
        return;
    }

    if search_text.Kind() != types::KindString {
        rewriter.err = Some(
            expression::ErrNotSupportedYet
                .GenWithStackByArgs("MATCH...AGAINST with non-string search expression"),
        );
        return;
    }

    // The LIKE fallback only translates a strict subset of MySQL FTS search
    // strings (alphanumeric words, optionally prefixed with + or - in boolean
    // mode). Anything outside that subset would tokenize differently in MySQL
    // FTS than a substring LIKE match, so refuse it here. If the same MATCH
    // is independently native-viable (FTS index + supported modifier),
    // delegate to the native builtin so TiFlash serves it correctly; otherwise
    // surface the error to the user.
    if let Err(error) =
        expression::ValidateFTSSearchStringForLikeFallback(search_text.GetString(), *Modifier)
    {
        if ftsNativeViable(rewriter, *Modifier, column_count, stack_length) {
            matchAgainstToBuiltin(rewriter, node, column_count, stack_length);
            return;
        }
        rewriter.err = Some(error);
        return;
    }

    ctxStackPop(rewriter, column_count + 1);
    match convertMatchAgainstToLike(rewriter, columns, search_text.GetString(), *Modifier) {
        Ok(result) => ctxStackAppend(rewriter, result, types::EmptyName.clone()),
        Err(error) => rewriter.err = Some(error),
    }
}

/// MATCH AGAINST → LIKE 的具体转换。
fn convertMatchAgainstToLike(
    rewriter: &expressionRewriter<'_>,
    columns: Vec<expression::ExprBox>,
    search_text: String,
    modifier: u8,
) -> Result<expression::ExprBox, errors::Error> {
    expression::BuildFTSToILikeExpression(rewriter.sctx, columns, search_text, modifier)
}

// regexpToScalarFunc 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// REGEXP 相关 → 标量函数。
pub fn regexpToScalarFunc(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Regexp { Not, Type, .. } = &node.Kind else {
        return;
    };
    let length = rewriter.ctxStack.len();
    let result = (|| -> Result<expression::ExprBox, errors::Error> {
        expression::CheckArgsNotMultiColumnRow(&vec![
            rewriter.ctxStack[length - 2].CloneExpr(),
            rewriter.ctxStack[length - 1].CloneExpr(),
        ])?;
        notToExpression(
            rewriter,
            *Not,
            ast::Regexp,
            Type.DeepCopy(),
            vec![
                rewriter.ctxStack[length - 2].CloneExpr(),
                rewriter.ctxStack[length - 1].CloneExpr(),
            ],
        )
    })();
    match result {
        Ok(function) => {
            ctxStackPop(rewriter, 2);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// rowToScalarFunc 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// ROW 构造 → 标量函数。
pub fn rowToScalarFunc(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Row(values) = &node.Kind else {
        return;
    };
    let stack_length = rewriter.ctxStack.len();
    let row_length = values.len();
    let rows = rewriter.ctxStack[stack_length - row_length..]
        .iter()
        .map(|value| value.CloneExpr())
        .collect::<Vec<_>>();
    let return_type = rows[0].GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
    ctxStackPop(rewriter, row_length);
    match newFunction(rewriter, ast::RowFunc, return_type, rows) {
        Ok(function) => ctxStackAppend(rewriter, function, types::EmptyName.clone()),
        Err(error) => rewriter.err = Some(error),
    }
}

// wrapExpWithCast 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 按目标类型为表达式包裹 CAST。
pub fn wrapExpWithCast(
    rewriter: &expressionRewriter<'_>,
) -> (
    expression::ExprBox,
    expression::ExprBox,
    expression::ExprBox,
) {
    let stack_length = rewriter.ctxStack.len();
    let mut expression_value = rewriter.ctxStack[stack_length - 3].CloneExpr();
    let mut left = rewriter.ctxStack[stack_length - 2].CloneExpr();
    let mut right = rewriter.ctxStack[stack_length - 1].CloneExpr();
    match expression::ResolveType4Between(
        rewriter.sctx.GetEvalCtx(),
        [expression_value.as_ref(), left.as_ref(), right.as_ref()],
    ) {
        types::ETInt => {
            expression_value = expression::WrapWithCastAsInt(rewriter.sctx, expression_value, None);
            left = expression::WrapWithCastAsInt(rewriter.sctx, left, None);
            right = expression::WrapWithCastAsInt(rewriter.sctx, right, None);
        }
        types::ETReal => {
            expression_value = expression::WrapWithCastAsReal(rewriter.sctx, expression_value);
            left = expression::WrapWithCastAsReal(rewriter.sctx, left);
            right = expression::WrapWithCastAsReal(rewriter.sctx, right);
        }
        types::ETDecimal => {
            expression_value = expression::WrapWithCastAsDecimal(rewriter.sctx, expression_value);
            left = expression::WrapWithCastAsDecimal(rewriter.sctx, left);
            right = expression::WrapWithCastAsDecimal(rewriter.sctx, right);
        }
        types::ETString => {
            let cast_string = |value: expression::ExprBox| {
                if value
                    .GetType(rewriter.sctx.GetEvalCtx())
                    .EvalType()
                    .IsStringKind()
                {
                    value
                } else {
                    expression::WrapWithCastAsString(rewriter.sctx, value)
                }
            };
            expression_value = cast_string(expression_value);
            left = cast_string(left);
            right = cast_string(right);
        }
        types::ETDuration => {
            let return_type = types::NewFieldType(mysql::TypeDuration);
            expression_value = expression::WrapWithCastAsTime(
                rewriter.sctx,
                expression_value,
                return_type.DeepCopy(),
            );
            left = expression::WrapWithCastAsTime(rewriter.sctx, left, return_type.DeepCopy());
            right = expression::WrapWithCastAsTime(rewriter.sctx, right, *return_type);
        }
        types::ETDatetime => {
            let return_type = types::NewFieldType(mysql::TypeDatetime);
            expression_value = expression::WrapWithCastAsTime(
                rewriter.sctx,
                expression_value,
                return_type.DeepCopy(),
            );
            left = expression::WrapWithCastAsTime(rewriter.sctx, left, return_type.DeepCopy());
            right = expression::WrapWithCastAsTime(rewriter.sctx, right, *return_type);
        }
        _ => {}
    }
    (expression_value, left, right)
}

/// 构造 collation CAST 函数。
fn buildCastCollationFunction(
    rewriter: &expressionRewriter<'_>,
    expression_value: expression::ExprBox,
    collation: &expression::ExprCollation,
    enum_or_set_real_type_is_string: bool,
) -> expression::ExprBox {
    let source_type = expression_value.GetType(rewriter.sctx.GetEvalCtx());
    if source_type.EvalType() != types::ETString || source_type.GetCollate() == collation.Collation
    {
        return expression_value;
    }
    let mut target_type = source_type.DeepCopy();
    if source_type.Hybrid() {
        if !enum_or_set_real_type_is_string {
            return expression_value;
        }
        target_type = *types::NewFieldType(mysql::TypeVarString);
    } else if collation.Charset == charset::CharsetBin {
        target_type.SetType(mysql::TypeVarString);
    }
    target_type.SetCharset(collation.Charset.clone());
    target_type.SetCollate(collation.Collation.clone());
    expression::BuildCastFunction(rewriter.sctx, &expression_value, &target_type)
}

// betweenToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// BETWEEN AND → expression。
pub fn betweenToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Between { Not, .. } = &node.Kind else {
        return;
    };
    let result = (|| -> Result<expression::ExprBox, errors::Error> {
        let stack_length = rewriter.ctxStack.len();
        expression::CheckArgsNotMultiColumnRow(&rewriter.ctxStack[stack_length - 3..])?;
        let (mut expression_value, mut left, mut right) = wrapExpWithCast(rewriter);
        let collation = expression::CheckAndDeriveCollationFromExprs(
            rewriter.sctx,
            "BETWEEN",
            types::ETInt,
            &[expression_value.as_ref(), left.as_ref(), right.as_ref()],
        )?;
        let left_type = expression::GetAccurateCmpType(
            rewriter.sctx.GetEvalCtx(),
            expression_value.as_ref(),
            left.as_ref(),
        );
        let right_type = expression::GetAccurateCmpType(
            rewriter.sctx.GetEvalCtx(),
            expression_value.as_ref(),
            right.as_ref(),
        );
        let enum_or_set_is_string = left_type != types::ETInt && right_type != types::ETInt;
        expression_value = buildCastCollationFunction(
            rewriter,
            expression_value,
            &collation,
            enum_or_set_is_string,
        );
        left = buildCastCollationFunction(rewriter, left, &collation, enum_or_set_is_string);
        right = buildCastCollationFunction(rewriter, right, &collation, enum_or_set_is_string);
        let return_type = types::NewFieldType(mysql::TypeTiny);
        let lower = expression::NewFunction(
            rewriter.sctx,
            ast::GE,
            return_type.DeepCopy(),
            vec![expression_value.CloneExpr(), left],
        )?;
        let upper = expression::NewFunction(
            rewriter.sctx,
            ast::LE,
            return_type.DeepCopy(),
            vec![expression_value, right],
        )?;
        let function = newFunction(
            rewriter,
            ast::LogicAnd,
            return_type.DeepCopy(),
            vec![lower, upper],
        )?;
        if *Not {
            newFunction(rewriter, ast::UnaryNot, *return_type, vec![function])
        } else {
            Ok(function)
        }
    })();
    match result {
        Ok(function) => {
            ctxStackPop(rewriter, 3);
            ctxStackAppend(rewriter, function, types::EmptyName.clone());
        }
        Err(error) => rewriter.err = Some(error),
    }
}

// rewriteFuncCall handles a FuncCallExpr and generates a customized function.
// It should return true if for the given FuncCallExpr a rewrite is performed so that original behavior is skipped.
// Otherwise it should return false to indicate (the caller) that original behavior needs to be performed.
// rewriteFuncCall 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 特殊函数调用的改写钩子；返回是否已处理。
pub fn rewriteFuncCall(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) -> bool {
    let ast::ExprKind::Function { FnName, Args, .. } = &node.Kind else {
        return false;
    };
    match FnName.L.as_str() {
        ast::Ifnull => {
            if Args.len() != 2 {
                rewriter.err =
                    Some(expression::ErrIncorrectParameterCount.GenWithStackByArgs(&FnName.O));
                return true;
            }
            let stack_length = rewriter.ctxStack.len();
            let left = rewriter.ctxStack[stack_length - 2].CloneExpr();
            let right = rewriter.ctxStack[stack_length - 1].CloneExpr();
            let column = left.as_any().downcast_ref::<expression::Column>();
            let left_type = left.GetType(rewriter.sctx.GetEvalCtx());
            let is_enum_or_set = matches!(left_type.GetType(), mysql::TypeEnum | mysql::TypeSet);
            if column.is_some_and(|column| {
                column
                    .RetType
                    .as_ref()
                    .is_some_and(|field_type| mysql::HasNotNullFlag(field_type.GetFlag()))
            }) && !is_enum_or_set
            {
                let mut return_type = match expression::InferType4ControlFuncs(
                    rewriter.sctx,
                    ast::Ifnull,
                    left.as_ref(),
                    right.as_ref(),
                ) {
                    Ok(return_type) => return_type,
                    Err(error) => {
                        rewriter.err = Some(error);
                        return true;
                    }
                };
                return_type.AddFlag(
                    (left_type.GetFlag() & mysql::NotNullFlag)
                        | (right.GetType(rewriter.sctx.GetEvalCtx()).GetFlag()
                            & mysql::NotNullFlag),
                );
                let right_type = right.GetType(rewriter.sctx.GetEvalCtx());
                if left_type.GetType() == mysql::TypeNull && right_type.GetType() == mysql::TypeNull
                {
                    return_type.SetType(mysql::TypeNull);
                    return_type.SetFlen(0);
                    return_type.SetDecimal(0);
                    types_dependency::field::SetBinChsClnFlag(&mut return_type);
                }
                if left_type.GetType() != return_type.GetType()
                    || left_type.GetCharset() != return_type.GetCharset()
                    || left_type.GetCollate() != return_type.GetCollate()
                    || left_type.GetFlen() != return_type.GetFlen()
                    || left_type.GetDecimal() != return_type.GetDecimal()
                    || left_type.GetFlag() != return_type.GetFlag()
                {
                    return false;
                }
                ctxStackPop(rewriter, Args.len());
                ctxStackAppend(rewriter, left, types::EmptyName.clone());
                return true;
            }
            false
        }
        ast::Nullif => {
            if Args.len() != 2 {
                rewriter.err =
                    Some(expression::ErrIncorrectParameterCount.GenWithStackByArgs(&FnName.O));
                return true;
            }
            let stack_length = rewriter.ctxStack.len();
            let first = rewriter.ctxStack[stack_length - 2].CloneExpr();
            let second = rewriter.ctxStack[stack_length - 1].CloneExpr();
            let comparison =
                match constructBinaryOpFunction(rewriter, first.CloneExpr(), second, ast::EQ) {
                    Ok(comparison) => comparison,
                    Err(error) => {
                        rewriter.err = Some(error);
                        return true;
                    }
                };
            let mut value_branch = first;
            let mut return_type = value_branch.GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
            return_type.DelFlag(mysql::NotNullFlag);
            if !return_type.EvalType().IsStringKind() {
                return_type.SetCharset(charset::CharsetBin.to_owned());
                return_type.SetCollate(charset::CollationBin.to_owned());
            }
            setExprRetType(&mut value_branch, return_type.DeepCopy());
            let null_value = Box::new(expression::Constant::with_type(
                types::NewDatum(&()),
                *types::NewFieldType(mysql::TypeNull),
            ));
            match newFunction(
                rewriter,
                ast::If,
                return_type,
                vec![comparison, null_value, value_branch],
            ) {
                Ok(function) => {
                    ctxStackPop(rewriter, Args.len());
                    ctxStackAppend(rewriter, function, types::EmptyName.clone());
                }
                Err(error) => rewriter.err = Some(error),
            }
            true
        }
        _ => false,
    }
}

// setExprRetType 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 设置表达式返回类型。
pub fn setExprRetType(expression_value: &mut expression::ExprBox, return_type: types::FieldType) {
    *expression_value.GetTypeMut() = return_type;
}

// funcCallToExpressionWithPlanCtx 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 在规划器上下文中将函数调用转为 expression。
pub fn funcCallToExpressionWithPlanCtx(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) {
    let ast::ExprKind::Function { FnName, Args, .. } = &node.Kind else {
        return;
    };
    let stack_length = rewriter.ctxStack.len();
    let arguments = rewriter.ctxStack[stack_length - Args.len()..]
        .iter()
        .map(|argument| argument.CloneExpr())
        .collect::<Vec<_>>();
    if let Err(error) = expression::CheckArgsNotMultiColumnRow(&arguments) {
        rewriter.err = Some(error);
        return;
    }
    ctxStackPop(rewriter, Args.len());
    match FnName.L.as_str() {
        ast::Grouping => {
            // grouping function should fetch the underlying grouping-sets meta and rewrite the args here.
            // eg: grouping(a) actually is try to find in which grouping-set that the column 'a' is remained,
            // collecting those gid as a collection and filling it into the grouping function meta. Besides,
            // the first arg of grouping function should be rewritten as gid column defined/passed by Expand
            // from the bottom up.
            let Some(roll_expand) = plan_context.rollExpand else {
                rewriter.err = Some(
                    plannererrors::ErrInvalidGroupFuncUse
                        .GenWithStackByArgs(&[])
                        .into(),
                );
                ctxStackAppend(
                    rewriter,
                    Box::new(expression::NewNull()),
                    types::EmptyName.clone(),
                );
                return;
            };
            // whether there is some duplicate grouping sets, gpos is only be used in shuffle keys and group keys
            // rather than grouping function.
            // eg: rollup(a,a,b), the decided grouping sets are {a,a,b},{a,a,null},{a,null,null},{null,null,null}
            // for the second and third grouping set: {a,a,null} and {a,null,null}, a here is the col ref of original
            // column `a`. So from the static layer, this two grouping set are equivalent, we don't need to copy col
            // `a double times at the every beginning and resort to gpos to distinguish them.
            //  {col-a, col-b, gid, gpos}
            //  {a, b, 0, 1}, {a, null, 1, 2}, {a, null, 1, 3}, {null, null, 2, 4}
            // grouping function still only need to care about gid is enough, gpos what group and shuffle keys cared.
            if arguments.len() > 64 {
                rewriter.err = Some(
                    plannererrors::ErrInvalidNumberOfArgs
                        .GenWithStackByArgs(&["GROUPING".into(), 64.into()])
                        .into(),
                );
                ctxStackAppend(
                    rewriter,
                    Box::new(expression::NewNull()),
                    types::EmptyName.clone(),
                );
                return;
            }
            // resolve grouping args in group by items or not.
            let resolved_arguments = roll_expand.ResolveGroupingFuncArgsInGroupBy(&arguments);
            let mut resolved_columns = Vec::with_capacity(resolved_arguments.len());
            for (index, argument) in resolved_arguments.iter().enumerate() {
                let Some(column) = argument.as_any().downcast_ref::<expression::Column>() else {
                    rewriter.err = Some(
                        plannererrors::ErrFieldInGroupingNotGroupBy
                            .GenWithStackByArgs(&[format!("#{index}").into()])
                            .into(),
                    );
                    ctxStackAppend(
                        rewriter,
                        Box::new(expression::NewNull()),
                        types::EmptyName.clone(),
                    );
                    return;
                };
                resolved_columns.push(column.Clone());
            }
            let Some(gid) = roll_expand.GID.as_ref() else {
                rewriter.err = Some(errors::New("GROUPING requires an Expand GID column"));
                ctxStackAppend(
                    rewriter,
                    Box::new(expression::NewNull()),
                    types::EmptyName.clone(),
                );
                return;
            };
            let new_argument = Box::new(gid.Clone());
            let grouping_marks = resolved_columns
                .iter()
                .map(|column| match roll_expand.GroupingMode {
                    logicalop::GroupingMode::ModeBitAnd => {
                        let mut mark = 0_u64;
                        for candidate in roll_expand.DistinctGroupByCol.iter().rev() {
                            mark <<= 1;
                            if candidate.UniqueID == column.UniqueID {
                                mark |= 1;
                            }
                        }
                        std::collections::HashSet::from([mark])
                    }
                    logicalop::GroupingMode::ModeNumericSet => roll_expand
                        .RollupID2GIDS
                        .get(&column.UniqueID)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                })
                .collect::<Vec<_>>();
            let grouping_mode = match roll_expand.GroupingMode {
                logicalop::GroupingMode::ModeBitAnd => expression::PlannerGroupingMode::ModeBitAnd,
                logicalop::GroupingMode::ModeNumericSet => {
                    expression::PlannerGroupingMode::ModeNumericSet
                }
            };
            match newFunction(
                rewriter,
                &FnName.L,
                *types::NewFieldType(mysql::TypeLonglong),
                vec![new_argument],
            ) {
                Ok(mut function) => {
                    let initialized = function
                        .as_any_mut()
                        .downcast_mut::<expression::ScalarFunction>()
                        .and_then(|grouping_function| {
                            grouping_function
                                .Function
                                .as_any()
                                .downcast_ref::<expression::BuiltinGroupingImplSig>()
                        })
                        .ok_or_else(|| errors::New("GROUPING implementation mismatch"))
                        .and_then(|implementation| {
                            implementation.SetMetadata(grouping_mode, grouping_marks)
                        });
                    match initialized {
                        Ok(()) => {
                            ctxStackAppend(rewriter, function, types::EmptyName.clone());
                        }
                        Err(error) => rewriter.err = Some(error),
                    }
                }
                Err(error) => rewriter.err = Some(error),
            }
        }
        _ => {
            rewriter.err = Some(errors::New(format!("invalid function: {}", FnName.L)));
            ctxStackAppend(
                rewriter,
                Box::new(expression::NewNull()),
                types::EmptyName.clone(),
            );
        }
    }
}

// funcCallToExpression 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 普通函数调用 AST → expression。
pub fn funcCallToExpression(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Function { FnName, Args, .. } = &node.Kind else {
        return;
    };
    let stack_length = rewriter.ctxStack.len();
    let arguments = rewriter.ctxStack[stack_length - Args.len()..]
        .iter()
        .map(|argument| argument.CloneExpr())
        .collect::<Vec<_>>();
    if let Err(error) = expression::CheckArgsNotMultiColumnRow(&arguments) {
        rewriter.err = Some(error);
        return;
    }
    if rewriteFuncCall(rewriter, node) {
        return;
    }
    ctxStackPop(rewriter, Args.len());
    let result = if useCache(rewriter) && expression::IsDeferredFunctions(rewriter.sctx, &FnName.L)
    {
        // When the expression is unix_timestamp and the number of argument is not zero,
        // we deal with it as normal expression.
        if FnName.L == ast::UnixTimestamp && !Args.is_empty() {
            newFunction(
                rewriter,
                &FnName.L,
                *types::NewFieldType(mysql::TypeUnspecified),
                arguments,
            )
        } else {
            expression::NewFunctionBase(
                rewriter.sctx,
                &FnName.L,
                *types::NewFieldType(mysql::TypeUnspecified),
                arguments,
            )
            .map(|function| {
                let return_type = function.GetType(rewriter.sctx.GetEvalCtx()).DeepCopy();
                let mut constant =
                    expression::Constant::with_type(types::NewDatum(&()), return_type);
                constant.DeferredExpr = Some(function);
                Box::new(constant) as expression::ExprBox
            })
        }
    } else {
        newFunction(
            rewriter,
            &FnName.L,
            *types::NewFieldType(mysql::TypeUnspecified),
            arguments,
        )
    };
    match result {
        Ok(function) => ctxStackAppend(rewriter, function, types::EmptyName.clone()),
        Err(error) => rewriter.err = Some(error),
    }
}

// Now TableName in expression only used by sequence function like nextval(seq).
// The function arg should be evaluated as a table name rather than normal column name like mysql does.
// toTable 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 表名 AST 节点处理（如 DEFAULT 相关）。
pub fn toTable(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::TableName(table_name) = &node.Kind else {
        return;
    };
    let full_name = if table_name.Schema.L.is_empty() {
        table_name.Name.L.clone()
    } else {
        format!("{}.{}", table_name.Schema.L, table_name.Name.L)
    };
    ctxStackAppend(
        rewriter,
        Box::new(expression::Constant::with_type(
            types::NewStringDatum(full_name),
            *types::NewFieldType(mysql::TypeString),
        )),
        types::EmptyName.clone(),
    );
}

// toParamMarker 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 预处理语句参数占位符 → ParamMarker 表达式。
pub fn toParamMarker(rewriter: &mut expressionRewriter<'_>, offset: usize) {
    let datum = match rewriter.sctx.GetEvalCtx().GetParamValue(offset) {
        Ok(datum) => datum,
        Err(error) => {
            rewriter.err = Some(errors::New(error.to_string()));
            return;
        }
    };
    let mut return_type = *types::NewFieldType(mysql::TypeUnspecified);
    types::InferParamTypeFromDatum(&datum, &mut return_type);
    let mut value = expression::Constant::with_type(datum, return_type);
    value.ParamMarker = rewriter
        .sctx
        .IsUseCache()
        .then(|| expression::ParamMarker::new(offset));
    initConstantRepertoire(rewriter.sctx.GetEvalCtx(), &mut value);
    adjustUTF8MB4Collation(
        rewriter,
        value
            .RetType
            .as_mut()
            .expect("parameter marker return type is always present"),
    );
    ctxStackAppend(rewriter, Box::new(value), types::EmptyName.clone());
}

// clause 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 读取当前改写所处的 SQL 子句编码。
pub fn clause(rewriter: &expressionRewriter<'_>) -> clauseCode {
    rewriter
        .planCtx
        .as_ref()
        .map_or(expressionClause, |context| context.builder.curClause)
}

// shouldRemapRedundantBaseColumn 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 是否应重映射 NATURAL/USING Join 产生的冗余基列。
pub fn shouldRemapRedundantBaseColumn(
    plan_context: Option<&exprRewriterPlanCtx<'_>>,
    clause_code: clauseCode,
    name: &types::FieldName,
) -> bool {
    let Some(plan_context) = plan_context else {
        return false;
    };
    if clause_code != whereClause && clause_code != havingClause {
        return false;
    }
    // DML 的 JOIN Schema 为合并（非合并列）顺序，避免与 SELECT 的 coalesced 映射混用。
    // UPDATE/DELETE build JOIN schema/output names in merged (non-coalesced) order.
    // Skip redundant-column remap in DML to avoid mixing coalesced mapping semantics.
    if plan_context.builder.inUpdateStmt || plan_context.builder.inDeleteStmt {
        return false;
    }
    name.Redundant && !name.OrigTblName.L.is_empty()
}

// toColumn 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 列名格式化为字符串。
fn columnNameString(column_name: &ast::ColumnName) -> String {
    [
        column_name.Schema.O.as_str(),
        column_name.Table.O.as_str(),
        column_name.Name.O.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(".")
}

/// 列引用解析为 Column 表达式。
pub fn toColumn(rewriter: &mut expressionRewriter<'_>, node: &ast::ExprNode) {
    let ast::ExprKind::Column(column_name) = &node.Kind else {
        return;
    };
    let index = match expression::FindFieldName(&rewriter.names, column_name) {
        Ok(index) => index,
        Err(_) => {
            rewriter.err = Some(
                plannererrors::ErrAmbiguous
                    .GenWithStackByArgs(&[
                        column_name.Name.O.clone().into(),
                        clauseMsg[fieldList].into(),
                    ])
                    .into(),
            );
            return;
        }
    };
    if let Some(index) = index {
        let mut column = rewriter.schema.as_ref().unwrap().Columns[index].Clone();
        let mut name = rewriter.names.0[index].clone().unwrap();
        // HAVING resolution in Go uses the builder's column mapper to address
        // auxiliary SELECT fields. The Rust AST walker clones nested nodes, so
        // pointer-keyed mapper entries are not stable; allow those deliberately
        // hidden projection fields while rewriting HAVING itself. The final
        // SELECT projection removes them before exposing the query schema.
        if column.IsHidden && clause(rewriter) != havingClause {
            rewriter.err = Some(
                plannererrors::ErrUnknownColumn
                    .GenWithStackByArgs(&[
                        column_name.Name.O.clone().into(),
                        clauseMsg[clause(rewriter)].into(),
                    ])
                    .into(),
            );
            return;
        }
        if shouldRemapRedundantBaseColumn(rewriter.planCtx.as_deref(), clause(rewriter), &name) {
            // JOIN ... USING/NATURAL keeps redundant side in FullSchema for name-resolution,
            // but the executable Join.Schema() only keeps canonical visible columns.
            // For qualified base-table references (OrigTblName != ""), remap redundant
            // column to canonical output to avoid carrying an unresolvable redundant column
            // into later physical ResolveIndices.
            if let Some((mapped_column, mapped_name)) =
                resolveRedundantColumnFromNaturalUsingJoinPlan(
                    rewriter.planCtx.as_ref().unwrap().plan.as_ref(),
                    &column,
                )
            {
                column = mapped_column;
                name = mapped_name;
            }
        }
        ctxStackAppend(rewriter, Box::new(column), name);
        return;
    }
    if rewriter.planCtx.is_none()
        && rewriter.sourceTable.is_some_and(|table| {
            column_name.Table.L.is_empty() || table.Name.L == column_name.Table.L
        })
    {
        let table = rewriter.sourceTable.unwrap();
        let Some(column_info) = table.FindPublicColumnByName(&column_name.Name.L) else {
            rewriter.err = Some(
                plannererrors::ErrUnknownColumn
                    .GenWithStackByArgs(&[
                        column_name.Name.O.clone().into(),
                        clauseMsg[clause(rewriter)].into(),
                    ])
                    .into(),
            );
            return;
        };
        if column_info.Hidden {
            rewriter.err = Some(
                plannererrors::ErrUnknownColumn
                    .GenWithStackByArgs(&[
                        column_name.Name.O.clone().into(),
                        clauseMsg[clause(rewriter)].into(),
                    ])
                    .into(),
            );
            return;
        }
        ctxStackAppend(
            rewriter,
            Box::new({
                let mut column = expression::Column::new(
                    column_info.FieldType.DeepCopy(),
                    column_info.ID,
                    column_info.ID,
                    0,
                );
                column.OrigName = format!("{}.{}", table.Name.L, column_info.Name.L);
                column
            }),
            std::sync::Arc::new(types::FieldName {
                ColName: column_name.Name.clone(),
                ..Default::default()
            }),
        );
        return;
    }

    let Some(mut plan_context) = rewriter.planCtx.take() else {
        rewriter.err = Some(
            plannererrors::ErrUnknownColumn
                .GenWithStackByArgs(&[
                    columnNameString(column_name).into(),
                    clauseMsg[clause(rewriter)].into(),
                ])
                .into(),
        );
        return;
    };
    match findFieldNameFromNaturalUsingJoin(plan_context.plan.as_ref(), column_name) {
        Err(error) => {
            rewriter.err = Some(error);
            rewriter.planCtx = Some(plan_context);
            return;
        }
        Ok(Some((mut column, mut name))) => {
            if shouldRemapRedundantBaseColumn(Some(&plan_context), clause(rewriter), &name) {
                if let Some((mapped_column, mapped_name)) =
                    resolveRedundantColumnFromNaturalUsingJoinPlan(
                        plan_context.plan.as_ref(),
                        &column,
                    )
                {
                    column = mapped_column;
                    name = mapped_name;
                }
            }
            ctxStackAppend(rewriter, Box::new(column), name);
            rewriter.planCtx = Some(plan_context);
            return;
        }
        Ok(None) => {}
    }
    for outer_index in (0..plan_context.builder.outerSchemas.len()).rev() {
        let outer_schema = &plan_context.builder.outerSchemas[outer_index];
        let outer_names = &plan_context.builder.outerNames[outer_index];
        match expression::FindFieldName(outer_names, column_name) {
            Ok(Some(index)) => {
                let column = outer_schema.Columns[index].Clone();
                ctxStackAppend(
                    rewriter,
                    Box::new(expression::CorrelatedColumn {
                        column,
                        data: Some(std::sync::Arc::new(std::sync::RwLock::new(
                            types::Datum::default(),
                        ))),
                    }),
                    outer_names.0[index].clone().unwrap(),
                );
                rewriter.planCtx = Some(plan_context);
                return;
            }
            Err(_) => {
                rewriter.err = Some(
                    plannererrors::ErrAmbiguous
                        .GenWithStackByArgs(&[
                            column_name.Name.O.clone().into(),
                            clauseMsg[fieldList].into(),
                        ])
                        .into(),
                );
                rewriter.planCtx = Some(plan_context);
                return;
            }
            _ => {}
        }
    }
    if plan_context
        .plan
        .as_any()
        .is::<logicalop::LogicalUnionAll>()
        && !column_name.Table.O.is_empty()
    {
        rewriter.err = Some(
            plannererrors::ErrTablenameNotAllowedHere
                .GenWithStackByArgs(&[
                    column_name.Table.O.clone().into(),
                    "SELECT".into(),
                    clauseMsg[plan_context.builder.curClause].into(),
                ])
                .into(),
        );
        rewriter.planCtx = Some(plan_context);
        return;
    }
    if plan_context.builder.curClause == globalOrderByClause {
        plan_context.builder.curClause = orderByClause;
    }
    rewriter.err = Some(
        plannererrors::ErrUnknownColumn
            .GenWithStackByArgs(&[
                columnNameString(column_name).into(),
                clauseMsg[plan_context.builder.curClause].into(),
            ])
            .into(),
    );
    rewriter.planCtx = Some(plan_context);
}

// findFieldNameFromNaturalUsingJoin 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 从 NATURAL/USING Join 查找字段名。
pub fn findFieldNameFromNaturalUsingJoin(
    plan: &dyn logicalop::LogicalPlan,
    column_name: &ast::ColumnName,
) -> Result<Option<(expression::Column, std::sync::Arc<types::FieldName>)>, errors::Error> {
    if plan.as_any().is::<logicalop::LogicalLimit>()
        || plan.as_any().is::<logicalop::LogicalSelection>()
        || plan.as_any().is::<logicalop::LogicalTopN>()
        || plan.as_any().is::<logicalop::LogicalSort>()
        || plan.as_any().is::<logicalop::LogicalMaxOneRow>()
    {
        return findFieldNameFromNaturalUsingJoin(plan.Children()[0].as_ref(), column_name);
    }
    if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
        if let Some(full_schema) = join.FullSchema.as_ref() {
            if let Some(index) = expression::FindFieldName(&join.FullNames, column_name)? {
                return Ok(Some((
                    full_schema.Columns[index].Clone(),
                    join.FullNames.0[index].clone().unwrap(),
                )));
            }
        }
    } else if let Some(apply) = plan.as_any().downcast_ref::<logicalop::LogicalApply>() {
        // LogicalApply embeds LogicalJoin, so it also has FullSchema/FullNames for USING/NATURAL joins.
        // When FullSchema is nil, treat Apply as a transparent wrapper and recurse into the outer
        // (left) child, which may itself be a LogicalJoin with FullSchema for a USING/NATURAL join.
        let Some(full_schema) = apply.LogicalJoin.FullSchema.as_ref() else {
            return findFieldNameFromNaturalUsingJoin(apply.Children()[0].as_ref(), column_name);
        };
        if let Some(index) = expression::FindFieldName(&apply.LogicalJoin.FullNames, column_name)? {
            return Ok(Some((
                full_schema.Columns[index].Clone(),
                apply.LogicalJoin.FullNames.0[index].clone().unwrap(),
            )));
        }
    }
    Ok(None)
}

// resolveRedundantColumnFromNaturalUsingJoinPlan 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 解析 NATURAL/USING Join 计划中的冗余列。
pub fn resolveRedundantColumnFromNaturalUsingJoinPlan(
    plan: &dyn logicalop::LogicalPlan,
    column: &expression::Column,
) -> Option<(expression::Column, std::sync::Arc<types::FieldName>)> {
    if plan.as_any().is::<logicalop::LogicalLimit>()
        || plan.as_any().is::<logicalop::LogicalSelection>()
        || plan.as_any().is::<logicalop::LogicalTopN>()
        || plan.as_any().is::<logicalop::LogicalSort>()
        || plan.as_any().is::<logicalop::LogicalMaxOneRow>()
    {
        // These nodes preserve child's column identity; continue tracing down.
        return resolveRedundantColumnFromNaturalUsingJoinPlan(plan.Children()[0].as_ref(), column);
    }
    if let Some(apply) = plan.as_any().downcast_ref::<logicalop::LogicalApply>() {
        // LogicalApply embeds LogicalJoin, so handle it the same way for USING/NATURAL remapping.
        if apply.LogicalJoin.JoinType == base::JoinType::InnerJoin
            && apply
                .LogicalJoin
                .FullSchema
                .as_ref()
                .is_some_and(|schema| schema.Contains(column))
        {
            if let Some(mapped) = apply.LogicalJoin.ResolveRedundantColumn(column) {
                let index = apply.LogicalJoin.Schema().ColumnIndex(mapped)?;
                return Some((
                    mapped.Clone(),
                    apply.LogicalJoin.OutputNames().0[index].clone()?,
                ));
            }
        }
        for child in apply.Children() {
            if let Some(mapped) =
                resolveRedundantColumnFromNaturalUsingJoinPlan(child.as_ref(), column)
            {
                return Some(mapped);
            }
        }
    } else if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
        // Remapping is only defined for inner JOIN ... USING/NATURAL semantics.
        // When an ancestor join contains this column but has no mapping, continue
        // descending so child joins can provide the canonical output mapping.
        if join.JoinType == base::JoinType::InnerJoin
            && join
                .FullSchema
                .as_ref()
                .is_some_and(|schema| schema.Contains(column))
        {
            if let Some(mapped) = join.ResolveRedundantColumn(column) {
                let index = join.Schema().ColumnIndex(mapped)?;
                return Some((mapped.Clone(), join.OutputNames().0[index].clone()?));
            }
        }
        for child in join.Children() {
            if let Some(mapped) =
                resolveRedundantColumnFromNaturalUsingJoinPlan(child.as_ref(), column)
            {
                return Some(mapped);
            }
        }
    }
    None
}

// evalDefaultExprForTable 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 按表元信息求值 DEFAULT 表达式。
pub fn evalDefaultExprForTable(
    rewriter: &mut expressionRewriter<'_>,
    node: &ast::ExprNode,
    table_info: &model::TableInfo,
) {
    let ast::ExprKind::NamedDefault(column_name) = &node.Kind else {
        rewriter.err = Some(errors::New("DEFAULT requires a column name"));
        return;
    };
    let index = match expression::FindFieldName(&rewriter.names, column_name) {
        Ok(Some(index)) => index,
        Ok(None) => {
            rewriter.err = Some(errors::New(format!(
                "Unknown column '{}' in 'field list'",
                column_name.Name.O
            )));
            return;
        }
        Err(error) => {
            rewriter.err = Some(error);
            return;
        }
    };
    let name = rewriter.names.0[index].clone().unwrap();
    evalFieldDefaultValue(rewriter, &name, table_info);
}

// evalDefaultExprWithPlanCtx 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 在规划器上下文中求值 DEFAULT。
pub fn evalDefaultExprWithPlanCtx(
    rewriter: &mut expressionRewriter<'_>,
    plan_context: &mut exprRewriterPlanCtx<'_>,
    node: &ast::ExprNode,
) {
    let ast::ExprKind::NamedDefault(column_name) = &node.Kind else {
        rewriter.err = Some(errors::New("DEFAULT requires a column name"));
        return;
    };
    let mut name = None;
    // Here we will find the corresponding column for default function. At the same time, we need to consider the issue
    // of subquery and name space.
    // For example, we have two tables t1(a int default 1, b int) and t2(a int default -1, c int). Consider the following SQL:
    // 		select a from t1 where a > (select default(a) from t2)
    // Refer to the behavior of MySQL, we need to find column a in table t2. If table t2 does not have column a, then find it
    // in table t1. If there are none, return an error message.
    // Based on the above description, we need to look in er.b.allNames from back to front.
    for names in plan_context.builder.allNames.iter().rev() {
        match expression::FindFieldName(names, column_name) {
            Ok(Some(index)) => {
                name = names.0[index].clone();
                break;
            }
            Ok(None) => {}
            Err(error) => {
                rewriter.err = Some(error);
                return;
            }
        }
    }
    if name.is_none() {
        match expression::FindFieldName(&rewriter.names, column_name) {
            Ok(Some(index)) => name = rewriter.names.0[index].clone(),
            Ok(None) => {
                rewriter.err = Some(errors::New(format!(
                    "Unknown column '{}' in 'field list'",
                    column_name.Name.O
                )));
                return;
            }
            Err(error) => {
                rewriter.err = Some(error);
                return;
            }
        }
    }
    let name = name.unwrap();
    let mut database_name = name.DBName.clone();
    if database_name.O.is_empty() {
        // if database name is not specified, use current database name
        database_name = ast::NewCIStr(&plan_context.builder.ctx.GetSessionVars().CurrentDB());
    }
    if name.OrigTblName.O.is_empty() {
        // column is evaluated by some expressions, for example:
        // `select default(c) from (select (a+1) as c from t) as t0`
        // in such case, a 'no default' error is returned
        rewriter.err = Some(
            table::ErrNoDefaultValue
                .GenWithStackByArgs(&[name.ColName.O.clone().into()])
                .into(),
        );
        return;
    }
    let database_name = infoschema::CiString::new(database_name.O.clone());
    let table_name = infoschema::CiString::new(name.OrigTblName.O.clone());
    let table_info = match plan_context
        .builder
        .is
        .ModelTableInfoByName(&database_name, &table_name)
    {
        Ok(table_info) => table_info,
        Err(error) => {
            rewriter.err = Some(errors::New(error.to_string()));
            return;
        }
    };
    evalFieldDefaultValue(rewriter, &name, &table_info);
    if rewriter.err.is_some() {
        return;
    }
}

// evalFieldDefaultValue 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 求值字段默认值。
pub fn evalFieldDefaultValue(
    rewriter: &mut expressionRewriter<'_>,
    field: &types::FieldName,
    table_info: &model::TableInfo,
) {
    let mut column_name = field.OrigColName.L.clone();
    if column_name.is_empty() {
        // in some cases, OrigColName is empty, use ColName instead
        column_name = field.ColName.L.clone();
    }
    let Some(column) = table_info.FindPublicColumnByName(&column_name) else {
        rewriter.err = Some(
            plannererrors::ErrUnknownColumn
                .GenWithStackByArgs(&[column_name.clone().into(), "field_list".into()])
                .into(),
        );
        return;
    };
    let value = if hasCurrentDatetimeDefault(column)
        && matches!(column.GetType(), mysql::TypeDatetime | mysql::TypeTimestamp)
    {
        match expression::GetTimeValue(
            rewriter.sctx,
            ast::CurrentTimestamp,
            column.GetType(),
            column.GetDecimal(),
            None,
        ) {
            Ok(time) => {
                expression::Constant::with_type(time, *types::NewFieldType(column.GetType()))
            }
            Err(error) => {
                rewriter.err = Some(error);
                return;
            }
        }
    } else {
        // for other columns, just use what it is
        let table_column = table::ToColumn(Box::new(column.clone()));
        match table::GetColDefaultValue(rewriter.sctx, table_column.as_ref()) {
            Ok(datum) => expression::Constant::with_type(datum, column.FieldType.DeepCopy()),
            Err(error) => {
                rewriter.err = Some(error);
                return;
            }
        }
    };
    ctxStackAppend(rewriter, Box::new(value), types::EmptyName.clone());
}

// hasCurrentDatetimeDefault checks if column has current_timestamp default value
// hasCurrentDatetimeDefault 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 列默认值是否为 CURRENT_TIMESTAMP/DATETIME 类。
pub fn hasCurrentDatetimeDefault(column: &model::ColumnInfo) -> bool {
    matches!(
        column.DefaultValue.as_ref(),
        Some(model::DefaultValue::String(value))
            if String::from_utf8_lossy(value).eq_ignore_ascii_case(ast::CurrentTimestamp)
    )
}

// hasLimit checks if the plan already contains a LIMIT operator
// hasLimit 对应 Go 的同名函数；保留参数、返回值、关键分支与错误传播，外部类型待 Rust 模块接线。
/// 逻辑计划子树是否包含 Limit。
pub fn hasLimit(plan: &dyn logicalop::LogicalPlan) -> bool {
    // Check if this is a LogicalLimit
    if plan.as_any().is::<logicalop::LogicalLimit>() {
        return true;
    }
    // Recursively check children
    plan.Children().iter().any(|child| hasLimit(child.as_ref()))
}
