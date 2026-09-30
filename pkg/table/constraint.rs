// Copyright 2026 AsterSQL.
// Copyright 2023-2023 PingCAP, Inc.
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

// CHECK-constraint metadata and validation helpers.
//
// CHECK 约束（CHECK constraint）元数据加载与合法性校验：清理无效列引用、
// 解析表达式 AST、禁止非确定性函数/变量/子查询，以及与外键动作、
// AUTO_INCREMENT 列的冲突检查。

use expression_dependency::BuildContext;
use model_dependency::group_4 as constraint_model;
use parser_ast_dependency as ast;

/// 本模块共享错误类型别名。
type ConstraintError = errors_dependency::SharedError;

/// Runtime wrapper for canonical CHECK-constraint metadata.
/// 规范 CHECK 约束元数据的运行时包装。
#[derive(Clone)]
pub struct Constraint {
    /// 底层 model 层约束信息。
    pub ConstraintInfo: Box<constraint_model::ConstraintInfo>,
}

/// Loads valid CHECK constraints and lazily removes metadata referring to a
/// missing or non-public column, matching the Go package's in-memory repair.
/// 加载有效 CHECK 约束，并惰性移除引用缺失或非 Public 列的元数据（对齐 Go 内存修复）。
pub fn LoadCheckConstraint(
    table_info: &mut constraint_model::TableInfo,
) -> Result<Vec<Box<Constraint>>, ConstraintError> {
    removeInvalidCheckConstraintsInfo(table_info);
    Ok(table_info
        .Constraints
        .iter()
        .cloned()
        .map(|constraint_info| {
            Box::new(Constraint {
                ConstraintInfo: Box::new(constraint_info),
            })
        })
        .collect())
}

/// 过滤掉依赖列不存在或不在 Public 状态的 CHECK 约束。
fn removeInvalidCheckConstraintsInfo(table_info: &mut constraint_model::TableInfo) {
    let valid = table_info
        .Constraints
        .iter()
        .filter(|constraint| {
            constraint
                .ConstraintCols
                .iter()
                .all(|column| table_info.FindPublicColumnByName(&column.L).is_some())
        })
        .cloned()
        .collect();
    table_info.Constraints = valid;
}

/// Parses and builds the stored SQL expression into the executable expression
/// representation used by the rest of the table package.
/// 将存储的 SQL 表达式解析、构建为 table 包使用的可执行表达式。
pub fn BuildConstraintExprWithCtx(
    context: &dyn BuildContext,
    constraint_info: &constraint_model::ConstraintInfo,
    table_info: &constraint_model::TableInfo,
    database_name: &str,
) -> Result<Box<dyn expression_dependency::Expression>, ConstraintError> {
    buildConstraintExpression(
        context,
        &constraint_info.ExprString,
        database_name,
        table_info,
    )
}

/// 用表元数据解析并构建 CHECK 表达式。
fn buildConstraintExpression(
    context: &dyn BuildContext,
    expression: &str,
    database_name: &str,
    table_info: &constraint_model::TableInfo,
) -> Result<Box<dyn expression_dependency::Expression>, ConstraintError> {
    expression_dependency::ParseSimpleExpr(
        context,
        expression,
        vec![expression_dependency::WithTableInfo(
            database_name,
            table_info,
        )],
    )
    .map_err(ConstraintError::new)
}

/// Checks AST constructs that MySQL forbids in CHECK constraints.
/// 检查 MySQL 在 CHECK 约束中禁止的 AST 构造。
pub fn IsSupportedExpr(constraint: &ast::Constraint) -> (bool, Option<ConstraintError>) {
    let Some(expression) = constraint.Expr.as_ref() else {
        return (true, None);
    };
    let mut checker = checkConstraintChecker {
        allowed: true,
        reason: None,
        name: constraint.Name.clone(),
    };
    checker.check(expression);
    (checker.allowed, checker.reason)
}

/// CHECK 中禁止的非确定性/会话相关函数名列表（小写）。
const UNSUPPORTED_FUNCTIONS: &[&str] = &[
    "now",
    "current_timestamp",
    "curdate",
    "current_date",
    "curtime",
    "current_time",
    "localtime",
    "localtimestamp",
    "unix_timestamp",
    "utc_date",
    "utc_timestamp",
    "utc_time",
    "connection_id",
    "current_user",
    "session_user",
    "version",
    "found_rows",
    "last_insert_id",
    "system_user",
    "user",
    "rand",
    "row_count",
    "get_lock",
    "is_free_lock",
    "is_used_lock",
    "release_lock",
    "release_all_locks",
    "load_file",
    "uuid",
    "uuid_v4",
    "uuid_v7",
    "uuid_short",
    "sleep",
    "embed_text",
];

