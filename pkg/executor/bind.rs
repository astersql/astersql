// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// SQL Plan Binding（执行计划绑定）语句执行器。
//
// 对应 Go 的 `SQLBindExec`：处理 CREATE/DROP/FLUSH/RELOAD/SET STATUS 等
// binding 管理操作。会话级与全局级通过 `isGlobal` 区分；具体存储与
// 广播操作由 `SQLBindBackend` 注入。

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SQL binding 操作类型。
pub enum SQLBindOpType {
    Create,
    Drop,
    DropByDigest,
    Flush,
    Reload,
    ReloadCluster,
    SetStatus,
    SetStatusByDigest,
    Unknown(i32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条 binding 操作的细节字段（规范化 SQL、绑定 SQL、摘要等）。
pub struct SQLBindOpDetail {
    pub NormdOrigSQL: String,
    pub Db: String,
    pub BindSQL: String,
    pub Charset: String,
    pub Collation: String,
    pub NewStatus: String,
    pub Source: String,
    pub SQLDigest: String,
    pub PlanDigest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条完整的 binding 记录，用于写入会话或全局绑定表。
pub struct Binding {
    pub OriginalSQL: String,
    pub Db: String,
    pub BindSQL: String,
    pub Charset: String,
    pub Collation: String,
    pub Status: String,
    pub Source: String,
    pub SQLDigest: String,
    pub PlanDigest: String,
}

/// 可重置的输出 Chunk（binding 语句不产出行，仅 reset）。
pub trait ResettableChunk {
    fn reset(&mut self);
}

/// `SQLBindExec` 所需的会话/领域侧实时操作边界。
/// Live session/domain operations required by `SQLBindExec`.
pub trait SQLBindBackend<C> {
    type Error;
    type StatementContext;

    fn error(&self, message: String) -> Self::Error;
    fn normalize_digest_for_binding(&self, sql: &str) -> String;
    fn drop_session_bindings(&mut self, digests: &[String]) -> Result<(), Self::Error>;
    fn drop_global_bindings(&mut self, digests: &[String]) -> (u64, Result<(), Self::Error>);
    fn add_affected_rows(&mut self, rows: u64);
    fn set_global_binding_status(
        &mut self,
        status: &str,
        digest: &str,
    ) -> (bool, Result<(), Self::Error>);
    fn append_warning(&mut self, warning: &str);

    fn take_statement_context(&mut self) -> Self::StatementContext;
    fn current_set_var_hint_restore(&self) -> Vec<(String, String)>;
    fn add_set_var_hint_restore(
        &mut self,
        context: &mut Self::StatementContext,
        name: String,
        value: String,
    );
    fn restore_statement_context(&mut self, context: Self::StatementContext);

    fn create_session_bindings(
        &mut self,
        context: &C,
        bindings: &[Binding],
    ) -> Result<(), Self::Error>;
    fn create_global_bindings(
        &mut self,
        context: &C,
        bindings: &[Binding],
    ) -> Result<(), Self::Error>;
    fn load_bindings(&mut self, full_load: bool, from_remote: bool) -> Result<(), Self::Error>;
    fn broadcast(&mut self, context: &C, sql: &str) -> Result<(), Self::Error>;
}

/// SQL binding 执行器：持有操作类型、细节列表与全局/远程标志。
pub struct SQLBindExec<B> {
    pub BaseExecutor: B,
    pub isGlobal: bool,
    pub sqlBindOp: SQLBindOpType,
    pub details: Vec<SQLBindOpDetail>,
    pub isFromRemote: bool,
}

impl<B> SQLBindExec<B> {
    /// 执行一步 binding 操作；按 `sqlBindOp` 分派到具体处理函数。
    pub fn Next<C, Q>(&mut self, context: &C, request: &mut Q) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
        Q: ResettableChunk,
    {
        request.reset();
        match self.sqlBindOp {
            SQLBindOpType::Create => self.createSQLBind(context),
            SQLBindOpType::Drop => self.dropSQLBind(),
            SQLBindOpType::DropByDigest => self.dropSQLBindByDigest(),
            SQLBindOpType::Flush => self.flushBindings(),
            SQLBindOpType::Reload => self.reloadBindings(),
            SQLBindOpType::ReloadCluster => self.reloadClusterBindings(context),
            SQLBindOpType::SetStatus => self.setBindingStatus(),
            SQLBindOpType::SetStatusByDigest => self.setBindingStatusByDigest(),
            SQLBindOpType::Unknown(value) => Err(self
                .BaseExecutor
                .error(format!("unsupported SQL bind operation: {value}"))),
        }
    }

    /// 要求 details 恰好一条，否则报错。
    fn require_one_detail<C>(&self, operation: &str) -> Result<&SQLBindOpDetail, B::Error>
    where
        B: SQLBindBackend<C>,
    {
        if self.details.len() != 1 {
            return Err(self.BaseExecutor.error(format!(
                "SQLBindExec: {operation} should only have one SQLBindOpDetail"
            )));
        }
        Ok(&self.details[0])
    }

    /// 按规范化 SQL 对应 digest 删除 binding（会话或全局）。
    pub fn dropSQLBind<C>(&mut self) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        let digest = self
            .require_one_detail::<C>("dropSQLBind")?
            .SQLDigest
            .clone();
        let digests = [digest];
        if !self.isGlobal {
            return self.BaseExecutor.drop_session_bindings(&digests);
        }
        let (affected_rows, result) = self.BaseExecutor.drop_global_bindings(&digests);
        self.BaseExecutor.add_affected_rows(affected_rows);
        result
    }

    /// 按显式 SQLDigest 列表删除 binding。
    pub fn dropSQLBindByDigest<C>(&mut self) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        let mut digests = Vec::with_capacity(self.details.len());
        for detail in &self.details {
            if detail.SQLDigest.is_empty() {
                return Err(self.BaseExecutor.error(
                    "SQLBindExec: dropSQLBindByDigest shouldn't contain empty SQLDigest".into(),
                ));
            }
            digests.push(detail.SQLDigest.clone());
        }
        if !self.isGlobal {
            return self.BaseExecutor.drop_session_bindings(&digests);
        }
        let (affected_rows, result) = self.BaseExecutor.drop_global_bindings(&digests);
        self.BaseExecutor.add_affected_rows(affected_rows);
        result
    }

    /// 按规范化原文计算 digest 并设置全局 binding 状态。
    pub fn setBindingStatus<C>(&mut self) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        let detail = self.require_one_detail::<C>("setBindingStatus")?;
        let status = detail.NewStatus.clone();
        let digest = self
            .BaseExecutor
            .normalize_digest_for_binding(&detail.NormdOrigSQL);
        self.set_binding_status::<C>(&status, &digest)
    }

