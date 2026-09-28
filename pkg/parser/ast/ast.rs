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

// SQL 抽象语法树（AST）核心接口与语句标签。
//
// AST 是解析器将 SQL 文本转换成的树形结构。本模块定义 Node/Expr/Stmt 等 trait、
// 表达式标志位，以及 `GetStmtLabel`（按语句种类生成监控/日志用标签，与 Go 类型 switch 对齐）。
use std::io::Write;

pub use crate::{Node, Visitor};
pub use parser_types::types::FieldType;

/// 表达式标志：字面常量（无特殊子结构）。
pub const FlagConstant: u64 = 0;
/// 含预处理参数占位符 `?`。
pub const FlagHasParamMarker: u64 = 1 << 0;
/// 含函数调用。
pub const FlagHasFunc: u64 = 1 << 1;
/// 含列/表等引用。
pub const FlagHasReference: u64 = 1 << 2;
/// 含聚合函数（如 SUM/COUNT）。
pub const FlagHasAggregateFunc: u64 = 1 << 3;
/// 含子查询。
pub const FlagHasSubquery: u64 = 1 << 4;
/// 含用户/系统变量。
pub const FlagHasVariable: u64 = 1 << 5;
/// 含 DEFAULT 表达式。
pub const FlagHasDefault: u64 = 1 << 6;
/// 已预求值。
pub const FlagPreEvaluated: u64 = 1 << 7;
/// 含窗口函数。
pub const FlagHasWindowFunc: u64 = 1 << 8;

/// 表达式节点：在 Node 之上增加类型（FieldType）与标志位。
pub trait ExprNode: Node {
    fn SetType(&mut self, field_type: FieldType);
    fn GetType(&self) -> &FieldType;
    fn SetFlag(&mut self, flag: u64);
    fn GetFlag(&self) -> u64;
    fn Format(&self, output: &mut dyn Write) -> std::io::Result<()>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 可选 BINARY 字符集修饰：是否 binary 及字符集名。
pub struct OptBinary {
    pub IsBinary: bool,
    pub Charset: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 向量元素类型（单字节 MySQL 类型编号）。
pub struct VectorElementType {
    pub Tp: u8,
}

/// 函数表达式节点标记 trait。
pub trait FuncNode: ExprNode {
    fn functionExpression(&self);
}

/// 语句节点；SEMCommand 返回语义命令名。
pub trait StmtNode: Node {
    fn statement(&self);
    fn SEMCommand(&self) -> String;
}

/// DDL（数据定义语言，如 CREATE/ALTER/DROP）语句节点。
pub trait DDLNode: StmtNode {
    fn ddlStatement(&self);
}

/// DML（数据操纵语言，如 SELECT/INSERT/UPDATE/DELETE）语句节点。
pub trait DMLNode: StmtNode {
    fn dmlStatement(&self);
}

/// 可产生结果集的节点（如表引用、子查询）。
pub trait ResultSetNode: Node {
    fn resultSet(&self);
}

/// 含敏感信息的语句（如密码），提供脱敏 SecureText。
pub trait SensitiveStmtNode: StmtNode {
    fn SecureText(&self) -> String;
}

/// 供 GetStmtLabel 区分语句种类的具体枚举，对应 Go 类型 switch。
/// Concrete statement distinctions used by GetStmtLabel's Go type switch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementKind {
    AlterTable,
    AnalyzeTable,
    Begin,
    Commit,
    CompactTable,
    CreateDatabase,
    CreateIndex,
    CreateTable,
    CreateView,
    CreateUser,
    Delete,
    DropDatabase,
    DropIndex,
    DropTable { is_view: bool },
    Explain { show: bool, analyze: bool },
    Insert { is_replace: bool },
    ImportInto,
    LoadData,
    Rollback,
    Select,
    Set,
    SetPassword,
    Show,
    TruncateTable,
    Update,
    Grant,
    Revoke,
    Deallocate,
    Execute,
    Prepare,
    Use,
    CreateBinding,
    DropBinding,
    Trace,
    Shutdown,
    Savepoint,
    OptimizeTable,
    Other,
}

/// 生成与 Go 相同的语句标签，并遵守相同的特例优先级（如 DropView、DescTable、Replace）。
/// Generates the same labels and observes the same special-case priority as Go.
pub fn GetStmtLabel(statement: &StatementKind) -> String {
    let label = match statement {
        StatementKind::AlterTable => "AlterTable",
        StatementKind::AnalyzeTable => "AnalyzeTable",
        StatementKind::Begin => "Begin",
        StatementKind::Commit => "Commit",
        StatementKind::CompactTable => "CompactTable",
        StatementKind::CreateDatabase => "CreateDatabase",
        StatementKind::CreateIndex => "CreateIndex",
        StatementKind::CreateTable => "CreateTable",
        StatementKind::CreateView => "CreateView",
        StatementKind::CreateUser => "CreateUser",
        StatementKind::Delete => "Delete",
        StatementKind::DropDatabase => "DropDatabase",
        StatementKind::DropIndex => "DropIndex",
        StatementKind::DropTable { is_view: true } => "DropView",
        StatementKind::DropTable { is_view: false } => "DropTable",
        StatementKind::Explain { show: true, .. } => "DescTable",
        StatementKind::Explain { analyze: true, .. } => "ExplainAnalyzeSQL",
        StatementKind::Explain { .. } => "ExplainSQL",
        StatementKind::Insert { is_replace: true } => "Replace",
        StatementKind::Insert { is_replace: false } => "Insert",
        StatementKind::ImportInto => "ImportInto",
        StatementKind::LoadData => "LoadData",
        StatementKind::Rollback => "Rollback",
        StatementKind::Select => "Select",
        StatementKind::Set | StatementKind::SetPassword => "Set",
        StatementKind::Show => "Show",
        StatementKind::TruncateTable => "TruncateTable",
        StatementKind::Update => "Update",
        StatementKind::Grant => "Grant",
        StatementKind::Revoke => "Revoke",
        StatementKind::Deallocate => "Deallocate",
        StatementKind::Execute => "Execute",
        StatementKind::Prepare => "Prepare",
        StatementKind::Use => "Use",
        StatementKind::CreateBinding => "CreateBinding",
        StatementKind::DropBinding => "DropBinding",
        StatementKind::Trace => "Trace",
        StatementKind::Shutdown => "Shutdown",
        StatementKind::Savepoint => "Savepoint",
        StatementKind::OptimizeTable => "Optimize",
        StatementKind::Other => "other",
    };
    label.to_owned()
}