/// 递归遍历表达式 AST，标记不支持的构造。
#[allow(non_camel_case_types)]
struct checkConstraintChecker {
    /// 当前是否仍允许该表达式。
    allowed: bool,
    /// 拒绝原因（若有）。
    reason: Option<ConstraintError>,
    /// 约束名，用于错误消息。
    name: String,
}

impl checkConstraintChecker {
    /// 因具名函数被禁止而拒绝。
    fn reject_named_function(&mut self, function: &str) {
        self.allowed = false;
        self.reason = Some(
            dbterror_dependency::ErrCheckConstraintNamedFuncIsNotAllowed
                .GenWithStackByArgs(&[self.name.clone().into(), function.to_owned().into()]),
        );
    }

    /// 因函数类构造（如子查询）被禁止而拒绝。
    fn reject_function(&mut self) {
        self.allowed = false;
        self.reason = Some(
            dbterror_dependency::ErrCheckConstraintFuncIsNotAllowed
                .GenWithStackByArgs(&[self.name.clone().into()]),
        );
    }

    /// 深度优先检查单个表达式节点。
    fn check(&mut self, expression: &ast::ExprNode) {
        if !self.allowed {
            return;
        }
        use ast::ExprKind;
        match &expression.Kind {
            ExprKind::Function { FnName, Args, .. } => {
                if UNSUPPORTED_FUNCTIONS.contains(&FnName.L.as_str()) {
                    self.reject_named_function(&FnName.L);
                    return;
                }
                self.check_all(Args);
            }
            ExprKind::Variable { .. } => {
                self.allowed = false;
                self.reason = Some(
                    dbterror_dependency::ErrCheckConstraintVariables
                        .GenWithStackByArgs(&[self.name.clone().into()]),
                );
            }
            ExprKind::Subquery { .. }
            | ExprKind::CompareSubquery { .. }
            | ExprKind::InSubquery { .. }
            | ExprKind::ExistsSubquery { .. } => self.reject_function(),
            ExprKind::DefaultValue | ExprKind::NamedDefault(_) => {
                self.reject_named_function("default")
            }
            ExprKind::AggregateFunction { Args, Order, .. } => {
                self.check_all(Args);
                for item in Order {
                    self.check(&item.Expr);
                }
            }
            ExprKind::Binary { L, R, .. } => {
                self.check(L);
                self.check(R);
            }
            ExprKind::Unary { V, .. } | ExprKind::Parentheses(V) => self.check(V),
            ExprKind::IsTruth { Expr, .. }
            | ExprKind::IsNull { Expr, .. }
            | ExprKind::Collate { Expr, .. }
            | ExprKind::Cast { Expr, .. }
            | ExprKind::JSONSumCrc32 { Expr, .. } => self.check(Expr),
            ExprKind::InList { Expr, List, .. } => {
                self.check(Expr);
                self.check_all(List);
            }
            ExprKind::Between {
                Expr, Left, Right, ..
            } => {
                self.check(Expr);
                self.check(Left);
                self.check(Right);
            }
            ExprKind::Like { Expr, Pattern, .. } | ExprKind::Regexp { Expr, Pattern, .. } => {
                self.check(Expr);
                self.check(Pattern);
            }
            ExprKind::Row(items) => self.check_all(items),
            ExprKind::MatchAgainst { Against, .. } => self.check(Against),
            ExprKind::Case {
                Value,
                WhenClauses,
                ElseClause,
            } => {
                if let Some(value) = Value {
                    self.check(value);
                }
                for clause in WhenClauses {
                    self.check(&clause.Expr);
                    self.check(&clause.Result);
                }
                if let Some(value) = ElseClause {
                    self.check(value);
                }
            }
            ExprKind::WindowFunction { Args, .. } => self.check_all(Args),
            ExprKind::Value(_)
            | ExprKind::IntroducedValue { .. }
            | ExprKind::Column(_)
            | ExprKind::MaxValue
            | ExprKind::TimeUnit(_)
            | ExprKind::GetFormatSelector(_)
            | ExprKind::TrimDirection(_)
            | ExprKind::TableName(_)
            | ExprKind::ParamMarker { .. } => {}
        }
    }

    /// 检查表达式列表，遇拒绝即提前停止。
    fn check_all(&mut self, expressions: &[ast::ExprNode]) {
        for expression in expressions {
            self.check(expression);
            if !self.allowed {
                break;
            }
        }
    }
}

