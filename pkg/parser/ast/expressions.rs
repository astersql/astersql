// Copyright 2015 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// 表达式 AST 节点与 SQL 还原逻辑，自 `expressions.go` 移植。
//
// 涵盖字面量、列引用、二元/一元运算、BETWEEN/IN/LIKE、CASE、子查询、
// 变量与全文 MATCH...AGAINST 等节点；通过 `RestoreCtx` 与优先级规则
// 决定是否省略冗余括号，并支持 Visitor 深度优先遍历。

// Expression AST nodes and SQL restoration behavior ported from expressions.go.

#![allow(non_snake_case)]

use std::any::Any;
use std::collections::HashSet;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};

/// 还原操作结果：成功或带错误消息的失败。
pub type RestoreResult<T = ()> = Result<T, String>;

/// 控制 SQL 还原行为的位标志集合（空格、括号、省略 schema/表名等）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestoreFlags(u32);

impl RestoreFlags {
    /// 二元运算符两侧输出空格。
    pub const SPACES_AROUND_BINARY: Self = Self(1 << 0);
    /// 为二元运算强制加括号。
    pub const BRACKET_AROUND_BINARY: Self = Self(1 << 1);
    /// 为 BETWEEN 表达式强制加括号。
    pub const BRACKET_AROUND_BETWEEN: Self = Self(1 << 2);
    /// 按优先级省略可安全去掉的括号。
    pub const SKIP_REDUNDANT_PARENTHESES: Self = Self(1 << 3);
    /// 还原列名时省略 schema 前缀。
    pub const WITHOUT_SCHEMA_NAME: Self = Self(1 << 4);
    /// 还原列名时省略表名前缀。
    pub const WITHOUT_TABLE_NAME: Self = Self(1 << 5);

    /// 空标志集合。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 是否包含另一组标志的全部位。
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for RestoreFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RestoreFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// 当前节点相对父二元运算的左右侧，用于结合律/括号判定。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum BinarySide {
    #[default]
    None,
    Left,
    Right,
}

/// SQL 还原上下文：输出缓冲、标志、父运算符信息与 CTE 名集合。
#[derive(Debug, Default)]
pub struct RestoreCtx {
    output: String,
    pub flags: RestoreFlags,
    parent_binary_op: Option<Op>,
    parent_binary_side: BinarySide,
    in_unary_operation: bool,
    pub cte_names: HashSet<String>,
}

impl RestoreCtx {
    /// 以给定标志构造还原上下文。
    pub fn new(flags: RestoreFlags) -> Self {
        Self {
            flags,
            ..Self::default()
        }
    }

    /// 取出已累积的 SQL 文本。
    pub fn finish(self) -> String {
        self.output
    }

    /// 原样追加文本。
    fn write_plain(&mut self, text: &str) {
        self.output.push_str(text);
    }

    /// 以大写关键字形式追加。
    fn write_keyword(&mut self, text: &str) {
        self.output.push_str(&text.to_ascii_uppercase());
    }

    /// 以反引号标识符形式追加并转义。
    fn write_name(&mut self, name: &str) {
        self.output.push('`');
        self.output.push_str(&name.replace('`', "``"));
        self.output.push('`');
    }

    /// 以单引号字符串字面量形式追加并转义。
    fn write_string(&mut self, value: &str) {
        self.output.push('\'');
        self.output.push_str(&value.replace('\'', "''"));
        self.output.push('\'');
    }
}

/// 表达式运算符枚举，含逻辑、比较、算术、位运算与一元运算。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Op {
    LogicOr,
    LogicXor,
    LogicAnd,
    Eq,
    Ne,
    NullEq,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    Like,
    Regexp,
    IsNull,
    IsTruth,
    IsFalsity,
    MemberOf,
    Between,
    BitOr,
    BitAnd,
    LeftShift,
    RightShift,
    Plus,
    Minus,
    Mul,
    Div,
    IntDiv,
    Mod,
    Xor,
    Collate,
    UnaryPlus,
    UnaryMinus,
    Not,
    Not2,
    BitNeg,
}

