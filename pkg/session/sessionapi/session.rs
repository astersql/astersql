// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 客户端连接级 `Session` trait 定义。
//
// 对应 Go 侧一条客户端连接的完整生命周期 API：SQL 执行、事务（提交/回滚）、
// 鉴权、预编译语句、进程信息与会话状态迁移等。

use crate::{
    ast, auth, conn, extension, resolve, sessionctx, sessionstates, sessmgr, sqlexec, txninfo,
};
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

/// 匹配用户身份失败时的内部错误类型。
#[derive(Debug)]
struct IdentityNotFound;

impl std::fmt::Display for IdentityNotFound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("identity not found")
    }
}

impl std::error::Error for IdentityNotFound {}

/// 全局「身份未找到」错误，对应 Go 的 `ErrIdentityNotFound`。
pub static ErrIdentityNotFound: LazyLock<sessionctx::GoError> =
    LazyLock::new(|| Box::new(IdentityNotFound));

/// Session has the lifecycle of one client connection and retains the full Go
/// execution, transaction, authentication and connection-state API surface.
///
/// 单条客户端连接的会话接口：覆盖执行、事务、鉴权与连接状态。
pub trait Session: sessionctx::Context {
    /// Bound by the formal expression crate when its root package is canonical.
    /// 预编译语句参数表达式类型（由表达式 crate 绑定）。
    type PreparedExpression;
    /// 鉴权过程使用的连接抽象。
    type AuthConnection: conn::AuthConn<Context = sessionctx::ExecutionContext>;
    /// 会话状态导入/导出处理器。
    type StateHandler: sessionctx::SessionStatesHandler<SessionContext = Self, SessionStates = Self::SessionStates>;

