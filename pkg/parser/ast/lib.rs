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

// `parser_ast` crate 入口：导出 AST 节点、访问者协议与辅助子模块。
//
// 对应 Go `pkg/parser/ast` 的核心语句/表达式类型面，含优化器提示、
// DML/DDL、SHOW、BRIE、窗口与集合运算等；`integration` 子模块提供集成视图。

#![allow(non_snake_case, non_upper_case_globals, dead_code, non_fmt_panics)]

extern crate self as parser_ast;

extern crate parser_auth;
extern crate parser_charset;
extern crate parser_mysql;
extern crate parser_types;
extern crate serde;
extern crate serde_json;
extern crate url;

/// 集成用 AST 类型聚合模块。
pub mod integration;
pub mod materialized;
pub mod sql_restore;

/// 元数据 JSON 编解码辅助模块。
pub mod metadata_json {
    use serde::{Serialize, de::DeserializeOwned};

    pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
        serde_json::to_vec(value).map_err(|error| error.to_string())
    }

    pub fn decode<T: DeserializeOwned>(encoded: &[u8]) -> Result<T, String> {
        serde_json::from_slice(encoded).map_err(|error| error.to_string())
    }
}

use std::{any::Any, fmt, mem::size_of};

/// AST 访问者接口：enter/leave 控制子树遍历。
pub trait Visitor {
    fn enter(&mut self, input: &dyn Node) -> bool;
    fn leave(&mut self, input: &dyn Node) -> bool;
    // TableName is a value type in this Rust AST, so it has no Node base fields.
    fn enter_table_name(&mut self, _input: &TableName) -> bool {
        false
    }
    fn leave_table_name(&mut self, _input: &TableName) -> bool {
        true
    }
}

/// Visits an AST without replacing nodes. A `true` enter result skips children.
pub trait InPlaceVisitor {
    fn enter(&mut self, input: &mut dyn Node) -> bool;
    fn leave(&mut self, input: &mut dyn Node) -> bool;
    fn enter_table_name(&mut self, _input: &mut TableName) -> bool {
        false
    }
    fn leave_table_name(&mut self, _input: &mut TableName) -> bool {
        true
    }
}

/// Walks a mutable AST in the same child order as `Node::accept`.
pub fn Walk(node: &mut dyn Node, visitor: &mut dyn InPlaceVisitor) -> bool {
    node.accept_in_place(visitor)
}

/// AST 节点基础接口，支持类型擦除与访问者遍历。
pub trait Node: Any {
    fn node_text(&self) -> &base::AstNode;
    fn node_text_mut(&mut self) -> &mut base::AstNode;
    fn Text(&self) -> String {
        self.node_text().Text()
    }
    fn OriginalText(&self) -> &[u8] {
        self.node_text().OriginalText()
    }
    fn SetText(&mut self, encoding: Option<base::EncodingRef>, text: &[u8]) {
        self.node_text_mut().SetText(encoding, text);
    }
    fn SetNoBackslashEscapes(&mut self, value: bool) {
        self.node_text_mut().SetNoBackslashEscapes(value);
    }
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    fn accept(&self, visitor: &mut dyn Visitor) -> bool;
    fn accept_in_place(&mut self, visitor: &mut dyn InPlaceVisitor) -> bool;
}

/// DO 语句节点：求值表达式但不返回结果集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DoStmt {
    pub node_text: base::AstNode,
    pub Exprs: Vec<ExprNode>,
}

/// CALL 语句节点：调用存储过程。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CallStmt {
    pub node_text: base::AstNode,
    pub Procedure: ExprNode,
}

/// SHOW 语句子类型枚举，对应各类 SHOW 命令。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShowStmtType {
    #[default]
    None,
    Engines,
    Databases,
    Tables,
    TableStatus,
    Columns,
    Warnings,
    Charset,
    Variables,
    Status,
    Collation,
    CreateTable,
    CreateView,
    CreateUser,
    CreateSequence,
    CreatePlacementPolicy,
    Grants,
    MaskingPolicies,
    Triggers,
    ProcedureStatus,
    FunctionStatus,
    Index,
    ProcessList,
    CreateDatabase,
    Config,
    Events,
    StatsExtended,
    StatsMeta,
    StatsHistograms,
    StatsTopN,
    StatsBuckets,
    StatsHealthy,
    StatsLocked,
    HistogramsInFlight,
    ColumnStatsUsage,
    Plugins,
    Profile,
    Profiles,
    MasterStatus,
    Privileges,
    Errors,
    Bindings,
    BindingCacheStatus,
    OpenTables,
    AnalyzeStatus,
    Regions,
    Builtins,
    TableNextRowId,
    Backups,
    Restores,
    Imports,
    CreateImport,
    Placement,
    PlacementForDatabase,
    PlacementForTable,
    PlacementForPartition,
    PlacementLabels,
    SessionStates,
    CreateResourceGroup,
    ImportJobs,
    ImportGroups,
    CreateProcedure,
    BinlogStatus,
    ReplicaStatus,
    Distributions,
    DistributionJobs,
    Affinity,
}

/// SHOW 语句节点及其过滤/目标对象字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowStmt {
    pub node_text: base::AstNode,
    pub Tp: ShowStmtType,
    pub DBName: String,
    pub Table: Option<TableName>,
    pub Procedure: Option<TableName>,
    pub Partition: CIStr,
    pub Column: Option<ColumnName>,
    pub IndexName: CIStr,
    pub ResourceGroupName: String,
    pub Flag: isize,
    pub Full: bool,
    pub IfNotExists: bool,
    pub Extended: bool,
    pub CountWarningsOrErrors: bool,
    pub GlobalScope: bool,
    pub Pattern: Option<ExprNode>,
    pub Where: Option<ExprNode>,
    pub ShowGroupKey: String,
    pub ImportJobID: Option<i64>,
    pub ImportJobRaw: bool,
    pub DistributionJobID: Option<i64>,
    pub User: Option<parser_auth::parser::auth::auth::UserIdentity>,
    pub Roles: Vec<parser_auth::parser::auth::auth::RoleIdentity>,
    pub ShowProfileTypes: Vec<ProfileType>,
    pub ShowProfileArgs: Option<i64>,
    pub ShowProfileLimit: Option<Limit>,
}

/// 大小写不敏感标识符：保留原始串 O 与小写串 L。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, serde::Serialize)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CIStrInput {
    Object { O: String, L: String },
    String(String),
}

impl<'de> serde::Deserialize<'de> for CIStr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match CIStrInput::deserialize(deserializer)? {
            CIStrInput::Object { O, L } => Self { O, L },
            CIStrInput::String(value) => NewCIStr(&value),
        })
    }
}

impl fmt::Display for CIStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.O)
    }
}

impl CIStr {
    pub fn hash64<H: std::hash::Hasher>(&self, hasher: &mut H) {
        hasher.write(self.L.as_bytes());
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.L == other.L
    }

    pub fn memory_usage(&self) -> i64 {
        (size_of::<String>() * 2 + self.O.len() + self.L.len()) as i64
    }
}

/// 由原始字符串构造 CIStr，同时生成小写形式。
pub fn NewCIStr(value: impl AsRef<str>) -> CIStr {
    let value = value.as_ref();
    CIStr {
        O: value.to_owned(),
        L: value.to_lowercase(),
    }
}

/// 提示中的表引用（库名、表名、查询块、分区）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HintTable {
    pub DBName: CIStr,
    pub TableName: CIStr,
    pub QBName: CIStr,
    pub PartitionList: Vec<CIStr>,
}

/// LEADING 列表元素：单表或嵌套子列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeadingItem {
    Table(HintTable),
    List(LeadingList),
}

/// LEADING 提示中的表顺序列表，可嵌套。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LeadingList {
    pub Items: Vec<LeadingItem>,
}

/// 优化器提示时间范围结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HintTimeRange {
    pub From: String,
    pub To: String,
}

/// 优化器提示SETVar结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HintSetVar {
    pub VarName: String,
    pub Value: String,
}

/// 优化器提示载荷：数值、布尔、名称、LEADING 列表等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HintData {
    None,
    Unsigned(u64),
    Signed(i64),
    Boolean(bool),
    Name(String),
    CIStr(CIStr),
    TimeRange(HintTimeRange),
    SetVar(HintSetVar),
    Leading(LeadingList),
}

impl Default for HintData {
    fn default() -> Self {
        Self::None
    }
}

/// 表级优化器提示（optimizer hint）节点。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableOptimizerHint {
    pub HintName: CIStr,
    pub QBName: CIStr,
    pub HintData: HintData,
    pub Tables: Vec<HintTable>,
    pub Indexes: Vec<CIStr>,
}

/// 深度优先展平 LEADING 嵌套列表为表序列。
// 深度优先展开嵌套 LEADING 列表，保持与 Go FlattenLeadingList 相同顺序。
pub fn FlattenLeadingList(list: &LeadingList) -> Vec<HintTable> {
    fn visit(items: &[LeadingItem], output: &mut Vec<HintTable>) {
        for item in items {
            match item {
                LeadingItem::Table(table) => output.push(table.clone()),
                LeadingItem::List(list) => visit(&list.Items, output),
            }
        }
    }

    let mut output = Vec::new();
    visit(&list.Items, &mut output);
    output
}

/// 脱敏策略限制操作位掩码类型。
pub type MaskingPolicyRestrictOps = u8;
/// 脱敏策略限制操作无常量。
pub const MaskingPolicyRestrictOpNone: MaskingPolicyRestrictOps = 0;
/// 脱敏策略限制操作INSERTINTOSELECT常量。
pub const MaskingPolicyRestrictOpInsertIntoSelect: MaskingPolicyRestrictOps = 1;
/// 脱敏策略限制操作UPDATESELECT常量。
pub const MaskingPolicyRestrictOpUpdateSelect: MaskingPolicyRestrictOps = 2;
/// 脱敏策略限制操作DELETESELECT常量。
pub const MaskingPolicyRestrictOpDeleteSelect: MaskingPolicyRestrictOps = 4;
/// 脱敏策略限制操作CTAS常量。
pub const MaskingPolicyRestrictOpCTAS: MaskingPolicyRestrictOps = 8;
/// 脱敏策略限制名称INSERTINTOSELECT常量。
pub const MaskingPolicyRestrictNameInsertIntoSelect: &str = "INSERT_INTO_SELECT";
/// 脱敏策略限制名称UPDATESELECT常量。
pub const MaskingPolicyRestrictNameUpdateSelect: &str = "UPDATE_SELECT";
/// 脱敏策略限制名称DELETESELECT常量。
pub const MaskingPolicyRestrictNameDeleteSelect: &str = "DELETE_SELECT";
/// 脱敏策略限制名称CTAS常量。
pub const MaskingPolicyRestrictNameCTAS: &str = "CTAS";

/// 限定表名（库、表、schema）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableName {
    pub Schema: CIStr,
    pub Name: CIStr,
    pub PartitionNames: Vec<CIStr>,
    pub IndexHints: Vec<IndexHint>,
}

/// 索引提示类型（USE/IGNORE/FORCE/ORDER 等）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum IndexHintType {
    #[default]
    Use = 1,
    Ignore = 2,
    Force = 3,
    OrderIndex = 4,
    NoOrderIndex = 5,
}

/// 优化器提示USE常量。
pub const HintUse: IndexHintType = IndexHintType::Use;
/// 优化器提示Ignore常量。
pub const HintIgnore: IndexHintType = IndexHintType::Ignore;
/// 优化器提示Force常量。
pub const HintForce: IndexHintType = IndexHintType::Force;
/// 优化器提示排序索引常量。
pub const HintOrderIndex: IndexHintType = IndexHintType::OrderIndex;
/// 优化器提示No排序索引常量。
pub const HintNoOrderIndex: IndexHintType = IndexHintType::NoOrderIndex;

/// 索引提示作用域（SCAN/JOIN/ORDER BY/GROUP BY）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum IndexHintScope {
    #[default]
    Scan = 1,
    Join = 2,
    OrderBy = 3,
    GroupBy = 4,
}

/// 优化器提示FORScan常量。
pub const HintForScan: IndexHintScope = IndexHintScope::Scan;
/// 优化器提示FOR连接常量。
pub const HintForJoin: IndexHintScope = IndexHintScope::Join;
/// 优化器提示FOR排序BY常量。
pub const HintForOrderBy: IndexHintScope = IndexHintScope::OrderBy;
/// 优化器提示FOR组BY常量。
pub const HintForGroupBy: IndexHintScope = IndexHintScope::GroupBy;

/// USE/IGNORE/FORCE INDEX 等索引提示。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexHint {
    pub IndexNames: Vec<CIStr>,
    pub HintType: IndexHintType,
    pub HintScope: IndexHintScope,
}

/// FROM 子句中的表来源（表、子查询、连接等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableSource {
    pub Source: TableName,
    pub QuerySource: Option<NodeRef>,
    pub AsName: CIStr,
    pub TableSample: Option<TableSample>,
    pub AsOf: Option<AsOfClause>,
    pub Lateral: bool,
    pub ColumnNames: Vec<CIStr>,
}

/// 采样方法类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SampleMethodType {
    #[default]
    None,
    System,
    Bernoulli,
    TiDBRegion,
}

/// 采样子句单位类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SampleClauseUnitType {
    #[default]
    Default,
    Row,
    Percent,
}

/// 表采样结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableSample {
    pub SampleMethod: SampleMethodType,
    pub Expr: Option<ExprNode>,
    pub SampleClauseUnit: SampleClauseUnitType,
    pub RepeatableSeed: Option<ExprNode>,
}

/// 连接类型（内连接、外连接、半连接等）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JoinType {
    #[default]
    CrossJoin,
    LeftJoin,
    RightJoin,
}

/// 可产生结果集的节点种类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultSetNode {
    TableSource(TableSource),
    Join(Box<Join>),
}

/// 连接节点，描述左右子树、连接类型与 ON 条件。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Join {
    pub Left: Option<Box<ResultSetNode>>,
    pub Right: Option<Box<ResultSetNode>>,
    pub Tp: JoinType,
    pub On: Option<ExprNode>,
    pub Using: Vec<ColumnName>,
    pub NaturalJoin: bool,
    pub StraightJoin: bool,
    pub ExplicitParens: bool,
}

/// 表引用子句包装。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableRefsClause {
    pub TableRefs: Join,
}

/// 列名（库/表/列三段式）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnName {
    pub Schema: CIStr,
    pub Table: CIStr,
    pub Name: CIStr,
}

/// CASE 中的 WHEN ... THEN ... 子句。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WhenClause {
    pub Expr: ExprNode,
    pub Result: ExprNode,
}

/// GET_FORMAT 选择器（DATE/TIME/DATETIME/TIMESTAMP）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GetFormatSelectorType {
    #[default]
    Date,
    Datetime,
    Time,
}

/// TRIM 方向（BOTH/LEADING/TRAILING）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrimDirectionType {
    #[default]
    Both,
    Leading,
    Trailing,
}

/// CAST 函数形态（Cast/Convert/Binary）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CastFunctionType {
    Cast,
    Convert,
    Binary,
}

/// Typed payload of a SQL literal. Floating-point values retain their IEEE
/// bits so AST equality is exact and does not need lossy string comparison.
/// 常量值的内部数据形态枚举。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueDatum {
    Null,
    Bool(bool),
    Int64(i64),
    Uint64(u64),
    Float32(u32),
    Float64(u64),
    Decimal(String),
    String(String),
    Bytes(Vec<u8>),
    BitLiteral(Vec<u8>),
    HexLiteral(Vec<u8>),
}

/// Rust equivalent of Go's driver.ValueExpr: the datum, inferred field type,
/// and projection offset travel together through every planner consumer.
/// 常量值表达式及其 Datum 载荷。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueExpr {
    pub Datum: ValueDatum,
    pub Type: parser_types::types::FieldType,
    pub ProjectionOffset: i32,
}

impl Default for ValueExpr {
    fn default() -> Self {
        match ExprNode::NullValue().Kind {
            ExprKind::Value(value) => value,
            _ => unreachable!(),
        }
    }
}

