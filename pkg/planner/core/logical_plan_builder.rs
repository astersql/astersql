// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

// 逻辑计划构建器（简化骨架实现）。
//
// 将 SELECT / JOIN / 聚合 / 窗口 / CTE / UPDATE / DELETE 等语句骨架
// 转为 `PlanNode` 树，并维护优化标志位。与 `logical_plan_builder_runtime`
// 的完整 AST 运行时构建相对；本文件偏测试与算法骨架。

use crate::planbuilder::{
    BuilderError, PlanBuilder, Privilege, Result, TableInfo, Value, clauseCode, cteInfo, visitInfo,
};
use crate::task::{Expression, FieldType, JoinType, PlanKind, PlanNode, StatsInfo, TypeCode};
use std::collections::{HashMap, HashSet};

/// 谓词下推优化标志位。
pub const FlagPredicatePushDown: u64 = 1;
/// 构建唯一键信息优化标志位。
pub const FlagBuildKeyInfo: u64 = 1 << 1;
/// 解相关（Decorrelate）优化标志位。
pub const FlagDecorrelate: u64 = 1 << 2;
/// 常量传播优化标志位。
pub const FlagConstantPropagation: u64 = 1 << 3;
/// 聚合下推优化标志位。
pub const FlagPushDownAgg: u64 = 1 << 4;
/// TopN 下推优化标志位。
pub const FlagPushDownTopN: u64 = 1 << 5;
/// 外连接消除优化标志位。
pub const FlagEliminateOuterJoin: u64 = 1 << 6;

/// SELECT 列表中的单个投影字段（表达式、别名或通配符）。
#[derive(Clone, Debug)]
pub struct SelectField {
    pub expr: Expression,
    pub alias: Option<String>,
    pub wildcard: bool,
    pub table_wildcard: Option<String>,
}
/// ORDER BY / 窗口 ORDER BY 中的排序项。
#[derive(Clone, Debug)]
pub struct ByItem {
    pub expr: Expression,
    pub desc: bool,
}
/// LIMIT 子句的 Offset 与 Count 字面量。
#[derive(Clone, Debug)]
pub struct LimitClause {
    pub offset: Option<Value>,
    pub count: Option<Value>,
}
/// 集合运算类型：UNION / INTERSECT / EXCEPT 及其 ALL 变体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOprType {
    Union,
    UnionAll,
    Intersect,
    IntersectAll,
    Except,
    ExceptAll,
}
/// 连接规格：类型、左右输入、ON/USING、NATURAL/LATERAL/STRAIGHT。
#[derive(Clone, Debug)]
pub struct JoinSpec {
    pub join_type: JoinType,
    pub left: Box<ResultSet>,
    pub right: Box<ResultSet>,
    pub on: Vec<Expression>,
    pub using_columns: Vec<String>,
    pub natural: bool,
    pub lateral: bool,
    pub straight: bool,
}
/// FROM 子句结果集：表、连接、子查询、集合运算、VALUES 或 CTE。
#[derive(Clone, Debug)]
pub enum ResultSet {
    Table(TableSource),
    Join(JoinSpec),
    Select(Box<SelectStmt>),
    SetOperation(Vec<(SetOprType, SelectStmt)>),
    Values(Vec<Vec<Value>>),
    Cte(String),
}
/// 表数据源：基表信息、别名、LATERAL 与存储引擎偏好。
#[derive(Clone, Debug)]
pub struct TableSource {
    pub table: TableInfo,
    pub alias: Option<String>,
    pub lateral: bool,
    pub prefer_tiflash: bool,
    pub prefer_tikv: bool,
}
/// SELECT 语句骨架：FROM/字段/WHERE/GROUP/HAVING/ORDER/LIMIT/窗口/CTE。
#[derive(Clone, Debug, Default)]
pub struct SelectStmt {
    pub from: Option<ResultSet>,
    pub fields: Vec<SelectField>,
    pub where_clause: Option<Expression>,
    pub group_by: Vec<Expression>,
    pub having: Option<Expression>,
    pub order_by: Vec<ByItem>,
    pub limit: Option<LimitClause>,
    pub distinct: bool,
    pub windows: Vec<WindowSpec>,
    pub with: Vec<CteDef>,
}

/// 窗口帧边界（ PRECEDING / FOLLOWING / CURRENT ROW 等）。
#[derive(Clone, Debug)]
pub struct FrameBound {
    pub kind: BoundKind,
    pub value: Option<i64>,
}
/// 窗口帧边界种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundKind {
    Preceding,
    Following,
    CurrentRow,
    UnboundedPreceding,
    UnboundedFollowing,
}
/// 窗口帧定义：ROWS/RANGE 与起止边界。
#[derive(Clone, Debug)]
pub struct WindowFrame {
    pub rows: bool,
    pub start: FrameBound,
    pub end: FrameBound,
}
/// 窗口规格：名称、引用、PARTITION BY、ORDER BY 与帧。
#[derive(Clone, Debug)]
pub struct WindowSpec {
    pub name: String,
    pub reference: Option<String>,
    pub partition_by: Vec<Expression>,
    pub order_by: Vec<ByItem>,
    pub frame: Option<WindowFrame>,
}
/// 窗口函数调用及其规格绑定。
#[derive(Clone, Debug)]
pub struct WindowFunc {
    pub name: String,
    pub args: Vec<Expression>,
    pub spec_name: Option<String>,
    pub spec: Option<WindowSpec>,
}
/// 同一窗口规格下的一组窗口函数。
#[derive(Clone, Debug)]
pub struct windowFuncs {
    pub spec: WindowSpec,
    pub funcs: Vec<WindowFunc>,
}

/// CTE 定义：名称、列、种子查询、可选递归分支与物化提示。
#[derive(Clone, Debug)]
pub struct CteDef {
    pub name: String,
    pub columns: Vec<String>,
    pub seed: Box<SelectStmt>,
    pub recursive: Option<Box<SelectStmt>>,
    pub materialized: Option<bool>,
}

/// UPDATE 赋值：目标表、列下标与右侧表达式。
#[derive(Clone, Debug)]
pub struct Assignment {
    pub table_id: i64,
    pub column: usize,
    pub expression: Expression,
}
/// UPDATE 语句：源、赋值列表、WHERE/ORDER/LIMIT。
#[derive(Clone, Debug)]
pub struct UpdateStmt {
    pub source: ResultSet,
    pub assignments: Vec<Assignment>,
    pub where_clause: Option<Expression>,
    pub order_by: Vec<ByItem>,
    pub limit: Option<LimitClause>,
    pub ignore: bool,
}
/// DELETE 语句：源、目标表、WHERE/ORDER/LIMIT。
#[derive(Clone, Debug)]
pub struct DeleteStmt {
    pub source: ResultSet,
    pub tables: Vec<i64>,
    pub where_clause: Option<Expression>,
    pub order_by: Vec<ByItem>,
    pub limit: Option<LimitClause>,
    pub ignore: bool,
}
/// 表在计划 Schema 中的列区间与 handle 列位置。
#[derive(Clone, Debug)]
pub struct TblColPosInfo {
    pub table_id: i64,
    pub start: usize,
    pub end: usize,
    pub handle_columns: Vec<usize>,
}
/// 单表 UPDATE 所需的赋值与可写列集合。
#[derive(Clone, Debug)]
pub struct tblUpdateInfo {
    pub table_id: i64,
    pub assignments: Vec<Assignment>,
    pub writable_columns: Vec<usize>,
}

/// 遍历 ORDER BY 表达式以收集其中聚合的解析器。
#[derive(Clone, Debug)]
pub struct aggOrderByResolver {
    pub select_fields: Vec<SelectField>,
    pub aggregates: Vec<Expression>,
    pub err: Option<String>,
}
impl aggOrderByResolver {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        if expr.name.starts_with("agg:") {
            self.aggregates.push(expr.clone());
            false
        } else {
            true
        }
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&mut self, _expr: &Expression) -> bool {
        self.err.is_none()
    }
}
/// 判断参数标记是否可作为位置表达式（正整数）。
pub fn canParamMarkerBePositionExpr(value: &Value) -> bool {
    matches!(value, Value::Int(v) if *v > 0) || matches!(value, Value::UInt(v) if *v > 0)
}

/// 收集用户变量及其推断类型的遍历器。
#[derive(Clone, Debug, Default)]
pub struct userVarTypeProcessor {
    pub user_vars: HashMap<String, FieldType>,
}
impl userVarTypeProcessor {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        if let Some(name) = expr.name.strip_prefix("user_var:") {
            if let Some(tp) = &expr.return_type {
                self.user_vars.insert(name.into(), tp.clone());
            }
        }
        true
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}

/// 将 grouping() 改写为基于 grouping_id 的位测试。
#[derive(Clone, Debug, Default)]
pub struct resolveGroupingTraverseAction {
    pub grouping_id_column: usize,
}
impl resolveGroupingTraverseAction {
    /// `Transform`：逻辑计划构建辅助。
    pub fn Transform(&self, mut expr: Expression) -> Expression {
        if expr.name == "grouping" {
            expr.name = "grouping_bit_test".into();
            expr.column = Some(self.grouping_id_column);
        }
        expr
    }
}

/// 从计划 labels 中提取 column: 前缀的列名。
fn schema_names(plan: &PlanNode) -> Vec<String> {
    plan.labels
        .keys()
        .filter_map(|k| k.strip_prefix("column:").map(str::to_string))
        .collect()
}
/// 按 Schema 下标构造列引用表达式列表。
fn column_exprs(plan: &PlanNode) -> Vec<Expression> {
    plan.schema
        .iter()
        .enumerate()
        .map(|(i, tp)| Expression {
            column: Some(i),
            return_type: Some(tp.clone()),
            ..Expression::default()
        })
        .collect()
}
/// 合并左右 Schema；左外连接时右侧清除 unsigned 以允许 NULL。
fn merge_schema(left: &PlanNode, right: &PlanNode, join: JoinType) -> Vec<FieldType> {
    let mut schema = left.schema.clone();
    let mut rhs = right.schema.clone();
    if matches!(join, JoinType::LeftOuter) {
        for tp in &mut rhs {
            tp.unsigned = false;
        }
    }
    schema.extend(rhs);
    schema
}
/// 表达式名或列下标是否匹配给定列名。
fn expr_matches_column(expr: &Expression, name: &str) -> bool {
    expr.name.eq_ignore_ascii_case(name) || expr.column.is_some_and(|c| c.to_string() == name)
}