    /// 返回当前会话状态标志位（如是否在事务中、autocommit 等）。
    fn Status(&self) -> u16;
    /// 返回最近一次自动生成的插入 ID（AUTO_INCREMENT）。
    fn LastInsertID(&self) -> u64;
    /// 返回最近一条语句的提示消息。
    fn LastMessage(&self) -> String;
    /// 返回最近一条语句影响的行数。
    fn AffectedRows(&self) -> u64;
    /// 解析并执行一段 SQL 文本，可能返回多个结果集。
    fn Execute(
        &mut self,
        ctx: &sessionctx::ExecutionContext,
        sql: &str,
    ) -> Result<Vec<Box<dyn sqlexec::RecordSet>>, sessionctx::GoError>;
    /// 执行已解析的单条语句 AST。
    fn ExecuteStmt(
        &mut self,
        ctx: &sessionctx::ExecutionContext,
        stmt: &dyn ast::StmtNode,
    ) -> Result<Box<dyn sqlexec::RecordSet>, sessionctx::GoError>;
    /// 将 SQL 文本解析为语句 AST 列表。
    fn Parse(
        &self,
        ctx: &sessionctx::ExecutionContext,
        sql: &str,
    ) -> Result<Vec<Box<dyn ast::StmtNode>>, sessionctx::GoError>;
    /// 执行内核内部 SQL（可带类型擦除参数），不走完整客户端协议路径。
    fn ExecuteInternal(
        &mut self,
        ctx: &sessionctx::ExecutionContext,
        sql: &str,
        args: &[Any],
    ) -> Result<Box<dyn sqlexec::RecordSet>, sessionctx::GoError>;
    /// 返回会话的可读描述字符串。
    fn String(&self) -> String;
    /// 提交当前事务（两阶段提交由存储层完成）。
    fn CommitTxn(&mut self, ctx: &sessionctx::ExecutionContext) -> Result<(), sessionctx::GoError>;
    /// 回滚当前事务。
    fn RollbackTxn(&mut self, ctx: &sessionctx::ExecutionContext);
    /// 预编译语句，返回语句 ID、参数个数与结果列元数据。
    fn PrepareStmt(
        &mut self,
        sql: &str,
    ) -> Result<(u32, i32, Vec<resolve::ResultField>), sessionctx::GoError>;
    /// 执行已预编译语句。
    fn ExecutePreparedStmt(
        &mut self,
        ctx: &sessionctx::ExecutionContext,
        stmt_id: u32,
        params: &[Self::PreparedExpression],
    ) -> Result<Box<dyn sqlexec::RecordSet>, sessionctx::GoError>;
    /// 丢弃预编译语句。
    fn DropPreparedStmt(&mut self, stmt_id: u32) -> Result<(), sessionctx::GoError>;
    /// 注册某类会话状态的导入/导出处理器。
    fn SetSessionStatesHandler(
        &mut self,
        state_type: sessionstates::SessionStateType,
        handler: Self::StateHandler,
    );
    /// 设置客户端能力标志（capability flags）。
    fn SetClientCapability(&mut self, capability: u32);
    /// 设置连接 ID。
    fn SetConnectionID(&mut self, id: u64);
    /// 设置当前正在执行的 MySQL 命令字节。
    fn SetCommandValue(&mut self, command: u8);
    /// 设置压缩算法标识。
    fn SetCompressionAlgorithm(&mut self, algorithm: i32);
    /// 设置压缩级别。
    fn SetCompressionLevel(&mut self, level: i32);
    /// 更新进程列表中展示的 SQL、开始时间与超时等信息。
    fn SetProcessInfo(
        &mut self,
        sql: &str,
        start: SystemTime,
        command: u8,
        max_execution_time: u64,
    );
    /// 绑定 TLS 连接状态。
    fn SetTLSState(&mut self, state: Option<&rustls::ServerConnection>);
    /// 设置连接排序规则（collation）ID。
    fn SetCollation(&mut self, id: i32) -> Result<(), sessionctx::GoError>;
    /// 绑定全局会话管理器。
    fn SetSessionManager(&mut self, manager: Option<Arc<dyn sessmgr::Manager>>);
    /// 关闭会话并释放相关资源。
    fn Close(&mut self);
    /// 对用户做完整鉴权（含密码校验）。
    fn Auth(
        &mut self,
        user: &auth::UserIdentity,
        auth_data: &[u8],
        salt: &[u8],
        connection: &mut Self::AuthConnection,
    ) -> Result<(), sessionctx::GoError>;
    /// 仅校验身份是否存在，不做密码验证。
    fn AuthWithoutVerification(
        &self,
        ctx: &sessionctx::ExecutionContext,
        user: &auth::UserIdentity,
    ) -> bool;
    /// 查询指定用户应使用的鉴权插件名。
    fn AuthPluginForUser(
        &self,
        ctx: &sessionctx::ExecutionContext,
        user: &auth::UserIdentity,
    ) -> Result<String, sessionctx::GoError>;
    /// 按用户名与远端主机匹配权限系统中的身份。
    fn MatchIdentity(
        &self,
        ctx: &sessionctx::ExecutionContext,
        username: &str,
        remote_host: &str,
    ) -> Result<auth::UserIdentity, sessionctx::GoError>;
    /// 返回当前事务摘要信息（若有）。
    fn TxnInfo(&self) -> Option<&txninfo::TxnInfo>;
    /// 在执行语句前准备事务上下文（懒开启事务等）。
    fn PrepareTxnCtx(
        &mut self,
        ctx: &sessionctx::ExecutionContext,
        stmt: &dyn ast::StmtNode,
    ) -> Result<(), sessionctx::GoError>;
    /// 返回表的字段列表（对应 COM_FIELD_LIST）。
    fn FieldList(&self, table_name: &str)
    -> Result<Vec<resolve::ResultField>, sessionctx::GoError>;
    /// 设置客户端端口信息。
    fn SetPort(&mut self, port: &str);
    /// 注入会话扩展点集合；`None` 对应 Go 的 `nil` 指针。
    fn SetExtensions(&mut self, extensions: Option<Arc<extension::SessionExtensions>>);
}

/// 类型擦除的内部 SQL 参数容器（对应 Go `...interface{}`）。
pub type Any = Box<dyn std::any::Any + Send + Sync>;