impl ValueExpr {
    pub fn text(&self) -> String {
        match &self.Datum {
            ValueDatum::Null => "NULL".to_owned(),
            ValueDatum::Bool(value) => if *value { "TRUE" } else { "FALSE" }.to_owned(),
            ValueDatum::Int64(value) => value.to_string(),
            ValueDatum::Uint64(value) => value.to_string(),
            ValueDatum::Float32(bits) => f32::from_bits(*bits).to_string(),
            ValueDatum::Float64(bits) => f64::from_bits(*bits).to_string(),
            ValueDatum::Decimal(value) | ValueDatum::String(value) => value.clone(),
            ValueDatum::Bytes(value) => String::from_utf8_lossy(value).into_owned(),
            ValueDatum::BitLiteral(value) => {
                format!(
                    "0b{}",
                    value
                        .iter()
                        .map(|byte| format!("{byte:08b}"))
                        .collect::<String>()
                )
            }
            ValueDatum::HexLiteral(value) => {
                format!(
                    "0x{}",
                    value
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                )
            }
        }
    }

    pub fn as_str(&self) -> &str {
        match &self.Datum {
            ValueDatum::Decimal(value) | ValueDatum::String(value) => value,
            _ => panic!("as_str is only valid for string and decimal literals"),
        }
    }

    pub fn eq_ignore_ascii_case(&self, other: &str) -> bool {
        self.text().eq_ignore_ascii_case(other)
    }
}

impl std::fmt::Display for ValueExpr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.text())
    }
}

impl PartialEq<str> for ValueExpr {
    fn eq(&self, other: &str) -> bool {
        self.text() == other
    }
}

impl PartialEq<&str> for ValueExpr {
    fn eq(&self, other: &&str) -> bool {
        self.text() == *other
    }
}

impl PartialEq<String> for ValueExpr {
    fn eq(&self, other: &String) -> bool {
        self.text() == *other
    }
}

/// 表达式节点种类标签，覆盖解析器支持的各表达式形态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExprKind {
    Value(ValueExpr),
    IntroducedValue {
        Value: String,
        Charset: String,
        Collation: String,
        Binary: bool,
    },
    Column(ColumnName),
    Variable {
        Name: String,
        IsGlobal: bool,
        IsInstance: bool,
        IsSystem: bool,
        ExplicitScope: bool,
        Value: Option<Box<ExprNode>>,
    },
    Function {
        Schema: CIStr,
        FnName: CIStr,
        Args: Vec<ExprNode>,
    },
    AggregateFunction {
        Name: String,
        Args: Vec<ExprNode>,
        Distinct: bool,
        Order: Vec<ByItem>,
    },
    Binary {
        Op: String,
        L: Box<ExprNode>,
        R: Box<ExprNode>,
    },
    Unary {
        Op: String,
        V: Box<ExprNode>,
    },
    IsTruth {
        Expr: Box<ExprNode>,
        Not: bool,
        True: bool,
    },
    IsNull {
        Expr: Box<ExprNode>,
        Not: bool,
    },
    InList {
        Expr: Box<ExprNode>,
        List: Vec<ExprNode>,
        Not: bool,
        Type: parser_types::types::FieldType,
    },
    Between {
        Expr: Box<ExprNode>,
        Left: Box<ExprNode>,
        Right: Box<ExprNode>,
        Not: bool,
    },
    Like {
        Expr: Box<ExprNode>,
        Pattern: Box<ExprNode>,
        Not: bool,
        Escape: String,
        Explicit: bool,
        IsLike: bool,
        Type: parser_types::types::FieldType,
    },
    Regexp {
        Expr: Box<ExprNode>,
        Pattern: Box<ExprNode>,
        Not: bool,
        Type: parser_types::types::FieldType,
    },
    Row(Vec<ExprNode>),
    Collate {
        Expr: Box<ExprNode>,
        Collation: String,
    },
    NamedDefault(ColumnName),
    MaxValue,
    MatchAgainst {
        ColumnNames: Vec<ColumnName>,
        Against: Box<ExprNode>,
        Modifier: u8,
    },
    Case {
        Value: Option<Box<ExprNode>>,
        WhenClauses: Vec<WhenClause>,
        ElseClause: Option<Box<ExprNode>>,
    },
    WindowFunction {
        Name: String,
        Args: Vec<ExprNode>,
        Distinct: bool,
        IgnoreNull: bool,
        FromLast: bool,
        Spec: Box<WindowSpec>,
    },
    TimeUnit(TimeUnitType),
    GetFormatSelector(GetFormatSelectorType),
    TrimDirection(TrimDirectionType),
    TableName(TableName),
    Parentheses(Box<ExprNode>),
    ParamMarker {
        Offset: usize,
    },
    DefaultValue,
    Subquery {
        Query: NodeRef,
        MultiRows: bool,
        Exists: bool,
    },
    CompareSubquery {
        Op: String,
        L: Box<ExprNode>,
        R: Box<ExprNode>,
        All: bool,
    },
    InSubquery {
        Expr: Box<ExprNode>,
        Sel: Box<ExprNode>,
        Not: bool,
    },
    ExistsSubquery {
        Sel: Box<ExprNode>,
        Not: bool,
    },
    Cast {
        Expr: Box<ExprNode>,
        Tp: parser_types::types::FieldType,
        FunctionType: CastFunctionType,
        ExplicitCharSet: bool,
    },
    JSONSumCrc32 {
        Expr: Box<ExprNode>,
        Tp: parser_types::types::FieldType,
        ExplicitCharSet: bool,
    },
}

/// 对 Node 的引用包装，便于在图结构中共享节点。
#[derive(Clone)]
pub struct NodeRef(std::rc::Rc<std::cell::RefCell<Option<Box<dyn Node>>>>);
impl NodeRef {
    pub fn new(node: Box<dyn Node>) -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(Some(node))))
    }

    pub fn take(&self) -> Option<Box<dyn Node>> {
        self.0.borrow_mut().take()
    }

    pub fn with_node<R>(&self, f: impl FnOnce(&dyn Node) -> R) -> Option<R> {
        self.0.borrow().as_deref().map(f)
    }

    pub fn with_node_mut<R>(&self, f: impl FnOnce(&mut dyn Node) -> R) -> Option<R> {
        self.0.borrow_mut().as_deref_mut().map(f)
    }
}
impl std::fmt::Debug for NodeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NodeRef")
    }
}
impl PartialEq for NodeRef {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for NodeRef {}

/// 表达式节点，携带表达式种类与可选求值结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExprNode {
    pub node_text: base::AstNode,
    pub Kind: ExprKind,
    pub OriginTextPosition: i32,
    /// Expression flags populated by SetFlag; Cell follows the shared visitor contract.
    pub Flag: std::cell::Cell<u64>,
}

/// Typed expression visitor matching Go's `ast.Visitor` contract for ExprNode.
///
/// The general [`Visitor`] above is intentionally read-only and serves statement
/// walking. Planner rewriting needs the Go enter/leave replacement semantics,
/// so expression nodes expose this separate, strongly typed contract.
/// 表达式节点访问者特质接口。
pub trait ExprNodeVisitor {
    fn Enter(&mut self, input: &ExprNode) -> (ExprNode, bool);
    fn Leave(&mut self, input: &ExprNode) -> (ExprNode, bool);
}

impl Default for ExprNode {
    fn default() -> Self {
        Self::Value(String::new())
    }
}

