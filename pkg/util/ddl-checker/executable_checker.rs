// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// SQL 可执行性检查器：在隔离 session 中解析并试跑语句，判断 DDL 依赖表是否满足。
//
// 对应 Go `executable_checker.go`。用于同步/迁移前预检：解析 AST、收集
// 「必须已存在 / 必须不存在」的表名，并可实际 Execute。DDL 指数据定义语言。

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// 检查执行上下文：请求标识与取消标志（对齐 Go context 的最小子集）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionContext {
    /// 请求/追踪 ID。
    pub request_id: String,
    /// 是否已取消。
    pub cancelled: bool,
}

impl ExecutionContext {
    /// 返回默认后台上下文（无请求 ID、未取消）。
    pub fn background() -> Self {
        Self::default()
    }
}

/// 检查器错误：消息加可选底层 cause，实现 `std::error::Error`。
#[derive(Debug)]
pub struct CheckerError {
    /// 面向调用方的错误文案。
    message: String,
    /// 可选的底层错误链。
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl CheckerError {
    /// 仅带消息的错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// 包装任意 Error，消息取 `to_string()`，并保留 source。
    pub fn trace(error: impl Error + Send + Sync + 'static) -> Self {
        Self {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }

    /// 返回错误消息文本。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CheckerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CheckerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// 检查器统一 Result 别名。
pub type CheckerResult<T> = Result<T, CheckerError>;

/// RENAME TABLE 的一对旧表名 → 新表名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenameTablePair {
    /// 重命名前的表名。
    pub old_table: String,
    /// 重命名后的表名。
    pub new_table: String,
}

/// 检查器关心的语句分类（由 AST 映射而来），驱动表依赖分析。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Statement {
    /// TRUNCATE TABLE，依赖表已存在。
    TruncateTable { table: String },
    /// CREATE INDEX，依赖表已存在。
    CreateIndex { table: String },
    /// DROP TABLE，依赖所列表面已存在。
    DropTable { tables: Vec<String> },
    /// DROP INDEX，依赖表已存在。
    DropIndex { table: String },
    /// ALTER TABLE，依赖表已存在。
    AlterTable { table: String },
    /// RENAME TABLE，旧名须存在、新名须不存在。
    RenameTable { pairs: Vec<RenameTablePair> },
    /// CREATE TABLE，新表名须不存在。
    CreateTable { table: String },
    /// 其它 DDL（库级/视图等），不贡献表名列表。
    OtherDdl,
    /// 非 DDL（DML/查询等）。
    NonDdl,
}

/// 可执行 SQL 的 session 适配：执行、字符集信息、关闭。
pub trait CheckerSession: Send {
    /// 在给定上下文执行一条 SQL。
    fn execute(&mut self, context: &ExecutionContext, sql: &str) -> CheckerResult<()>;
    /// 返回 (charset, collation)。
    fn charset_info(&self) -> (String, String);
    /// 关闭 session。
    fn close(&mut self);
}

/// SQL 解析适配：按 charset/collation 解析单条语句为 `Statement`。
pub trait CheckerParser: Send {
    fn parse_one_statement(
        &mut self,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> CheckerResult<Statement>;
}

/// Builds the same logger/mock-store/bootstrap/session/parser stack as the Go constructor.
/// All operations are required; production implementations cannot silently succeed.
/// 组装与 Go 构造函数相同的 logger/session/parser 栈；任一步失败即返回错误。
pub trait ExecutableCheckerFactory {
    /// 初始化错误级别日志。
    fn initialize_error_logger(&self) -> CheckerResult<()>;
    /// 创建已 bootstrap 的检查 session。
    fn create_bootstrapped_session(&self) -> CheckerResult<Box<dyn CheckerSession>>;
    /// 创建语句解析器。
    fn create_parser(&self) -> CheckerResult<Box<dyn CheckerParser>>;
}

// ExecutableChecker is a part of TiDB to check the SQL's executability.
/// 持有 session 与 parser，提供 Execute / 表存在探测 / Parse / 一次性 Close。
pub struct ExecutableChecker {
    /// 执行 SQL 的会话。
    session: Box<dyn CheckerSession>,
    /// 单语句解析器。
    parser: Box<dyn CheckerParser>,
    /// 是否已关闭（CAS 保证 Close 只成功一次）。
    isClosed: AtomicBool,
}

// NewExecutableChecker creates a new ExecutableChecker.
/// 经 factory 初始化 logger、session、parser 后构造检查器。
pub fn NewExecutableChecker(
    factory: &dyn ExecutableCheckerFactory,
) -> CheckerResult<ExecutableChecker> {
    factory.initialize_error_logger()?;
    let session = factory.create_bootstrapped_session()?;
    let parser = factory.create_parser()?;
    Ok(ExecutableChecker {
        session,
        parser,
        isClosed: AtomicBool::new(false),
    })
}

impl ExecutableChecker {
    /// 由已有 session/parser 组装，跳过 factory（测试注入用）。
    pub fn from_parts(session: Box<dyn CheckerSession>, parser: Box<dyn CheckerParser>) -> Self {
        Self {
            session,
            parser,
            isClosed: AtomicBool::new(false),
        }
    }