impl PlanBuilder {
    /// 为 GROUPING SETS 构建 Expand 算子并改写 grouping()。
    pub fn buildExpand(
        &mut self,
        mut plan: PlanNode,
        group_items: &[Expression],
    ) -> Result<(PlanNode, Vec<Expression>)> {
        if group_items.is_empty() {
            return Ok((plan, Vec::new()));
        }
        let grouping_sets: Vec<_> = group_items
            .iter()
            .filter(|e| e.name.starts_with("grouping_set:"))
            .cloned()
            .collect();
        if grouping_sets.is_empty() {
            return Ok((plan, group_items.to_vec()));
        }
        let grouping_id = plan.schema.len();
        plan.schema.push(FieldType {
            code: TypeCode::UInt,
            flen: 64,
            decimal: 0,
            unsigned: true,
        });
        let mut expand = PlanNode::new(PlanKind::Expand);
        expand.group_items = grouping_sets.clone();
        expand.schema = plan.schema.clone();
        expand.stats = StatsInfo {
            row_count: plan.rows() * grouping_sets.len() as f64,
            ..plan.stats.clone()
        };
        expand.children = vec![plan];
        let items = group_items
            .iter()
            .cloned()
            .map(|e| {
                resolveGroupingTraverseAction {
                    grouping_id_column: grouping_id,
                }
                .Transform(e)
            })
            .collect();
        Ok((expand, items))
    }
    /// 构建 HashAgg，必要时先 Expand，并打开聚合相关优化标志。
    pub fn buildAggregation(
        &mut self,
        mut plan: PlanNode,
        agg_funcs: &[Expression],
        group_items: &[Expression],
    ) -> Result<PlanNode> {
        self.optFlag |= FlagBuildKeyInfo | FlagPushDownAgg;
        let (child, groups) = self.buildExpand(plan, group_items)?;
        plan = PlanNode::new(PlanKind::HashAgg);
        plan.agg_funcs = agg_funcs.to_vec();
        plan.group_items = groups.clone();
        plan.schema = agg_funcs
            .iter()
            .chain(&groups)
            .filter_map(|e| e.return_type.clone())
            .collect();
        plan.stats.row_count = child
            .rows()
            .min(child.rows() / groups.len().max(1) as f64)
            .max(1.0);
        plan.children = vec![child];
        Ok(plan)
    }
    /// 构建 FROM：有则走结果集，无则 TableDual。
    pub fn buildTableRefs(&mut self, from: Option<&ResultSet>) -> Result<PlanNode> {
        match from {
            Some(node) => self.buildResultSetNode(node, false),
            None => Ok(self.buildTableDual()),
        }
    }
    /// 按 ResultSet 变体分派构建表/连接/子查询/集合运算/VALUES/CTE。
    pub fn buildResultSetNode(&mut self, node: &ResultSet, is_cte: bool) -> Result<PlanNode> {
        match node {
            ResultSet::Table(table) => self.buildDataSource(table),
            ResultSet::Join(join) => self.buildJoin(join),
            ResultSet::Select(select) => self.buildSelect(select),
            ResultSet::SetOperation(selects) => self.buildSetOpr(selects),
            ResultSet::Values(rows) => {
                let mut dual = self.buildTableDual();
                dual.stats.row_count = rows.len() as f64;
                Ok(dual)
            }
            ResultSet::Cte(name) => self
                .tryBuildCTE(name, None)
                .and_then(|p| p.ok_or_else(|| BuilderError(format!("CTE {name} is not visible")))),
        }
        .map(|mut p| {
            if is_cte {
                p.labels.insert("is_cte".into(), 1.0);
            }
            p
        })
    }
    /// 构建普通 Join；遇 LATERAL 则转 Apply。
    pub fn buildJoin(&mut self, join: &JoinSpec) -> Result<PlanNode> {
        if join.lateral || containsLateralTableSource(&join.right) {
            return self.buildLateralJoin(join);
        }
        let left = self.buildResultSetNode(&join.left, false)?;
        let right = self.buildResultSetNode(&join.right, false)?;
        let mut plan = PlanNode::new(PlanKind::Other("LogicalJoin".into()));
        plan.join_type = join.join_type;
        plan.conditions = join.on.clone();
        plan.flags.keep_order = join.straight;
        plan.schema = merge_schema(&left, &right, join.join_type);
        plan.stats.row_count = match join.join_type {
            JoinType::Inner => left.rows() * right.rows() * 0.1,
            JoinType::LeftOuter => left.rows().max(left.rows() * right.rows() * 0.1),
            JoinType::RightOuter => right.rows().max(left.rows() * right.rows() * 0.1),
            JoinType::Semi | JoinType::AntiSemi => left.rows(),
        };
        plan.children = vec![left, right];
        if join.natural {
            self.buildNaturalJoin(&mut plan)?;
        } else if !join.using_columns.is_empty() {
            self.buildUsingClause(&mut plan, &join.using_columns)?;
        }
        Ok(plan)
    }
    /// 将 LATERAL 连接建成 Apply。
    pub fn buildLateralJoin(&mut self, join: &JoinSpec) -> Result<PlanNode> {
        let left = self.buildResultSetNode(&join.left, false)?;
        let right = self.buildResultSetNode(&join.right, false)?;
        Ok(self.buildApplyWithJoinType(left, right, join.join_type, self.noDecorrelate))
    }
    /// 为 USING 列添加等值条件并记录元数据。
    pub fn buildUsingClause(&self, plan: &mut PlanNode, using: &[String]) -> Result<()> {
        let left_len = plan.children.first().map(|p| p.schema.len()).unwrap_or(0);
        for name in using {
            let left = plan.children[0].labels.get(&format!("column:{name}"));
            let right = plan.children[1].labels.get(&format!("column:{name}"));
            if left.is_none() || right.is_none() {
                return Err(BuilderError(format!("unknown column {name} in USING")));
            }
            plan.conditions.push(Expression {
                name: "eq".into(),
                column: left.map(|v| *v as usize),
                ..Expression::default()
            });
        }
        plan.labels
            .insert("using_columns".into(), using.len() as f64);
        plan.labels
            .insert("left_schema_len".into(), left_len as f64);
        Ok(())
    }
    /// 按同名列集合转为 USING 连接。
    pub fn buildNaturalJoin(&self, plan: &mut PlanNode) -> Result<()> {
        let left: HashSet<_> = schema_names(&plan.children[0]).into_iter().collect();
        let right: HashSet<_> = schema_names(&plan.children[1]).into_iter().collect();
        let common: Vec<_> = left.intersection(&right).cloned().collect();
        self.buildUsingClause(plan, &common)
    }
    /// 标记已合并的公共列数量。
    pub fn coalesceCommonColumns(&self, plan: &mut PlanNode, filter: &HashSet<String>) {
        plan.labels
            .insert("coalesced_columns".into(), filter.len() as f64);
    }
    /// 构建 Selection；假条件退化为零行 Dual；可改写为 IndexScan。
    pub fn buildSelection(
        &mut self,
        mut plan: PlanNode,
        condition: Option<&Expression>,
    ) -> Result<PlanNode> {
        let Some(condition) = condition else {
            return Ok(plan);
        };
        if condition.name == "true" {
            return Ok(plan);
        }
        if condition.name == "false" {
            let mut dual = self.buildTableDual();
            dual.schema = plan.schema;
            dual.stats.row_count = 0.0;
            return Ok(dual);
        }
        self.optFlag |= FlagPredicatePushDown;
        if matches!(plan.kind, PlanKind::TableScan) {
            let predicate_identifiers = condition
                .name
                .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .filter(|identifier| !identifier.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<HashSet<_>>();
            let usable_index = plan.labels.keys().any(|label| {
                label
                    .strip_prefix("index-leading:")
                    .and_then(|metadata| metadata.rsplit_once(':'))
                    .is_some_and(|(_, column)| predicate_identifiers.contains(column))
            });
            if usable_index {
                plan.kind = PlanKind::IndexScan;
                plan.stats.row_count = (plan.rows() * 0.1).max(1.0);
            }
        }
        let mut select = PlanNode::new(PlanKind::Selection);
        select.conditions = vec![condition.clone()];
        select.schema = plan.schema.clone();
        select.stats.row_count = plan.rows() * 0.8;
        select.children = vec![plan];
        Ok(select)
    }
    /// 构建单个投影字段对应的表达式与输出名。
    pub fn buildProjectionField(
        &self,
        plan: &PlanNode,
        field: &SelectField,
    ) -> Result<(Expression, String)> {
        if field.wildcard {
            return Err(BuilderError(
                "wildcard must be unfolded before projection".into(),
            ));
        }
        let name = field.alias.clone().unwrap_or_else(|| {
            if field.expr.name.is_empty() {
                field
                    .expr
                    .column
                    .map(|i| format!("column_{i}"))
                    .unwrap_or_else(|| "expr".into())
            } else {
                field.expr.name.clone()
            }
        });
        if let Some(column) = field.expr.column {
            if column >= plan.schema.len() {
                return Err(BuilderError(format!("column {column} is out of range")));
            }
        }
        Ok((field.expr.clone(), name))
    }
    /// `buildProjectionFieldNameFromColumns`：逻辑计划构建辅助。
    pub fn buildProjectionFieldNameFromColumns(
        &self,
        field: &SelectField,
        original: &str,
    ) -> (String, String, String, String, String) {
        let column = field.alias.clone().unwrap_or_else(|| original.into());
        (
            column.clone(),
            original.into(),
            String::new(),
            String::new(),
            String::new(),
        )
    }
    /// `buildProjectionFieldNameFromExpressions`：逻辑计划构建辅助。
    pub fn buildProjectionFieldNameFromExpressions(&self, field: &SelectField) -> Result<String> {
        let name = field
            .alias
            .clone()
            .unwrap_or_else(|| field.expr.name.clone());
        if name.is_empty() {
            Err(BuilderError(
                "projection expression has no display name".into(),
            ))
        } else {
            Ok(name)
        }
    }
    /// `preprocessUserVarTypes`：逻辑计划构建辅助。
    pub fn preprocessUserVarTypes(&self, fields: &[SelectField]) -> HashMap<String, FieldType> {
        let mut processor = userVarTypeProcessor::default();
        for field in fields {
            processor.Enter(&field.expr);
        }
        processor.user_vars
    }
    /// `replaceGroupingFunc`：逻辑计划构建辅助。
    pub fn replaceGroupingFunc(&self, expr: Expression, grouping_id: usize) -> Expression {
        resolveGroupingTraverseAction {
            grouping_id_column: grouping_id,
        }
        .Transform(expr)
    }
    /// `implicitProjectGroupingSetCols`：逻辑计划构建辅助。
    pub fn implicitProjectGroupingSetCols(
        &self,
        plan: &mut PlanNode,
        expressions: &mut Vec<Expression>,
        names: &mut Vec<String>,
    ) {
        for item in &plan.group_items {
            if !expressions.iter().any(|e| e.column == item.column) {
                expressions.push(item.clone());
                names.push(format!("grouping_{}", names.len()));
                if let Some(tp) = &item.return_type {
                    plan.schema.push(tp.clone());
                }
            }
        }
    }
    /// `buildProjection`：逻辑计划构建辅助。
    pub fn buildProjection(&mut self, plan: PlanNode, fields: &[SelectField]) -> Result<PlanNode> {
        let fields = self.unfoldWildStar(&plan, fields)?;
        let mut expressions = Vec::new();
        let mut names = Vec::new();
        for field in &fields {
            let (expr, name) = self.buildProjectionField(&plan, field)?;
            expressions.push(expr);
            names.push(name);
        }
        let mut projection = PlanNode::new(PlanKind::Projection);
        projection.schema = expressions
            .iter()
            .map(|e| {
                e.return_type.clone().unwrap_or(FieldType {
                    code: TypeCode::Null,
                    flen: 0,
                    decimal: 0,
                    unsigned: false,
                })
            })
            .collect();
        projection.expressions = expressions;
        projection.stats = plan.stats.clone();
        for (idx, name) in names.iter().enumerate() {
            projection
                .labels
                .insert(format!("column:{name}"), idx as f64);
        }
        projection.children = vec![plan];
        Ok(projection)
    }
    /// PlanBuilder：委托 build_distinct_runtime。
    pub fn buildDistinct(&mut self, child: PlanNode, length: usize) -> Result<PlanNode> {
        if length > child.schema.len() {
            return Err(BuilderError("DISTINCT key length exceeds schema".into()));
        }
        self.optFlag |= FlagBuildKeyInfo | FlagPushDownAgg;
        let mut agg = PlanNode::new(PlanKind::HashAgg);
        agg.group_items = column_exprs(&child).into_iter().take(length).collect();
        agg.agg_funcs = column_exprs(&child)
            .into_iter()
            .map(|mut e| {
                e.name = "first_row".into();
                e
            })
            .collect();
        agg.schema = child.schema.clone();
        agg.stats.row_count = child.rows().min(child.rows() / 2.0).max(1.0);
        agg.children = vec![child];
        Ok(agg)
    }
    /// `buildProjection4Union`：逻辑计划构建辅助。
    pub fn buildProjection4Union(&self, union: &mut PlanNode) -> Result<()> {
        let Some(first) = union.children.first() else {
            return Err(BuilderError("UNION requires children".into()));
        };
        let width = first.schema.len();
        if union
            .children
            .iter()
            .any(|child| child.schema.len() != width)
        {
            return Err(BuilderError(
                "UNION operands have different column counts".into(),
            ));
        }
        let schema = (0..width)
            .map(|index| {
                union
                    .children
                    .iter()
                    .skip(1)
                    .fold(first.schema[index].clone(), |result, child| {
                        unionJoinFieldType(&result, &child.schema[index])
                    })
            })
            .collect::<Vec<_>>();
        for child in &mut union.children {
            if child.schema != schema {
                let old = std::mem::replace(child, PlanNode::new(PlanKind::Projection));
                child.schema = schema.clone();
                child.expressions = column_exprs(&old);
                child.children = vec![old];
            }
        }
        union.schema = schema;
        Ok(())
    }
    /// `setUnionFlen`：逻辑计划构建辅助。
    pub fn setUnionFlen(&self, result: &mut FieldType, columns: &[Expression]) {
        result.flen = columns
            .iter()
            .filter_map(|e| e.return_type.as_ref().map(|t| t.flen))
            .max()
            .unwrap_or(result.flen);
        result.decimal = columns
            .iter()
            .filter_map(|e| e.return_type.as_ref().map(|t| t.decimal))
            .max()
            .unwrap_or(result.decimal);
    }
    /// `buildSetOpr`：逻辑计划构建辅助。
    pub fn buildSetOpr(&mut self, selects: &[(SetOprType, SelectStmt)]) -> Result<PlanNode> {
        let mut plans = Vec::new();
        let mut operations = Vec::new();
        for (op, select) in selects {
            plans.push(self.buildSelect(select)?);
            operations.push(*op);
        }
        let mut result = plans.remove(0);
        for (op, rhs) in operations.into_iter().skip(1).zip(plans) {
            result = match op {
                SetOprType::Union | SetOprType::UnionAll => {
                    self.buildUnion(vec![result, rhs], op == SetOprType::Union)?
                }
                SetOprType::Intersect | SetOprType::IntersectAll => {
                    self.buildSemiJoinForSetOperator(result, rhs, false)?
                }
                SetOprType::Except | SetOprType::ExceptAll => {
                    self.buildSemiJoinForSetOperator(result, rhs, true)?
                }
            };
        }
        Ok(result)
    }
    /// `buildSemiJoinForSetOperator`：逻辑计划构建辅助。
    pub fn buildSemiJoinForSetOperator(
        &mut self,
        left: PlanNode,
        right: PlanNode,
        anti: bool,
    ) -> Result<PlanNode> {
        if left.schema.len() != right.schema.len() {
            return Err(BuilderError(
                "set operands have different column counts".into(),
            ));
        }
        self.buildSemiJoin(left, right, Vec::new(), false, anti, false)
    }
    /// `buildIntersect`：逻辑计划构建辅助。
    pub fn buildIntersect(&mut self, selects: Vec<PlanNode>) -> Result<PlanNode> {
        selects
            .into_iter()
            .reduce(|left, right| {
                self.buildSemiJoinForSetOperator(left, right, false)
                    .unwrap()
            })
            .ok_or_else(|| BuilderError("INTERSECT needs input".into()))
    }
    /// `buildExcept`：逻辑计划构建辅助。
    pub fn buildExcept(&mut self, mut selects: Vec<PlanNode>) -> Result<PlanNode> {
        let first = selects
            .drain(..1)
            .next()
            .ok_or_else(|| BuilderError("EXCEPT needs input".into()))?;
        selects.into_iter().try_fold(first, |left, right| {
            self.buildSemiJoinForSetOperator(left, right, true)
        })
    }
    /// `buildUnion`：逻辑计划构建辅助。
    pub fn buildUnion(&mut self, selects: Vec<PlanNode>, distinct: bool) -> Result<PlanNode> {
        let mut union = self.buildUnionAll(selects)?;
        if distinct {
            let length = union.schema.len();
            union = self.buildDistinct(union, length)?;
        }
        Ok(union)
    }
    /// `divideUnionSelectPlans`：逻辑计划构建辅助。
    pub fn divideUnionSelectPlans(
        &self,
        selects: Vec<PlanNode>,
        operations: &[SetOprType],
    ) -> (Vec<PlanNode>, Vec<PlanNode>) {
        let mut distinct = Vec::new();
        let mut all = Vec::new();
        for (idx, plan) in selects.into_iter().enumerate() {
            if operations
                .get(idx)
                .is_some_and(|op| *op == SetOprType::Union)
            {
                distinct.push(plan);
            } else {
                all.push(plan);
            }
        }
        (distinct, all)
    }
    /// `buildUnionAll`：逻辑计划构建辅助。
    pub fn buildUnionAll(&mut self, plans: Vec<PlanNode>) -> Result<PlanNode> {
        if plans.is_empty() {
            return Err(BuilderError("UNION ALL needs input".into()));
        }
        let mut union = PlanNode::new(PlanKind::UnionAll);
        union.children = plans;
        self.buildProjection4Union(&mut union)?;
        union.stats.row_count = union.children.iter().map(PlanNode::rows).sum();
        Ok(union)
    }
    /// `buildSort`：逻辑计划构建辅助。
    pub fn buildSort(&mut self, plan: PlanNode, items: &[ByItem]) -> Result<PlanNode> {
        self.buildSortWithCheck(plan, items, false, 0)
    }
    /// `buildSortWithCheck`：逻辑计划构建辅助。
    pub fn buildSortWithCheck(
        &mut self,
        plan: PlanNode,
        items: &[ByItem],
        distinct: bool,
        visible_length: usize,
    ) -> Result<PlanNode> {
        if distinct {
            for (idx, item) in items.iter().enumerate() {
                self.checkOrderByInDistinct(item, idx, &plan, visible_length)?;
            }
        }
        let mut sort = PlanNode::new(PlanKind::Sort);
        sort.by_items = items
            .iter()
            .map(|i| {
                let mut e = i.expr.clone();
                e.name = if i.desc {
                    format!("desc:{}", e.name)
                } else {
                    format!("asc:{}", e.name)
                };
                e
            })
            .collect();
        sort.schema = plan.schema.clone();
        sort.stats = plan.stats.clone();
        sort.children = vec![plan];
        Ok(sort)
    }
    /// `checkOrderByInDistinct`：逻辑计划构建辅助。
    pub fn checkOrderByInDistinct(
        &self,
        item: &ByItem,
        idx: usize,
        plan: &PlanNode,
        length: usize,
    ) -> Result<()> {
        if item
            .expr
            .column
            .is_some_and(|c| c >= length.min(plan.schema.len()))
        {
            Err(BuilderError(format!(
                "ORDER BY expression {idx} is not in SELECT DISTINCT list"
            )))
        } else {
            Ok(())
        }
    }
    /// PlanBuilder：委托 build_limit_runtime。
    pub fn buildLimit(&mut self, plan: PlanNode, limit: &LimitClause) -> Result<PlanNode> {
        self.optFlag |= FlagPushDownTopN;
        let offset = extractLimitValue(limit.offset.as_ref(), 0)?;
        let count = extractLimitValue(limit.count.as_ref(), u64::MAX)?.min(u64::MAX - offset);
        if offset.saturating_add(count) == 0 {
            let mut dual = self.buildTableDual();
            dual.schema = plan.schema;
            return Ok(dual);
        }
        let mut node = PlanNode::new(PlanKind::Limit);
        node.offset = offset;
        node.count = count;
        node.schema = plan.schema.clone();
        node.stats.row_count = plan
            .rows()
            .saturating_sub_f64(offset as f64)
            .min(count as f64);
        node.children = vec![plan];
        Ok(node)
    }
    /// `resolveHavingAndOrderBy`：逻辑计划构建辅助。
    pub fn resolveHavingAndOrderBy(
        &self,
        select: &SelectStmt,
        plan: &PlanNode,
    ) -> Result<(Option<Expression>, Vec<ByItem>)> {
        let fields: Vec<_> = select.fields.iter().map(|f| f.expr.clone()).collect();
        let resolve = |mut expr: Expression| {
            if expr
                .name
                .parse::<usize>()
                .ok()
                .is_some_and(|i| i > 0 && i <= fields.len())
            {
                expr = fields[expr.name.parse::<usize>().unwrap() - 1].clone();
            }
            expr
        };
        let having = select.having.clone().map(resolve);
        let order: Vec<ByItem> = select
            .order_by
            .iter()
            .cloned()
            .map(|mut item| {
                item.expr = resolve(item.expr);
                item
            })
            .collect();
        if plan.schema.is_empty() && (!order.is_empty() || having.is_some()) {
            return Err(BuilderError(
                "cannot resolve HAVING/ORDER BY on empty schema".into(),
            ));
        }
        Ok((having, order))
    }
    /// `extractAggFuncsInExprs`：逻辑计划构建辅助。
    pub fn extractAggFuncsInExprs(
        &self,
        exprs: &[Expression],
    ) -> (Vec<Expression>, HashMap<String, usize>) {
        let funcs: Vec<_> = exprs
            .iter()
            .filter(|e| e.name.starts_with("agg:"))
            .cloned()
            .collect();
        let map = funcs
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect();
        (funcs, map)
    }
    /// `extractAggFuncsInSelectFields`：逻辑计划构建辅助。
    pub fn extractAggFuncsInSelectFields(
        &self,
        fields: &[SelectField],
    ) -> (Vec<Expression>, HashMap<String, usize>) {
        self.extractAggFuncsInExprs(&fields.iter().map(|f| f.expr.clone()).collect::<Vec<_>>())
    }
    /// `extractAggFuncsInByItems`：逻辑计划构建辅助。
    pub fn extractAggFuncsInByItems(&self, items: &[ByItem]) -> Vec<Expression> {
        self.extractAggFuncsInExprs(&items.iter().map(|i| i.expr.clone()).collect::<Vec<_>>())
            .0
    }
    /// `extractCorrelatedAggFuncs`：逻辑计划构建辅助。
    pub fn extractCorrelatedAggFuncs(&self, agg: &[Expression]) -> Vec<Expression> {
        agg.iter()
            .filter(|e| e.name.contains("correlated"))
            .cloned()
            .collect()
    }
    /// `resolveWindowFunction`：逻辑计划构建辅助。
    pub fn resolveWindowFunction(&self, select: &SelectStmt) -> Vec<WindowFunc> {
        select
            .fields
            .iter()
            .filter(|f| f.expr.name.starts_with("window:"))
            .map(|f| WindowFunc {
                name: f.expr.name.clone(),
                args: Vec::new(),
                spec_name: None,
                spec: select.windows.first().cloned(),
            })
            .collect()
    }
    /// `resolveCorrelatedAggregates`：逻辑计划构建辅助。
    pub fn resolveCorrelatedAggregates(&self, select: &SelectStmt) -> HashMap<String, usize> {
        self.extractCorrelatedAggFuncs(&self.extractAggFuncsInSelectFields(&select.fields).0)
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect()
    }
    /// `checkOnlyFullGroupBy`：逻辑计划构建辅助。
    pub fn checkOnlyFullGroupBy(&self, plan: &PlanNode, select: &SelectStmt) -> Result<()> {
        let groups: HashSet<_> = select.group_by.iter().filter_map(|e| e.column).collect();
        for field in &select.fields {
            if !field.expr.name.starts_with("agg:")
                && field.expr.column.is_some_and(|c| !groups.contains(&c))
            {
                return Err(BuilderError(format!(
                    "column {} is not in GROUP BY",
                    field.expr.name
                )));
            }
        }
        if groups.is_empty()
            && self.extractAggFuncsInSelectFields(&select.fields).0.len() > 0
            && plan.rows() > 1.0
        {
            self.checkOnlyFullGroupByWithOutGroupClause(select)?;
        }
        Ok(())
    }
    /// `checkOnlyFullGroupByWithGroupClause`：逻辑计划构建辅助。
    pub fn checkOnlyFullGroupByWithGroupClause(&self, select: &SelectStmt) -> Result<()> {
        self.checkOnlyFullGroupBy(&PlanNode::default(), select)
    }
    /// `checkOnlyFullGroupByWithOutGroupClause`：逻辑计划构建辅助。
    pub fn checkOnlyFullGroupByWithOutGroupClause(&self, select: &SelectStmt) -> Result<()> {
        if select
            .fields
            .iter()
            .any(|f| !f.expr.name.starts_with("agg:") && f.expr.column.is_some())
        {
            Err(BuilderError(
                "mixing aggregate and nonaggregate columns without GROUP BY".into(),
            ))
        } else {
            Ok(())
        }
    }
    /// `resolveGbyExprs`：逻辑计划构建辅助。
    pub fn resolveGbyExprs(
        &self,
        group_by: &[Expression],
        fields: &[SelectField],
    ) -> Result<Vec<Expression>> {
        group_by
            .iter()
            .map(|expr| {
                if let Ok(position) = expr.name.parse::<usize>() {
                    fields
                        .get(position.saturating_sub(1))
                        .map(|f| f.expr.clone())
                        .ok_or_else(|| BuilderError("GROUP BY position out of range".into()))
                } else {
                    Ok(expr.clone())
                }
            })
            .collect()
    }
    /// `rewriteGbyExprs`：逻辑计划构建辅助。
    pub fn rewriteGbyExprs(
        &self,
        plan: PlanNode,
        items: Vec<Expression>,
    ) -> Result<(PlanNode, Vec<Expression>, bool)> {
        let rollup = items.iter().any(|e| e.name.starts_with("rollup:"));
        Ok((plan, items, rollup))
    }
    /// 展开 SELECT * / t.* 通配符。
    pub fn unfoldWildStar(
        &self,
        plan: &PlanNode,
        fields: &[SelectField],
    ) -> Result<Vec<SelectField>> {
        let mut out = Vec::new();
        for field in fields {
            if !field.wildcard {
                out.push(field.clone());
                continue;
            }
            for (index, tp) in plan.schema.iter().enumerate() {
                out.push(SelectField {
                    expr: Expression {
                        column: Some(index),
                        return_type: Some(tp.clone()),
                        ..Expression::default()
                    },
                    alias: None,
                    wildcard: false,
                    table_wildcard: None,
                });
            }
        }
        Ok(out)
    }
    /// `addAliasName`：逻辑计划构建辅助。
    pub fn addAliasName(&self, fields: &mut [SelectField]) {
        for field in fields {
            if field.alias.is_none() && !field.expr.name.is_empty() {
                field.alias = Some(field.expr.name.clone());
            }
        }
    }
    /// `pushTableHints`：逻辑计划构建辅助。
    pub fn pushTableHints(&mut self, hints: &[String], current_level: i32) {
        self.unusedViewHints
            .extend(hints.iter().map(|h| format!("{current_level}:{h}")));
    }
    /// `popVisitInfo`：逻辑计划构建辅助。
    pub fn popVisitInfo(&mut self, count: usize) {
        self.visitInfo
            .truncate(self.visitInfo.len().saturating_sub(count));
    }
    /// `popTableHints`：逻辑计划构建辅助。
    pub fn popTableHints(&mut self, count: usize) {
        self.unusedViewHints
            .truncate(self.unusedViewHints.len().saturating_sub(count));
    }
    /// `TableHints`：逻辑计划构建辅助。
    pub fn TableHints(&self) -> &[String] {
        &self.unusedViewHints
    }
    /// `buildSelect`：逻辑计划构建辅助。
    pub fn buildSelect(&mut self, select: &SelectStmt) -> Result<PlanNode> {
        let ctes = self.buildWith(&select.with)?;
        let mut plan = self.buildTableRefs(select.from.as_ref())?;
        plan = self.buildSelection(plan, select.where_clause.as_ref())?;
        let (agg, _) = self.extractAggFuncsInSelectFields(&select.fields);
        if !agg.is_empty() || !select.group_by.is_empty() {
            plan = self.buildAggregation(plan, &agg, &select.group_by)?;
        }
        plan = self.buildProjection(plan, &select.fields)?;
        if select.distinct {
            let length = plan.schema.len();
            plan = self.buildDistinct(plan, length)?;
        }
        let (having, order) = self.resolveHavingAndOrderBy(select, &plan)?;
        plan = self.buildSelection(plan, having.as_ref())?;
        if !select.windows.is_empty() {
            let funcs = self.resolveWindowFunction(select);
            plan = self.buildWindowFunctions(plan, &funcs, &select.windows)?;
        }
        if !order.is_empty() {
            plan = self.buildSort(plan, &order)?;
        }
        if let Some(limit) = &select.limit {
            plan = self.buildLimit(plan, limit)?;
        }
        Ok(self.tryToBuildSequence(&ctes, plan))
    }
    /// `tryToBuildSequence`：逻辑计划构建辅助。
    pub fn tryToBuildSequence(&self, ctes: &[cteInfo], plan: PlanNode) -> PlanNode {
        let materialized: Vec<_> = ctes
            .iter()
            .filter(|c| c.useRecursive || !c.nonRecursive)
            .map(|c| {
                let mut p = PlanNode::new(PlanKind::CteStorage);
                p.labels.insert(format!("cte:{}", c.name), 1.0);
                p
            })
            .collect();
        if materialized.is_empty() {
            plan
        } else {
            let mut sequence = PlanNode::new(PlanKind::Sequence);
            sequence.children = materialized
                .into_iter()
                .chain(std::iter::once(plan.clone()))
                .collect();
            sequence.schema = plan.schema;
            sequence.stats = plan.stats;
            sequence
        }
    }
    /// `buildTableDual`：逻辑计划构建辅助。
    pub fn buildTableDual(&self) -> PlanNode {
        let mut dual = PlanNode::new(PlanKind::Other("TableDual".into()));
        dual.stats.row_count = 1.0;
        dual
    }
    /// `tryBuildCTE`：逻辑计划构建辅助。
    pub fn tryBuildCTE(&self, name: &str, alias: Option<&str>) -> Result<Option<PlanNode>> {
        let Some(cte) = self
            .outerCTEs
            .iter()
            .rev()
            .find(|c| c.name.eq_ignore_ascii_case(name) && (!c.nonRecursive || !c.isBuilding))
        else {
            return Ok(None);
        };
        if cte.enterSubquery && cte.isBuilding {
            return Err(BuilderError(format!(
                "recursive CTE {name} cannot be referenced from subquery"
            )));
        }
        let mut plan = PlanNode::new(PlanKind::Cte);
        plan.labels
            .insert(format!("cte:{}", alias.unwrap_or(name)), 1.0);
        Ok(Some(plan))
    }
    /// `computeCTEInlineFlag`：逻辑计划构建辅助。
    pub fn computeCTEInlineFlag(&self, cte: &CteDef, reference_count: usize) -> bool {
        cte.materialized == Some(false)
            || (cte.materialized.is_none() && cte.recursive.is_none() && reference_count <= 1)
    }
    /// `buildDataSourceFromCTEMerge`：逻辑计划构建辅助。
    pub fn buildDataSourceFromCTEMerge(&mut self, cte: &CteDef) -> Result<PlanNode> {
        self.buildSelect(&cte.seed)
    }
    /// `buildDataSource`：逻辑计划构建辅助。
    pub fn buildDataSource(&mut self, source: &TableSource) -> Result<PlanNode> {
        let mut scan = PlanNode::new(PlanKind::TableScan);
        scan.schema = source
            .table
            .columns
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| c.field_type.clone())
            .collect();
        scan.stats.row_count = 1000.0;
        for (idx, col) in source
            .table
            .columns
            .iter()
            .filter(|c| !c.hidden)
            .enumerate()
        {
            scan.labels
                .insert(format!("column:{}", col.name), idx as f64);
        }
        for index in source
            .table
            .indices
            .iter()
            .filter(|index| !index.invisible && !index.vector)
        {
            if let Some(column) = index
                .columns
                .first()
                .and_then(|offset| source.table.columns.get(*offset))
            {
                scan.labels.insert(
                    format!(
                        "index-leading:{}:{}",
                        index.name.to_ascii_lowercase(),
                        column.name.to_ascii_lowercase()
                    ),
                    index.id as f64,
                );
            }
        }
        if source.prefer_tiflash && source.prefer_tikv {
            self.warnings
                .push("conflicting READ_FROM_STORAGE hints".into());
        }
        scan.store = if source.prefer_tiflash {
            crate::task::StoreType::TiFlash
        } else {
            crate::task::StoreType::TiKv
        };
        self.visitInfo.push(visitInfo {
            privilege: Privilege::Select,
            db: source.table.db.clone(),
            table: source.table.name.clone(),
            column: String::new(),
            error: String::new(),
            alterWritable: false,
            dynamicPrivs: Vec::new(),
            dynamicWithGrant: false,
        });
        Ok(scan)
    }
    /// `buildMemTable`：逻辑计划构建辅助。
    pub fn buildMemTable(&mut self, table: &TableInfo) -> Result<PlanNode> {
        let mut plan = self.buildDataSource(&TableSource {
            table: table.clone(),
            alias: None,
            lateral: false,
            prefer_tiflash: false,
            prefer_tikv: false,
        })?;
        plan.kind = PlanKind::Other("MemTable".into());
        Ok(plan)
    }
    /// `timeRangeForSummaryTable`：逻辑计划构建辅助。
    pub fn timeRangeForSummaryTable(&self, now: i64, duration_seconds: i64) -> (i64, i64) {
        (now.saturating_sub(duration_seconds.max(0)), now)
    }
    /// `checkRecursiveView`：逻辑计划构建辅助。
    pub fn checkRecursiveView(&self, db: &str, table: &str) -> Result<()> {
        if self
            .outerCTEs
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(table) && c.isBuilding)
        {
            Err(BuilderError(format!("recursive view {db}.{table}")))
        } else {
            Ok(())
        }
    }
    /// `BuildDataSourceFromView`：逻辑计划构建辅助。
    pub fn BuildDataSourceFromView(
        &mut self,
        table: &TableInfo,
        select: &SelectStmt,
    ) -> Result<PlanNode> {
        self.checkRecursiveView(&table.db, &table.name)?;
        let plan = self.buildSelect(select)?;
        self.buildProjUponView(table, plan)
    }
    /// `buildProjUponView`：逻辑计划构建辅助。
    pub fn buildProjUponView(&self, table: &TableInfo, plan: PlanNode) -> Result<PlanNode> {
        if table.columns.len() != plan.schema.len() {
            return Err(BuilderError("view column count differs from query".into()));
        }
        let mut projection = PlanNode::new(PlanKind::Projection);
        projection.schema = table.columns.iter().map(|c| c.field_type.clone()).collect();
        projection.expressions = column_exprs(&plan);
        projection.children = vec![plan];
        Ok(projection)
    }
    /// 构造 LogicalApply 并打开解相关等相关优化标志。
    pub fn buildApplyWithJoinType(
        &mut self,
        outer: PlanNode,
        inner: PlanNode,
        join_type: JoinType,
        mark_no_decorrelate: bool,
    ) -> PlanNode {
        self.optFlag |=
            FlagPredicatePushDown | FlagBuildKeyInfo | FlagDecorrelate | FlagConstantPropagation;
        if matches!(join_type, JoinType::LeftOuter) {
            self.optFlag |= FlagEliminateOuterJoin;
        }
        let mut apply = PlanNode::new(PlanKind::Apply);
        apply.join_type = join_type;
        apply.schema = merge_schema(&outer, &inner, join_type);
        apply.flags.use_cache = !mark_no_decorrelate;
        apply.children = vec![outer, inner];
        apply
    }
    /// 构造半连接形态的 LogicalApply。
    pub fn buildSemiApply(
        &mut self,
        outer: PlanNode,
        inner: PlanNode,
        conditions: Vec<Expression>,
        as_scalar: bool,
        not: bool,
        force: bool,
    ) -> Result<PlanNode> {
        let join = self.buildSemiJoin(outer, inner, conditions, as_scalar, not, force)?;
        Ok(self.buildApplyWithJoinType(
            join.children[0].clone(),
            join.children[1].clone(),
            join.join_type,
            self.noDecorrelate,
        ))
    }
    /// 包装 MaxOneRow 算子并清除 NOT NULL。
    pub fn buildMaxOneRow(&self, plan: PlanNode) -> PlanNode {
        let mut max = PlanNode::new(PlanKind::Other("MaxOneRow".into()));
        max.schema = plan.schema.clone();
        max.stats.row_count = plan.rows().min(1.0);
        max.children = vec![plan];
        max
    }
    /// 构造半/反半连接 LogicalJoin。
    pub fn buildSemiJoin(
        &mut self,
        outer: PlanNode,
        inner: PlanNode,
        conditions: Vec<Expression>,
        as_scalar: bool,
        not: bool,
        force_rewrite: bool,
    ) -> Result<PlanNode> {
        let mut join = PlanNode::new(PlanKind::Other("LogicalJoin".into()));
        join.join_type = if not {
            JoinType::AntiSemi
        } else {
            JoinType::Semi
        };
        join.conditions = conditions;
        join.schema = outer.schema.clone();
        if as_scalar {
            join.schema.push(FieldType {
                code: TypeCode::Int,
                flen: 1,
                decimal: 0,
                unsigned: false,
            });
        }
        join.stats.row_count = outer.rows();
        join.labels.insert(
            "force_rewrite".into(),
            if force_rewrite { 1.0 } else { 0.0 },
        );
        join.children = vec![outer, inner];
        Ok(join)
    }
    /// `buildProjectionForWindow`：逻辑计划构建辅助。
    pub fn buildProjectionForWindow(
        &self,
        plan: PlanNode,
        spec: &WindowSpec,
        args: &[Expression],
    ) -> PlanNode {
        let mut projection = PlanNode::new(PlanKind::Projection);
        projection.expressions = column_exprs(&plan);
        for expr in spec
            .partition_by
            .iter()
            .chain(spec.order_by.iter().map(|i| &i.expr))
            .chain(args)
        {
            if !projection
                .expressions
                .iter()
                .any(|e| e.name == expr.name && e.column == expr.column)
            {
                projection.expressions.push(expr.clone());
            }
        }
        projection.schema = projection
            .expressions
            .iter()
            .filter_map(|e| e.return_type.clone())
            .collect();
        projection.children = vec![plan];
        projection
    }
    /// `buildArgs4WindowFunc`：逻辑计划构建辅助。
    pub fn buildArgs4WindowFunc(&self, args: &[Expression]) -> Result<Vec<Expression>> {
        if args.iter().any(|a| a.name == "invalid") {
            Err(BuilderError("invalid window argument".into()))
        } else {
            Ok(args.to_vec())
        }
    }
    /// `buildByItemsForWindow`：逻辑计划构建辅助。
    pub fn buildByItemsForWindow(&self, items: &[ByItem]) -> Vec<Expression> {
        items
            .iter()
            .map(|i| {
                let mut e = i.expr.clone();
                if i.desc {
                    e.name = format!("desc:{}", e.name);
                }
                e
            })
            .collect()
    }
    /// `buildWindowFunctionFrameBound`：逻辑计划构建辅助。
    pub fn buildWindowFunctionFrameBound(
        &self,
        bound: &FrameBound,
        order_items: &[ByItem],
    ) -> Result<FrameBound> {
        if matches!(bound.kind, BoundKind::Preceding | BoundKind::Following)
            && bound.value.is_none()
        {
            return Err(BuilderError("window frame bound requires value".into()));
        }
        if !order_items.is_empty() && bound.value.is_some_and(|v| v < 0) {
            return Err(BuilderError(
                "window frame offset must be nonnegative".into(),
            ));
        }
        Ok(bound.clone())
    }
    /// `buildWindowFunctionFrame`：逻辑计划构建辅助。
    pub fn buildWindowFunctionFrame(&self, spec: &WindowSpec) -> Result<Option<WindowFrame>> {
        let Some(frame) = &spec.frame else {
            return Ok(None);
        };
        let start = self.buildWindowFunctionFrameBound(&frame.start, &spec.order_by)?;
        let end = self.buildWindowFunctionFrameBound(&frame.end, &spec.order_by)?;
        if matches!(start.kind, BoundKind::UnboundedFollowing)
            || matches!(end.kind, BoundKind::UnboundedPreceding)
        {
            return Err(BuilderError("invalid window frame".into()));
        }
        Ok(Some(WindowFrame {
            rows: frame.rows,
            start,
            end,
        }))
    }
    /// `checkWindowFuncArgs`：逻辑计划构建辅助。
    pub fn checkWindowFuncArgs(&self, funcs: &[WindowFunc]) -> Result<()> {
        for function in funcs {
            let n = function.args.len();
            if matches!(
                function.name.to_ascii_lowercase().as_str(),
                "row_number" | "rank" | "dense_rank"
            ) && n != 0
            {
                return Err(BuilderError(format!(
                    "{} takes no arguments",
                    function.name
                )));
            }
            if matches!(function.name.to_ascii_lowercase().as_str(), "lead" | "lag")
                && !(1..=3).contains(&n)
            {
                return Err(BuilderError(format!(
                    "{} takes one to three arguments",
                    function.name
                )));
            }
        }
        Ok(())
    }
    /// `buildWindowFunctions`：逻辑计划构建辅助。
    pub fn buildWindowFunctions(
        &mut self,
        mut plan: PlanNode,
        funcs: &[WindowFunc],
        specs: &[WindowSpec],
    ) -> Result<PlanNode> {
        self.checkWindowFuncArgs(funcs)?;
        let grouped = self.groupWindowFuncs(funcs, specs)?;
        for group in grouped {
            let args: Vec<_> = group.funcs.iter().flat_map(|f| f.args.clone()).collect();
            plan = self.buildProjectionForWindow(plan, &group.spec, &args);
            let mut window = PlanNode::new(PlanKind::Window);
            window.expressions = group
                .funcs
                .iter()
                .map(|f| Expression {
                    name: f.name.clone(),
                    return_type: Some(FieldType {
                        code: TypeCode::Float,
                        flen: 22,
                        decimal: 6,
                        unsigned: false,
                    }),
                    ..Expression::default()
                })
                .collect();
            window.schema = plan.schema.clone();
            window.schema.extend(
                window
                    .expressions
                    .iter()
                    .filter_map(|e| e.return_type.clone()),
            );
            window.by_items = self.buildByItemsForWindow(&group.spec.order_by);
            window.children = vec![plan];
            plan = window;
        }
        Ok(plan)
    }
    /// `checkOriginWindowFuncs`：逻辑计划构建辅助。
    pub fn checkOriginWindowFuncs(&self, funcs: &[WindowFunc], order: &[ByItem]) -> Result<()> {
        if funcs.iter().any(|f| f.name.eq_ignore_ascii_case("ntile")) && order.is_empty() {
            Err(BuilderError("NTILE requires ORDER BY".into()))
        } else {
            Ok(())
        }
    }
    /// `checkOriginWindowSpec`：逻辑计划构建辅助。
    pub fn checkOriginWindowSpec(&self, spec: &WindowSpec) -> Result<()> {
        if spec.name.is_empty() && spec.reference.as_deref() == Some("") {
            Err(BuilderError("window reference is empty".into()))
        } else {
            Ok(())
        }
    }
    /// `checkOriginWindowFrameBound`：逻辑计划构建辅助。
    pub fn checkOriginWindowFrameBound(&self, bound: &FrameBound, spec: &WindowSpec) -> Result<()> {
        self.buildWindowFunctionFrameBound(bound, &spec.order_by)
            .map(|_| ())
    }
    /// `handleDefaultFrame`：逻辑计划构建辅助。
    pub fn handleDefaultFrame(&self, spec: &WindowSpec, function: &str) -> (WindowSpec, bool) {
        if spec.frame.is_some() {
            return (spec.clone(), false);
        }
        let mut spec = spec.clone();
        let ordered = !spec.order_by.is_empty();
        spec.frame = Some(WindowFrame {
            rows: !ordered,
            start: FrameBound {
                kind: BoundKind::UnboundedPreceding,
                value: None,
            },
            end: FrameBound {
                kind: if ordered {
                    BoundKind::CurrentRow
                } else {
                    BoundKind::UnboundedFollowing
                },
                value: None,
            },
        });
        (spec, function.eq_ignore_ascii_case("row_number"))
    }
    /// `groupWindowFuncs`：逻辑计划构建辅助。
    pub fn groupWindowFuncs(
        &self,
        funcs: &[WindowFunc],
        specs: &[WindowSpec],
    ) -> Result<Vec<windowFuncs>> {
        let map = buildWindowSpecs(specs)?;
        let mut groups: Vec<windowFuncs> = Vec::new();
        for function in funcs {
            let spec = if let Some(spec) = &function.spec {
                spec.clone()
            } else if let Some(name) = &function.spec_name {
                map.get(&getWindowName(name))
                    .cloned()
                    .ok_or_else(|| BuilderError(format!("unknown window {name}")))?
            } else {
                WindowSpec {
                    name: String::new(),
                    reference: None,
                    partition_by: Vec::new(),
                    order_by: Vec::new(),
                    frame: None,
                }
            };
            if let Some(group) = groups.iter_mut().find(|g| specEqual(&g.spec, &spec)) {
                group.funcs.push(function.clone());
            } else {
                groups.push(windowFuncs {
                    spec,
                    funcs: vec![function.clone()],
                });
            }
        }
        Ok(groups)
    }
    /// `buildCte`：逻辑计划构建辅助。
    pub fn buildCte(&mut self, cte: &CteDef, recursive: bool) -> Result<PlanNode> {
        let info = cteInfo {
            name: cte.name.clone(),
            nonRecursive: !recursive,
            useRecursive: recursive,
            isBuilding: true,
            enterSubquery: false,
        };
        self.outerCTEs.push(info);
        let mut plan = if recursive {
            self.buildRecursiveCTE(cte)?
        } else {
            self.buildSelect(&cte.seed)?
        };
        self.adjustCTEPlanOutputName(&mut plan, cte)?;
        if let Some(info) = self.outerCTEs.last_mut() {
            info.isBuilding = false;
        }
        Ok(plan)
    }
    /// `buildRecursiveCTE`：逻辑计划构建辅助。
    pub fn buildRecursiveCTE(&mut self, cte: &CteDef) -> Result<PlanNode> {
        let seed = self.buildSelect(&cte.seed)?;
        let recursive = self.buildSelect(
            cte.recursive
                .as_deref()
                .ok_or_else(|| BuilderError("recursive CTE requires recursive term".into()))?,
        )?;
        self.buildProjection4CTEUnion(seed, recursive)
    }
    /// `adjustCTEPlanOutputName`：逻辑计划构建辅助。
    pub fn adjustCTEPlanOutputName(&self, plan: &mut PlanNode, cte: &CteDef) -> Result<()> {
        if !cte.columns.is_empty() && cte.columns.len() != plan.schema.len() {
            return Err(BuilderError("CTE column count mismatch".into()));
        }
        for (idx, name) in cte.columns.iter().enumerate() {
            plan.labels.insert(format!("column:{name}"), idx as f64);
        }
        Ok(())
    }
    /// `prepareCTECheckForSubQuery`：逻辑计划构建辅助。
    pub fn prepareCTECheckForSubQuery(&mut self) -> Vec<usize> {
        let mut modified = Vec::new();
        for (idx, cte) in self.outerCTEs.iter_mut().enumerate() {
            if cte.isBuilding && !cte.enterSubquery {
                cte.enterSubquery = true;
                modified.push(idx);
            }
        }
        modified
    }
    /// `genCTETableNameForError`：逻辑计划构建辅助。
    pub fn genCTETableNameForError(&self) -> String {
        self.outerCTEs
            .iter()
            .rev()
            .find(|c| c.isBuilding)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "CTE".into())
    }
    /// `buildWith`：逻辑计划构建辅助。
    pub fn buildWith(&mut self, definitions: &[CteDef]) -> Result<Vec<cteInfo>> {
        let start = self.outerCTEs.len();
        for cte in definitions {
            self.buildCte(cte, cte.recursive.is_some())?;
        }
        Ok(self.outerCTEs[start..].to_vec())
    }
    /// `buildProjection4CTEUnion`：逻辑计划构建辅助。
    pub fn buildProjection4CTEUnion(
        &self,
        seed: PlanNode,
        recursive: PlanNode,
    ) -> Result<PlanNode> {
        if seed.schema.len() != recursive.schema.len() {
            return Err(BuilderError(
                "recursive CTE terms have different column counts".into(),
            ));
        }
        let mut union = PlanNode::new(PlanKind::UnionAll);
        union.schema = getResultCTESchema(&seed.schema, &recursive.schema);
        union.children = vec![seed, recursive];
        Ok(union)
    }
    /// `tblInfoFromCol`：逻辑计划构建辅助。
    pub fn tblInfoFromCol<'a>(
        &self,
        tables: &'a [TableInfo],
        column: usize,
    ) -> Option<&'a TableInfo> {
        tables.iter().find(|table| column < table.columns.len())
    }
    /// `buildJoinFuncDepend`：逻辑计划构建辅助。
    pub fn buildJoinFuncDepend(&self, plan: &PlanNode) -> HashMap<usize, usize> {
        let mut dependencies = HashMap::new();
        if plan.children.len() == 2 {
            let left = plan.children[0].schema.len();
            for condition in &plan.conditions {
                if condition.name == "eq" {
                    if let Some(column) = condition.column {
                        dependencies.insert(column, column.saturating_add(left));
                    }
                }
            }
        }
        dependencies
    }
    /// `buildUpdateLists`：逻辑计划构建辅助。
    pub fn buildUpdateLists(
        &mut self,
        assignments: &[Assignment],
        plan: PlanNode,
    ) -> Result<(Vec<Assignment>, PlanNode, bool)> {
        let mut seen = HashSet::new();
        let mut constant = true;
        for assignment in assignments {
            if assignment.column >= plan.schema.len() {
                return Err(BuilderError(format!(
                    "update column {} is out of range",
                    assignment.column
                )));
            }
            if !seen.insert((assignment.table_id, assignment.column)) {
                return Err(BuilderError("column assigned twice".into()));
            }
            constant &= assignment.expression.column.is_none();
        }
        Ok((assignments.to_vec(), plan, constant))
    }
    /// `buildUpdate`：逻辑计划构建辅助。
    pub fn buildUpdate(&mut self, update: &UpdateStmt) -> Result<PlanNode> {
        let mut plan = self.buildResultSetNode(&update.source, false)?;
        plan = self.buildSelection(plan, update.where_clause.as_ref())?;
        if !update.order_by.is_empty() {
            plan = self.buildSort(plan, &update.order_by)?;
        }
        if let Some(limit) = &update.limit {
            plan = self.buildLimit(plan, limit)?;
        }
        let (assignments, child, constant) = self.buildUpdateLists(&update.assignments, plan)?;
        let mut update_plan = PlanNode::new(PlanKind::Other("Update".into()));
        update_plan.schema = child.schema.clone();
        update_plan.children = vec![child];
        update_plan
            .labels
            .insert("assignments".into(), assignments.len() as f64);
        update_plan
            .labels
            .insert("all_constant".into(), if constant { 1.0 } else { 0.0 });
        update_plan
            .labels
            .insert("ignore".into(), if update.ignore { 1.0 } else { 0.0 });
        Ok(update_plan)
    }
    /// `buildDelete`：逻辑计划构建辅助。
    pub fn buildDelete(&mut self, delete: &DeleteStmt) -> Result<PlanNode> {
        let mut plan = self.buildResultSetNode(&delete.source, false)?;
        plan = self.buildSelection(plan, delete.where_clause.as_ref())?;
        if !delete.order_by.is_empty() {
            plan = self.buildSort(plan, &delete.order_by)?;
        }
        if let Some(limit) = &delete.limit {
            plan = self.buildLimit(plan, limit)?;
        }
        let positions = pruneAndBuildColPositionInfoForDelete(&delete.tables, &plan)?;
        let mut delete_plan = PlanNode::new(PlanKind::Other("Delete".into()));
        delete_plan.children = vec![plan];
        delete_plan
            .labels
            .insert("tables".into(), positions.len() as f64);
        delete_plan
            .labels
            .insert("ignore".into(), if delete.ignore { 1.0 } else { 0.0 });
        Ok(delete_plan)
    }
}