impl Op {
    /// 运算符对应的 SQL 文本。
    fn sql(self) -> &'static str {
        match self {
            Self::LogicOr => "OR",
            Self::LogicXor => "XOR",
            Self::LogicAnd => "AND",
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::NullEq => "<=>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::In => "IN",
            Self::Like => "LIKE",
            Self::Regexp => "REGEXP",
            Self::IsNull => "IS NULL",
            Self::IsTruth => "IS TRUE",
            Self::IsFalsity => "IS FALSE",
            Self::MemberOf => "MEMBER OF",
            Self::Between => "BETWEEN",
            Self::BitOr => "|",
            Self::BitAnd => "&",
            Self::LeftShift => "<<",
            Self::RightShift => ">>",
            Self::Plus | Self::UnaryPlus => "+",
            Self::Minus | Self::UnaryMinus => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::IntDiv => "DIV",
            Self::Mod => "%",
            Self::Xor => "^",
            Self::Collate => "COLLATE",
            Self::Not => "NOT",
            Self::Not2 => "!",
            Self::BitNeg => "~",
        }
    }

    /// 是否以关键字形式输出（两侧需空格）。
    fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::LogicOr | Self::LogicXor | Self::LogicAnd | Self::IntDiv
        )
    }

    /// 运算符优先级，数值越大绑定越紧。
    fn precedence(self) -> u8 {
        match self {
            Self::LogicOr => 1,
            Self::LogicXor => 2,
            Self::LogicAnd => 3,
            Self::Between => 4,
            Self::Eq
            | Self::Ne
            | Self::NullEq
            | Self::Lt
            | Self::Le
            | Self::Gt
            | Self::Ge
            | Self::In
            | Self::Like
            | Self::Regexp
            | Self::IsNull
            | Self::IsTruth
            | Self::IsFalsity
            | Self::MemberOf => 5,
            Self::BitOr => 6,
            Self::BitAnd => 7,
            Self::LeftShift | Self::RightShift => 8,
            Self::Plus | Self::Minus => 9,
            Self::Mul | Self::Div | Self::IntDiv | Self::Mod => 10,
            Self::Xor => 11,
            Self::Collate => 12,
            Self::UnaryPlus | Self::UnaryMinus | Self::Not | Self::Not2 | Self::BitNeg => 13,
        }
    }

    /// 与同级子运算符是否可结合从而省略括号。
    fn is_associative_with(self, child: Self) -> bool {
        self == child
            && matches!(
                self,
                Self::LogicAnd | Self::LogicOr | Self::BitAnd | Self::BitOr | Self::Xor
            )
    }
}

/// 大小写不敏感字符串：保留原文并缓存小写形式供比较。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiString {
    pub original: String,
    pub lowercase: String,
}

impl CiString {
    /// 由原文构造，同时生成 ASCII 小写副本。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lowercase = original.to_ascii_lowercase();
        Self {
            original,
            lowercase,
        }
    }
}

impl Default for CiString {
    fn default() -> Self {
        Self::new("")
    }
}

/// 列名三段式：schema.table.column（均可为空前缀）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnName {
    pub schema: CiString,
    pub table: CiString,
    pub name: CiString,
}

impl ColumnName {
    /// 由三段字符串构造列名。
    pub fn new(schema: &str, table: &str, name: &str) -> Self {
        Self {
            schema: CiString::new(schema),
            table: CiString::new(table),
            name: CiString::new(name),
        }
    }

    /// 按标志与 CTE 名集合还原列名。
    pub fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        // CTE 表名或 WITHOUT_SCHEMA_NAME 时跳过 schema 前缀
        if !self.schema.original.is_empty()
            && !ctx.cte_names.contains(&self.table.lowercase)
            && !ctx.flags.contains(RestoreFlags::WITHOUT_SCHEMA_NAME)
        {
            ctx.write_name(&self.schema.original);
            ctx.write_plain(".");
        }
        if !self.table.original.is_empty() && !ctx.flags.contains(RestoreFlags::WITHOUT_TABLE_NAME)
        {
            ctx.write_name(&self.table.original);
            ctx.write_plain(".");
        }
        ctx.write_name(&self.name.original);
        Ok(())
    }

    /// 以默认标志还原为 SQL 字符串。
    pub fn to_sql(&self) -> String {
        self.to_sql_with_flags(RestoreFlags::empty())
    }

    /// 以指定标志还原为 SQL 字符串。
    pub fn to_sql_with_flags(&self, flags: RestoreFlags) -> String {
        let mut ctx = RestoreCtx::new(flags);
        self.restore(&mut ctx)
            .expect("ColumnName restore cannot fail");
        ctx.finish()
    }

    /// 用原文点号拼接的列名（不加反引号）。
    pub fn original_column_name(&self) -> String {
        let mut parts = Vec::new();
        if !self.schema.original.is_empty() {
            parts.push(self.schema.original.as_str());
        }
        if !self.table.original.is_empty() {
            parts.push(self.table.original.as_str());
        }
        parts.push(self.name.original.as_str());
        parts.join(".")
    }

    /// 按小写比较；空 schema/table 视为通配匹配。
    pub fn matches(&self, other: &Self) -> bool {
        (self.schema.lowercase.is_empty() || self.schema.lowercase == other.schema.lowercase)
            && (self.table.lowercase.is_empty() || self.table.lowercase == other.table.lowercase)
            && self.name.lowercase == other.name.lowercase
    }
}