    // Execute executes the SQL to check its executability.
    /// 在 session 中执行 SQL，用于验证可执行性。
    pub fn Execute(&mut self, context: &ExecutionContext, sql: &str) -> CheckerResult<()> {
        self.session.execute(context, sql)
    }

    // IsTableExist returns whether the table with the specified name exists.
    /// 通过 `select 0 from \`table\` limit 1` 探测表是否存在。
    pub fn IsTableExist(&mut self, context: &ExecutionContext, tableName: &str) -> bool {
        self.session
            .execute(context, &format!("select 0 from `{tableName}` limit 1"))
            .is_ok()
    }

    // CreateTable creates a new table with the specified SQL.
    /// 执行建表 SQL（委托 Execute）。
    pub fn CreateTable(&mut self, context: &ExecutionContext, sql: &str) -> CheckerResult<()> {
        self.Execute(context, sql)
    }

    // DropTable drops the specified table.
    /// 执行 `drop table if exists` 删除指定表。
    pub fn DropTable(&mut self, context: &ExecutionContext, tableName: &str) -> CheckerResult<()> {
        self.Execute(context, &format!("drop table if exists `{tableName}`"))
    }

    // Close closes the ExecutableChecker exactly once.
    /// 原子地关闭一次：重复 Close 返回 already closed 错误。
    pub fn Close(&mut self) -> CheckerResult<()> {
        if self
            .isClosed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(CheckerError::new("ExecutableChecker is already closed"));
        }
        self.session.close();
        Ok(())
    }

    // Parse parses one query with the session charset and collation.
    /// 用 session 的 charset/collation 解析单条 SQL 为 `Statement`。
    pub fn Parse(&mut self, sql: &str) -> CheckerResult<Statement> {
        let (charset, collation) = self.session.charset_info();
        self.parser.parse_one_statement(sql, &charset, &collation)
    }

    /// 返回检查器是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.isClosed.load(Ordering::Acquire)
    }
}

// GetTablesNeededExist reports the table names that must already exist.
/// 返回语句执行前必须已存在的表名列表；非 DDL 返回错误。
pub fn GetTablesNeededExist(stmt: &Statement) -> CheckerResult<Vec<String>> {
    match stmt {
        Statement::TruncateTable { table }
        | Statement::CreateIndex { table }
        | Statement::DropIndex { table }
        | Statement::AlterTable { table } => Ok(vec![table.clone()]),
        Statement::DropTable { tables } => Ok(tables.clone()),
        // RENAME 只取第一对旧表名，对齐 Go 行为。
        Statement::RenameTable { pairs } => pairs
            .first()
            .map(|pair| vec![pair.old_table.clone()])
            .ok_or_else(|| CheckerError::new("rename table statement contains no table pair")),
        Statement::CreateTable { .. } | Statement::OtherDdl => Ok(Vec::new()),
        Statement::NonDdl => Err(CheckerError::new("stmt is not a DDLNode")),
    }
}

// GetTablesNeededNonExist reports the table names that must not already exist.
/// 返回语句执行前必须尚不存在的表名列表；非 DDL 返回错误。
pub fn GetTablesNeededNonExist(stmt: &Statement) -> CheckerResult<Vec<String>> {
    match stmt {
        Statement::CreateTable { table } => Ok(vec![table.clone()]),
        // RENAME 只取第一对新表名，对齐 Go 行为。
        Statement::RenameTable { pairs } => pairs
            .first()
            .map(|pair| vec![pair.new_table.clone()])
            .ok_or_else(|| CheckerError::new("rename table statement contains no table pair")),
        Statement::TruncateTable { .. }
        | Statement::CreateIndex { .. }
        | Statement::DropTable { .. }
        | Statement::DropIndex { .. }
        | Statement::AlterTable { .. }
        | Statement::OtherDdl => Ok(Vec::new()),
        Statement::NonDdl => Err(CheckerError::new("stmt is not a DDLNode")),
    }
}

// IsDDL reports whether the statement is a DDL node.
/// 判断语句是否为 DDL（非 `NonDdl`）。
pub fn IsDDL(stmt: &Statement) -> bool {
    !matches!(stmt, Statement::NonDdl)
}