impl ExprNode {
    pub fn GetFlag(&self) -> u64 {
        self.Flag.get()
    }
    pub fn SetFlag(&self, flag: u64) {
        self.Flag.set(flag);
    }
    pub fn PredicateType() -> parser_types::types::FieldType {
        let mut field_type = parser_types::types::NewFieldType(parser_mysql::r#type::TypeTiny);
        field_type.SetFlen(1);
        field_type.SetDecimal(0);
        field_type
    }
    fn typed_value(datum: ValueDatum, charset_name: &str, collation: &str) -> Self {
        let mut field_type = parser_types::types::FieldType::default();
        let binary = |field_type: &mut parser_types::types::FieldType| {
            field_type.SetCharset(parser_charset::charset::CharsetBin.to_owned());
            field_type.SetCollate(parser_charset::charset::CollationBin.to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
        };
        match &datum {
            ValueDatum::Null => {
                field_type.SetType(parser_mysql::r#type::TypeNull);
                field_type.SetFlen(0);
                field_type.SetDecimal(0);
                binary(&mut field_type);
            }
            ValueDatum::Bool(_) => {
                field_type.SetType(parser_mysql::r#type::TypeLonglong);
                field_type.SetFlen(1);
                field_type.SetDecimal(0);
                field_type.AddFlag(parser_mysql::r#type::IsBooleanFlag);
                binary(&mut field_type);
            }
            ValueDatum::Int64(value) => {
                field_type.SetType(parser_mysql::r#type::TypeLonglong);
                field_type.SetFlen(value.to_string().len() as isize);
                field_type.SetDecimal(0);
                binary(&mut field_type);
            }
            ValueDatum::Uint64(value) => {
                field_type.SetType(parser_mysql::r#type::TypeLonglong);
                field_type.AddFlag(parser_mysql::r#type::UnsignedFlag);
                field_type.SetFlen(value.to_string().len() as isize);
                field_type.SetDecimal(0);
                binary(&mut field_type);
            }
            ValueDatum::Float32(bits) => {
                field_type.SetType(parser_mysql::r#type::TypeFloat);
                field_type.SetFlen(f32::from_bits(*bits).to_string().len() as isize);
                field_type.SetDecimal(parser_types::types::UnspecifiedLength);
                binary(&mut field_type);
            }
            ValueDatum::Float64(bits) => {
                field_type.SetType(parser_mysql::r#type::TypeDouble);
                field_type.SetFlen(f64::from_bits(*bits).to_string().len() as isize);
                field_type.SetDecimal(parser_types::types::UnspecifiedLength);
                binary(&mut field_type);
            }
            ValueDatum::Decimal(value) => {
                field_type.SetType(parser_mysql::r#type::TypeNewDecimal);
                field_type.SetFlen(value.len() as isize);
                field_type.SetDecimal(
                    value
                        .split_once('.')
                        .map_or(0, |(_, fraction)| fraction.len() as isize),
                );
                binary(&mut field_type);
            }
            ValueDatum::String(value) => {
                field_type.SetType(parser_mysql::r#type::TypeVarString);
                field_type.SetFlen(value.chars().count() as isize);
                field_type.SetDecimal(parser_types::types::UnspecifiedLength);
                field_type.SetCharset(charset_name.to_owned());
                field_type.SetCollate(collation.to_owned());
            }
            ValueDatum::Bytes(value) => {
                field_type.SetType(parser_mysql::r#type::TypeBlob);
                field_type.SetFlen(value.len() as isize);
                field_type.SetDecimal(parser_types::types::UnspecifiedLength);
                binary(&mut field_type);
            }
            ValueDatum::BitLiteral(value) => {
                field_type.SetType(parser_mysql::r#type::TypeVarString);
                field_type.SetFlen((value.len() * 8) as isize);
                field_type.SetDecimal(0);
                binary(&mut field_type);
            }
            ValueDatum::HexLiteral(value) => {
                field_type.SetType(parser_mysql::r#type::TypeVarString);
                field_type.SetFlen((value.len() * 3) as isize);
                field_type.SetDecimal(0);
                field_type.AddFlag(parser_mysql::r#type::UnsignedFlag);
                binary(&mut field_type);
            }
        }
        Self {
            node_text: Default::default(),
            Kind: ExprKind::Value(ValueExpr {
                Datum: datum,
                Type: field_type,
                ProjectionOffset: -1,
            }),
            OriginTextPosition: 0,
            Flag: Default::default(),
        }
    }

    pub fn Value(value: String) -> Self {
        Self::StringValue(
            value,
            parser_mysql::charset::DefaultCharset,
            parser_mysql::charset::DefaultCollationName,
        )
    }
    pub fn NullValue() -> Self {
        Self::typed_value(ValueDatum::Null, "", "")
    }
    pub fn BoolValue(value: bool) -> Self {
        Self::typed_value(ValueDatum::Bool(value), "", "")
    }
    pub fn IntValue(value: i64) -> Self {
        Self::typed_value(ValueDatum::Int64(value), "", "")
    }
    pub fn UintValue(value: u64) -> Self {
        Self::typed_value(ValueDatum::Uint64(value), "", "")
    }
    pub fn FloatValue(value: f64) -> Self {
        Self::typed_value(ValueDatum::Float64(value.to_bits()), "", "")
    }
    pub fn Float32Value(value: f32) -> Self {
        Self::typed_value(ValueDatum::Float32(value.to_bits()), "", "")
    }
    pub fn DecimalValue(value: String) -> Self {
        Self::typed_value(ValueDatum::Decimal(value), "", "")
    }
    pub fn StringValue(value: String, charset_name: &str, collation: &str) -> Self {
        Self::typed_value(ValueDatum::String(value), charset_name, collation)
    }
    pub fn BitValue(value: Vec<u8>, charset_name: &str, collation: &str) -> Self {
        Self::typed_value(ValueDatum::BitLiteral(value), charset_name, collation)
    }
    pub fn HexValue(value: Vec<u8>, charset_name: &str, collation: &str) -> Self {
        Self::typed_value(ValueDatum::HexLiteral(value), charset_name, collation)
    }
    pub fn Column(value: ColumnName) -> Self {
        Self {
            node_text: Default::default(),
            Kind: ExprKind::Column(value),
            OriginTextPosition: 0,
            Flag: Default::default(),
        }
    }
    pub fn Function(Schema: CIStr, FnName: CIStr, Args: Vec<ExprNode>) -> Self {
        Self {
            node_text: Default::default(),
            Kind: ExprKind::Function {
                Schema,
                FnName,
                Args,
            },
            OriginTextPosition: 0,
            Flag: Default::default(),
        }
    }
    pub fn Binary(Op: String, L: Box<ExprNode>, R: Box<ExprNode>) -> Self {
        Self {
            node_text: Default::default(),
            Kind: ExprKind::Binary { Op, L, R },
            OriginTextPosition: 0,
            Flag: Default::default(),
        }
    }
    pub fn Unary(Op: String, V: Box<ExprNode>) -> Self {
        Self {
            node_text: Default::default(),
            Kind: ExprKind::Unary { Op, V },
            OriginTextPosition: 0,
            Flag: Default::default(),
        }
    }
    pub fn Parentheses(V: Box<ExprNode>) -> Self {
        Self {
            node_text: Default::default(),
            Kind: ExprKind::Parentheses(V),
            OriginTextPosition: 0,
            Flag: Default::default(),
        }
    }
    pub fn ParamMarker(Offset: usize) -> Self {
        Self {
            node_text: Default::default(),
            Kind: ExprKind::ParamMarker { Offset },
            OriginTextPosition: Offset as i32,
            Flag: Default::default(),
        }
    }
    pub fn SetOriginTextPosition(&mut self, offset: i32) {
        self.OriginTextPosition = offset;
    }

    /// Reports whether this node is either form of Go `DefaultExpr`.
    pub fn IsDefaultExpr(&self) -> bool {
        matches!(
            self.Kind,
            ExprKind::DefaultValue | ExprKind::NamedDefault(_)
        )
    }

    /// Returns the optional column name carried by Go `DefaultExpr.Name`.
    pub fn DefaultName(&self) -> Option<&ColumnName> {
        match &self.Kind {
            ExprKind::NamedDefault(name) => Some(name),
            ExprKind::DefaultValue => None,
            _ => None,
        }
    }

    /// Walks and optionally replaces this expression using Go's Enter/Leave order.
    pub fn Accept<V: ExprNodeVisitor + ?Sized>(&self, visitor: &mut V) -> (ExprNode, bool) {
        let (mut node, skip_children) = visitor.Enter(self);
        if !skip_children && !node.accept_children(visitor) {
            return (node, false);
        }
        visitor.Leave(&node)
    }

    fn accept_children<V: ExprNodeVisitor + ?Sized>(&mut self, visitor: &mut V) -> bool {
        fn accept_one<V: ExprNodeVisitor + ?Sized>(node: &mut ExprNode, visitor: &mut V) -> bool {
            let (replacement, ok) = node.Accept(visitor);
            *node = replacement;
            ok
        }
        fn accept_box<V: ExprNodeVisitor + ?Sized>(
            node: &mut Box<ExprNode>,
            visitor: &mut V,
        ) -> bool {
            accept_one(node.as_mut(), visitor)
        }
        fn accept_many<V: ExprNodeVisitor + ?Sized>(
            nodes: &mut [ExprNode],
            visitor: &mut V,
        ) -> bool {
            nodes.iter_mut().all(|node| accept_one(node, visitor))
        }
        fn accept_by_items<V: ExprNodeVisitor + ?Sized>(
            items: &mut [ByItem],
            visitor: &mut V,
        ) -> bool {
            items
                .iter_mut()
                .all(|item| accept_one(&mut item.Expr, visitor))
        }

        match &mut self.Kind {
            ExprKind::Variable { Value, .. } => Value
                .as_mut()
                .is_none_or(|value| accept_box(value, visitor)),
            ExprKind::Function { Args, .. } => accept_many(Args, visitor),
            ExprKind::AggregateFunction { Args, Order, .. } => {
                accept_many(Args, visitor) && accept_by_items(Order, visitor)
            }
            ExprKind::Binary { L, R, .. } => accept_box(L, visitor) && accept_box(R, visitor),
            ExprKind::Unary { V, .. }
            | ExprKind::Parentheses(V)
            | ExprKind::Collate { Expr: V, .. }
            | ExprKind::Cast { Expr: V, .. }
            | ExprKind::JSONSumCrc32 { Expr: V, .. } => accept_box(V, visitor),
            ExprKind::IsTruth { Expr, .. } | ExprKind::IsNull { Expr, .. } => {
                accept_box(Expr, visitor)
            }
            ExprKind::InList { Expr, List, .. } => {
                accept_box(Expr, visitor) && accept_many(List, visitor)
            }
            ExprKind::Between {
                Expr, Left, Right, ..
            } => {
                accept_box(Expr, visitor) && accept_box(Left, visitor) && accept_box(Right, visitor)
            }
            ExprKind::Like { Expr, Pattern, .. } | ExprKind::Regexp { Expr, Pattern, .. } => {
                accept_box(Expr, visitor) && accept_box(Pattern, visitor)
            }
            ExprKind::Row(values) => accept_many(values, visitor),
            ExprKind::MatchAgainst { Against, .. } => accept_box(Against, visitor),
            ExprKind::Case {
                Value,
                WhenClauses,
                ElseClause,
            } => {
                Value
                    .as_mut()
                    .is_none_or(|value| accept_box(value, visitor))
                    && WhenClauses.iter_mut().all(|clause| {
                        accept_one(&mut clause.Expr, visitor)
                            && accept_one(&mut clause.Result, visitor)
                    })
                    && ElseClause
                        .as_mut()
                        .is_none_or(|value| accept_box(value, visitor))
            }
            ExprKind::WindowFunction { Args, Spec, .. } => {
                accept_many(Args, visitor)
                    && accept_by_items(&mut Spec.PartitionBy, visitor)
                    && accept_by_items(&mut Spec.OrderBy, visitor)
                    && Spec.Frame.as_mut().is_none_or(|frame| {
                        frame
                            .Extent
                            .Start
                            .Expr
                            .as_mut()
                            .is_none_or(|value| accept_one(value, visitor))
                            && frame
                                .Extent
                                .End
                                .Expr
                                .as_mut()
                                .is_none_or(|value| accept_one(value, visitor))
                    })
            }
            ExprKind::CompareSubquery { L, R, .. } => {
                accept_box(L, visitor) && accept_box(R, visitor)
            }
            ExprKind::InSubquery { Expr, Sel, .. } => {
                accept_box(Expr, visitor) && accept_box(Sel, visitor)
            }
            ExprKind::ExistsSubquery { Sel, .. } => accept_box(Sel, visitor),
            ExprKind::Value(_)
            | ExprKind::IntroducedValue { .. }
            | ExprKind::Column(_)
            | ExprKind::NamedDefault(_)
            | ExprKind::MaxValue
            | ExprKind::TimeUnit(_)
            | ExprKind::GetFormatSelector(_)
            | ExprKind::TrimDirection(_)
            | ExprKind::TableName(_)
            | ExprKind::ParamMarker { .. }
            | ExprKind::DefaultValue
            | ExprKind::Subquery { .. } => true,
        }
    }
}

/// Values accepted by [`NewValueExpr`], corresponding to the concrete values
/// installed by TiDB's parser driver.
/// 将常见 Rust 值转为 ValueDatum 的转换特质。
pub trait IntoValueDatum {
    fn into_value_datum(self) -> ValueDatum;
}

impl IntoValueDatum for ValueDatum {
    fn into_value_datum(self) -> ValueDatum {
        self
    }
}

macro_rules! signed_value_datum {
    ($($type:ty),+ $(,)?) => {$ (
        impl IntoValueDatum for $type {
            fn into_value_datum(self) -> ValueDatum { ValueDatum::Int64(self as i64) }
        }
    )+};
}
signed_value_datum!(i8, i16, i32, i64, isize);

macro_rules! unsigned_value_datum {
    ($($type:ty),+ $(,)?) => {$ (
        impl IntoValueDatum for $type {
            fn into_value_datum(self) -> ValueDatum { ValueDatum::Uint64(self as u64) }
        }
    )+};
}
unsigned_value_datum!(u8, u16, u32, u64, usize);

impl IntoValueDatum for bool {
    fn into_value_datum(self) -> ValueDatum {
        ValueDatum::Bool(self)
    }
}
impl IntoValueDatum for f32 {
    fn into_value_datum(self) -> ValueDatum {
        ValueDatum::Float32(self.to_bits())
    }
}
impl IntoValueDatum for f64 {
    fn into_value_datum(self) -> ValueDatum {
        ValueDatum::Float64(self.to_bits())
    }
}
impl IntoValueDatum for String {
    fn into_value_datum(self) -> ValueDatum {
        ValueDatum::String(self)
    }
}
impl IntoValueDatum for &str {
    fn into_value_datum(self) -> ValueDatum {
        ValueDatum::String(self.to_owned())
    }
}
impl<T: IntoValueDatum> IntoValueDatum for Option<T> {
    fn into_value_datum(self) -> ValueDatum {
        self.map_or(ValueDatum::Null, IntoValueDatum::into_value_datum)
    }
}

/// Creates a typed parser value expression with Go-compatible field metadata.
/// 构造带字符集/排序规则信息的 ValueExpr。
pub fn NewValueExpr(value: impl IntoValueDatum, charset_name: &str, collation: &str) -> ExprNode {
    ExprNode::typed_value(value.into_value_datum(), charset_name, collation)
}

/// 通配符投影（如 t.*）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WildCardField {
    pub Schema: CIStr,
    pub Table: CIStr,
}

/// SELECT 列表中的单个投影项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectField {
    pub Offset: usize,
    /// Parser-captured source text for the projection expression.
    pub OriginalText: String,
    pub WildCard: Option<WildCardField>,
    pub Expr: Option<ExprNode>,
    pub AsName: CIStr,
    pub Auxiliary: bool,
    pub AuxiliaryColInAgg: bool,
    pub AuxiliaryColInOrderBy: bool,
}

/// SELECT 投影列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldList {
    pub Fields: Vec<SelectField>,
}

/// ORDER BY / GROUP BY 项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ByItem {
    pub Expr: ExprNode,
    pub Desc: bool,
}

/// ALTER排序项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterOrderItem {
    pub Column: ColumnName,
    pub Desc: bool,
}

/// LIMIT/OFFSET 子句。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Limit {
    pub Count: Option<ExprNode>,
    pub Offset: Option<ExprNode>,
}

/// LIMIT简单结构体。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LimitSimple {
    pub Offset: u64,
    pub Count: u64,
}
/// BDR角色枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BDRRole {
    #[default]
    Primary,
    Secondary,
}
/// 剖析类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProfileType {
    #[default]
    Cpu,
    Memory,
    BlockIo,
    ContextSwitch,
    PageFaults,
    Ipc,
    Swaps,
    Source,
    All,
}

/// 赋值结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Assignment {
    pub Column: ColumnName,
    pub Expr: ExprNode,
}

/// INSERT 语句 AST。
#[derive(Default)]
pub struct InsertStmt {
    pub node_text: base::AstNode,
    pub IsReplace: bool,
    pub Priority: i32,
    pub IgnoreErr: bool,
    pub Table: Option<TableRefsClause>,
    pub Columns: Vec<ColumnName>,
    pub Lists: Vec<Vec<ExprNode>>,
    pub Setlist: bool,
    pub OnDuplicate: Vec<Assignment>,
    pub Select: Option<Box<dyn Node>>,
    pub TableHints: Vec<TableOptimizerHint>,
    pub PartitionNames: Vec<CIStr>,
    pub Returning: Vec<SelectField>,
    pub RowAlias: CIStr,
    pub ColumnAliases: Vec<CIStr>,
}

/// UPDATE 语句 AST。
#[derive(Default)]
pub struct UpdateStmt {
    pub node_text: base::AstNode,
    pub Priority: i32,
    pub TableRefs: Option<TableRefsClause>,
    pub List: Vec<Assignment>,
    pub Where: Option<ExprNode>,
    pub Order: Vec<ByItem>,
    pub Limit: Option<Limit>,
    pub IgnoreErr: bool,
    pub MultipleTable: bool,
    pub TableHints: Vec<TableOptimizerHint>,
    pub Returning: Vec<SelectField>,
    pub With: Option<WithClauseRef>,
}

/// DELETE 语句 AST。
#[derive(Default)]
pub struct DeleteStmt {
    pub node_text: base::AstNode,
    pub Priority: i32,
    pub TableRefs: Option<TableRefsClause>,
    pub Tables: Vec<TableName>,
    pub Where: Option<ExprNode>,
    pub Order: Vec<ByItem>,
    pub Limit: Option<Limit>,
    pub IgnoreErr: bool,
    pub Quick: bool,
    pub IsMultiTable: bool,
    pub BeforeFrom: bool,
    pub TableHints: Vec<TableOptimizerHint>,
    pub Returning: Vec<SelectField>,
    pub With: Option<WithClauseRef>,
}

/// 列选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColumnOptionType {
    #[default]
    None,
    PrimaryKey,
    NotNull,
    AutoIncrement,
    DefaultValue,
    UniqueKey,
    Null,
    OnUpdate,
    Fulltext,
    Comment,
    Generated,
    Reference,
    Collate,
    Check,
    ColumnFormat,
    Storage,
    AutoRandom,
    SecondaryEngineAttribute,
    MariaDBRowStart,
    MariaDBRowEnd,
}

/// 自动随机选项结构体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoRandomOption {
    pub ShardBits: isize,
    pub RangeBits: isize,
}
impl Default for AutoRandomOption {
    fn default() -> Self {
        Self {
            ShardBits: parser_types::types::UnspecifiedLength,
            RangeBits: parser_types::types::UnspecifiedLength,
        }
    }
}

/// 列选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnOption {
    pub Tp: ColumnOptionType,
    pub Expr: Option<ExprNode>,
    pub Stored: bool,
    pub StrValue: String,
    pub PrimaryKeyTp: PrimaryKeyType,
    pub Enforced: bool,
    pub ConstraintName: String,
    pub AutoRandOpt: AutoRandomOption,
    pub Refer: Option<ReferenceDef>,
}

/// 列定义结构体。
#[derive(Clone, Debug)]
pub struct ColumnDef {
    pub Name: ColumnName,
    pub Tp: parser_types::types::FieldType,
    pub Options: Vec<ColumnOption>,
}

/// 列名称或用户Var结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnNameOrUserVar {
    pub ColumnName: Option<ColumnName>,
    pub UserVar: Option<ExprNode>,
}

/// 表选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableOptionType {
    #[default]
    Charset,
    Collate,
    AutoIncrement,
    AutoIdCache,
    AutoRandomBase,
    AvgRowLength,
    Connection,
    CheckSum,
    TableCheckSum,
    Password,
    Compression,
    KeyBlockSize,
    DelayKeyWrite,
    RowFormat,
    StatsPersistent,
    StatsAutoRecalc,
    StatsSamplePages,
    StatsBuckets,
    StatsTopN,
    StatsSampleRate,
    StatsColsChoice,
    StatsColList,
    ShardRowID,
    PreSplitRegion,
    PackKeys,
    StorageMedia,
    SecondaryEngineNull,
    SecondaryEngine,
    Union,
    Encryption,
    TTL,
    TTLEnable,
    TTLJobInterval,
    AutoextendSize,
    Affinity,
    PageChecksum,
    PageCompressed,
    PageCompressionLevel,
    Transactional,
    Sequence,
    IetfQuotes,
    Comment,
    Engine,
    EngineAttribute,
    StorageClass,
    SecondaryEngineAttribute,
    StartTransaction,
    InsertMethod,
    DataDirectory,
    IndexDirectory,
    MaxRows,
    MinRows,
    Nodegroup,
    Tablespace,
    PrimaryRegion,
    Regions,
    FollowerCount,
    VoterCount,
    LearnerCount,
    Schedule,
    Constraints,
    LeaderConstraints,
    LearnerConstraints,
    FollowerConstraints,
    VoterConstraints,
    SurvivalPreferences,
    Policy,
}
pub const TableOptionCompressionNone: &str = "NONE";

/// 表选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableOption {
    pub Tp: TableOptionType,
    pub StrValue: String,
    pub UintValue: u64,
    pub BoolValue: bool,
    pub Default: bool,
    pub Value: Option<ExprNode>,
    pub ColumnName: Option<ColumnName>,
    pub TimeUnitValue: Option<TimeUnitType>,
    pub TableNames: Vec<TableName>,
}

/// 数据库选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DatabaseOptionType {
    #[default]
    Charset,
    Collate,
    Encryption,
    PrimaryRegion,
    Regions,
    FollowerCount,
    VoterCount,
    LearnerCount,
    Schedule,
    Constraints,
    LeaderConstraints,
    FollowerConstraints,
    VoterConstraints,
    LearnerConstraints,
    SurvivalPreferences,
    Policy,
    TiFlashReplica,
}
/// 数据库选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DatabaseOption {
    pub Tp: DatabaseOptionType,
    pub Value: String,
    pub UintValue: u64,
    pub TiFlashReplica: Option<TiFlashReplicaSpec>,
}

/// 临时关键字枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TemporaryKeyword {
    #[default]
    None,
    Global,
    Local,
}
/// ON重复键处理类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnDuplicateKeyHandlingType {
    #[default]
    Error,
    Ignore,
    Replace,
}
/// 分区类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PartitionType {
    #[default]
    None,
    Key,
    Hash,
    Range,
    List,
    SystemTime,
}
/// 分区键算法结构体。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartitionKeyAlgorithm {
    pub Type: u64,
}
/// 分区区间表达式结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionIntervalExpr {
    pub Expr: Option<ExprNode>,
    pub TimeUnit: TimeUnitType,
}
/// 分区区间结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionInterval {
    pub IntervalExpr: PartitionIntervalExpr,
    pub FirstRangeEnd: Option<ExprNode>,
    pub LastRangeEnd: Option<ExprNode>,
    pub NullPart: bool,
    pub MaxValPart: bool,
}
/// 分区方法结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionMethod {
    pub Tp: PartitionType,
    pub Linear: bool,
    pub Expr: Option<ExprNode>,
    pub ColumnNames: Vec<ColumnName>,
    pub KeyAlgorithm: PartitionKeyAlgorithm,
    pub Num: u64,
    pub Interval: Option<PartitionInterval>,
    pub Unit: TimeUnitType,
    pub Limit: u64,
}
/// 分区定义子句枚举。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum PartitionDefinitionClause {
    #[default]
    None,
    LessThan(Vec<ExprNode>),
    In(Vec<Vec<ExprNode>>),
    History {
        Current: bool,
    },
}
/// 子分区定义结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubPartitionDefinition {
    pub Name: CIStr,
    pub Options: Vec<TableOption>,
}
/// 分区定义结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub Name: CIStr,
    pub Clause: PartitionDefinitionClause,
    pub Options: Vec<TableOption>,
    pub Sub: Vec<SubPartitionDefinition>,
}
/// 分区选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionOptions {
    pub PartitionMethod: PartitionMethod,
    pub Sub: Option<PartitionMethod>,
    pub Definitions: Vec<PartitionDefinition>,
    pub UpdateIndexes: Vec<Constraint>,
}