impl fmt::Display for ColumnName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if !self.schema.lowercase.is_empty() {
            parts.push(self.schema.lowercase.as_str());
        }
        if !self.table.lowercase.is_empty() {
            parts.push(self.table.lowercase.as_str());
        }
        parts.push(self.name.lowercase.as_str());
        write!(f, "{}", parts.join("."))
    }
}

/// SQL 字面量值：NULL、布尔、整型、浮点文本与字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(String),
    String(String),
}

impl Value {
    /// 将字面量写入还原上下文（字符串带 `_UTF8MB4` 前缀）。
    fn restore(&self, ctx: &mut RestoreCtx) {
        match self {
            Self::Null => ctx.write_keyword("NULL"),
            Self::Bool(value) => ctx.write_keyword(if *value { "TRUE" } else { "FALSE" }),
            Self::Int(value) => ctx.write_plain(&value.to_string()),
            Self::UInt(value) => ctx.write_plain(&value.to_string()),
            Self::Float(value) => ctx.write_plain(value),
            Self::String(value) => {
                ctx.write_plain("_UTF8MB4");
                ctx.write_string(value);
            }
        }
    }
}

/// 值表达式接口：存取运行时值与投影偏移。
pub trait ValueExpr {
    fn set_value(&mut self, value: Box<dyn Any>);
    fn get_value(&self) -> &dyn Any;
    fn projection_offset(&self) -> i32;
    fn set_projection_offset(&mut self, offset: i32);
}

/// 预处理参数占位符表达式接口（顺序、偏移、执行期状态）。
pub trait ParamMarkerExpr: ValueExpr {
    fn set_order(&mut self, order: i32);
    fn order(&self) -> i32;
    fn offset(&self) -> i32;
    fn in_execute(&self) -> bool;
    fn clone_box(&self) -> Box<dyn ParamMarkerExpr>;
    fn as_any(&self) -> &dyn Any;

    /// 深度优先接受 Visitor：enter → 子节点 → leave。
    fn accept(&mut self, visitor: &mut dyn Visitor) -> bool;
}

impl Clone for Box<dyn ParamMarkerExpr> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// BETWEEN / NOT BETWEEN 表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BetweenExpr {
    pub expr: Box<Expr>,
    pub left: Box<Expr>,
    pub right: Box<Expr>,
    pub not: bool,
}

/// 二元运算表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryOperationExpr {
    pub op: Op,
    pub left: Box<Expr>,
    pub right: Box<Expr>,
}

/// CASE 中的 WHEN ... THEN ... 子句。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WhenClause {
    pub expr: Expr,
    pub result: Expr,
}

/// CASE 表达式：可选比较值、WHEN 列表与 ELSE。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CaseExpr {
    pub value: Option<Box<Expr>>,
    pub when_clauses: Vec<WhenClause>,
    pub else_clause: Option<Box<Expr>>,
}

/// 标量子查询或存在性子查询的载体（以 SQL 文本保存查询体）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubqueryExpr {
    pub query: String,
    pub evaluated: bool,
    pub correlated: bool,
    pub multi_rows: bool,
    pub exists: bool,
}

/// 与子查询比较：`expr op ANY/ALL (subquery)`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompareSubqueryExpr {
    pub left: Box<Expr>,
    pub op: Op,
    pub right: Box<Expr>,
    pub all: bool,
}

/// 表名：schema + name（表达式侧精简版）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableName {
    pub schema: CiString,
    pub name: CiString,
}

/// 表名表达式包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableNameExpr {
    pub name: TableName,
}

/// 列名表达式包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnNameExpr {
    pub name: ColumnName,
}

impl ColumnNameExpr {
    /// 由列名构造列名表达式。
    pub fn new(name: ColumnName) -> Self {
        Self { name }
    }
}

/// DEFAULT 或 DEFAULT(col) 表达式。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DefaultExpr {
    pub name: Option<ColumnName>,
}

/// EXISTS / NOT EXISTS 子查询。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExistsSubqueryExpr {
    pub select: Box<Expr>,
    pub not: bool,
}

/// IN / NOT IN 列表或子查询。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatternInExpr {
    pub expr: Box<Expr>,
    pub list: Vec<Expr>,
    pub not: bool,
    pub select: Option<Box<Expr>>,
}

/// IS NULL / IS NOT NULL。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsNullExpr {
    pub expr: Box<Expr>,
    pub not: bool,
}