/// Checks whether the referenced columns include the table's AUTO_INCREMENT
/// column. This uses the formal root metadata because it owns executable column
/// flags and the parser AST's canonical `CIStr` identity.
/// 判断引用列是否包含表的 AUTO_INCREMENT（自增）列。
pub fn ContainsAutoIncrementCol(
    columns: &[ast::CIStr],
    table_info: &model_dependency::TableInfo,
) -> bool {
    table_info
        .Columns
        .iter()
        .find(|column| parser_mysql_dependency::r#type::HasAutoIncrementFlag(column.GetFlag()))
        .is_some_and(|auto| columns.iter().any(|column| column.L == auto.Name.L))
}

/// Rejects a CHECK constraint that depends on an FK column with ON UPDATE or
/// ON DELETE action. `Some` selects persisted FK metadata, including an empty
/// slice; `None` selects CREATE TABLE AST constraints, matching Go nil rules.
/// 拒绝依赖带 ON UPDATE/ON DELETE 动作的外键列的 CHECK。
/// `Some` 走持久化 FK 元数据（含空切片）；`None` 走 CREATE TABLE AST（对齐 Go nil）。
pub fn HasForeignKeyRefAction(
    foreign_keys: Option<Vec<Box<constraint_model::FKInfo>>>,
    constraints: &[Box<ast::Constraint>],
    check_constraint: &ast::Constraint,
    depended_columns: &[ast::CIStr],
) -> Result<(), ConstraintError> {
    if let Some(foreign_keys) = foreign_keys {
        return checkForeignKeyRefActionByFKInfo(&foreign_keys, check_constraint, depended_columns);
    }
    // AST 路径：扫描 CREATE TABLE 中的外键约束。
    for constraint in constraints {
        if constraint.Tp != ast::ConstraintType::ForeignKey {
            continue;
        }
        let Some(reference) = constraint.Refer.as_ref() else {
            continue;
        };
        if reference.OnDelete.ReferOpt == ast::ReferOptionType::None
            && reference.OnUpdate.ReferOpt == ast::ReferOptionType::None
        {
            continue;
        }
        let foreign_key_columns = constraint
            .Keys
            .iter()
            .filter_map(|key| key.Column.as_ref().map(|column| &column.Name));
        for depended in depended_columns {
            if foreign_key_columns
                .clone()
                .any(|column| column.L == depended.L)
            {
                return Err(foreign_key_action_error(depended, check_constraint));
            }
        }
    }
    Ok(())
}

/// 基于已持久化的 FKInfo 检查外键引用动作冲突。
fn checkForeignKeyRefActionByFKInfo(
    foreign_keys: &[Box<constraint_model::FKInfo>],
    check_constraint: &ast::Constraint,
    depended_columns: &[ast::CIStr],
) -> Result<(), ConstraintError> {
    for foreign_key in foreign_keys {
        if foreign_key.OnDelete == 0 && foreign_key.OnUpdate == 0 {
            continue;
        }
        for depended in depended_columns {
            if foreign_key.Cols.iter().any(|column| column.L == depended.L) {
                return Err(foreign_key_action_error(depended, check_constraint));
            }
        }
    }
    Ok(())
}

/// 构造「CHECK 使用了带 FK 引用动作列」的错误。
fn foreign_key_action_error(
    column: &ast::CIStr,
    check_constraint: &ast::Constraint,
) -> ConstraintError {
    dbterror_dependency::ErrCheckConstraintUsingFKReferActionColumn.GenWithStackByArgs(&[
        column.L.clone().into(),
        check_constraint.Name.clone().into(),
    ])
}

/// 判断列名是否出现在给定列列表中（按小写名比较）。
fn hasSpecifiedCol(columns: &[ast::CIStr], column: &ast::CIStr) -> bool {
    columns.iter().any(|candidate| candidate.L == column.L)
}

/// Validates that the built expression carries MySQL's boolean type flag.
/// 校验构建后的表达式具有 MySQL 布尔类型标志。
pub fn IfCheckConstraintExprBoolType(
    context: &dyn BuildContext,
    info: &constraint_model::ConstraintInfo,
    table_info: &constraint_model::TableInfo,
) -> Result<(), ConstraintError> {
    let expression =
        BuildConstraintExprWithCtx(context, info, table_info, &context.GetEvalCtx().CurrentDB())?;
    if !parser_mysql_dependency::r#type::HasIsBooleanFlag(
        expression.GetType(context.GetEvalCtx()).GetFlag(),
    ) {
        return Err(dbterror_dependency::ErrNonBooleanExprForCheckConstraint
            .GenWithStackByArgs(&[info.Name.O.clone().into()]));
    }
    Ok(())
}