/// 行格式类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RowFormatType {
    #[default]
    Default,
    Dynamic,
    Fixed,
    Compressed,
    Redundant,
    Compact,
    TokuDefault,
    TokuFast,
    TokuSmall,
    TokuZlib,
    TokuZstd,
    TokuQuickLz,
    TokuLzma,
    TokuSnappy,
    TokuUncompressed,
}

/// CREATE TABLE 语句 AST。
#[derive(Default)]
pub struct CreateTableStmt {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub TemporaryKeyword: TemporaryKeyword,
    pub OnCommitDelete: bool,
    pub Table: TableName,
    pub ReferTable: Option<TableName>,
    pub Cols: Vec<ColumnDef>,
    pub Constraints: Vec<Constraint>,
    pub Options: Vec<TableOption>,
    pub Partition: Option<PartitionOptions>,
    pub SplitIndex: Vec<SplitIndexOption>,
    pub OnDuplicate: OnDuplicateKeyHandlingType,
    pub Select: Option<Box<dyn Node>>,
}

/// 视图算法枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ViewAlgorithm {
    #[default]
    Undefined,
    Merge,
    Temptable,
}

/// 视图安全性枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ViewSecurity {
    #[default]
    Definer,
    Invoker,
}

/// 视图检查选项枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ViewCheckOption {
    #[default]
    Cascaded,
    Local,
}

/// Materialized-view refresh mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MViewRefreshMethod {
    #[default]
    Fast,
    Unknown(i32),
}

impl std::fmt::Display for MViewRefreshMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Fast => "REFRESH FAST",
            Self::Unknown(_) => "UNKNOWN",
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MViewRefreshClause {
    pub Method: MViewRefreshMethod,
    pub StartWith: Option<ExprNode>,
    pub Next: Option<ExprNode>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MLogPurgeClause {
    pub Immediate: bool,
    pub StartWith: Option<ExprNode>,
    pub Next: Option<ExprNode>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MLogAccumulationAlertClause {
    pub Rows: i64,
}

pub struct CreateMaterializedViewStmt {
    pub node_text: base::AstNode,
    pub ViewName: Option<TableName>,
    pub Cols: Vec<CIStr>,
    pub Comment: String,
    pub Refresh: Option<MViewRefreshClause>,
    pub Attributes: String,
    pub Options: Vec<TableOption>,
    pub Select: Option<Box<dyn Node>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateMaterializedViewLogStmt {
    pub node_text: base::AstNode,
    pub Table: Option<TableName>,
    pub Cols: Vec<CIStr>,
    pub Options: Vec<TableOption>,
    pub Purge: Option<MLogPurgeClause>,
    pub AccumulationAlert: Option<MLogAccumulationAlertClause>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlterMaterializedViewActionType {
    #[default]
    Comment,
    Refresh,
    Attributes,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterMaterializedViewAction {
    pub node_text: base::AstNode,
    pub Tp: AlterMaterializedViewActionType,
    pub Comment: String,
    pub Refresh: Option<MViewRefreshClause>,
    pub Attributes: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterMaterializedViewStmt {
    pub node_text: base::AstNode,
    pub ViewName: Option<TableName>,
    pub Actions: Vec<AlterMaterializedViewAction>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlterMaterializedViewLogActionType {
    #[default]
    Purge,
    AddColumn,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterMaterializedViewLogAction {
    pub node_text: base::AstNode,
    pub Tp: AlterMaterializedViewLogActionType,
    pub Purge: Option<MLogPurgeClause>,
    pub Cols: Vec<CIStr>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterMaterializedViewLogStmt {
    pub node_text: base::AstNode,
    pub Table: Option<TableName>,
    pub Actions: Vec<AlterMaterializedViewLogAction>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropMaterializedViewStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub ViewName: Option<TableName>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropMaterializedViewLogStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub Table: Option<TableName>,
}

/// CREATE VIEW 语句 AST。
pub struct CreateViewStmt {
    pub node_text: base::AstNode,
    pub OrReplace: bool,
    pub ViewName: TableName,
    pub Cols: Vec<CIStr>,
    pub Select: Box<dyn Node>,
    pub SchemaCols: Vec<CIStr>,
    pub Algorithm: ViewAlgorithm,
    pub Definer: parser_auth::parser::auth::auth::UserIdentity,
    pub Security: ViewSecurity,
    pub CheckOption: ViewCheckOption,
}

/// ALTER表类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlterTableType {
    #[default]
    Option,
    AddColumns,
    AddConstraint,
    DropColumn,
    DropPrimaryKey,
    DropIndex,
    DropForeignKey,
    ModifyColumn,
    ChangeColumn,
    RenameColumn,
    RenameTable,
    AlterColumn,
    Lock,
    Writeable,
    Algorithm,
    RenameIndex,
    Force,
    AddPartitions,
    DropPartition,
    TruncatePartition,
    Partition,
    EnableKeys,
    DisableKeys,
    RemovePartitioning,
    WithValidation,
    WithoutValidation,
    SecondaryLoad,
    SecondaryUnload,
    RebuildPartition,
    ReorganizePartition,
    ExchangePartition,
    ImportTablespace,
    DiscardTablespace,
    IndexInvisible,
    OrderByColumns,
    Cache,
    NoCache,
    RemoveTTL,
    SplitIndex,
    ReorganizeLastPartition,
    ReorganizeFirstPartition,
    PartitionAttributes,
    PartitionOptions,
    SetTiFlashReplica,
    AddLastPartition,
    AddStatistics,
    CheckPartitions,
    CoalescePartitions,
    DropFirstPartition,
    DropStatistics,
    OptimizePartition,
    RepairPartition,
    ImportPartitionTablespace,
    DiscardPartitionTablespace,
    AlterCheck,
    DropCheck,
    ModifyMaskingPolicyExpression,
    ModifyMaskingPolicyRestrictOn,
    AddMaskingPolicy,
    EnableMaskingPolicy,
    DisableMaskingPolicy,
    DropMaskingPolicy,
    Attributes,
    StatsOptions,
}

/// 列位置类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColumnPositionType {
    #[default]
    None,
    First,
    After,
}

/// 列位置结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnPosition {
    pub Tp: ColumnPositionType,
    pub RelativeColumn: Option<ColumnName>,
}

/// 锁类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LockType {
    None,
    #[default]
    Default,
    Shared,
    Exclusive,
}

/// 算法类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlgorithmType {
    #[default]
    Default,
    Copy,
    Inplace,
    Instant,
}

/// ALTER表规格结构体。
#[derive(Clone, Debug, Default)]
pub struct AlterTableSpec {
    pub IfExists: bool,
    pub IfNotExists: bool,
    pub Tp: AlterTableType,
    pub Name: String,
    pub IndexName: CIStr,
    pub NewTable: Option<TableName>,
    pub NewColumns: Vec<ColumnDef>,
    pub OldColumnName: Option<ColumnName>,
    pub NewColumnName: Option<ColumnName>,
    pub Position: ColumnPosition,
    pub LockType: LockType,
    pub Algorithm: AlgorithmType,
    pub FromKey: CIStr,
    pub ToKey: CIStr,
    pub Writeable: bool,
    pub PartitionNames: Vec<CIStr>,
    pub NoWriteToBinlog: bool,
    pub SplitIndex: Option<SplitIndexOption>,
    pub PartitionExpr: Option<ExprNode>,
    pub AttributesSpec: Option<AttributesSpec>,
    pub Options: Vec<TableOption>,
    pub TiFlashReplica: Option<TiFlashReplicaSpec>,
    pub Constraint: Option<Constraint>,
    pub Num: u64,
    pub OnAllPartitions: bool,
    pub Statistics: Option<StatisticsSpec>,
    pub OrderByList: Vec<AlterOrderItem>,
    pub Visibility: IndexVisibility,
    pub MaskingPolicyName: CIStr,
    pub MaskingPolicyExpr: Option<ExprNode>,
    pub MaskingPolicyRestrictOps: MaskingPolicyRestrictOps,
    pub StatsOptionsSpec: Option<StatsOptionsSpec>,
    pub WithValidation: bool,
    pub PartDefinitions: Vec<PartitionDefinition>,
    pub Partition: Option<PartitionOptions>,
    pub MaskingPolicyColumn: Option<ColumnName>,
    pub MaskingPolicyState: MaskingPolicyState,
}

/// TiFlash副本规格结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TiFlashReplicaSpec {
    pub Count: u64,
    pub Labels: Vec<String>,
    pub Hypo: bool,
}
/// 约束类型枚举。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ConstraintType {
    #[default]
    None,
    PrimaryKey,
    Fulltext,
    Index,
    Unique,
    ForeignKey,
    Check,
    Vector,
    Columnar,
}
/// 匹配类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MatchType {
    #[default]
    None,
    Full,
    Partial,
    Simple,
}
/// 引用选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReferOptionType {
    #[default]
    None,
    Restrict,
    Cascade,
    SetNull,
    NoAction,
    SetDefault,
}
/// ONDELETE选项结构体。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OnDeleteOpt {
    pub ReferOpt: ReferOptionType,
}
/// ONUPDATE选项结构体。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OnUpdateOpt {
    pub ReferOpt: ReferOptionType,
}
/// Reference定义结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReferenceDef {
    pub Table: TableName,
    pub IndexPartSpecifications: Vec<IndexPartSpecification>,
    pub OnDelete: OnDeleteOpt,
    pub OnUpdate: OnUpdateOpt,
    pub Match: MatchType,
}
/// 约束结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Constraint {
    pub Name: String,
    pub Enforced: bool,
    pub Tp: ConstraintType,
    pub Keys: Vec<IndexPartSpecification>,
    pub IsEmptyIndex: bool,
    pub IfNotExists: bool,
    pub Option: Option<IndexOption>,
    pub Refer: Option<ReferenceDef>,
    pub Expr: Option<ExprNode>,
}
/// 统计信息规格结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatisticsSpec {
    pub StatsName: String,
    pub StatsType: u8,
    pub Columns: Vec<ColumnName>,
}

/// ALTER TABLE 语句 AST。
#[derive(Clone, Debug, Default)]
pub struct AlterTableStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
    pub Specs: Vec<AlterTableSpec>,
}

/// DROP TABLE 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropTableStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub Tables: Vec<TableName>,
    pub IsView: bool,
    pub TemporaryKeyword: TemporaryKeyword,
}

/// CREATE数据库语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateDatabaseStmt {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub Name: String,
    pub Options: Vec<DatabaseOption>,
}

/// DROP数据库语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropDatabaseStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub Name: String,
}

/// ALTER数据库语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterDatabaseStmt {
    pub node_text: base::AstNode,
    pub Name: CIStr,
    pub AlterDefaultDatabase: bool,
    pub Options: Vec<DatabaseOption>,
}

/// ALTER实例语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterInstanceStmt {
    pub node_text: base::AstNode,
    pub ReloadTLS: bool,
    pub NoRollbackOnError: bool,
}
/// ALTER范围语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterRangeStmt {
    pub node_text: base::AstNode,
    pub RangeName: CIStr,
    pub PlacementOption: PlacementOption,
}

/// 字符串或用户Var结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StringOrUserVar {
    pub StringLit: String,
    pub UserVar: Option<ExprNode>,
}
/// 绑定状态类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BindingStatusType {
    #[default]
    Enabled,
    Disabled,
}
/// CREATE绑定语句结构体。
#[derive(Default)]
pub struct CreateBindingStmt {
    pub node_text: base::AstNode,
    pub OriginNode: Option<Box<dyn Node>>,
    pub HintedNode: Option<Box<dyn Node>>,
    pub GlobalScope: bool,
    pub PlanDigests: Vec<StringOrUserVar>,
}
/// DROP绑定语句结构体。
#[derive(Default)]
pub struct DropBindingStmt {
    pub node_text: base::AstNode,
    pub OriginNode: Option<Box<dyn Node>>,
    pub HintedNode: Option<Box<dyn Node>>,
    pub GlobalScope: bool,
    pub SQLDigests: Vec<StringOrUserVar>,
}
/// SET绑定语句结构体。
#[derive(Default)]
pub struct SetBindingStmt {
    pub node_text: base::AstNode,
    pub BindingStatusType: BindingStatusType,
    pub OriginNode: Option<Box<dyn Node>>,
    pub HintedNode: Option<Box<dyn Node>>,
    pub SQLDigest: String,
}

/// 分布表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DistributeTableStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
    pub PartitionNames: Vec<CIStr>,
    pub Rule: String,
    pub Engine: String,
    pub Timeout: String,
}

/// 取消分布任务语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CancelDistributionJobStmt {
    pub node_text: base::AstNode,
    pub JobID: i64,
}

/// 用户To用户结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserToUser {
    pub OldUser: parser_auth::parser::auth::auth::UserIdentity,
    pub NewUser: parser_auth::parser::auth::auth::UserIdentity,
}

/// 重命名用户语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenameUserStmt {
    pub node_text: base::AstNode,
    pub UserToUsers: Vec<UserToUser>,
}

/// DROP用户语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropUserStmt {
    pub node_text: base::AstNode,
    pub IsDropRole: bool,
    pub IfExists: bool,
    pub UserList: Vec<parser_auth::parser::auth::auth::UserIdentity>,
}

/// DROP存储过程语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropProcedureStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub ProcedureName: TableName,
}

/// DROP放置策略策略语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropPlacementPolicyStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub PolicyName: CIStr,
}

/// DROP资源组语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropResourceGroupStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub ResourceGroupName: CIStr,
}
/// CREATE资源组语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateResourceGroupStmt {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub ResourceGroupName: CIStr,
    pub ResourceGroupOptionList: Vec<ResourceGroupOption>,
}
/// ALTER资源组语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterResourceGroupStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub ResourceGroupName: CIStr,
    pub ResourceGroupOptionList: Vec<ResourceGroupOption>,
}
/// CREATE放置策略策略语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreatePlacementPolicyStmt {
    pub node_text: base::AstNode,
    pub OrReplace: bool,
    pub IfNotExists: bool,
    pub PolicyName: CIStr,
    pub PlacementOptions: Vec<PlacementOption>,
}
/// ALTER放置策略策略语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterPlacementPolicyStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub PolicyName: CIStr,
    pub PlacementOptions: Vec<PlacementOption>,
}
/// 脱敏策略状态结构体。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaskingPolicyState {
    pub Enabled: bool,
    pub Explicit: bool,
}

/// DROP查询监视语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropQueryWatchStmt {
    pub node_text: base::AstNode,
    pub IntValue: i64,
    pub GroupNameStr: CIStr,
    pub GroupNameExpr: Option<ExprNode>,
}

/// IMPORTINTO动作类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImportIntoActionTp {
    #[default]
    Cancel,
}

/// IMPORTINTO动作语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportIntoActionStmt {
    pub node_text: base::AstNode,
    pub Tp: ImportIntoActionTp,
    pub JobID: i64,
}

/// 序列选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SequenceOptionType {
    #[default]
    None,
    IncrementBy,
    StartWith,
    NoMinValue,
    MinValue,
    NoMaxValue,
    MaxValue,
    NoCache,
    Cache,
    NoCycle,
    Cycle,
    Restart,
    RestartWith,
}

/// 序列选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SequenceOption {
    pub Tp: SequenceOptionType,
    pub IntValue: i64,
}

/// CREATE序列语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateSequenceStmt {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub Name: TableName,
    pub SeqOptions: Vec<SequenceOption>,
    pub TblOptions: Vec<String>,
}

/// ALTER序列语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterSequenceStmt {
    pub node_text: base::AstNode,
    pub Name: TableName,
    pub IfExists: bool,
    pub SeqOptions: Vec<SequenceOption>,
}

/// DROP序列语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropSequenceStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub Sequences: Vec<TableName>,
}