/// IS TRUE / IS FALSE（及 NOT 变体）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsTruthExpr {
    pub expr: Box<Expr>,
    pub not: bool,
    pub true_value: i64,
}

/// LIKE / ILIKE 模式匹配（含转义字符）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatternLikeOrIlikeExpr {
    pub expr: Box<Expr>,
    pub pattern: Box<Expr>,
    pub not: bool,
    pub is_like: bool,
    pub escape: u8,
    pub escape_explicit: bool,
    pub pat_chars: Vec<u8>,
    pub pat_types: Vec<u8>,
}

/// 显式括号包裹的子表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParenthesesExpr {
    pub expr: Box<Expr>,
}

/// SELECT 列表位置引用（如 ORDER BY 1）及可选参数占位符。
#[derive(Default)]
pub struct PositionExpr {
    pub position: i32,
    pub parameter: Option<Box<dyn ParamMarkerExpr>>,
}

impl Clone for PositionExpr {
    fn clone(&self) -> Self {
        Self {
            position: self.position,
            parameter: self.parameter.clone(),
        }
    }
}

impl fmt::Debug for PositionExpr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PositionExpr")
            .field("position", &self.position)
            .field(
                "parameter",
                &self
                    .parameter
                    .as_deref()
                    .map(|marker| (marker.offset(), marker.order(), marker.in_execute())),
            )
            .finish()
    }
}

impl PartialEq for PositionExpr {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position
            && match (self.parameter.as_deref(), other.parameter.as_deref()) {
                (None, None) => true,
                (Some(left), Some(right)) => {
                    left.offset() == right.offset()
                        && left.order() == right.order()
                        && left.in_execute() == right.in_execute()
                }
                _ => false,
            }
    }
}

impl Eq for PositionExpr {}

/// REGEXP / NOT REGEXP 正则匹配。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatternRegexpExpr {
    pub expr: Box<Expr>,
    pub pattern: Box<Expr>,
    pub not: bool,
    pub compiled_pattern: Option<String>,
    pub expression_text: Option<String>,
}

/// ROW(...) 行构造器。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RowExpr {
    pub values: Vec<Expr>,
}

/// 一元运算（+/-、NOT、位取反等）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnaryOperationExpr {
    pub op: Op,
    pub value: Box<Expr>,
}

/// INSERT ... ON DUPLICATE 中的 VALUES(col) 引用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValuesExpr {
    pub column: ColumnNameExpr,
}

/// 用户变量 `@v` 或系统变量 `@@SESSION.x` 等。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VariableExpr {
    pub name: String,
    pub is_global: bool,
    pub is_instance: bool,
    pub is_system: bool,
    pub explicit_scope: bool,
    pub value: Option<Box<Expr>>,
}

/// 分区定义中的 MAXVALUE 哨兵值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MaxValueExpr;

/// 全文检索 AGAINST 修饰符位集合。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FulltextSearchModifier(u8);

impl FulltextSearchModifier {
    /// IN BOOLEAN MODE 修饰符。
    pub const BOOLEAN_MODE: Self = Self(1 << 0);
    /// WITH QUERY EXPANSION 修饰符。
    pub const QUERY_EXPANSION: Self = Self(1 << 4);

    /// 无修饰符。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 是否启用布尔模式。
    pub const fn is_boolean_mode(self) -> bool {
        self.0 & Self::BOOLEAN_MODE.0 != 0
    }

    /// 是否启用查询扩展。
    pub const fn with_query_expansion(self) -> bool {
        self.0 & Self::QUERY_EXPANSION.0 != 0
    }
}

impl BitOr for FulltextSearchModifier {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// MATCH (cols) AGAINST (...) 全文检索表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchAgainst {
    pub column_names: Vec<ColumnName>,
    pub against: Box<Expr>,
    pub modifier: FulltextSearchModifier,
}

/// 表达式 COLLATE collation 子句。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetCollationExpr {
    pub expr: Box<Expr>,
    pub collate: String,
}

/// 表达式 AST 代数数据类型，覆盖字面量到复合运算的全部节点。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    Value(Value),
    Raw(String),
    Between(BetweenExpr),
    Binary(BinaryOperationExpr),
    Case(CaseExpr),
    Subquery(SubqueryExpr),
    CompareSubquery(CompareSubqueryExpr),
    TableName(TableNameExpr),
    ColumnName(ColumnNameExpr),
    Default(DefaultExpr),
    ExistsSubquery(ExistsSubqueryExpr),
    PatternIn(PatternInExpr),
    IsNull(IsNullExpr),
    IsTruth(IsTruthExpr),
    PatternLike(PatternLikeOrIlikeExpr),
    Parentheses(ParenthesesExpr),
    Position(PositionExpr),
    PatternRegexp(PatternRegexpExpr),
    Row(RowExpr),
    Unary(UnaryOperationExpr),
    Values(ValuesExpr),
    Variable(VariableExpr),
    MaxValue(MaxValueExpr),
    MatchAgainst(MatchAgainst),
    SetCollation(SetCollationExpr),
}