    /// 按显式 SQLDigest 设置全局 binding 状态。
    pub fn setBindingStatusByDigest<C>(&mut self) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        let detail = self.require_one_detail::<C>("setBindingStatusByDigest")?;
        let status = detail.NewStatus.clone();
        let digest = detail.SQLDigest.clone();
        self.set_binding_status::<C>(&status, &digest)
    }

    /// 设置状态的公共路径；未实际变更时追加警告。
    fn set_binding_status<C>(&mut self, status: &str, digest: &str) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        let (changed, result) = self.BaseExecutor.set_global_binding_status(status, digest);
        if result.is_ok() && !changed {
            self.BaseExecutor.append_warning(
                "There are no bindings can be set the status. Please check the SQL text",
            );
        }
        result
    }

    /// 创建会话或全局 binding；临时保存并恢复 statement context / set_var hint。
    pub fn createSQLBind<C>(&mut self, context: &C) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        // 创建前取出语句上下文，结束后把 set_var hint 恢复写回
        let mut saved_statement_context = self.BaseExecutor.take_statement_context();
        let bindings: Vec<_> = self
            .details
            .iter()
            .map(|detail| Binding {
                OriginalSQL: detail.NormdOrigSQL.clone(),
                Db: detail.Db.clone(),
                BindSQL: detail.BindSQL.clone(),
                Charset: detail.Charset.clone(),
                Collation: detail.Collation.clone(),
                Status: "enabled".into(),
                Source: detail.Source.clone(),
                SQLDigest: detail.SQLDigest.clone(),
                PlanDigest: detail.PlanDigest.clone(),
            })
            .collect();

        // Go uses defer here, so the saved statement context must also be
        // restored while unwinding from an internal binding-creation panic.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if self.isGlobal {
                self.BaseExecutor.create_global_bindings(context, &bindings)
            } else {
                self.BaseExecutor
                    .create_session_bindings(context, &bindings)
            }
        }));

        for (name, value) in self.BaseExecutor.current_set_var_hint_restore() {
            self.BaseExecutor
                .add_set_var_hint_restore(&mut saved_statement_context, name, value);
        }
        self.BaseExecutor
            .restore_statement_context(saved_statement_context);
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// 增量加载 binding（非全量、非远程）。
    pub fn flushBindings<C>(&mut self) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        self.BaseExecutor.load_bindings(false, false)
    }

    /// 全量重新加载 binding；`isFromRemote` 控制是否从远端拉取。
    pub fn reloadBindings<C>(&mut self) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        self.BaseExecutor.load_bindings(true, self.isFromRemote)
    }

    /// 向集群广播 `ADMIN RELOAD BINDINGS`。
    pub fn reloadClusterBindings<C>(&mut self, context: &C) -> Result<(), B::Error>
    where
        B: SQLBindBackend<C>,
    {
        self.BaseExecutor
            .broadcast(context, "ADMIN RELOAD BINDINGS")
    }
}