/// 截断表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TruncateTableStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
}

/// 恢复表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoverTableStmt {
    pub node_text: base::AstNode,
    pub JobID: i64,
    pub Table: Option<TableName>,
    pub JobNum: i64,
}

/// 闪回回退ToTimestamp语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FlashBackToTimestampStmt {
    pub node_text: base::AstNode,
    pub FlashbackTS: Option<ExprNode>,
    pub FlashbackTSO: u64,
    pub Tables: Vec<TableName>,
    pub DBName: CIStr,
}

/// 闪回回退表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FlashBackTableStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
    pub NewName: String,
}

/// 闪回回退数据库语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FlashBackDatabaseStmt {
    pub node_text: base::AstNode,
    pub DBName: CIStr,
    pub NewName: String,
}

/// 索引键类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IndexKeyType {
    #[default]
    None,
    Unique,
    Spatial,
    Fulltext,
    Vector,
    Columnar,
}

/// 索引PartSpecification结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexPartSpecification {
    pub Column: Option<ColumnName>,
    pub Length: isize,
    pub Desc: bool,
    pub Expr: Option<ExprNode>,
}

/// 索引锁And算法结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexLockAndAlgorithm {
    pub LockTp: LockType,
    pub AlgorithmTp: AlgorithmType,
}

/// 空字符串结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NullString {
    pub String: String,
    pub Empty: bool,
}
/// 索引类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IndexType {
    #[default]
    Invalid,
    Btree,
    Hash,
    Rtree,
    Hypo,
    HNSW,
    Inverted,
}
/// 索引可见性枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IndexVisibility {
    #[default]
    Default,
    Visible,
    Invisible,
}
/// 主键键类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrimaryKeyType {
    #[default]
    Default,
    Clustered,
    NonClustered,
}
/// Split选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitOption {
    pub Lower: Vec<ExprNode>,
    pub Upper: Vec<ExprNode>,
    pub Num: i64,
    pub ValueLists: Vec<Vec<ExprNode>>,
}
/// SplitSyntax选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitSyntaxOption {
    pub HasRegionFor: bool,
    pub HasPartition: bool,
}
/// SplitRegion语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitRegionStmt {
    pub node_text: base::AstNode,
    pub SplitSyntaxOpt: SplitSyntaxOption,
    pub Table: TableName,
    pub PartitionNames: Vec<CIStr>,
    pub IndexName: CIStr,
    pub SplitOpt: SplitOption,
}
/// Split索引选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitIndexOption {
    pub PrimaryKey: bool,
    pub IndexName: CIStr,
    pub TableLevel: bool,
    pub SplitOpt: SplitOption,
}

/// Runaway监视类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunawayWatchType {
    #[default]
    Exact,
    Similar,
    Plan,
}
/// Runaway动作类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunawayActionType {
    #[default]
    DryRun,
    Cooldown,
    Kill,
    SwitchGroup,
}
/// 资源组Runaway动作选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupRunawayActionOption {
    pub Type: RunawayActionType,
    pub SwitchGroupName: CIStr,
}
/// RunawayRule类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunawayRuleType {
    #[default]
    ExecElapsed,
    ProcessedKeys,
    RequestUnit,
}
/// 资源组RunawayRule选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupRunawayRuleOption {
    pub Tp: RunawayRuleType,
    pub ExecElapsed: String,
    pub ProcessedKeys: i64,
    pub RequestUnit: i64,
}
/// 资源组Runaway监视选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupRunawayWatchOption {
    pub Type: RunawayWatchType,
    pub Duration: String,
}
/// 资源组Runaway选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResourceGroupRunawayOptionType {
    #[default]
    Rule,
    Action,
    Watch,
}
/// 资源组Runaway选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupRunawayOption {
    pub Tp: ResourceGroupRunawayOptionType,
    pub RuleOption: Option<ResourceGroupRunawayRuleOption>,
    pub ActionOption: Option<ResourceGroupRunawayActionOption>,
    pub WatchOption: Option<ResourceGroupRunawayWatchOption>,
}
/// Background选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackgroundOptionType {
    #[default]
    TaskNames,
    UtilizationLimit,
}
/// 资源组Background选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupBackgroundOption {
    pub Type: BackgroundOptionType,
    pub StrValue: String,
    pub UintValue: u64,
}
/// 资源组选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResourceGroupOptionType {
    #[default]
    RURate,
    Priority,
    Burstable,
    Runaway,
    Background,
}
/// Burstable类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BurstableType {
    #[default]
    Disable,
    Moderated,
    Unlimited,
}
/// 资源组选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupOption {
    pub Tp: ResourceGroupOptionType,
    pub UintValue: u64,
    pub Burstable: BurstableType,
    pub RunawayOptionList: Vec<ResourceGroupRunawayOption>,
    pub BackgroundOptions: Vec<ResourceGroupBackgroundOption>,
}
/// 放置策略选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlacementOptionType {
    #[default]
    PrimaryRegion,
    Regions,
    FollowerCount,
    VoterCount,
    LearnerCount,
    Schedule,
    Constraints,
    LeaderConstraints,
    FollowerConstraints,
    VoterConstraints,
    LearnerConstraints,
    SurvivalPreferences,
    Policy,
}
/// 放置策略选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlacementOption {
    pub Tp: PlacementOptionType,
    pub StrValue: String,
    pub UintValue: u64,
}
/// Attributes规格结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AttributesSpec {
    pub Default: bool,
    pub Attributes: String,
}
/// 统计信息选项规格结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatsOptionsSpec {
    pub Default: bool,
    pub StatsOptions: String,
}
/// 索引选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexOption {
    pub KeyBlockSize: u64,
    pub AddColumnarReplicaOnDemand: u64,
    pub Tp: IndexType,
    pub ParserName: CIStr,
    pub Comment: String,
    pub Visibility: IndexVisibility,
    pub PrimaryKeyTp: PrimaryKeyType,
    pub Global: bool,
    pub SplitOpt: Option<SplitOption>,
    pub AutoPreSplit: bool,
    pub SecondaryEngineAttr: String,
    pub Condition: Option<ExprNode>,
}

impl IndexOption {
    pub fn is_empty(&self) -> bool {
        self.PrimaryKeyTp == PrimaryKeyType::Default
            && self.KeyBlockSize == 0
            && self.Tp == IndexType::default()
            && self.ParserName.O.is_empty()
            && self.Comment.is_empty()
            && !self.Global
            && self.Visibility == IndexVisibility::Default
            && self.SplitOpt.is_none()
            && !self.AutoPreSplit
            && self.SecondaryEngineAttr.is_empty()
            && self.Condition.is_none()
    }

    pub fn restore_with_special_comments(&self, special_comments: bool) -> String {
        use ddl::{IndexOption as FormatIndexOption, IndexType as FormatIndexType};
        let tp = match self.Tp {
            IndexType::Invalid => FormatIndexType::Invalid,
            IndexType::Btree => FormatIndexType::Btree,
            IndexType::Hash => FormatIndexType::Hash,
            IndexType::Rtree => FormatIndexType::Rtree,
            IndexType::Hypo => FormatIndexType::Hypo,
            IndexType::HNSW => FormatIndexType::Hnsw,
            IndexType::Inverted => FormatIndexType::Inverted,
        };
        let option = FormatIndexOption {
            key_block_size: self.KeyBlockSize,
            tp,
            comment: self.Comment.clone(),
            parser_name: self.ParserName.O.clone(),
            visibility: match self.Visibility {
                IndexVisibility::Default => ddl::IndexVisibility::Default,
                IndexVisibility::Visible => ddl::IndexVisibility::Visible,
                IndexVisibility::Invisible => ddl::IndexVisibility::Invisible,
            },
            primary_key_tp: match self.PrimaryKeyTp {
                PrimaryKeyType::Default => ddl::PrimaryKeyType::Default,
                PrimaryKeyType::Clustered => ddl::PrimaryKeyType::Clustered,
                PrimaryKeyType::NonClustered => ddl::PrimaryKeyType::NonClustered,
            },
            global: self.Global,
            split_opt: self.SplitOpt.clone(),
            auto_pre_split: self.AutoPreSplit,
            secondary_engine_attr: self.SecondaryEngineAttr.clone(),
            add_columnar_replica_on_demand: i32::from(self.AddColumnarReplicaOnDemand > 0),
            condition: self.Condition.as_ref().map(Node::Text),
        };
        option.restore_with_special_comments(special_comments)
    }

    pub fn restore(&self) -> String {
        self.restore_with_special_comments(false)
    }
}

/// CREATE INDEX 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateIndexStmt {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub IndexName: String,
    pub Table: TableName,
    pub IndexPartSpecifications: Vec<IndexPartSpecification>,
    pub KeyType: IndexKeyType,
    pub Option: Option<IndexOption>,
    pub LockAlg: Option<IndexLockAndAlgorithm>,
}

/// DROP INDEX 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropIndexStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub IndexName: String,
    pub Table: TableName,
    pub IsHypo: bool,
    pub LockAlg: Option<IndexLockAndAlgorithm>,
}

/// 表To表结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableToTable {
    pub OldTable: TableName,
    pub NewTable: TableName,
}

/// 重命名表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenameTableStmt {
    pub node_text: base::AstNode,
    pub TableToTables: Vec<TableToTable>,
}

/// 列选择枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ColumnChoice {
    #[default]
    Default,
    All,
    Predicate,
    List,
}

/// ANALYZE选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AnalyzeOptionType {
    #[default]
    NumBuckets,
    NumTopN,
    CMSketchDepth,
    CMSketchWidth,
    NumSamples,
    SampleRate,
    NDVRate,
}

/// ANALYZE选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeOpt {
    pub Type: AnalyzeOptionType,
    pub Value: ExprNode,
}

/// 直方图Operation类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HistogramOperationType {
    #[default]
    Nop,
    Update,
    Drop,
}

/// ANALYZE表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeTableStmt {
    pub node_text: base::AstNode,
    pub TableNames: Vec<TableName>,
    pub PartitionNames: Vec<CIStr>,
    pub IndexNames: Vec<CIStr>,
    pub AnalyzeOpts: Vec<AnalyzeOpt>,
    pub IndexFlag: bool,
    pub Incremental: bool,
    pub NoWriteToBinLog: bool,
    pub HistogramOperation: HistogramOperationType,
    pub ColumnNames: Vec<CIStr>,
    pub ColumnChoice: ColumnChoice,
}

/// 压缩副本种类枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompactReplicaKind {
    #[default]
    All,
    TiFlash,
    TiKv,
}

/// 压缩表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactTableStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
    pub PartitionNames: Vec<CIStr>,
    pub ReplicaKind: CompactReplicaKind,
}

/// 优化表语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OptimizeTableStmt {
    pub node_text: base::AstNode,
    pub NoWriteToBinLog: bool,
    pub Tables: Vec<TableName>,
}

/// KILL语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KillStmt {
    pub node_text: base::AstNode,
    pub Query: bool,
    pub ConnectionID: u64,
    pub TiDBExtension: bool,
    pub Expr: Option<ExprNode>,
}

/// 加载统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadStatsStmt {
    pub node_text: base::AstNode,
    pub Path: String,
}

/// 锁统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LockStatsStmt {
    pub node_text: base::AstNode,
    pub Tables: Vec<TableName>,
}

/// 解锁统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnlockStatsStmt {
    pub node_text: base::AstNode,
    pub Tables: Vec<TableName>,
}

/// DROP统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropStatsStmt {
    pub node_text: base::AstNode,
    pub Tables: Vec<TableName>,
    pub PartitionNames: Vec<CIStr>,
    pub IsGlobalStats: bool,
}

/// 统计信息对象作用域枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatsObjectScope {
    #[default]
    Global,
    Database,
    Table,
}

/// 统计信息对象结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatsObject {
    pub StatsObjectScope: StatsObjectScope,
    pub DBName: CIStr,
    pub TableName: CIStr,
}

/// Refresh统计信息Mode枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RefreshStatsMode {
    #[default]
    Lite,
    Full,
}

/// Refresh统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefreshStatsStmt {
    pub node_text: base::AstNode,
    pub RefreshObjects: Vec<StatsObject>,
    pub RefreshMode: Option<RefreshStatsMode>,
    pub IsClusterWide: bool,
}

/// FLUSH语句类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FlushStmtType {
    #[default]
    Privileges,
    Status,
    TiDBPlugin,
    Hosts,
    Logs,
    Tables,
    ClientErrorsSummary,
    StatsDelta,
}

/// Log类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogType {
    #[default]
    Default,
    Binary,
    Engine,
    Error,
    General,
    Slow,
}

/// FLUSH语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FlushStmt {
    pub node_text: base::AstNode,
    pub Tp: FlushStmtType,
    pub NoWriteToBinLog: bool,
    pub LogType: LogType,
    pub Plugins: Vec<String>,
    pub Tables: Vec<TableName>,
    pub ReadLock: bool,
    pub IsCluster: bool,
    pub FlushObjects: Vec<StatsObject>,
}

/// 语句作用域枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatementScope {
    #[default]
    Session,
    Global,
    Instance,
}

/// 表锁类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TableLockType {
    #[default]
    Read,
    ReadLocal,
    Write,
    WriteLocal,
}

/// 表锁结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableLock {
    pub Table: TableName,
    pub Type: TableLockType,
}

/// 锁Tables语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LockTablesStmt {
    pub node_text: base::AstNode,
    pub TableLocks: Vec<TableLock>,
}

/// USE语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UseStmt {
    pub node_text: base::AstNode,
    pub DBName: String,
}

/// SET 变量赋值语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetStmt {
    pub node_text: base::AstNode,
    pub Variables: Vec<VariableAssignment>,
}

/// SETNames常量。
pub const SetNames: &str = "SetNAMES";
/// SET字符集常量。
pub const SetCharset: &str = "SetCharset";

/// 变量赋值结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VariableAssignment {
    pub Name: String,
    pub Value: ExprNode,
    pub IsInstance: bool,
    pub IsGlobal: bool,
    pub IsSystem: bool,
    pub ExtendValue: Option<ExprNode>,
}

/// BEGIN/START TRANSACTION 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BeginStmt {
    pub node_text: base::AstNode,
    pub Mode: String,
    pub CausalConsistencyOnly: bool,
    pub ReadOnly: bool,
    pub AsOf: Option<AsOfClause>,
}

/// BINLOG语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinlogStmt {
    pub node_text: base::AstNode,
    pub Str: String,
}

/// DEALLOCATE语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeallocateStmt {
    pub node_text: base::AstNode,
    pub Name: String,
}

/// PREPARE 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrepareStmt {
    pub node_text: base::AstNode,
    pub Name: String,
    pub SQLText: String,
    pub SQLVar: Option<String>,
}

/// EXECUTE 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecuteStmt {
    pub node_text: base::AstNode,
    pub Name: String,
    pub UsingVars: Vec<ExprNode>,
}

/// SHUTDOWN语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShutdownStmt {
    pub node_text: base::AstNode,
}

/// RESTART语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestartStmt {
    pub node_text: base::AstNode,
}

/// HELP语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HelpStmt {
    pub node_text: base::AstNode,
    pub Topic: String,
}

/// SAVEPOINT语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SavepointStmt {
    pub node_text: base::AstNode,
    pub Name: String,
}

/// 释放SAVEPOINT语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReleaseSavepointStmt {
    pub node_text: base::AstNode,
    pub Name: String,
}

/// 推荐索引语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendIndexStmt {
    pub node_text: base::AstNode,
    pub Action: String,
    pub SQL: String,
    pub ID: i64,
    pub Options: Vec<RecommendIndexOption>,
}

/// 推荐索引选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendIndexOption {
    pub Option: String,
    pub Value: ExprNode,
}

/// AsOF子句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AsOfClause {
    pub TsExpr: ExprNode,
}

/// 时间单位枚举（YEAR/MONTH/DAY 等）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeUnitType {
    #[default]
    Invalid,
    Microsecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
    SecondMicrosecond,
    MinuteMicrosecond,
    MinuteSecond,
    HourMicrosecond,
    HourSecond,
    HourMinute,
    DayMicrosecond,
    DaySecond,
    DayMinute,
    DayHour,
    YearMonth,
}