impl Expr {
    /// 尝试还原为 SQL，失败返回错误消息。
    pub fn try_to_sql(&self) -> RestoreResult<String> {
        self.try_to_sql_with_flags(RestoreFlags::empty())
    }

    /// 带标志尝试还原为 SQL。
    pub fn try_to_sql_with_flags(&self, flags: RestoreFlags) -> RestoreResult<String> {
        let mut ctx = RestoreCtx::new(flags);
        self.restore(&mut ctx)?;
        Ok(ctx.finish())
    }

    /// 还原为 SQL；失败时 panic。
    pub fn to_sql(&self) -> String {
        self.try_to_sql().expect("expression restore failed")
    }

    /// 带标志还原为 SQL；失败时 panic。
    pub fn to_sql_with_flags(&self, flags: RestoreFlags) -> String {
        self.try_to_sql_with_flags(flags)
            .expect("expression restore failed")
    }

    /// 将本表达式写入还原上下文。
    pub fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        match self {
            Self::Value(value) => value.restore(ctx),
            Self::Raw(sql) => ctx.write_plain(sql),
            Self::Between(node) => node.restore(ctx)?,
            Self::Binary(node) => node.restore(ctx)?,
            Self::Case(node) => node.restore(ctx)?,
            Self::Subquery(node) => {
                ctx.write_plain("(");
                with_reset_parent_context(ctx, |ctx| ctx.write_plain(&node.query));
                ctx.write_plain(")");
            }
            Self::CompareSubquery(node) => node.restore(ctx)?,
            Self::TableName(node) => {
                if !node.name.schema.original.is_empty() {
                    ctx.write_name(&node.name.schema.original);
                    ctx.write_plain(".");
                }
                ctx.write_name(&node.name.name.original);
            }
            Self::ColumnName(node) => node.name.restore(ctx)?,
            Self::Default(node) => {
                ctx.write_keyword("DEFAULT");
                if let Some(name) = &node.name {
                    ctx.write_plain("(");
                    name.restore(ctx)?;
                    ctx.write_plain(")");
                }
            }
            Self::ExistsSubquery(node) => {
                ctx.write_keyword(if node.not { "NOT EXISTS " } else { "EXISTS " });
                node.select.restore(ctx)?;
            }
            Self::PatternIn(node) => node.restore(ctx)?,
            Self::IsNull(node) => {
                restore_binary_child(ctx, &node.expr, Op::IsNull, BinarySide::Left)?;
                ctx.write_keyword(if node.not { " IS NOT NULL" } else { " IS NULL" });
            }
            Self::IsTruth(node) => {
                let op = if node.true_value > 0 {
                    Op::IsTruth
                } else {
                    Op::IsFalsity
                };
                restore_binary_child(ctx, &node.expr, op, BinarySide::Left)?;
                ctx.write_keyword(if node.not { " IS NOT" } else { " IS" });
                ctx.write_keyword(if node.true_value > 0 {
                    " TRUE"
                } else {
                    " FALSE"
                });
            }
            Self::PatternLike(node) => node.restore(ctx)?,
            Self::Parentheses(node) => node.restore(ctx)?,
            Self::Position(node) => ctx.write_plain(&node.position.to_string()),
            Self::PatternRegexp(node) => {
                restore_binary_child(ctx, &node.expr, Op::Regexp, BinarySide::Left)?;
                ctx.write_keyword(if node.not { " NOT REGEXP " } else { " REGEXP " });
                restore_binary_child(ctx, &node.pattern, Op::Regexp, BinarySide::Right)?;
            }
            Self::Row(node) => {
                ctx.write_keyword("ROW");
                ctx.write_plain("(");
                restore_list(ctx, &node.values)?;
                ctx.write_plain(")");
            }
            Self::Unary(node) => node.restore(ctx)?,
            Self::Values(node) => {
                ctx.write_keyword("VALUES");
                ctx.write_plain("(");
                node.column.name.restore(ctx)?;
                ctx.write_plain(")");
            }
            Self::Variable(node) => node.restore(ctx)?,
            Self::MaxValue(_) => ctx.write_keyword("MAXVALUE"),
            Self::MatchAgainst(node) => node.restore(ctx)?,
            Self::SetCollation(node) => {
                restore_binary_child(ctx, &node.expr, Op::Collate, BinarySide::Left)?;
                ctx.write_keyword(" COLLATE ");
                ctx.write_plain(&node.collate);
            }
        }
        Ok(())
    }

    /// 若节点对应二元类运算符则返回其 Op，供括号判定。
    fn restore_op(&self) -> Option<Op> {
        match self {
            Self::Binary(node) => Some(node.op),
            Self::Between(_) => Some(Op::Between),
            Self::CompareSubquery(node) => Some(node.op),
            Self::IsNull(_) => Some(Op::IsNull),
            Self::IsTruth(node) if node.true_value > 0 => Some(Op::IsTruth),
            Self::IsTruth(_) => Some(Op::IsFalsity),
            Self::PatternIn(_) => Some(Op::In),
            Self::PatternLike(_) => Some(Op::Like),
            Self::PatternRegexp(_) => Some(Op::Regexp),
            Self::SetCollation(_) => Some(Op::Collate),
            _ => None,
        }
    }

    pub fn accept<V: Visitor>(&mut self, visitor: &mut V) -> bool {
        fn visit_column<V: Visitor>(column: &mut ColumnName, visitor: &mut V) -> bool {
            let _skip = visitor.enter_column_name(column);
            visitor.leave_column_name(column)
        }
        fn visit_table<V: Visitor>(table: &mut TableName, visitor: &mut V) -> bool {
            let _skip = visitor.enter_table_name(table);
            visitor.leave_table_name(table)
        }
        // enter 返回 true：跳过子节点，直接 leave
        if visitor.enter(self) {
            return visitor.leave(self);
        }
        let children_ok = match self {
            Self::Between(node) => {
                node.expr.accept(visitor) && node.left.accept(visitor) && node.right.accept(visitor)
            }
            Self::Binary(node) => node.left.accept(visitor) && node.right.accept(visitor),
            Self::Case(node) => {
                node.value
                    .as_mut()
                    .is_none_or(|value| value.accept(visitor))
                    && node
                        .when_clauses
                        .iter_mut()
                        .all(|clause| clause.expr.accept(visitor) && clause.result.accept(visitor))
                    && node
                        .else_clause
                        .as_mut()
                        .is_none_or(|value| value.accept(visitor))
            }
            Self::CompareSubquery(node) => node.left.accept(visitor) && node.right.accept(visitor),
            Self::ExistsSubquery(node) => node.select.accept(visitor),
            Self::PatternIn(node) => {
                node.expr.accept(visitor)
                    && node.list.iter_mut().all(|item| item.accept(visitor))
                    && node
                        .select
                        .as_mut()
                        .is_none_or(|select| select.accept(visitor))
            }
            Self::IsNull(node) => node.expr.accept(visitor),
            Self::IsTruth(node) => node.expr.accept(visitor),
            Self::PatternLike(node) => node.expr.accept(visitor) && node.pattern.accept(visitor),
            Self::Parentheses(node) => node.expr.accept(visitor),
            Self::Position(node) => node
                .parameter
                .as_mut()
                .is_none_or(|value| value.accept(visitor)),
            Self::PatternRegexp(node) => node.expr.accept(visitor) && node.pattern.accept(visitor),
            Self::Row(node) => node.values.iter_mut().all(|value| value.accept(visitor)),
            Self::Unary(node) => node.value.accept(visitor),
            Self::Variable(node) => node
                .value
                .as_mut()
                .is_none_or(|value| value.accept(visitor)),
            Self::MatchAgainst(node) => {
                node.column_names
                    .iter_mut()
                    .all(|column| visit_column(column, visitor))
                    && node.against.accept(visitor)
            }
            Self::SetCollation(node) => node.expr.accept(visitor),
            Self::TableName(node) => visit_table(&mut node.name, visitor),
            Self::ColumnName(node) => visit_column(&mut node.name, visitor),
            Self::Values(node) => visit_column(&mut node.column.name, visitor),
            _ => true,
        };
        children_ok && visitor.leave(self)
    }
}