/// 为计划标注 TiFlash / TiKV 存储偏好。
pub fn setPreferredStoreType(plan: &mut PlanNode, tiflash: bool, tikv: bool) {
    plan.store = if tiflash && !tikv {
        crate::task::StoreType::TiFlash
    } else {
        crate::task::StoreType::TiKv
    };
}
/// 收集 Join 子树的完整 Schema 类型列表。
pub fn findJoinFullSchema(plan: &PlanNode) -> Vec<FieldType> {
    if matches!(plan.kind, PlanKind::Other(ref n) if n == "LogicalJoin") && plan.children.len() == 2
    {
        merge_schema(&plan.children[0], &plan.children[1], plan.join_type)
    } else {
        plan.schema.clone()
    }
}
/// 结果集树中是否含 LATERAL 表源。
pub fn containsLateralTableSource(node: &ResultSet) -> bool {
    match node {
        ResultSet::Table(t) => t.lateral,
        ResultSet::Join(j) => {
            j.lateral || containsLateralTableSource(&j.left) || containsLateralTableSource(&j.right)
        }
        ResultSet::Select(s) => s.from.as_ref().is_some_and(containsLateralTableSource),
        ResultSet::SetOperation(s) => s
            .iter()
            .any(|(_, s)| s.from.as_ref().is_some_and(containsLateralTableSource)),
        _ => false,
    }
}
/// 节点本身是否为立即 LATERAL 表源。
pub fn isImmediateLateralTableSource(node: &ResultSet) -> bool {
    matches!(node, ResultSet::Table(t) if t.lateral)
}
/// 为 Expand 生成字段名。
pub fn buildExpandFieldName(expr: &Expression, generated: &str) -> String {
    if expr.name.is_empty() {
        generated.into()
    } else {
        expr.name.clone()
    }
}
/// 在 NATURAL/USING Join 中按列下标回溯列名。
pub fn findColFromNaturalUsingJoin(plan: &PlanNode, column: usize) -> Option<String> {
    plan.labels.iter().find_map(|(k, v)| {
        (*v as usize == column)
            .then(|| k.strip_prefix("column:").map(str::to_string))
            .flatten()
    })
}
/// UNION 两侧字段类型对齐合并。
pub fn unionJoinFieldType(left: &FieldType, right: &FieldType) -> FieldType {
    let mixed_integer_sign = matches!(left.code, TypeCode::Int | TypeCode::UInt)
        && matches!(right.code, TypeCode::Int | TypeCode::UInt)
        && left.unsigned != right.unsigned;
    let integer_flen = left.flen.max(right.flen);
    let code = match (&left.code, &right.code) {
        (TypeCode::String, _) | (_, TypeCode::String) => TypeCode::String,
        (TypeCode::Float, _) | (_, TypeCode::Float) => TypeCode::Float,
        (TypeCode::Decimal, _) | (_, TypeCode::Decimal) => TypeCode::Decimal,
        (TypeCode::Int | TypeCode::UInt, TypeCode::Int | TypeCode::UInt)
            if mixed_integer_sign && integer_flen >= 20 =>
        {
            TypeCode::Decimal
        }
        (TypeCode::UInt, TypeCode::UInt) => TypeCode::UInt,
        _ => TypeCode::Int,
    };
    FieldType {
        code,
        flen: if mixed_integer_sign && integer_flen < 20 {
            20
        } else {
            integer_flen
        },
        decimal: left.decimal.max(right.decimal),
        unsigned: left.unsigned && right.unsigned,
    }
}
/// 从等值条件提取函数依赖列对。
pub fn buildFuncDependCol(condition: &Expression) -> Option<(usize, usize)> {
    if condition.name == "eq" {
        condition.column.map(|column| (column, column + 1))
    } else {
        None
    }
}
/// 从 WHERE 条件集合构建函数依赖映射。
pub fn buildWhereFuncDepend(conditions: &[Expression]) -> HashMap<usize, usize> {
    conditions.iter().filter_map(buildFuncDependCol).collect()
}
/// 检查列是否满足函数依赖约束。
pub fn checkColFuncDepend(
    column: usize,
    groups: &HashSet<usize>,
    dependencies: &HashMap<usize, usize>,
) -> bool {
    groups.contains(&column)
        || dependencies
            .iter()
            .any(|(determinant, dependent)| groups.contains(determinant) && *dependent == column)
}
/// 从字面量解析无符号整数。
pub fn getUintFromNode(value: &Value, require_integer: bool) -> (u64, bool, bool) {
    match value {
        Value::Null => (0, true, true),
        Value::UInt(v) => (*v, false, true),
        Value::Int(v) if *v >= 0 => (*v as u64, false, true),
        Value::Float(v) if !require_integer && *v >= 0.0 => (*v as u64, false, true),
        _ => (0, false, false),
    }
}
/// 检查参数是否为有符号/无符号 64 位整数。
pub fn CheckParamTypeInt64orUint64(value: &Value) -> (bool, u64) {
    let (value, null, valid) = getUintFromNode(value, true);
    (valid && !null, value)
}
/// 从 LIMIT 字面量提取 u64，缺省用 default。
pub fn extractLimitValue(value: Option<&Value>, default: u64) -> Result<u64> {
    let Some(value) = value else {
        return Ok(default);
    };
    let (value, null, valid) = getUintFromNode(value, true);
    if null {
        Ok(0)
    } else if valid {
        Ok(value)
    } else {
        Err(BuilderError(
            "LIMIT value must be a nonnegative integer".into(),
        ))
    }
}
/// 提取 LIMIT 的 count 与 offset。
pub fn extractLimitCountOffset(limit: &LimitClause) -> Result<(u64, u64)> {
    let offset = extractLimitValue(limit.offset.as_ref(), 0)?;
    let count = extractLimitValue(limit.count.as_ref(), u64::MAX)?.min(u64::MAX - offset);
    Ok((count, offset))
}
/// 从 SELECT 列表解析字段表达式。
pub fn resolveFromSelectFields(
    name: &str,
    fields: &[SelectField],
    ignore_alias: bool,
) -> Result<usize> {
    let mut found = None;
    for (idx, field) in fields.iter().enumerate() {
        let matches = (!ignore_alias
            && field
                .alias
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(name)))
            || expr_matches_column(&field.expr, name);
        if matches {
            if found.is_some() {
                return Err(BuilderError(format!("column {name} is ambiguous")));
            }
            found = Some(idx);
        }
    }
    found.ok_or_else(|| BuilderError(format!("unknown column {name}")))
}