impl TimeUnitType {
    pub fn duration_nanos(self) -> Option<u64> {
        match self {
            Self::Microsecond => Some(1_000),
            Self::Second => Some(1_000_000_000),
            Self::Minute => Some(60 * 1_000_000_000),
            Self::Hour => Some(60 * 60 * 1_000_000_000),
            Self::Day => Some(24 * 60 * 60 * 1_000_000_000),
            Self::Week => Some(7 * 24 * 60 * 60 * 1_000_000_000),
            _ => None,
        }
    }
}

/// PLAN REPLAYER 语句：转储/加载/捕获执行计划。
#[derive(Default)]
pub struct PlanReplayerStmt {
    pub node_text: base::AstNode,
    pub Stmt: Option<Box<dyn Node>>,
    pub Analyze: bool,
    pub Load: bool,
    pub File: String,
    pub Where: Option<ExprNode>,
    pub OrderBy: Vec<ByItem>,
    pub Limit: Option<Limit>,
    pub StmtList: Vec<String>,
    pub Capture: bool,
    pub Remove: bool,
    pub SQLDigest: String,
    pub PlanDigest: String,
    pub HistoricalStatsInfo: Option<AsOfClause>,
}

/// 流量操作类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrafficOpType {
    #[default]
    Capture,
    Replay,
    Show,
    Cancel,
}

/// 流量选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrafficOptionType {
    #[default]
    Duration,
    EncryptionMethod,
    Compress,
    Username,
    Password,
    Speed,
    ReadOnly,
}

/// 流量选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrafficOption {
    pub OptionType: TrafficOptionType,
    pub StrValue: String,
    pub BoolValue: bool,
    pub FloatValue: Option<ExprNode>,
}

/// 流量捕获/回放语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrafficStmt {
    pub node_text: base::AstNode,
    pub OpType: TrafficOpType,
    pub Dir: String,
    pub Options: Vec<TrafficOption>,
}

/// IN常量。
pub const MODE_IN: i32 = 0;
/// OUT常量。
pub const MODE_OUT: i32 = 1;
/// INOUT常量。
pub const MODE_INOUT: i32 = 2;

/// StoreParameter结构体。
#[derive(Clone)]
pub struct StoreParameter {
    pub Paramstatus: i32,
    pub ParamType: parser_types::types::FieldType,
    pub ParamName: String,
}

/// DROP统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DropStatisticsStmt {
    pub node_text: base::AstNode,
    pub StatsName: String,
}

/// CREATE统计信息语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateStatisticsStmt {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub StatsName: String,
    pub StatsType: u8,
    pub Table: TableName,
    pub Columns: Vec<ColumnName>,
}

/// 解锁Tables语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnlockTablesStmt {
    pub node_text: base::AstNode,
}

/// SET PASSWORD 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetPwdStmt {
    pub node_text: base::AstNode,
    pub User: Option<parser_auth::parser::auth::auth::UserIdentity>,
    pub Password: String,
    pub RetainCurrentPassword: bool,
}

/// SET会话状态语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetSessionStatesStmt {
    pub node_text: base::AstNode,
    pub SessionStates: String,
}

/// SET配置语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetConfigStmt {
    pub node_text: base::AstNode,
    pub Type: String,
    pub Instance: String,
    pub Name: String,
    pub Value: ExprNode,
}
/// SET资源组语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetResourceGroupStmt {
    pub node_text: base::AstNode,
    pub Name: CIStr,
}
/// SET角色选项枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SetRoleOpt {
    #[default]
    None,
    All,
    Regular,
    AllExcept,
    Default,
}
/// SET角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetRoleStmt {
    pub node_text: base::AstNode,
    pub SetRoleOpt: SetRoleOpt,
    pub RoleList: Vec<parser_auth::parser::auth::auth::RoleIdentity>,
}
/// SET默认角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetDefaultRoleStmt {
    pub node_text: base::AstNode,
    pub SetRoleOpt: SetRoleOpt,
    pub RoleList: Vec<parser_auth::parser::auth::auth::RoleIdentity>,
    pub UserList: Vec<parser_auth::parser::auth::auth::UserIdentity>,
}

/// IDENTIFIED BY/AS/WITH 认证选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthOption {
    pub AuthString: String,
    pub ByAuthString: bool,
    pub HashString: String,
    pub ByHashString: bool,
    pub AuthPlugin: String,
}

/// 双密码选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DualPasswordOptionType {
    #[default]
    None,
    RetainCurrent,
    DiscardOld,
}

/// CREATE/ALTER USER 中的单个用户规格。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserSpec {
    pub User: parser_auth::parser::auth::auth::UserIdentity,
    pub AuthOpt: Option<AuthOption>,
    pub DualPasswordOption: DualPasswordOptionType,
    pub IsRole: bool,
}

/// 资源选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceOptionType {
    MaxQueriesPerHour,
    MaxUpdatesPerHour,
    MaxConnectionsPerHour,
    MaxUserConnections,
}
/// 资源选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceOption {
    pub Type: ResourceOptionType,
    pub Count: i64,
}

/// 认证令牌或TLS选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AuthTokenOrTLSOptionType {
    #[default]
    TlsNone,
    Ssl,
    X509,
    Cipher,
    Issuer,
    Subject,
    SAN,
    TokenIssuer,
}
/// 认证令牌或TLS选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthTokenOrTLSOption {
    pub Type: AuthTokenOrTLSOptionType,
    pub Value: String,
}

/// 密码或锁选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordOrLockOptionType {
    Unlock,
    Lock,
    PasswordHistoryDefault,
    PasswordHistory,
    PasswordReuseDefault,
    PasswordReuseInterval,
    PasswordExpire,
    PasswordExpireInterval,
    PasswordExpireNever,
    PasswordExpireDefault,
    FailedLoginAttempts,
    PasswordLockTime,
    PasswordLockTimeUnbounded,
    PasswordRequireCurrentDefault,
}
/// 密码或锁选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordOrLockOption {
    pub Type: PasswordOrLockOptionType,
    pub Count: i64,
}

/// 注释或属性选项类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommentOrAttributeOptionType {
    UserComment,
    UserAttribute,
}
/// 注释或属性选项结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentOrAttributeOption {
    pub Type: CommentOrAttributeOptionType,
    pub Value: String,
}
/// 资源组名称选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupNameOption {
    pub Value: String,
}

/// CREATE USER 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateUserStmt {
    pub node_text: base::AstNode,
    pub IsCreateRole: bool,
    pub IfNotExists: bool,
    pub Specs: Vec<UserSpec>,
    pub AuthTokenOrTLSOptions: Vec<AuthTokenOrTLSOption>,
    pub ResourceOptions: Vec<ResourceOption>,
    pub PasswordOrLockOptions: Vec<PasswordOrLockOption>,
    pub CommentOrAttributeOption: Option<CommentOrAttributeOption>,
    pub ResourceGroupNameOption: Option<ResourceGroupNameOption>,
}

/// ALTER USER 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterUserStmt {
    pub node_text: base::AstNode,
    pub IfExists: bool,
    pub Specs: Vec<UserSpec>,
    pub CurrentAuth: Option<AuthOption>,
    pub CurrentDualPasswordOption: DualPasswordOptionType,
    pub AuthTokenOrTLSOptions: Vec<AuthTokenOrTLSOption>,
    pub ResourceOptions: Vec<ResourceOption>,
    pub PasswordOrLockOptions: Vec<PasswordOrLockOption>,
    pub CommentOrAttributeOption: Option<CommentOrAttributeOption>,
    pub ResourceGroupNameOption: Option<ResourceGroupNameOption>,
}

/// 权限元素结构体。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivElem {
    pub Priv: parser_mysql::privs::PrivilegeType,
    pub Cols: Vec<ColumnName>,
    pub Name: String,
}
impl Default for PrivElem {
    fn default() -> Self {
        Self {
            Priv: parser_mysql::privs::UsagePriv,
            Cols: Vec::new(),
            Name: String::new(),
        }
    }
}

/// 对象类型类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObjectTypeType {
    #[default]
    None,
    Table,
    Function,
    Procedure,
}
/// GRANT级别类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GrantLevelType {
    #[default]
    Global,
    DB,
    Table,
}
/// GRANT级别结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantLevel {
    pub Level: GrantLevelType,
    pub DBName: String,
    pub TableName: String,
}
/// GRANT 权限语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantStmt {
    pub node_text: base::AstNode,
    pub Privs: Vec<PrivElem>,
    pub ObjectType: ObjectTypeType,
    pub Level: GrantLevel,
    pub Users: Vec<UserSpec>,
    pub AuthTokenOrTLSOptions: Vec<AuthTokenOrTLSOption>,
    pub WithGrant: bool,
}
/// GRANT代理语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantProxyStmt {
    pub node_text: base::AstNode,
    pub LocalUser: parser_auth::parser::auth::auth::UserIdentity,
    pub ExternalUsers: Vec<parser_auth::parser::auth::auth::UserIdentity>,
    pub WithGrant: bool,
}
/// GRANT角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantRoleStmt {
    pub node_text: base::AstNode,
    pub Roles: Vec<parser_auth::parser::auth::auth::RoleIdentity>,
    pub Users: Vec<parser_auth::parser::auth::auth::UserIdentity>,
}
/// REVOKE 权限语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevokeStmt {
    pub node_text: base::AstNode,
    pub Privs: Vec<PrivElem>,
    pub ObjectType: ObjectTypeType,
    pub Level: GrantLevel,
    pub Users: Vec<UserSpec>,
}
/// REVOKE角色语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevokeRoleStmt {
    pub node_text: base::AstNode,
    pub Roles: Vec<parser_auth::parser::auth::auth::RoleIdentity>,
    pub Users: Vec<parser_auth::parser::auth::auth::UserIdentity>,
}

/// 字段项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FieldItemType {
    #[default]
    Terminated,
    Enclosed,
    Escaped,
    DefinedNullBy,
}
/// 字段项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldItem {
    pub Type: FieldItemType,
    pub Value: String,
    pub OptEnclosed: bool,
}
/// Fields子句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldsClause {
    pub Terminated: Option<String>,
    pub Enclosed: Option<String>,
    pub OptEnclosed: bool,
    pub Escaped: Option<String>,
    pub DefinedNullBy: Option<String>,
    pub NullValueOptEnclosed: bool,
}
/// Lines子句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinesClause {
    pub Starting: Option<String>,
    pub Terminated: Option<String>,
}
/// SELECTINTO类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectIntoType {
    #[default]
    Outfile,
    Dumpfile,
    Variables,
}
/// SELECTINTO选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectIntoOption {
    pub Tp: SelectIntoType,
    pub FileName: String,
    pub FieldsInfo: Option<FieldsClause>,
    pub LinesInfo: Option<LinesClause>,
}
/// 加载Data选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadDataOpt {
    pub Name: String,
    pub Value: Option<ExprNode>,
}
/// FileLoc引用枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FileLocRef {
    #[default]
    ServerOrRemote,
    Client,
}
/// 加载Data语句结构体。
#[derive(Default)]
pub struct LoadDataStmt {
    pub node_text: base::AstNode,
    pub LowPriority: bool,
    pub FileLocRef: FileLocRef,
    pub Path: String,
    pub Format: Option<String>,
    pub OnDuplicate: OnDuplicateKeyHandlingType,
    pub Table: TableName,
    pub Charset: Option<String>,
    pub FieldsInfo: Option<FieldsClause>,
    pub LinesInfo: Option<LinesClause>,
    pub IgnoreLines: Option<u64>,
    pub ColumnsAndUserVars: Vec<ColumnNameOrUserVar>,
    pub Columns: Vec<ColumnName>,
    pub ColumnAssignments: Vec<Assignment>,
    pub Options: Vec<LoadDataOpt>,
}
/// IMPORTINTO语句结构体。
#[derive(Default)]
pub struct ImportIntoStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
    pub ColumnsAndUserVars: Vec<ColumnNameOrUserVar>,
    pub ColumnAssignments: Vec<Assignment>,
    pub Path: String,
    pub Format: Option<String>,
    pub Select: Option<Box<dyn Node>>,
    pub Options: Vec<LoadDataOpt>,
}
/// NonTransactionalDML语句结构体。
#[derive(Default)]
pub struct NonTransactionalDMLStmt {
    pub node_text: base::AstNode,
    pub DryRun: i32,
    pub ShardColumn: ColumnName,
    pub Limit: u64,
    pub DMLStmt: Option<Box<dyn Node>>,
}
/// CREATE脱敏策略语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateMaskingPolicyStmt {
    pub node_text: base::AstNode,
    pub OrReplace: bool,
    pub IfNotExists: bool,
    pub PolicyName: CIStr,
    pub Table: TableName,
    pub Column: ColumnName,
    pub Expr: Option<ExprNode>,
    pub RestrictOps: MaskingPolicyRestrictOps,
    pub MaskingPolicyState: MaskingPolicyState,
}

/// 校准资源类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CalibrateResourceType {
    #[default]
    None,
    TPCC,
    OLTPReadWrite,
    OLTPReadOnly,
    OLTPWriteOnly,
    TPCH10,
}
/// 动态校准资源选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DynamicCalibrateResourceOptionType {
    #[default]
    StartTime,
    EndTime,
    Duration,
}
/// 动态校准资源选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DynamicCalibrateResourceOption {
    pub Tp: DynamicCalibrateResourceOptionType,
    pub Ts: Option<ExprNode>,
    pub StrValue: String,
    pub Unit: TimeUnitType,
}
/// 校准资源语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CalibrateResourceStmt {
    pub node_text: base::AstNode,
    pub Tp: CalibrateResourceType,
    pub DynamicCalibrateResourceOptionList: Vec<DynamicCalibrateResourceOption>,
}

/// 查询监视选项类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QueryWatchOptionType {
    #[default]
    ResourceGroup,
    Action,
    Type,
}
/// 查询监视资源组选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryWatchResourceGroupOption {
    pub GroupNameStr: CIStr,
    pub GroupNameExpr: Option<ExprNode>,
}
/// 查询监视文本选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryWatchTextOption {
    pub Type: RunawayWatchType,
    pub PatternExpr: ExprNode,
    pub TypeSpecified: bool,
}
/// 查询监视选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryWatchOption {
    pub Tp: QueryWatchOptionType,
    pub ResourceGroupOption: Option<QueryWatchResourceGroupOption>,
    pub ActionOption: Option<ResourceGroupRunawayActionOption>,
    pub TextOption: Option<QueryWatchTextOption>,
}
/// QUERY WATCH ADD 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddQueryWatchStmt {
    pub node_text: base::AstNode,
    pub QueryWatchOptionList: Vec<QueryWatchOption>,
}