/// Maps a parsed AST node onto the checker `Statement` enum used by
/// `GetTablesNeededExist` / `GetTablesNeededNonExist`, preserving the Go type-switch cases.
/// 将解析后的 AST 节点映射为检查器用的 `Statement` 枚举，分支与 Go type-switch 对齐。
pub fn StatementFromAst(node: &dyn astersql_parser_ast::Node) -> Statement {
    use astersql_parser_ast::{
        AlterTableStmt, CreateIndexStmt, CreateTableStmt, DropIndexStmt, DropTableStmt,
        RenameTableStmt, TruncateTableStmt,
    };

    // 按具体 DDL 语句类型依次 downcast，提取表名后返回对应变体。
    if let Some(stmt) = node.as_any().downcast_ref::<TruncateTableStmt>() {
        return Statement::TruncateTable {
            table: stmt.Table.Name.O.clone(),
        };
    }
    if let Some(stmt) = node.as_any().downcast_ref::<CreateIndexStmt>() {
        return Statement::CreateIndex {
            table: stmt.Table.Name.O.clone(),
        };
    }
    if let Some(stmt) = node.as_any().downcast_ref::<DropTableStmt>() {
        return Statement::DropTable {
            tables: stmt
                .Tables
                .iter()
                .map(|table| table.Name.O.clone())
                .collect(),
        };
    }
    if let Some(stmt) = node.as_any().downcast_ref::<DropIndexStmt>() {
        return Statement::DropIndex {
            table: stmt.Table.Name.O.clone(),
        };
    }
    if let Some(stmt) = node.as_any().downcast_ref::<AlterTableStmt>() {
        return Statement::AlterTable {
            table: stmt.Table.Name.O.clone(),
        };
    }
    if let Some(stmt) = node.as_any().downcast_ref::<RenameTableStmt>() {
        return Statement::RenameTable {
            pairs: stmt
                .TableToTables
                .iter()
                .map(|pair| RenameTablePair {
                    old_table: pair.OldTable.Name.O.clone(),
                    new_table: pair.NewTable.Name.O.clone(),
                })
                .collect(),
        };
    }
    if let Some(stmt) = node.as_any().downcast_ref::<CreateTableStmt>() {
        return Statement::CreateTable {
            table: stmt.Table.Name.O.clone(),
        };
    }

    // Remaining DDL nodes (DropDatabase, CreateDatabase, ...) match Go's
    // `case ast.DDLNode` branch and contribute empty table lists.
    if is_ddl_ast(node) {
        return Statement::OtherDdl;
    }
    Statement::NonDdl
}

/// 判断 AST 是否属于不单独提取表名的其它 DDL 节点（库/视图/序列/Placement 等）。
fn is_ddl_ast(node: &dyn astersql_parser_ast::Node) -> bool {
    use astersql_parser_ast::{
        AlterDatabaseStmt, AlterPlacementPolicyStmt, AlterResourceGroupStmt, AlterSequenceStmt,
        CleanupTableLockStmt, CreateDatabaseStmt, CreateMaskingPolicyStmt,
        CreatePlacementPolicyStmt, CreateResourceGroupStmt, CreateSequenceStmt, CreateViewStmt,
        DropDatabaseStmt, DropPlacementPolicyStmt, DropResourceGroupStmt, DropSequenceStmt,
        FlashBackDatabaseStmt, FlashBackTableStmt, FlashBackToTimestampStmt, LockTablesStmt,
        OptimizeTableStmt, RecoverTableStmt, RepairTableStmt, UnlockTablesStmt,
    };

    node.as_any().is::<CreateDatabaseStmt>()
        || node.as_any().is::<AlterDatabaseStmt>()
        || node.as_any().is::<DropDatabaseStmt>()
        || node.as_any().is::<CreateViewStmt>()
        || node.as_any().is::<CreateSequenceStmt>()
        || node.as_any().is::<CreateMaskingPolicyStmt>()
        || node.as_any().is::<AlterSequenceStmt>()
        || node.as_any().is::<DropSequenceStmt>()
        || node.as_any().is::<CreatePlacementPolicyStmt>()
        || node.as_any().is::<AlterPlacementPolicyStmt>()
        || node.as_any().is::<DropPlacementPolicyStmt>()
        || node.as_any().is::<CreateResourceGroupStmt>()
        || node.as_any().is::<AlterResourceGroupStmt>()
        || node.as_any().is::<DropResourceGroupStmt>()
        || node.as_any().is::<FlashBackDatabaseStmt>()
        || node.as_any().is::<FlashBackTableStmt>()
        || node.as_any().is::<FlashBackToTimestampStmt>()
        || node.as_any().is::<RecoverTableStmt>()
        || node.as_any().is::<LockTablesStmt>()
        || node.as_any().is::<UnlockTablesStmt>()
        || node.as_any().is::<CleanupTableLockStmt>()
        || node.as_any().is::<OptimizeTableStmt>()
        || node.as_any().is::<RepairTableStmt>()
}