impl BetweenExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        let brackets = ctx.flags.contains(RestoreFlags::BRACKET_AROUND_BETWEEN);
        if brackets {
            ctx.write_plain("(");
        }
        restore_binary_child(ctx, &self.expr, Op::Between, BinarySide::Left)?;
        ctx.write_keyword(if self.not {
            " NOT BETWEEN "
        } else {
            " BETWEEN "
        });
        restore_binary_child(ctx, &self.left, Op::Between, BinarySide::Right)?;
        ctx.write_keyword(" AND ");
        restore_binary_child(ctx, &self.right, Op::Between, BinarySide::Right)?;
        if brackets {
            ctx.write_plain(")");
        }
        Ok(())
    }
}

impl BinaryOperationExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        let brackets = ctx.flags.contains(RestoreFlags::BRACKET_AROUND_BINARY);
        let original_flags = ctx.flags;
        if brackets {
            ctx.write_plain("(");
            ctx.flags |= RestoreFlags::BRACKET_AROUND_BETWEEN;
        }
        restore_binary_child(ctx, &self.left, self.op, BinarySide::Left)?;
        let spaces = ctx.flags.contains(RestoreFlags::SPACES_AROUND_BINARY) || self.op.is_keyword();
        if spaces {
            ctx.write_plain(" ");
        }
        ctx.write_plain(self.op.sql());
        if spaces {
            ctx.write_plain(" ");
        }
        restore_binary_child(ctx, &self.right, self.op, BinarySide::Right)?;
        if brackets {
            ctx.write_plain(")");
        }
        ctx.flags = original_flags;
        Ok(())
    }
}