/// 存储过程Decl结构体。
#[derive(Clone, Debug)]
pub struct ProcedureDecl {
    pub DeclNames: Vec<String>,
    pub DeclType: parser_types::types::FieldType,
    pub DeclDefault: Option<ExprNode>,
}
/// 存储过程OpenCur结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcedureOpenCur {
    pub node_text: base::AstNode,
    pub CurName: String,
}
/// 存储过程CloseCur结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcedureCloseCur {
    pub node_text: base::AstNode,
    pub CurName: String,
}
/// 存储过程FETCHINTO结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcedureFetchInto {
    pub node_text: base::AstNode,
    pub CurName: String,
    pub Variables: Vec<String>,
}
/// 存储过程错误条件类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcedureErrorConType {
    #[default]
    SqlWarning,
    NotFound,
    SqlException,
}
/// 存储过程错误条件结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcedureErrorCon {
    pub node_text: base::AstNode,
    pub ErrorCon: ProcedureErrorConType,
}
/// 存储过程错误值结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcedureErrorVal {
    pub node_text: base::AstNode,
    pub ErrorNum: u64,
}
/// 存储过程错误状态结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcedureErrorState {
    pub node_text: base::AstNode,
    pub CodeStatus: String,
}
/// 存储过程游标结构体。
#[derive(Default)]
pub struct ProcedureCursor {
    pub node_text: base::AstNode,
    pub CurName: String,
    pub Selectstring: Option<Box<dyn Node>>,
}
/// 存储过程错误控制结构体。
#[derive(Default)]
pub struct ProcedureErrorControl {
    pub node_text: base::AstNode,
    pub ControlHandle: i32,
    pub ErrorCon: Vec<Box<dyn Any>>,
    pub Operate: Option<Box<dyn Node>>,
}
/// 存储过程代码块结构体。
#[derive(Default)]
pub struct ProcedureBlock {
    pub node_text: base::AstNode,
    pub ProcedureVars: Vec<Box<dyn Any>>,
    pub ProcedureProcStmts: Vec<Box<dyn Node>>,
}
/// 存储过程IF信息结构体。
#[derive(Default)]
pub struct ProcedureIfInfo {
    pub node_text: base::AstNode,
    pub IfBody: Option<Box<dyn Node>>,
}
/// 存储过程IF代码块结构体。
#[derive(Default)]
pub struct ProcedureIfBlock {
    pub node_text: base::AstNode,
    pub IfExpr: ExprNode,
    pub ProcedureIfStmts: Vec<Box<dyn Node>>,
    pub ProcedureElseStmt: Option<Box<dyn Node>>,
}
/// 存储过程ELSEIF代码块结构体。
#[derive(Default)]
pub struct ProcedureElseIfBlock {
    pub node_text: base::AstNode,
    pub ProcedureIfStmt: Option<Box<dyn Node>>,
}
/// 存储过程ELSE代码块结构体。
#[derive(Default)]
pub struct ProcedureElseBlock {
    pub node_text: base::AstNode,
    pub ProcedureIfStmts: Vec<Box<dyn Node>>,
}
/// 简单WHENTHEN语句结构体。
#[derive(Default)]
pub struct SimpleWhenThenStmt {
    pub node_text: base::AstNode,
    pub Expr: ExprNode,
    pub ProcedureStmts: Vec<Box<dyn Node>>,
}
/// 搜索WHENTHEN语句结构体。
#[derive(Default)]
pub struct SearchWhenThenStmt {
    pub node_text: base::AstNode,
    pub Expr: ExprNode,
    pub ProcedureStmts: Vec<Box<dyn Node>>,
}
/// 简单CASE语句结构体。
#[derive(Default)]
pub struct SimpleCaseStmt {
    pub node_text: base::AstNode,
    pub Condition: ExprNode,
    pub WhenCases: Vec<SimpleWhenThenStmt>,
    pub ElseCases: Vec<Box<dyn Node>>,
}
/// 搜索CASE语句结构体。
#[derive(Default)]
pub struct SearchCaseStmt {
    pub node_text: base::AstNode,
    pub WhenCases: Vec<SearchWhenThenStmt>,
    pub ElseCases: Vec<Box<dyn Node>>,
}
/// 存储过程WHILE语句结构体。
#[derive(Default)]
pub struct ProcedureWhileStmt {
    pub node_text: base::AstNode,
    pub Condition: ExprNode,
    pub Body: Vec<Box<dyn Node>>,
}
/// 存储过程REPEAT语句结构体。
#[derive(Default)]
pub struct ProcedureRepeatStmt {
    pub node_text: base::AstNode,
    pub Body: Vec<Box<dyn Node>>,
    pub Condition: ExprNode,
}
/// 存储过程标签代码块结构体。
#[derive(Default)]
pub struct ProcedureLabelBlock {
    pub node_text: base::AstNode,
    pub LabelName: String,
    pub Block: Option<Box<dyn Node>>,
    pub LabelError: bool,
    pub LabelEnd: String,
}
/// 存储过程标签循环结构体。
#[derive(Default)]
pub struct ProcedureLabelLoop {
    pub node_text: base::AstNode,
    pub LabelName: String,
    pub Block: Option<Box<dyn Node>>,
    pub LabelError: bool,
    pub LabelEnd: String,
}
/// 存储过程跳转结构体。
#[derive(Default)]
pub struct ProcedureJump {
    pub node_text: base::AstNode,
    pub Name: String,
    pub IsLeave: bool,
}
/// 存储过程信息结构体。
#[derive(Default)]
pub struct ProcedureInfo {
    pub node_text: base::AstNode,
    pub IfNotExists: bool,
    pub ProcedureName: TableName,
    pub ProcedureParam: Vec<StoreParameter>,
    pub ProcedureBody: Option<Box<dyn Node>>,
}

/// 事务完成类型（DEFAULT/CHAIN/RELEASE）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompletionType {
    #[default]
    Default,
    Chain,
    Release,
}

/// 完成类型默认常量。
pub const CompletionTypeDefault: CompletionType = CompletionType::Default;
/// 完成类型Chain常量。
pub const CompletionTypeChain: CompletionType = CompletionType::Chain;
/// 完成类型释放常量。
pub const CompletionTypeRelease: CompletionType = CompletionType::Release;
/// 乐观常量。
pub const Optimistic: &str = "OPTIMISTIC";
/// 悲观常量。
pub const Pessimistic: &str = "PESSIMISTIC";

/// COMMIT 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommitStmt {
    pub node_text: base::AstNode,
    pub CompletionType: CompletionType,
}

/// ROLLBACK 语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RollbackStmt {
    pub node_text: base::AstNode,
    pub CompletionType: CompletionType,
    pub SavepointName: String,
}

/// SELECT锁类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectLockType {
    #[default]
    None,
    ForUpdate,
    ForUpdateNoWait,
    ForUpdateWaitN,
    ForShare,
    ForShareNoWait,
    ForUpdateSkipLocked,
    ForShareSkipLocked,
}

/// SELECT锁信息结构体。
#[derive(Clone, Debug)]
pub struct SelectLockInfo {
    pub lock_type: SelectLockType,
    pub LockType: SelectLockType,
    pub WaitSec: u64,
    pub Tables: Vec<TableName>,
}

/// SELECT语句Opts结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectStmtOpts {
    pub Distinct: bool,
    pub ExplicitAll: bool,
    pub SQLCache: bool,
    pub TableHints: Vec<TableOptimizerHint>,
    pub Priority: i32,
    pub SQLSmallResult: bool,
    pub SQLBigResult: bool,
    pub SQLBufferResult: bool,
    pub CalcFoundRows: bool,
    pub StraightJoin: bool,
}

/// SELECT语句种类枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectStmtKind {
    #[default]
    Select,
    Table,
    Values,
}

/// 行表达式结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RowExpr {
    pub Values: Vec<ExprNode>,
}

/// 窗口帧类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrameType {
    #[default]
    Rows,
    Ranges,
    Groups,
}

/// 边界类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BoundType {
    #[default]
    CurrentRow,
    Preceding,
    Following,
}

/// 窗口帧边界结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameBound {
    pub Type: BoundType,
    pub UnBounded: bool,
    pub Expr: Option<ExprNode>,
    pub Unit: TimeUnitType,
}

/// 窗口帧范围结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameExtent {
    pub Start: FrameBound,
    pub End: FrameBound,
}

/// 窗口帧子句。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameClause {
    pub Type: FrameType,
    pub Extent: FrameExtent,
}

/// 窗口规格（PARTITION BY / ORDER BY / FRAME）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowSpec {
    pub Name: CIStr,
    pub Ref: CIStr,
    pub PartitionBy: Vec<ByItem>,
    pub OrderBy: Vec<ByItem>,
    pub Frame: Option<Box<FrameClause>>,
    pub OnlyAlias: bool,
}

/// SELECT 语句 AST。
#[derive(Default)]
pub struct SelectStmt {
    pub node_text: base::AstNode,
    pub Kind: SelectStmtKind,
    pub SelectStmtOpts: SelectStmtOpts,
    pub Distinct: bool,
    pub From: Option<TableRefsClause>,
    pub Where: Option<ExprNode>,
    pub Fields: FieldList,
    pub GroupBy: Vec<ByItem>,
    /// Whether the GROUP BY clause carries a trailing WITH ROLLUP modifier.
    pub GroupByRollup: bool,
    pub Having: Option<ExprNode>,
    pub OrderBy: Vec<ByItem>,
    pub Limit: Option<Limit>,
    pub TableHints: Vec<TableOptimizerHint>,
    pub IsInBraces: bool,
    pub QueryBlockOffset: isize,
    pub With: Option<WithClauseRef>,
    pub WithBeforeBraces: bool,
    pub lock_info: Option<SelectLockInfo>,
    pub children: Vec<Box<dyn Node>>,
    pub Lists: Vec<RowExpr>,
    pub WindowSpecs: Vec<WindowSpec>,
    pub SelectIntoOpt: Option<SelectIntoOption>,
}

impl SelectStmt {
    pub fn with_lock(lock_type: SelectLockType) -> Self {
        Self {
            lock_info: Some(SelectLockInfo {
                lock_type,
                LockType: lock_type,
                WaitSec: 0,
                Tables: Vec::new(),
            }),
            ..Self::default()
        }
    }
    pub fn with_child(child: Box<dyn Node>) -> Self {
        Self {
            children: vec![child],
            ..Self::default()
        }
    }
}

impl Node for SelectStmt {
    fn node_text(&self) -> &base::AstNode {
        &self.node_text
    }
    fn node_text_mut(&mut self) -> &mut base::AstNode {
        &mut self.node_text
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
    fn accept(&self, visitor: &mut dyn Visitor) -> bool {
        if visitor.enter(self) {
            return visitor.leave(self);
        }
        if !walk::Children::visit_children(self, visitor) {
            return false;
        }
        visitor.leave(self)
    }
    fn accept_in_place(&mut self, visitor: &mut dyn InPlaceVisitor) -> bool {
        if visitor.enter(self) {
            return visitor.leave(self);
        }
        if !walk::MutChildren::visit_children_mut(self, visitor) {
            return false;
        }
        visitor.leave(self)
    }
}

/// EXPLAIN 语句 AST。
pub struct ExplainStmt {
    pub node_text: base::AstNode,
    pub analyze: bool,
    pub stmt: Option<Box<dyn Node>>,
    pub Format: String,
    pub Explore: bool,
    pub SQLDigest: String,
    pub ReplayerFile: String,
    pub PlanDigest: String,
}

/// EXPLAINFOR语句结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplainForStmt {
    pub node_text: base::AstNode,
    pub Format: String,
    pub ConnectionID: u64,
}
impl ExplainStmt {
    pub fn new(analyze: bool, stmt: Box<dyn Node>) -> Self {
        Self {
            node_text: Default::default(),
            analyze,
            stmt: Some(stmt),
            Format: String::new(),
            Explore: false,
            SQLDigest: String::new(),
            ReplayerFile: String::new(),
            PlanDigest: String::new(),
        }
    }
}

/// SET运算类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SetOprType {
    #[default]
    Union,
    UnionAll,
    Except,
    ExceptAll,
    Intersect,
    IntersectAll,
}

/// 单个 CTE 定义。
pub struct CommonTableExpression {
    pub Name: CIStr,
    pub ColNameList: Vec<CIStr>,
    pub Query: Box<dyn Node>,
    pub IsRecursive: bool,
}

/// WITH 公共表表达式（CTE）子句。
#[derive(Default)]
pub struct WithClause {
    pub IsRecursive: bool,
    pub CTEs: Vec<CommonTableExpression>,
}

/// Shared WITH ownership mirrors the pointers retained by nested Go set operations.
/// Mutations through either list or SELECT must remain visible to both owners.
pub type WithClauseRef = std::rc::Rc<std::cell::RefCell<WithClause>>;

impl WithClause {
    pub fn into_shared(self) -> WithClauseRef {
        std::rc::Rc::new(std::cell::RefCell::new(self))
    }
}

/// SET运算SELECT列表结构体。
pub struct SetOprSelectList {
    pub node_text: base::AstNode,
    pub selects: Vec<Box<dyn Node>>,
    pub operators: Vec<Option<SetOprType>>,
    pub With: Option<WithClauseRef>,
    pub OrderBy: Vec<ByItem>,
    pub Limit: Option<Limit>,
    pub AfterSetOperator: Option<SetOprType>,
}
impl SetOprSelectList {
    pub fn new(selects: Vec<Box<dyn Node>>) -> Self {
        let operators = (0..selects.len()).map(|_| None).collect();
        Self {
            node_text: Default::default(),
            selects,
            operators,
            With: None,
            OrderBy: Vec::new(),
            Limit: None,
            AfterSetOperator: None,
        }
    }
}

/// 集合运算语句（UNION/INTERSECT/EXCEPT）。
pub struct SetOprStmt {
    pub node_text: base::AstNode,
    pub select_list: SetOprSelectList,
    pub OrderBy: Vec<ByItem>,
    pub Limit: Option<Limit>,
    pub With: Option<WithClauseRef>,
    pub IsInBraces: bool,
}
impl SetOprStmt {
    pub fn new(select_list: SetOprSelectList) -> Self {
        Self {
            node_text: Default::default(),
            select_list,
            OrderBy: Vec::new(),
            Limit: None,
            With: None,
            IsInBraces: false,
        }
    }
}

/// ADMIN语句类型枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminStmtType {
    ShowDdl,
    ShowDdlJobs,
    ShowSlow,
    CaptureBindings,
    ShowNextRowId,
    ShowDdlJobQueries,
    ShowDdlJobQueriesWithRange,
    CheckTable,
    WorkloadRepoCreate,
    ReloadExprPushdownBlacklist,
    ReloadOptRuleBlacklist,
    FlushBindings,
    EvolveBindings,
    ReloadBindings,
    ReloadClusterBindings,
    ReloadStatistics,
    ShowBdrRole,
    UnsetBdrRole,
    CancelDdlJobs,
    PauseDdlJobs,
    ResumeDdlJobs,
    CheckIndex,
    RecoverIndex,
    CleanupIndex,
    ChecksumTable,
    CheckIndexRange,
    PluginEnable,
    PluginDisable,
    FlushPlanCache,
    SetBdrRole,
    AlterDdlJob,
}

/// SHOW慢查询类型枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShowSlowType {
    #[default]
    Recent,
    Top,
}
/// SHOW慢查询种类枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShowSlowKind {
    #[default]
    Default,
    Internal,
    All,
}
/// SHOW慢查询结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowSlow {
    pub Tp: ShowSlowType,
    pub Kind: ShowSlowKind,
    pub Count: u64,
}
/// 句柄范围结构体。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HandleRange {
    pub Begin: i64,
    pub End: i64,
}
/// ALTER任务选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterJobOption {
    pub Name: String,
    pub Value: ExprNode,
}

/// ADMIN 诊断/维护语句 AST。
pub struct AdminStmt {
    pub node_text: base::AstNode,
    pub statement_type: AdminStmtType,
    pub job_ids: Vec<i64>,
    pub tables: Vec<TableName>,
    pub index: String,
    pub job_number: i64,
    pub where_expr: Option<ExprNode>,
    pub handle_ranges: Vec<HandleRange>,
    pub limit_simple: LimitSimple,
    pub show_slow: Option<ShowSlow>,
    pub plugins: Vec<String>,
    pub statement_scope: StatementScope,
    pub bdr_role: BDRRole,
    pub alter_job_options: Vec<AlterJobOption>,
}
impl AdminStmt {
    pub fn new(statement_type: AdminStmtType) -> Self {
        Self {
            node_text: Default::default(),
            statement_type,
            job_ids: Vec::new(),
            tables: Vec::new(),
            index: String::new(),
            job_number: 0,
            where_expr: None,
            handle_ranges: Vec::new(),
            limit_simple: LimitSimple::default(),
            show_slow: None,
            plugins: Vec::new(),
            statement_scope: StatementScope::default(),
            bdr_role: BDRRole::default(),
            alter_job_options: Vec::new(),
        }
    }
}

/// 清理表锁语句结构体。
#[derive(Default)]
pub struct CleanupTableLockStmt {
    pub node_text: base::AstNode,
    pub Tables: Vec<TableName>,
}
/// 修复表语句结构体。
pub struct RepairTableStmt {
    pub node_text: base::AstNode,
    pub Table: TableName,
    pub CreateStmt: Box<CreateTableStmt>,
}

