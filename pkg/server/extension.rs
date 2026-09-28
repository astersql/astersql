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

// Server 扩展事件钩子：连接生命周期与语句结束回调。
//
// 在连接握手/断开、语句成功/失败、SQL 解析失败及二进制 EXECUTE
// 结束时，向已注册的 `ExtensionListeners` 派发事件，供审计与观测使用。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 扩展层简易错误消息包装。
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 连接标识：连接 ID 与客户端主机。
pub struct ConnectionInfo {
    pub connection_id: u64,
    pub client_host: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 用户身份：用户名与主机名。
pub struct UserIdentity {
    pub username: String,
    pub hostname: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 相关表条目：库名 + 表名。
pub struct TableEntry {
    pub db: String,
    pub table: String,
}

#[derive(Clone, Debug, PartialEq)]
/// 计划缓存 / 预处理参数的数据单元格。
pub enum Datum {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Signed(i64),
    /// 无符号整数。
    Unsigned(u64),
    /// 浮点值。
    Float(f64),
    /// 字节/字符串载荷。
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 扩展可见的语句节点：二进制执行、USE 或普通 SQL。
pub enum Statement {
    /// 二进制预处理执行（COM_STMT_EXECUTE）。
    Execute { id: u32 },
    /// 切换当前库。
    Use { database: String },
    /// 普通 SQL 文本及其相关表。
    Sql {
        text: String,
        related_tables: Vec<TableEntry>,
    },
}

impl Statement {
    /// 将语句节点渲染为可展示文本。
    fn text(&self) -> String {
        match self {
            Self::Execute { id } => binaryExecuteStmtText(*id),
            Self::Use { database } => format!("USE `{}`", database.replace('`', "``")),
            Self::Sql { text, .. } => text.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 语句执行上下文：原始/归一化 SQL、digest、影响行与相关表。
pub struct StatementContext {
    pub original_sql: String,
    pub normalized_sql: String,
    pub digest: Option<String>,
    pub affected_rows: u64,
    pub tables: Vec<TableEntry>,
}

#[derive(Clone, Debug)]
/// 预处理语句缓存元数据。
pub struct PreparedMeta {
    pub statement: Statement,
    pub normalized_sql: String,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Default)]
/// 会话变量快照：连接信息、角色、库名、预处理与语句上下文。
pub struct SessionVars {
    pub connection_info: Option<ConnectionInfo>,
    pub session_alias: String,
    pub user: Option<UserIdentity>,
    pub active_roles: Vec<String>,
    pub current_db: String,
    pub prepared: HashMap<u32, PreparedMeta>,
    pub plan_cache_params: Vec<Datum>,
    pub stmt_context: StatementContext,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 连接事件类型（连通、断开、握手接受/拒绝）。
pub enum ConnEventTp {
    /// 连接已建立。
    Connected,
    /// 连接已断开。
    Disconnected,
    /// 握手成功接受。
    HandshakeAccepted,
    /// 握手被拒绝。
    HandshakeRejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 语句事件类型：成功或错误。
pub enum StmtEventTp {
    /// 语句执行成功。
    StmtSuccess,
    /// 语句执行失败。
    StmtError,
}

#[derive(Clone, Debug)]
/// 派发给扩展的连接事件载荷。
pub struct ConnEventInfo {
    pub connection_info: ConnectionInfo,
    pub session_alias: String,
    pub active_roles: Vec<String>,
    pub error: Option<Error>,
}

/// 扩展监听器：连接事件与语句事件回调。
pub trait ExtensionListeners: Send + Sync {
    fn has_stmt_event_listeners(&self) -> bool;
    fn on_connection_event(&self, event: ConnEventTp, info: ConnEventInfo);
    fn on_stmt_event(&self, event: StmtEventTp, info: stmtEventInfo);
}

/// 客户端连接视图：连接信息、会话变量与扩展监听器。
pub struct ClientConn {
    pub connection_info: ConnectionInfo,
    pub session_vars: Option<SessionVars>,
    pub extensions: Option<Arc<dyn ExtensionListeners>>,
}

/// 派发连接事件；无扩展时直接返回。
pub fn onExtensionConnEvent(cc: &ClientConn, event: ConnEventTp, error: Option<Error>) {
    // 未注册扩展时跳过派发。
    let Some(extensions) = &cc.extensions else {
        return;
    };
    let vars = cc.session_vars.as_ref();
    extensions.on_connection_event(
        event,
        ConnEventInfo {
            connection_info: vars
                .and_then(|value| value.connection_info.clone())
                .unwrap_or_else(|| cc.connection_info.clone()),
            session_alias: vars
                .map(|value| value.session_alias.clone())
                .unwrap_or_default(),
            active_roles: vars
                .map(|value| value.active_roles.clone())
                .unwrap_or_default(),
            error,
        },
    );
}

/// 语句结束时派发 StmtSuccess/StmtError；可附带预处理参数。
pub fn onExtensionStmtEnd(
    cc: &ClientConn,
    node: Statement,
    stmt_context_valid: bool,
    error: Option<Error>,
    prepared_params: Vec<Datum>,
) {
    // 未注册扩展时跳过派发。
    let Some(extensions) = &cc.extensions else {
        return;
    };
    // 无语句监听器则避免构造载荷。
    if !extensions.has_stmt_event_listeners() {
        return;
    }
    let Some(vars) = cc.session_vars.clone() else {
        return;
    };
    // Execute 节点携带语句 ID；其它节点记为 0。
    let execute_stmt_id = match node {
        Statement::Execute { id } => id,
        _ => 0,
    };
    let mut vars = vars;
    // 将本次实参写入会话 plan_cache_params 供监听器读取。
    if !prepared_params.is_empty() {
        vars.plan_cache_params = prepared_params;
    }
    // 无效上下文时传空 StatementContext，避免泄漏陈旧字段。
    let stmt_context = if stmt_context_valid {
        vars.stmt_context.clone()
    } else {
        StatementContext::default()
    };
    extensions.on_stmt_event(
        if error.is_some() {
            StmtEventTp::StmtError
        } else {
            StmtEventTp::StmtSuccess
        },
        stmtEventInfo {
            sess_vars: vars,
            sc: stmt_context,
            stmt_node: Some(node),
            execute_stmt_id,
            error,
            failed_parse_text: String::new(),
        },
    );
}

/// SQL 解析失败时派发 StmtError，并记录 failed_parse_text。
pub fn onExtensionSQLParseFailed(cc: &ClientConn, sql: String, error: Error) {
    // 未注册扩展时跳过派发。
    let Some(extensions) = &cc.extensions else {
        return;
    };
    // 无语句监听器则避免构造载荷。
    if !extensions.has_stmt_event_listeners() {
        return;
    }
    let Some(vars) = cc.session_vars.clone() else {
        return;
    };
    extensions.on_stmt_event(
        StmtEventTp::StmtError,
        stmtEventInfo {
            sess_vars: vars,
            sc: StatementContext::default(),
            stmt_node: None,
            execute_stmt_id: 0,
            error: Some(error),
            failed_parse_text: sql,
        },
    );
}

/// 二进制 COM_STMT_EXECUTE 结束：封装为 Execute 节点后复用语句结束钩子。
pub fn onExtensionBinaryExecuteEnd(
    cc: &ClientConn,
    statement_id: u32,
    args: Vec<Datum>,
    stmt_context_valid: bool,
    error: Option<Error>,
) {
    onExtensionStmtEnd(
        cc,
        Statement::Execute { id: statement_id },
        stmt_context_valid,
        error,
        args,
    );
}

#[allow(non_camel_case_types)]
#[derive(Clone, Debug)]
/// 语句事件载荷（保持 Go 侧非驼峰命名）。
pub struct stmtEventInfo {
    sess_vars: SessionVars,
    sc: StatementContext,
    stmt_node: Option<Statement>,
    execute_stmt_id: u32,
    error: Option<Error>,
    failed_parse_text: String,
}

impl stmtEventInfo {
    /// 按 execute_stmt_id 查找预处理缓存元数据。
    pub fn ensureExecutePreparedCache(&self) -> Option<&PreparedMeta> {
        self.sess_vars.prepared.get(&self.execute_stmt_id)
    }

    /// 优先返回语句上下文原始 SQL，否则回退到预处理或节点文本。
    pub fn ensureStmtContextOriginalSQL(&self) -> String {
        // 优先使用语句上下文中的原始 SQL。
        if !self.sc.original_sql.is_empty() {
            return self.sc.original_sql.clone();
        }
        self.ensureExecutePreparedCache()
            .map(|prepared| prepared.statement.text())
            .or_else(|| self.stmt_node.as_ref().map(Statement::text))
            .unwrap_or_default()
    }

    /// 会话中的连接信息。
    pub fn ConnectionInfo(&self) -> Option<&ConnectionInfo> {
        self.sess_vars.connection_info.as_ref()
    }
    /// 会话别名。
    pub fn SessionAlias(&self) -> &str {
        &self.sess_vars.session_alias
    }
    /// 当前语句节点。
    pub fn StmtNode(&self) -> Option<&Statement> {
        self.stmt_node.as_ref()
    }
    /// 若节点是 Execute 则返回，否则 None。
    pub fn ExecuteStmtNode(&self) -> Option<&Statement> {
        self.stmt_node
            .as_ref()
            .filter(|node| matches!(node, Statement::Execute { .. }))
    }
    /// 按 execute_stmt_id 取预处理语句节点。
    pub fn ExecutePreparedStmt(&self) -> Option<&Statement> {
        self.sess_vars
            .prepared
            .get(&self.execute_stmt_id)
            .map(|prepared| &prepared.statement)
    }
    /// 计划缓存参数（预处理实参）。
    pub fn PreparedParams(&self) -> &[Datum] {
        &self.sess_vars.plan_cache_params
    }
    /// 尽力还原原始语句文本（上下文 / 预处理 / 节点 / 解析失败文本）。
    pub fn OriginalText(&self) -> String {
        let original = self.ensureStmtContextOriginalSQL();
        if !original.is_empty() {
            return original;
        }
        if self.execute_stmt_id != 0 {
            return binaryExecuteStmtText(self.execute_stmt_id);
        }
        self.stmt_node
            .as_ref()
            .map(Statement::text)
            .unwrap_or_else(|| self.failed_parse_text.clone())
    }
    /// 返回归一化 SQL 与 digest；无上下文时回退到预处理或原文。
    pub fn SQLDigest(&self) -> (String, Option<String>) {
        if !self.sc.original_sql.is_empty() {
            return (self.sc.normalized_sql.clone(), self.sc.digest.clone());
        }
        if let Some(prepared) = self.sess_vars.prepared.get(&self.execute_stmt_id) {
            return (prepared.normalized_sql.clone(), prepared.digest.clone());
        }
        if self.execute_stmt_id != 0 {
            return (binaryExecuteStmtText(self.execute_stmt_id), None);
        }
        if !self.ensureStmtContextOriginalSQL().is_empty() {
            return (self.sc.normalized_sql.clone(), self.sc.digest.clone());
        }
        (self.OriginalText(), None)
    }
    /// 当前用户身份。
    pub fn User(&self) -> Option<&UserIdentity> {
        self.sess_vars.user.as_ref()
    }
    /// 当前激活角色列表。
    pub fn ActiveRoles(&self) -> &[String] {
        &self.sess_vars.active_roles
    }
    /// 当前数据库名。
    pub fn CurrentDB(&self) -> &str {
        &self.sess_vars.current_db
    }
    /// 影响行数；出错时返回 0。
    pub fn AffectedRows(&self) -> u64 {
        // 出错时协议侧影响行数视为 0。
        if self.error.is_some() {
            0
        } else {
            self.sc.affected_rows
        }
    }
    /// 相关表：USE 返回库条目，否则用上下文或 SQL 节点中的表并补全库名。
    pub fn RelatedTables(&self) -> Vec<TableEntry> {
        // USE 语句：相关“表”仅表示目标库。
        if let Some(Statement::Use { database }) = &self.stmt_node {
            return vec![TableEntry {
                db: database.clone(),
                table: String::new(),
            }];
        }
        if self.error.is_none() {
            return self.sc.tables.clone();
        }
        match &self.stmt_node {
            Some(Statement::Sql { related_tables, .. }) => related_tables
                .iter()
                .cloned()
                .map(|mut table| {
                    // 缺省库名时回填当前库。
                    if table.db.is_empty() {
                        table.db = self.sess_vars.current_db.clone();
                    }
                    table
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    /// 事件关联错误。
    pub fn GetError(&self) -> Option<&Error> {
        self.error.as_ref()
    }
}

/// 二进制 EXECUTE 的占位展示文本。
pub fn binaryExecuteStmtText(id: u32) -> String {
    format!("BINARY EXECUTE (ID {id})")
}