impl CaseExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        ctx.write_keyword("CASE");
        if let Some(value) = &self.value {
            ctx.write_plain(" ");
            value.restore(ctx)?;
        }
        for clause in &self.when_clauses {
            ctx.write_keyword(" WHEN ");
            clause.expr.restore(ctx)?;
            ctx.write_keyword(" THEN ");
            clause.result.restore(ctx)?;
        }
        if let Some(value) = &self.else_clause {
            ctx.write_keyword(" ELSE ");
            value.restore(ctx)?;
        }
        ctx.write_keyword(" END");
        Ok(())
    }
}

impl CompareSubqueryExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        restore_binary_child(ctx, &self.left, self.op, BinarySide::Left)?;
        let spaces = ctx.flags.contains(RestoreFlags::SPACES_AROUND_BINARY) || self.op.is_keyword();
        if spaces {
            ctx.write_plain(" ");
        }
        ctx.write_plain(self.op.sql());
        if spaces {
            ctx.write_plain(" ");
        }
        ctx.write_keyword(if self.all { "ALL " } else { "ANY " });
        self.right.restore(ctx)
    }
}

impl PatternInExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        restore_binary_child(ctx, &self.expr, Op::In, BinarySide::Left)?;
        ctx.write_keyword(if self.not { " NOT IN " } else { " IN " });
        if let Some(select) = &self.select {
            select.restore(ctx)
        } else {
            ctx.write_plain("(");
            restore_list(ctx, &self.list)?;
            ctx.write_plain(")");
            Ok(())
        }
    }
}

impl PatternLikeOrIlikeExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        restore_binary_child(ctx, &self.expr, Op::Like, BinarySide::Left)?;
        let keyword = match (self.is_like, self.not) {
            (true, true) => " NOT LIKE ",
            (true, false) => " LIKE ",
            (false, true) => " NOT ILIKE ",
            (false, false) => " ILIKE ",
        };
        ctx.write_keyword(keyword);
        restore_binary_child(ctx, &self.pattern, Op::Like, BinarySide::Right)?;
        if self.escape_explicit && self.escape != b'\\' {
            ctx.write_keyword(" ESCAPE ");
            if self.escape == 0 {
                ctx.write_string("");
            } else {
                ctx.write_string(&(self.escape as char).to_string());
            }
        }
        Ok(())
    }
}

impl ParenthesesExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        if ctx.flags.contains(RestoreFlags::SKIP_REDUNDANT_PARENTHESES)
            && can_restore_without_parentheses(ctx, &self.expr)
        {
            return self.expr.restore(ctx);
        }
        ctx.write_plain("(");
        let result = with_reset_parent_context(ctx, |ctx| self.expr.restore(ctx));
        ctx.write_plain(")");
        result
    }
}

impl UnaryOperationExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        ctx.write_plain(self.op.sql());
        if self.op == Op::Not {
            ctx.write_plain(" ");
        }
        let old = ctx.in_unary_operation;
        ctx.in_unary_operation = true;
        let result = self.value.restore(ctx);
        ctx.in_unary_operation = old;
        result
    }
}

impl VariableExpr {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        if self.is_system {
            ctx.write_plain("@@");
            if self.explicit_scope {
                ctx.write_keyword(if self.is_global {
                    "GLOBAL"
                } else if self.is_instance {
                    "INSTANCE"
                } else {
                    "SESSION"
                });
                ctx.write_plain(".");
            }
        } else {
            ctx.write_plain("@");
        }
        ctx.write_name(&self.name);
        if let Some(value) = &self.value {
            ctx.write_plain(":=");
            value.restore(ctx)?;
        }
        Ok(())
    }
}