/// TRACE语句结构体。
pub struct TraceStmt {
    pub node_text: base::AstNode,
    pub Stmt: Box<dyn Node>,
    pub Format: String,
    pub TracePlan: bool,
    pub TracePlanTarget: String,
}
impl TraceStmt {
    pub fn new(stmt: Box<dyn Node>) -> Self {
        Self {
            node_text: Default::default(),
            Stmt: stmt,
            Format: String::new(),
            TracePlan: false,
            TracePlanTarget: String::new(),
        }
    }
}

/// 备份恢复导入导出种类枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BRIEKind {
    #[default]
    Backup,
    CancelJob,
    StreamStart,
    StreamMetaData,
    StreamStatus,
    StreamPause,
    StreamResume,
    StreamStop,
    StreamPurge,
    Restore,
    RestorePIT,
    ShowJob,
    ShowQuery,
    ShowBackupMeta,
}

/// 备份恢复导入导出选项结构体。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BRIEOption {
    pub Tp: u16,
    pub UintValue: u64,
    pub StrValue: String,
}

/// 备份恢复导入导出选项RateLIMIT常量。
pub const BRIEOptionRateLimit: u16 = 16;
/// 备份恢复导入导出选项Concurrency常量。
pub const BRIEOptionConcurrency: u16 = 17;
/// 备份恢复导入导出选项Checksum常量。
pub const BRIEOptionChecksum: u16 = 18;
/// 备份恢复导入导出选项SendCreds常量。
pub const BRIEOptionSendCreds: u16 = 19;
/// 备份恢复导入导出选项Checkpoint常量。
pub const BRIEOptionCheckpoint: u16 = 20;
/// 备份恢复导入导出选项StartTS常量。
pub const BRIEOptionStartTS: u16 = 21;
/// 备份恢复导入导出选项UntilTS常量。
pub const BRIEOptionUntilTS: u16 = 22;
/// 备份恢复导入导出选项ChecksumConcurrency常量。
pub const BRIEOptionChecksumConcurrency: u16 = 23;
/// 备份恢复导入导出选项Encryption方法常量。
pub const BRIEOptionEncryptionMethod: u16 = 24;
/// 备份恢复导入导出选项Encryption键File常量。
pub const BRIEOptionEncryptionKeyFile: u16 = 25;
/// 备份恢复导入导出选项Backup时间Ago常量。
pub const BRIEOptionBackupTimeAgo: u16 = 26;
/// 备份恢复导入导出选项BackupTS常量。
pub const BRIEOptionBackupTS: u16 = 27;
/// 备份恢复导入导出选项BackupTSO常量。
pub const BRIEOptionBackupTSO: u16 = 28;
/// 备份恢复导入导出选项LastBackupTS常量。
pub const BRIEOptionLastBackupTS: u16 = 29;
/// 备份恢复导入导出选项LastBackupTSO常量。
pub const BRIEOptionLastBackupTSO: u16 = 30;
/// 备份恢复导入导出选项GCTTL常量。
pub const BRIEOptionGCTTL: u16 = 31;
/// 备份恢复导入导出选项Compression级别常量。
pub const BRIEOptionCompressionLevel: u16 = 32;
/// 备份恢复导入导出选项Compression常量。
pub const BRIEOptionCompression: u16 = 33;
/// 备份恢复导入导出选项Ignore统计信息常量。
pub const BRIEOptionIgnoreStats: u16 = 34;
/// 备份恢复导入导出选项加载统计信息常量。
pub const BRIEOptionLoadStats: u16 = 35;
/// 备份恢复导入导出选项Online常量。
pub const BRIEOptionOnline: u16 = 36;
/// 备份恢复导入导出选项FullBackupStorage常量。
pub const BRIEOptionFullBackupStorage: u16 = 37;
/// 备份恢复导入导出选项RestoredTS常量。
pub const BRIEOptionRestoredTS: u16 = 38;
/// 备份恢复导入导出选项WaitTiflashReady常量。
pub const BRIEOptionWaitTiflashReady: u16 = 39;
/// 备份恢复导入导出选项WITHSys表常量。
pub const BRIEOptionWithSysTable: u16 = 40;
/// 备份恢复导入导出选项ANALYZE常量。
pub const BRIEOptionAnalyze: u16 = 41;
/// 备份恢复导入导出选项Backend常量。
pub const BRIEOptionBackend: u16 = 42;
/// 备份恢复导入导出选项ON重复常量。
pub const BRIEOptionOnDuplicate: u16 = 43;
/// 备份恢复导入导出选项SkipSchemaFiles常量。
pub const BRIEOptionSkipSchemaFiles: u16 = 44;
/// 备份恢复导入导出选项Strict格式常量。
pub const BRIEOptionStrictFormat: u16 = 45;
/// 备份恢复导入导出选项TiKVImporter常量。
pub const BRIEOptionTiKVImporter: u16 = 46;
/// 备份恢复导入导出选项Resume常量。
pub const BRIEOptionResume: u16 = 47;
/// 备份恢复导入导出选项CSVBackslashEscape常量。
pub const BRIEOptionCSVBackslashEscape: u16 = 48;
/// 备份恢复导入导出选项CSVDelimiter常量。
pub const BRIEOptionCSVDelimiter: u16 = 49;
/// 备份恢复导入导出选项CSVHeader常量。
pub const BRIEOptionCSVHeader: u16 = 50;
/// 备份恢复导入导出选项CSVNot空常量。
pub const BRIEOptionCSVNotNull: u16 = 51;
/// 备份恢复导入导出选项CSV空常量。
pub const BRIEOptionCSVNull: u16 = 52;
/// 备份恢复导入导出选项CSVSeparator常量。
pub const BRIEOptionCSVSeparator: u16 = 53;
/// 备份恢复导入导出选项CSVTRIMLastSeparators常量。
pub const BRIEOptionCSVTrimLastSeparators: u16 = 54;
/// BRIECSVHeaderIsColumns常量。
pub const BRIECSVHeaderIsColumns: u64 = u64::MAX;

/// 备份/恢复/导入/导出（BRIE）语句 AST。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BRIEStmt {
    pub node_text: base::AstNode,
    pub Kind: BRIEKind,
    pub Schemas: Vec<String>,
    pub Tables: Vec<TableName>,
    pub Storage: String,
    pub JobID: i64,
    pub Options: Vec<BRIEOption>,
}

/// 用户/系统变量表达式。
pub struct VariableExpr {
    pub node_text: base::AstNode,
    pub is_system: bool,
    pub value: Option<()>,
}
impl VariableExpr {
    pub fn new(is_system: bool, has_value: bool) -> Self {
        Self {
            node_text: Default::default(),
            is_system,
            value: has_value.then_some(()),
        }
    }
}

macro_rules! simple_node {
    ($($name:ty),+ $(,)?) => {$ (
        impl Node for $name {
            fn node_text(&self) -> &base::AstNode { &self.node_text }
            fn node_text_mut(&mut self) -> &mut base::AstNode { &mut self.node_text }
            fn as_any(&self) -> &dyn Any { self }
            fn as_any_mut(&mut self) -> &mut dyn Any { self }
            fn into_any(self: Box<Self>) -> Box<dyn Any> { self }
            fn accept(&self, visitor: &mut dyn Visitor) -> bool {
                if visitor.enter(self) { return visitor.leave(self); }
                walk::Children::visit_children(self, visitor) && visitor.leave(self)
            }
            fn accept_in_place(&mut self, visitor: &mut dyn InPlaceVisitor) -> bool {
                if visitor.enter(self) { return visitor.leave(self); }
                walk::MutChildren::visit_children_mut(self, visitor) && visitor.leave(self)
            }
        }
    )+};
}

simple_node!(
    CreateMaterializedViewStmt,
    CreateMaterializedViewLogStmt,
    AlterMaterializedViewAction,
    AlterMaterializedViewStmt,
    AlterMaterializedViewLogAction,
    AlterMaterializedViewLogStmt,
    DropMaterializedViewStmt,
    DropMaterializedViewLogStmt,
    ExprNode,
    DoStmt,
    CallStmt,
    ShowStmt,
    ExplainStmt,
    ExplainForStmt,
    SetOprSelectList,
    SetOprStmt,
    AdminStmt,
    CleanupTableLockStmt,
    RepairTableStmt,
    TraceStmt,
    BRIEStmt,
    VariableExpr,
    InsertStmt,
    UpdateStmt,
    DeleteStmt,
    CreateTableStmt,
    CreateViewStmt,
    AlterTableStmt,
    DropTableStmt,
    CreateDatabaseStmt,
    DropDatabaseStmt,
    AlterDatabaseStmt,
    AlterInstanceStmt,
    AlterRangeStmt,
    CreateBindingStmt,
    DropBindingStmt,
    SetBindingStmt,
    DistributeTableStmt,
    SplitRegionStmt,
    CancelDistributionJobStmt,
    RenameUserStmt,
    DropUserStmt,
    DropProcedureStmt,
    DropPlacementPolicyStmt,
    DropResourceGroupStmt,
    CreateResourceGroupStmt,
    AlterResourceGroupStmt,
    CreatePlacementPolicyStmt,
    AlterPlacementPolicyStmt,
    DropQueryWatchStmt,
    ImportIntoActionStmt,
    CreateSequenceStmt,
    AlterSequenceStmt,
    DropSequenceStmt,
    TruncateTableStmt,
    RecoverTableStmt,
    FlashBackToTimestampStmt,
    FlashBackTableStmt,
    FlashBackDatabaseStmt,
    CreateIndexStmt,
    DropIndexStmt,
    RenameTableStmt,
    AnalyzeTableStmt,
    CompactTableStmt,
    OptimizeTableStmt,
    KillStmt,
    LoadStatsStmt,
    LockStatsStmt,
    UnlockStatsStmt,
    DropStatsStmt,
    RefreshStatsStmt,
    FlushStmt,
    LockTablesStmt,
    UseStmt,
    SetStmt,
    BeginStmt,
    BinlogStmt,
    CommitStmt,
    DeallocateStmt,
    PrepareStmt,
    ExecuteStmt,
    ShutdownStmt,
    RestartStmt,
    HelpStmt,
    SavepointStmt,
    ReleaseSavepointStmt,
    RecommendIndexStmt,
    PlanReplayerStmt,
    TrafficStmt,
    ProcedureCursor,
    ProcedureErrorControl,
    ProcedureBlock,
    ProcedureIfInfo,
    ProcedureIfBlock,
    ProcedureElseIfBlock,
    ProcedureElseBlock,
    SimpleWhenThenStmt,
    SearchWhenThenStmt,
    SimpleCaseStmt,
    SearchCaseStmt,
    ProcedureWhileStmt,
    ProcedureRepeatStmt,
    ProcedureLabelBlock,
    ProcedureLabelLoop,
    ProcedureJump,
    ProcedureInfo,
    GrantStmt,
    GrantProxyStmt,
    GrantRoleStmt,
    RevokeStmt,
    RevokeRoleStmt,
    LoadDataStmt,
    ImportIntoStmt,
    NonTransactionalDMLStmt,
    CreateMaskingPolicyStmt,
    DropStatisticsStmt,
    CreateStatisticsStmt,
    UnlockTablesStmt,
    SetPwdStmt,
    SetSessionStatesStmt,
    SetConfigStmt,
    SetResourceGroupStmt,
    SetRoleStmt,
    SetDefaultRoleStmt,
    CalibrateResourceStmt,
    AddQueryWatchStmt,
    ProcedureOpenCur,
    ProcedureCloseCur,
    ProcedureFetchInto,
    ProcedureErrorCon,
    ProcedureErrorVal,
    ProcedureErrorState,
    CreateUserStmt,
    AlterUserStmt,
    RollbackStmt
);

/// ast模块。
#[path = "ast.rs"]
pub mod ast;
/// base模块。
#[path = "base.rs"]
pub mod base;
/// ddl模块。
#[path = "ddl.rs"]
pub mod ddl;
/// dml模块。
#[path = "dml.rs"]
pub mod dml;
/// expressions模块。
#[path = "expressions.rs"]
pub mod expressions;
/// 表达式标志位（flag）传播子模块。
#[path = "flag.rs"]
pub mod flag;
/// functions模块。
#[path = "functions.rs"]
pub mod functions;
pub use functions::{
    AggFuncApproxCountDistinct, AggFuncApproxPercentile, AggFuncAvg, AggFuncBitAnd, AggFuncBitOr,
    AggFuncBitXor, AggFuncCount, AggFuncFirstRow, AggFuncGroupConcat, AggFuncJsonArrayagg,
    AggFuncJsonObjectAgg, AggFuncMax, AggFuncMin, AggFuncStddevPop, AggFuncStddevSamp, AggFuncSum,
    AggFuncSumInt, AggFuncVarPop, AggFuncVarSamp, BitNeg, Case, CurrentTimestamp, EQ,
    FTSMysqlMatchAgainst, GE, GT, GetVar, Grouping, If, Ifnull, Ilike, In, IsFalsity, IsNull,
    IsTruthWithoutNull, LE, LT, Like, LogicAnd, Minus, NE, NullEQ, Nullif, Regexp, RowFunc, SetVar,
    UnaryMinus, UnaryNot, UnixTimestamp,
};
/// misc模块。
#[path = "misc.rs"]
pub mod misc;
/// model模块。
#[path = "model.rs"]
pub mod model;
/// procedure模块。
#[path = "procedure.rs"]
pub mod procedure;
/// sem模块。
#[path = "sem.rs"]
pub mod sem;
/// stats模块。
#[path = "stats.rs"]
pub mod stats;
/// util模块。
#[path = "util.rs"]
pub mod util;

#[cfg(test)]
#[path = "ast_1_aster_unit_test.rs"]
mod ast_1_aster_unit_test;
#[cfg(test)]
#[path = "base_test.rs"]
mod base_test;
#[cfg(test)]
#[path = "canonical_contract_test.rs"]
mod canonical_contract_test;
#[cfg(test)]
#[path = "ddl_2_aster_unit_test.rs"]
mod ddl_2_aster_unit_test;
#[cfg(test)]
#[path = "ddl_test.rs"]
mod ddl_test;
#[cfg(test)]
#[path = "dml_3_aster_unit_test.rs"]
mod dml_3_aster_unit_test;
#[cfg(test)]
#[path = "dml_test.rs"]
mod dml_test;
#[cfg(test)]
#[path = "expressions_4_aster_unit_test.rs"]
mod expressions_4_aster_unit_test;
#[cfg(test)]
#[path = "expressions_test.rs"]
mod expressions_test;
#[cfg(test)]
#[path = "flag_5_aster_unit_test.rs"]
mod flag_5_aster_unit_test;
#[cfg(test)]
#[path = "flag_test.rs"]
mod flag_test;
#[cfg(test)]
#[path = "format_test.rs"]
mod format_test;
#[cfg(test)]
#[path = "functions_test.rs"]
mod functions_test;
#[cfg(test)]
#[path = "integration_9_aster_unit_test.rs"]
mod integration_9_aster_unit_test;
#[cfg(test)]
#[path = "misc_6_aster_unit_test.rs"]
mod misc_6_aster_unit_test;
#[cfg(test)]
#[path = "misc_test.rs"]
mod misc_test;
#[cfg(test)]
#[path = "model_7_aster_unit_test.rs"]
mod model_7_aster_unit_test;
#[cfg(test)]
#[path = "model_test.rs"]
mod model_test;
#[cfg(test)]
#[path = "planner_expr_contract_11_aster_unit_test.rs"]
mod planner_expr_contract_11_aster_unit_test;
#[cfg(test)]
#[path = "procedure_test.rs"]
mod procedure_test;
#[cfg(test)]
#[path = "sem_test.rs"]
mod sem_test;
#[cfg(test)]
#[path = "stats_test.rs"]
mod stats_test;
#[cfg(test)]
#[path = "typed_expr_10_aster_unit_test.rs"]
mod typed_expr_10_aster_unit_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
#[cfg(test)]
#[path = "util_tests.rs"]
mod util_tests;

mod node_flags;
mod walk;
pub use node_flags::{HasAggFlag, HasWindowFlag, SetFlag};
#[cfg(test)]
mod materialized_test;
#[cfg(test)]
mod node_flags_test;