/// 解析 HAVING / 窗口 / ORDER BY 中相对 SELECT 列表的引用。
#[derive(Default)]
pub struct havingWindowAndOrderbyExprResolver {
    clauses: Vec<clauseCode>,
    fields: Vec<SelectField>,
}
impl havingWindowAndOrderbyExprResolver {
    /// `pushCurClause`：逻辑计划构建辅助。
    pub fn pushCurClause(&mut self, clause: clauseCode) {
        self.clauses.push(clause);
    }
    /// `popCurClause`：逻辑计划构建辅助。
    pub fn popCurClause(&mut self) {
        self.clauses.pop();
    }
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&self, expr: &Expression) -> bool {
        !expr.name.starts_with("subquery:")
    }
    /// `resolveFromPlan`：逻辑计划构建辅助。
    pub fn resolveFromPlan(
        &self,
        name: &str,
        plan: &PlanNode,
        fields_first: bool,
    ) -> Result<usize> {
        if fields_first {
            if let Ok(index) = resolveFromSelectFields(name, &self.fields, false) {
                return Ok(index);
            }
        }
        plan.labels
            .get(&format!("column:{name}"))
            .map(|v| *v as usize)
            .ok_or_else(|| BuilderError(format!("unknown column {name}")))
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}
/// 收集并提升相关聚合的解析器。
#[derive(Default)]
pub struct correlatedAggregateResolver {
    pub correlated: Vec<Expression>,
}
impl correlatedAggregateResolver {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        if expr.name.contains("correlated") && expr.name.starts_with("agg:") {
            self.correlated.push(expr.clone());
            false
        } else {
            true
        }
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
    /// `resolveSelect`：逻辑计划构建辅助。
    pub fn resolveSelect(&mut self, select: &SelectStmt) {
        self.collectFromSelectFields(&select.fields);
        self.collectFromGroupBy(&select.group_by);
        if let Some(where_clause) = &select.where_clause {
            self.collectFromWhere(where_clause);
        }
        if let Some(from) = &select.from {
            self.collectFromTableRefs(from);
        }
    }
    /// `collectFromTableRefs`：逻辑计划构建辅助。
    pub fn collectFromTableRefs(&mut self, result: &ResultSet) {
        match result {
            ResultSet::Select(select) => self.resolveSelect(select),
            ResultSet::Join(join) => {
                self.collectFromTableRefs(&join.left);
                self.collectFromTableRefs(&join.right);
            }
            ResultSet::SetOperation(items) => {
                for (_, select) in items {
                    self.resolveSelect(select);
                }
            }
            _ => {}
        }
    }
    /// `collectFromSelectFields`：逻辑计划构建辅助。
    pub fn collectFromSelectFields(&mut self, fields: &[SelectField]) {
        for field in fields {
            self.Enter(&field.expr);
        }
    }
    /// `collectFromGroupBy`：逻辑计划构建辅助。
    pub fn collectFromGroupBy(&mut self, groups: &[Expression]) {
        for group in groups {
            self.Enter(group);
        }
    }
    /// `collectFromWhere`：逻辑计划构建辅助。
    pub fn collectFromWhere(&mut self, condition: &Expression) {
        self.Enter(condition);
    }
}
/// GROUP BY 表达式解析与列归属检查。
#[derive(Default)]
pub struct gbyResolver {
    pub fields: Vec<SelectField>,
    pub resolved: Vec<Expression>,
}
impl gbyResolver {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        if let Ok(index) = expr.name.parse::<usize>() {
            if let Some(field) = self.fields.get(index.saturating_sub(1)) {
                self.resolved.push(field.expr.clone());
                return false;
            }
        }
        true
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}
/// 表达式校验错误的位置描述。
#[derive(Clone, Debug)]
pub struct ErrExprLoc {
    pub clause: String,
    pub index: usize,
}
/// 检查表达式是否在 GROUP BY 中或为单值。
pub fn checkExprInGroupByOrIsSingleValue(
    expr: &Expression,
    groups: &HashSet<usize>,
    single: &HashSet<usize>,
) -> bool {
    expr.column
        .is_none_or(|c| groups.contains(&c) || single.contains(&c))
        || expr.name.starts_with("agg:")
}
/// 将列记入 GROUP BY / 单值列名集合。
pub fn addGbyOrSingleValueColName(expr: &Expression, set: &mut HashSet<usize>) {
    if let Some(column) = expr.column {
        set.insert(column);
    }
}
/// 从 WHERE 提取单值列名。
pub fn extractSingeValueColNamesFromWhere(expr: &Expression, set: &mut HashSet<usize>) {
    if expr.name.starts_with("eq_const:") {
        addGbyOrSingleValueColName(expr, set);
    }
}
/// 聚合表达式中的全部列名。
pub fn allColFromAggExprNode(expr: &Expression, names: &mut HashSet<usize>) {
    if expr.name.starts_with("agg:") {
        if let Some(column) = expr.column {
            names.insert(column);
        }
    }
}
/// 表达式中的全部列名。
pub fn allColFromExprNode(expr: &Expression, names: &mut HashSet<usize>) {
    if let Some(column) = expr.column {
        names.insert(column);
    }
}
/// 展开 SELECT * / t.* 通配符。
pub fn unfoldWildStar(
    field: &SelectField,
    output: &[String],
    columns: &[FieldType],
) -> Vec<SelectField> {
    if !field.wildcard {
        return vec![field.clone()];
    }
    columns
        .iter()
        .enumerate()
        .map(|(index, tp)| SelectField {
            expr: Expression {
                name: output.get(index).cloned().unwrap_or_default(),
                column: Some(index),
                return_type: Some(tp.clone()),
                ..Expression::default()
            },
            alias: None,
            wildcard: false,
            table_wildcard: None,
        })
        .collect()
}
/// 为 DataSource 追加物理表 ID 列。
pub fn addExtraPhysTblIDColumn4DS(plan: &mut PlanNode) -> usize {
    let column = plan.schema.len();
    plan.schema.push(FieldType {
        code: TypeCode::Int,
        flen: 20,
        decimal: 0,
        unsigned: false,
    });
    plan.labels
        .insert("column:_tidb_physical_table_id".into(), column as f64);
    column
}
/// 从统计版本表取物理表最新版本。
pub fn getLatestVersionFromStatsTable(versions: &HashMap<i64, u64>, physical_id: i64) -> u64 {
    versions.get(&physical_id).copied().unwrap_or(0)
}
/// 标记 CTE 处于 Apply 上下文。
pub fn setIsInApplyForCTE(plan: &mut PlanNode, apply_schema: &[FieldType]) {
    if plan.kind == PlanKind::Cte {
        plan.labels.insert("in_apply".into(), 1.0);
        plan.labels
            .insert("apply_schema".into(), apply_schema.len() as f64);
    }
    for child in &mut plan.children {
        setIsInApplyForCTE(child, apply_schema);
    }
}
/// 按 handle 名查找表在名字列表中的偏移。
pub fn getTableOffset(names: &[String], handle: &str) -> Result<usize> {
    names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(handle))
        .ok_or_else(|| BuilderError(format!("handle column {handle} not found")))
}
/// 解析表 ID 到 handle 列下标。
pub fn resolveIndicesForTblID2Handle(
    handles: &HashMap<i64, Vec<usize>>,
    schema_len: usize,
) -> Result<HashMap<i64, Vec<usize>>> {
    for list in handles.values() {
        if list.iter().any(|i| *i >= schema_len) {
            return Err(BuilderError("handle index out of range".into()));
        }
    }
    Ok(handles.clone())
}
/// 为可写列构建列到 handle 映射。
pub fn buildColumns2HandleWithWrtiableColumns(tables: &[TableInfo]) -> HashMap<i64, Vec<usize>> {
    tables
        .iter()
        .map(|table| {
            let handles = table
                .columns
                .iter()
                .enumerate()
                .filter(|(_, c)| c.primary_key && !c.generated)
                .map(|(i, _)| i)
                .collect();
            (table.id, handles)
        })
        .collect()
}
/// 初始化表列位置信息。
pub fn initColPosInfo(
    table_id: i64,
    names: &[String],
    handles: Vec<usize>,
) -> Result<TblColPosInfo> {
    if handles.iter().any(|h| *h >= names.len()) {
        return Err(BuilderError("handle column is outside table schema".into()));
    }
    Ok(TblColPosInfo {
        table_id,
        start: 0,
        end: names.len(),
        handle_columns: handles,
    })
}
/// 构建 UPDATE 多表列位置信息。
pub fn buildUpdateTblColPosInfos(
    tables: &[TableInfo],
    assignments: &[Assignment],
) -> Result<Vec<TblColPosInfo>> {
    let handles = buildColumns2HandleWithWrtiableColumns(tables);
    tables
        .iter()
        .filter(|table| assignments.iter().any(|a| a.table_id == table.id))
        .map(|table| {
            initColPosInfo(
                table.id,
                &table
                    .columns
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>(),
                handles.get(&table.id).cloned().unwrap_or_default(),
            )
        })
        .collect()
}
/// 构建单表 DELETE 列位置信息。
pub fn buildSingleTableColPosInfoForDelete(
    table: &TableInfo,
    pre_pruned: usize,
) -> Result<TblColPosInfo> {
    let handles = buildColumns2HandleWithWrtiableColumns(std::slice::from_ref(table))
        .remove(&table.id)
        .unwrap_or_default();
    let mut info = initColPosInfo(
        table.id,
        &table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
        handles,
    )?;
    info.start = pre_pruned;
    info.end += pre_pruned;
    Ok(info)
}
/// 裁剪并构建单表 DELETE 列位置。
pub fn pruneAndBuildSingleTableColPosInfoForDelete(
    table: &TableInfo,
    plan: &mut PlanNode,
) -> Result<TblColPosInfo> {
    let required: HashSet<_> = buildColumns2HandleWithWrtiableColumns(std::slice::from_ref(table))
        .remove(&table.id)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut new_schema = Vec::new();
    let mut remap = HashMap::new();
    for (idx, tp) in plan.schema.iter().cloned().enumerate() {
        if required.contains(&idx) {
            remap.insert(idx, new_schema.len());
            new_schema.push(tp);
        }
    }
    plan.schema = new_schema;
    initColPosInfo(
        table.id,
        &(0..plan.schema.len())
            .map(|i| i.to_string())
            .collect::<Vec<_>>(),
        required
            .into_iter()
            .filter_map(|i| remap.get(&i).copied())
            .collect(),
    )
}
/// 裁剪并构建多表 DELETE 列位置。
pub fn pruneAndBuildColPositionInfoForDelete(
    table_ids: &[i64],
    plan: &PlanNode,
) -> Result<Vec<TblColPosInfo>> {
    table_ids
        .iter()
        .map(|id| {
            initColPosInfo(
                *id,
                &(0..plan.schema.len())
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>(),
                vec![0],
            )
        })
        .collect()
}
/// 校验 UPDATE 赋值列表合法性。
pub fn CheckUpdateList(
    assign_flags: &[i32],
    update: &PlanNode,
    new_tables: &HashMap<i64, TableInfo>,
) -> Result<()> {
    if assign_flags.iter().any(|flag| *flag < 0) {
        return Err(BuilderError("invalid assignment flag".into()));
    }
    if update.labels.get("assignments").copied().unwrap_or(0.0) as usize != assign_flags.len() {
        return Err(BuilderError("assignment count mismatch".into()));
    }
    if new_tables.is_empty() {
        return Err(BuilderError("updated table set is empty".into()));
    }
    Ok(())
}
/// 名称是否为已知 CTE。
pub fn isCTE(name: &str, ctes: &[cteInfo]) -> bool {
    ctes.iter().any(|cte| cte.name.eq_ignore_ascii_case(name))
}
/// `getWindowName`：逻辑计划构建辅助。
pub fn getWindowName(name: &str) -> String {
    name.to_ascii_lowercase()
}
/// `getAllByItems`：逻辑计划构建辅助。
pub fn getAllByItems(mut items: Vec<ByItem>, spec: &WindowSpec) -> Vec<ByItem> {
    items.clear();
    items.extend(
        spec.partition_by
            .iter()
            .cloned()
            .map(|expr| ByItem { expr, desc: false }),
    );
    items.extend(spec.order_by.clone());
    items
}
/// `restoreByItemText`：逻辑计划构建辅助。
pub fn restoreByItemText(item: &ByItem) -> String {
    item.expr.name.clone()
}
/// `compareItems`：逻辑计划构建辅助。
pub fn compareItems(left: &[ByItem], right: &[ByItem]) -> bool {
    for (left, right) in left.iter().zip(right) {
        match restoreByItemText(left).cmp(&restoreByItemText(right)) {
            std::cmp::Ordering::Less => return true,
            std::cmp::Ordering::Greater => return false,
            std::cmp::Ordering::Equal => match left.desc.cmp(&right.desc) {
                std::cmp::Ordering::Less => return true,
                std::cmp::Ordering::Greater => return false,
                std::cmp::Ordering::Equal => {}
            },
        }
    }
    left.len() < right.len()
}
/// `sortWindowSpecs`：逻辑计划构建辅助。
pub fn sortWindowSpecs(mut groups: Vec<windowFuncs>) -> Vec<windowFuncs> {
    groups.sort_by(|left, right| {
        let left = getAllByItems(Vec::new(), &left.spec);
        let right = getAllByItems(Vec::new(), &right.spec);
        if compareItems(&left, &right) {
            std::cmp::Ordering::Greater
        } else if compareItems(&right, &left) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    });
    groups
}
/// `extractWindowFuncs`：逻辑计划构建辅助。
pub fn extractWindowFuncs(fields: &[SelectField]) -> Vec<WindowFunc> {
    fields
        .iter()
        .filter(|f| f.expr.name.starts_with("window:"))
        .map(|f| WindowFunc {
            name: f.expr.name.clone(),
            args: Vec::new(),
            spec_name: None,
            spec: None,
        })
        .collect()
}
/// `appendIfAbsentWindowSpec`：逻辑计划构建辅助。
pub fn appendIfAbsentWindowSpec(mut specs: Vec<WindowSpec>, spec: WindowSpec) -> Vec<WindowSpec> {
    if !specs.iter().any(|s| specEqual(s, &spec)) {
        specs.push(spec);
    }
    specs
}
/// `specEqual`：逻辑计划构建辅助。
pub fn specEqual(left: &WindowSpec, right: &WindowSpec) -> bool {
    left.partition_by
        .iter()
        .map(|e| (&e.name, e.column))
        .eq(right.partition_by.iter().map(|e| (&e.name, e.column)))
        && left.order_by.len() == right.order_by.len()
        && left.order_by.iter().zip(&right.order_by).all(|(a, b)| {
            a.desc == b.desc && a.expr.name == b.expr.name && a.expr.column == b.expr.column
        })
        && match (&left.frame, &right.frame) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                left.rows == right.rows
                    && left.start.kind == right.start.kind
                    && left.start.value == right.start.value
                    && left.end.kind == right.end.kind
                    && left.end.value == right.end.value
            }
            _ => false,
        }
}
/// `resolveWindowSpec`：逻辑计划构建辅助。
pub fn resolveWindowSpec(
    spec: &mut WindowSpec,
    specs: &HashMap<String, WindowSpec>,
    stack: &mut HashSet<String>,
) -> Result<()> {
    let Some(reference) = spec.reference.clone() else {
        return Ok(());
    };
    let key = getWindowName(&reference);
    if !stack.insert(key.clone()) {
        return Err(BuilderError("circular window specification".into()));
    }
    let mut referenced = specs
        .get(&key)
        .cloned()
        .ok_or_else(|| BuilderError(format!("unknown window {reference}")))?;
    resolveWindowSpec(&mut referenced, specs, stack)?;
    mergeWindowSpec(spec, &referenced)?;
    stack.remove(&key);
    Ok(())
}
/// `mergeWindowSpec`：逻辑计划构建辅助。
pub fn mergeWindowSpec(spec: &mut WindowSpec, reference: &WindowSpec) -> Result<()> {
    if reference.frame.is_some() {
        return Err(BuilderError("cannot inherit framed window".into()));
    }
    if !spec.partition_by.is_empty() {
        return Err(BuilderError("cannot override PARTITION BY".into()));
    }
    if !spec.order_by.is_empty() && !reference.order_by.is_empty() {
        return Err(BuilderError("cannot override ORDER BY".into()));
    }
    if !reference.order_by.is_empty() {
        spec.order_by = reference.order_by.clone();
    }
    spec.partition_by = reference.partition_by.clone();
    spec.reference = None;
    Ok(())
}
/// `buildWindowSpecs`：逻辑计划构建辅助。
pub fn buildWindowSpecs(specs: &[WindowSpec]) -> Result<HashMap<String, WindowSpec>> {
    let raw: HashMap<_, _> = specs
        .iter()
        .map(|s| (getWindowName(&s.name), s.clone()))
        .collect();
    if raw.len() != specs.len() {
        return Err(BuilderError("duplicate window name".into()));
    }
    let mut result = HashMap::new();
    for (name, mut spec) in raw.clone() {
        resolveWindowSpec(&mut spec, &raw, &mut HashSet::new())?;
        result.insert(name, spec);
    }
    Ok(result)
}
/// `ExtractTableList`：逻辑计划构建辅助。
pub fn ExtractTableList(node: &ResultSet, aliases: bool) -> Vec<String> {
    let mut out = Vec::new();
    collectTableName(node, &mut out, aliases);
    out
}
/// `collectTableName`：逻辑计划构建辅助。
pub fn collectTableName(node: &ResultSet, output: &mut Vec<String>, aliases: bool) {
    match node {
        ResultSet::Table(t) => output.push(if aliases {
            t.alias.clone().unwrap_or_else(|| t.table.name.clone())
        } else {
            t.table.name.clone()
        }),
        ResultSet::Join(j) => {
            collectTableName(&j.left, output, aliases);
            collectTableName(&j.right, output, aliases);
        }
        ResultSet::Select(s) => {
            if let Some(from) = &s.from {
                collectTableName(from, output, aliases);
            }
        }
        ResultSet::SetOperation(items) => {
            for (_, s) in items {
                if let Some(from) = &s.from {
                    collectTableName(from, output, aliases);
                }
            }
        }
        _ => {}
    }
}
/// `itemTransformer` 类型定义。
#[derive(Default)]
pub struct itemTransformer;
impl itemTransformer {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&self, expr: &Expression) -> bool {
        !expr.name.starts_with("subquery:")
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}
/// `colResolverForOnlyFullGroupBy` 类型定义。
#[derive(Default)]
pub struct colResolverForOnlyFullGroupBy {
    pub columns: HashSet<usize>,
}
impl colResolverForOnlyFullGroupBy {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        if let Some(column) = expr.column {
            self.columns.insert(column);
        }
        !expr.name.starts_with("agg:")
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}
/// `aggColNameResolver` 类型定义。
#[derive(Default)]
pub struct aggColNameResolver {
    pub columns: HashSet<usize>,
}
impl aggColNameResolver {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        allColFromAggExprNode(expr, &mut self.columns);
        false
    }
}
/// `colNameResolver` 类型定义。
#[derive(Default)]
pub struct colNameResolver {
    pub columns: HashSet<usize>,
}
impl colNameResolver {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        allColFromExprNode(expr, &mut self.columns);
        true
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}
/// `updatableTableListResolver` 类型定义。
#[derive(Default)]
pub struct updatableTableListResolver {
    pub tables: Vec<String>,
}
impl updatableTableListResolver {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, node: &ResultSet) -> bool {
        self.tables.extend(ExtractTableList(node, true));
        false
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _node: &ResultSet) -> bool {
        true
    }
}
/// `tableListExtractor` 类型定义。
#[derive(Default)]
pub struct tableListExtractor {
    pub tables: Vec<String>,
}
impl tableListExtractor {
    /// `Enter`：逻辑计划构建辅助。
    pub fn Enter(&mut self, node: &ResultSet) -> bool {
        self.tables.extend(ExtractTableList(node, false));
        false
    }
    /// `Leave`：逻辑计划构建辅助。
    pub fn Leave(&self, _node: &ResultSet) -> bool {
        true
    }
}
/// 追加动态权限检查项，保持原有顺序。
pub fn appendDynamicVisitInfo(
    mut visits: Vec<visitInfo>,
    privileges: &[String],
    with_grant: bool,
    error: &str,
) -> Vec<visitInfo> {
    visits.push(visitInfo {
        privilege: Privilege::Dynamic(privileges.first().cloned().unwrap_or_default()),
        db: String::new(),
        table: String::new(),
        column: String::new(),
        error: error.into(),
        alterWritable: false,
        dynamicPrivs: privileges.to_vec(),
        dynamicWithGrant: with_grant,
    });
    visits
}
/// `appendVisitInfo`：逻辑计划构建辅助。
pub fn appendVisitInfo(
    mut visits: Vec<visitInfo>,
    privilege: Privilege,
    db: &str,
    table: &str,
    column: &str,
    error: &str,
) -> Vec<visitInfo> {
    visits.push(visitInfo {
        privilege,
        db: db.into(),
        table: table.into(),
        column: column.into(),
        error: error.into(),
        alterWritable: false,
        dynamicPrivs: Vec::new(),
        dynamicWithGrant: false,
    });
    visits
}
/// `getInnerFromParenthesesAndUnaryPlus`：逻辑计划构建辅助。
pub fn getInnerFromParenthesesAndUnaryPlus(expr: &Expression) -> Expression {
    let mut expr = expr.clone();
    while let Some(inner) = expr
        .name
        .strip_prefix("(+")
        .and_then(|s| s.strip_suffix(')'))
    {
        expr.name = inner.into();
    }
    expr
}
/// `hasMPPJoinHints`：逻辑计划构建辅助。
pub fn hasMPPJoinHints(preference: u32) -> bool {
    preference & 0b11_0000 != 0
}
/// `isJoinHintSupportedInMPPMode`：逻辑计划构建辅助。
pub fn isJoinHintSupportedInMPPMode(preference: u32) -> bool {
    preference & !0b11_1111 == 0
}
/// 恢复 prepareCTECheckForSubQuery 改动的 CTE 条目。
pub fn resetCTECheckForSubQuery(indices: &[usize], ctes: &mut [cteInfo]) {
    for index in indices {
        if let Some(cte) = ctes.get_mut(*index) {
            cte.enterSubquery = false;
        }
    }
}
/// `getResultCTESchema`：逻辑计划构建辅助。
pub fn getResultCTESchema(seed: &[FieldType], recursive: &[FieldType]) -> Vec<FieldType> {
    seed.iter()
        .zip(recursive)
        .map(|(a, b)| unionJoinFieldType(a, b))
        .collect()
}

trait SaturatingSubF64 {
    fn saturating_sub_f64(self, rhs: f64) -> f64;
}
impl SaturatingSubF64 for f64 {
    fn saturating_sub_f64(self, rhs: f64) -> f64 {
        (self - rhs).max(0.0)
    }
}