impl MatchAgainst {
    fn restore(&self, ctx: &mut RestoreCtx) -> RestoreResult {
        ctx.write_keyword("MATCH");
        ctx.write_plain(" (");
        for (index, column) in self.column_names.iter().enumerate() {
            if index != 0 {
                ctx.write_plain(",");
            }
            column.restore(ctx)?;
        }
        ctx.write_plain(") ");
        ctx.write_keyword("AGAINST");
        ctx.write_plain(" (");
        self.against.restore(ctx)?;
        // BOOLEAN MODE 与 QUERY EXPANSION 互斥
        if self.modifier.is_boolean_mode() {
            ctx.write_plain(" IN BOOLEAN MODE");
            if self.modifier.with_query_expansion() {
                return Err("BOOLEAN MODE doesn't support QUERY EXPANSION".into());
            }
        } else if self.modifier.with_query_expansion() {
            ctx.write_plain(" WITH QUERY EXPANSION");
        }
        ctx.write_plain(")");
        Ok(())
    }
}

/// 以逗号分隔还原表达式列表。
fn restore_list(ctx: &mut RestoreCtx, values: &[Expr]) -> RestoreResult {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            ctx.write_plain(",");
        }
        value.restore(ctx)?;
    }
    Ok(())
}

/// 在设定父运算符与左右侧后还原子表达式，供优先级括号逻辑使用。
fn restore_binary_child(
    ctx: &mut RestoreCtx,
    expr: &Expr,
    parent_op: Op,
    side: BinarySide,
) -> RestoreResult {
    let old_op = ctx.parent_binary_op;
    let old_side = ctx.parent_binary_side;
    ctx.parent_binary_op = Some(parent_op);
    ctx.parent_binary_side = side;
    let result = expr.restore(ctx);
    ctx.parent_binary_op = old_op;
    ctx.parent_binary_side = old_side;
    result
}

/// 临时清空父运算符/一元上下文后执行闭包，再恢复。
fn with_reset_parent_context<T>(
    ctx: &mut RestoreCtx,
    restore: impl FnOnce(&mut RestoreCtx) -> T,
) -> T {
    let old_op = ctx.parent_binary_op;
    let old_side = ctx.parent_binary_side;
    let old_unary = ctx.in_unary_operation;
    ctx.parent_binary_op = None;
    ctx.parent_binary_side = BinarySide::None;
    ctx.in_unary_operation = false;
    let result = restore(ctx);
    ctx.parent_binary_op = old_op;
    ctx.parent_binary_side = old_side;
    ctx.in_unary_operation = old_unary;
    result
}

/// 依据优先级与结合性判断是否可安全省略显式括号。
fn can_restore_without_parentheses(ctx: &RestoreCtx, expr: &Expr) -> bool {
    if ctx.in_unary_operation {
        return false;
    }
    if matches!(expr, Expr::Unary(_)) && ctx.parent_binary_op.is_some() {
        return false;
    }
    let Some(child_op) = expr.restore_op() else {
        return true;
    };
    let Some(parent_op) = ctx.parent_binary_op else {
        return true;
    };
    // 子优先级更高可省略括号；同级则看左侧或可结合
    let parent_precedence = parent_op.precedence();
    let child_precedence = child_op.precedence();
    if child_precedence > parent_precedence {
        true
    } else if child_precedence < parent_precedence {
        false
    } else {
        ctx.parent_binary_side == BinarySide::Left || parent_op.is_associative_with(child_op)
    }
}

/// 表达式访问者：`enter` 返回 true 表示跳过子节点（对齐 Go Visitor.Enter）。
pub trait Visitor {
    /// Return true to skip children, matching Go's Visitor.Enter contract.
    fn enter(&mut self, node: &mut Expr) -> bool;
    fn leave(&mut self, node: &mut Expr) -> bool;

    fn enter_column_name(&mut self, _node: &mut ColumnName) -> bool {
        false
    }
    fn leave_column_name(&mut self, _node: &mut ColumnName) -> bool {
        true
    }
    fn enter_table_name(&mut self, _node: &mut TableName) -> bool {
        false
    }
    fn leave_table_name(&mut self, _node: &mut TableName) -> bool {
        true
    }

    fn enter_param_marker(&mut self, _node: &mut dyn ParamMarkerExpr) -> bool {
        false
    }

    fn leave_param_marker(&mut self, _node: &mut dyn ParamMarkerExpr) -> bool {
        true
    }
}

/// 比较两表达式是否结构相等。
pub fn expression_deep_equal(a: &Expr, b: &Expr) -> bool {
    a == b
}

/// Go 风格命名别名：[`expression_deep_equal`]。
pub fn ExpressionDeepEqual(a: &Expr, b: &Expr) -> bool {
    expression_deep_equal(a, b)
}
