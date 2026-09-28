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

// DDL（Data Definition Language，数据定义语言）语句执行器。
//
// `DDLExec` 负责一次性生命周期：本地临时表预处理、语句内新事务、按语句种类
// 分发到 Domain/DDL 子系统，以及失败时的 schema changed 错误转换与会话状态收尾。
#![allow(non_snake_case)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 解析后的 DDL 语句种类，含若干暂不支持的 Flashback 变体。
pub enum DdlStatementKind {
    AlterDatabase,
    AlterTable,
    CreateIndex,
    CreateDatabase,
    FlashbackDatabase,
    CreateTable,
    CreateView,
    DropIndex,
    DropDatabase,
    DropView,
    DropTable,
    RecoverTable,
    FlashbackTable,
    FlashbackCluster,
    RenameTable,
    TruncateTable,
    LockTables,
    UnlockTables,
    CleanupTableLock,
    RepairTable,
    CreateSequence,
    DropSequence,
    AlterSequence,
    CreateMaskingPolicy,
    CreatePlacementPolicy,
    DropPlacementPolicy,
    AlterPlacementPolicy,
    CreateResourceGroup,
    DropResourceGroup,
    AlterResourceGroup,
    UnsupportedFlashbackTableToTimestamp,
    UnsupportedFlashbackDatabaseToTimestamp,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 可下发到 `DdlRuntime::execute` 的具体 DDL 操作。
pub enum DdlOperation {
    AlterDatabase,
    AlterTable,
    CreateIndex,
    CreateDatabase,
    FlashbackDatabase,
    CreateTable,
    CreateView,
    DropIndex,
    DropDatabase,
    DropView,
    DropTable,
    RecoverTable,
    FlashbackTable,
    FlashbackCluster,
    RenameTable,
    TruncateTable,
    LockTables,
    UnlockTables,
    CleanupTableLock,
    RepairTable,
    CreateSequence,
    DropSequence,
    AlterSequence,
    CreateMaskingPolicy,
    CreatePlacementPolicy,
    DropPlacementPolicy,
    AlterPlacementPolicy,
    CreateResourceGroup,
    DropResourceGroup,
    AlterResourceGroup,
}

/// 本地临时表预处理结果：继续常规 DDL、已完成，或仅需删除列出的临时表。
pub enum LocalTemporaryPreprocess<TableName> {
    Continue,
    Completed,
    DropOnly(Vec<TableName>),
}

/// Domain、DDL、infoschema、sessiontxn 与临时表操作的生产边界。
/// `DDLExec` 拥有一次性生命周期、本地临时表路由与 DDL 后会话状态迁移；
/// 不允许提供可静默成功的默认实现。
/// Mandatory production boundary for Domain, DDL, infoschema, sessiontxn and
/// temporary-table operations. `DDLExec` owns the one-shot lifecycle, local
/// temporary-table routing and post-DDL session state transitions. There is no
/// default implementation that can silently report success.
pub trait DdlRuntime {
    type Context;
    type Chunk;
    type Statement: Clone;
    type TableName: Clone;
    type Table;
    type Job: Clone;
    type TableInfo: Clone;
    type RecoverSchemaInfo;
    type Error;

    fn reset_chunk(&mut self, chunk: &mut Self::Chunk);
    fn statement_kind(&self, statement: &Self::Statement) -> DdlStatementKind;
    fn preprocess_local_temporary_tables(
        &mut self,
        statement: &mut Self::Statement,
    ) -> Result<LocalTemporaryPreprocess<Self::TableName>, Self::Error>;
    fn new_transaction_in_statement(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<(), Self::Error>;
    fn execute(
        &mut self,
        context: &mut Self::Context,
        operation: DdlOperation,
        statement: &Self::Statement,
    ) -> Result<(), Self::Error>;
    fn ddl_job_was_queued(&self) -> bool;
    fn reset_ddl_job_state(&mut self);
    fn should_convert_to_schema_changed(&self, was_queued: bool, error: &Self::Error) -> bool;
    fn convert_to_schema_changed(&mut self, error: Self::Error) -> Self::Error;
    fn refresh_transaction_infoschema(&mut self);
    fn leave_transaction(&mut self);
    fn unsupported_ddl(&self, message: &str) -> Self::Error;

    fn local_temporary_table(
        &mut self,
        schema: &str,
        table: &str,
    ) -> Result<Option<Self::Table>, Self::Error>;
    fn statement_table_name(&self, statement: &Self::Statement) -> Option<(String, String)>;
    fn create_local_temporary_table(
        &mut self,
        statement: &Self::Statement,
    ) -> Result<(), Self::Error>;
    fn drop_local_temporary_table(&mut self, table: &Self::TableName) -> Result<(), Self::Error>;
    fn truncate_local_temporary_table(
        &mut self,
        schema: &str,
        table: &str,
    ) -> Result<(), Self::Error>;
    fn local_temporary_ddl_error(&self, operation: &str) -> Self::Error;

    fn recover_table_by_job_id(
        &mut self,
        statement: &Self::Statement,
    ) -> Result<(Self::Job, Self::TableInfo), Self::Error>;
    fn recover_table_by_name(
        &mut self,
        statement: &Self::Statement,
    ) -> Result<(Self::Job, Self::TableInfo), Self::Error>;
    fn recover_table_uses_name(&self, statement: &Self::Statement) -> bool;
    fn recover_database_by_name(
        &mut self,
        statement: &Self::Statement,
    ) -> Result<Self::RecoverSchemaInfo, Self::Error>;
    fn execute_recover_table(
        &mut self,
        statement: &Self::Statement,
        job: Self::Job,
        table: Self::TableInfo,
    ) -> Result<(), Self::Error>;
    fn execute_recover_database(
        &mut self,
        statement: &Self::Statement,
        schema: Self::RecoverSchemaInfo,
    ) -> Result<(), Self::Error>;

    fn job_is_drop_or_truncate_table(&self, job: &Self::Job) -> bool;
    fn job_snapshot_is_after_gc(&self, job: &Self::Job, gc_safe_point: u64) -> bool;
    fn table_at_job_snapshot(
        &mut self,
        job: &Self::Job,
    ) -> Result<Option<Self::TableInfo>, Self::Error>;
}

/// DDL 执行器：持有运行时、当前语句与 `done` 一次性标志。
pub struct DDLExec<R: DdlRuntime> {
    pub runtime: R,
    pub stmt: R::Statement,
    pub done: bool,
}

impl<R: DdlRuntime> DDLExec<R> {
    /// 将错误转换为 schema changed，提示客户端刷新元数据。
    pub fn toErr(&mut self, error: R::Error) -> R::Error {
        self.runtime.convert_to_schema_changed(error)
    }

    /// 查询本地临时表是否存在，返回 `(表对象, 是否存在)`。
    pub fn getLocalTemporaryTable(
        &mut self,
        schema: &str,
        table: &str,
    ) -> Result<(Option<R::Table>, bool), R::Error> {
        let table = self.runtime.local_temporary_table(schema, table)?;
        let exists = table.is_some();
        Ok((table, exists))
    }

    /// 执行一次 DDL：预处理临时表、开事务、分发并刷新 infoschema。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        // DDL 为一次性算子：再次 Next 直接返回
        if self.done {
            return Ok(());
        }
        self.done = true;

        match self
            .runtime
            .preprocess_local_temporary_tables(&mut self.stmt)?
        {
            // 本地临时表可能短路完成或仅需删除临时表
            LocalTemporaryPreprocess::Completed => return Ok(()),
            LocalTemporaryPreprocess::DropOnly(tables) => {
                return self.dropLocalTemporaryTables(tables);
            }
            LocalTemporaryPreprocess::Continue => {}
        }

        // 在语句事务中执行 DDL；若 job 已入队且需转换则改为 schema changed
        self.runtime.new_transaction_in_statement(context)?;
        let result = self.dispatchDDL(context);
        let was_queued = self.runtime.ddl_job_was_queued();
        self.runtime.reset_ddl_job_state();
        if let Err(error) = result {
            if self
                .runtime
                .should_convert_to_schema_changed(was_queued, &error)
            {
                return Err(self.toErr(error));
            }
            return Err(error);
        }
        self.runtime.refresh_transaction_infoschema();
        self.runtime.leave_transaction();
        Ok(())
    }

    /// 按 `DdlStatementKind` 分发到对应 `execute*` 方法。
    pub fn dispatchDDL(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        match self.runtime.statement_kind(&self.stmt) {
            DdlStatementKind::AlterDatabase => self.executeAlterDatabase(context),
            DdlStatementKind::AlterTable => self.executeAlterTable(context),
            DdlStatementKind::CreateIndex => self.executeCreateIndex(context),
            DdlStatementKind::CreateDatabase => self.executeCreateDatabase(context),
            DdlStatementKind::FlashbackDatabase => self.executeFlashbackDatabase(context),
            DdlStatementKind::CreateTable => self.executeCreateTable(context),
            DdlStatementKind::CreateView => self.executeCreateView(context),
            DdlStatementKind::DropIndex => self.executeDropIndex(context),
            DdlStatementKind::DropDatabase => self.executeDropDatabase(context),
            DdlStatementKind::DropView => self.executeDropView(context),
            DdlStatementKind::DropTable => self.executeDropTable(context),
            DdlStatementKind::RecoverTable => self.executeRecoverTable(context),
            DdlStatementKind::FlashbackTable => self.executeFlashbackTable(context),
            DdlStatementKind::FlashbackCluster => self.executeFlashBackCluster(context),
            DdlStatementKind::RenameTable => self.executeRenameTable(context),
            DdlStatementKind::TruncateTable => self.executeTruncateTable(context),
            DdlStatementKind::LockTables => self.executeLockTables(context),
            DdlStatementKind::UnlockTables => self.executeUnlockTables(context),
            DdlStatementKind::CleanupTableLock => self.executeCleanupTableLock(context),
            DdlStatementKind::RepairTable => self.executeRepairTable(context),
            DdlStatementKind::CreateSequence => self.executeCreateSequence(context),
            DdlStatementKind::DropSequence => self.executeDropSequence(context),
            DdlStatementKind::AlterSequence => self.executeAlterSequence(context),
            DdlStatementKind::CreateMaskingPolicy => self.executeCreateMaskingPolicy(context),
            DdlStatementKind::CreatePlacementPolicy => self.executeCreatePlacementPolicy(context),
            DdlStatementKind::DropPlacementPolicy => self.executeDropPlacementPolicy(context),
            DdlStatementKind::AlterPlacementPolicy => self.executeAlterPlacementPolicy(context),
            DdlStatementKind::CreateResourceGroup => self.executeCreateResourceGroup(context),
            DdlStatementKind::DropResourceGroup => self.executeDropResourceGroup(context),
            DdlStatementKind::AlterResourceGroup => self.executeAlterResourceGroup(context),
            DdlStatementKind::UnsupportedFlashbackTableToTimestamp => Err(self
                .runtime
                .unsupported_ddl("Unsupported FLASHBACK table TO TIMESTAMP")),
            DdlStatementKind::UnsupportedFlashbackDatabaseToTimestamp => Err(self
                .runtime
                .unsupported_ddl("Unsupported FLASHBACK database TO TIMESTAMP")),
            DdlStatementKind::Other => Ok(()),
        }
    }

    /// 调用运行时执行指定 `DdlOperation`。
    fn executeOperation(
        &mut self,
        context: &mut R::Context,
        operation: DdlOperation,
    ) -> Result<(), R::Error> {
        self.runtime.execute(context, operation, &self.stmt)
    }

    /// TRUNCATE：本地临时表走本地路径，否则下发 TruncateTable。
    pub fn executeTruncateTable(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        if let Some((schema, table)) = self.runtime.statement_table_name(&self.stmt) {
            if self.getLocalTemporaryTable(&schema, &table)?.1 {
                return self.runtime.truncate_local_temporary_table(&schema, &table);
            }
        }
        self.executeOperation(context, DdlOperation::TruncateTable)
    }

    /// RENAME TABLE。
    pub fn executeRenameTable(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::RenameTable)
    }
    /// CREATE DATABASE。
    pub fn executeCreateDatabase(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreateDatabase)
    }
    /// ALTER DATABASE。
    pub fn executeAlterDatabase(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::AlterDatabase)
    }
    /// CREATE TABLE。
    pub fn executeCreateTable(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreateTable)
    }
    /// 在会话中创建本地临时表。
    pub fn createSessionTemporaryTable(&mut self) -> Result<(), R::Error> {
        self.runtime.create_local_temporary_table(&self.stmt)
    }
    /// CREATE VIEW。
    pub fn executeCreateView(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreateView)
    }
    /// CREATE INDEX；拒绝本地临时表。
    pub fn executeCreateIndex(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.rejectLocalTemporaryTable("CREATE INDEX")?;
        self.executeOperation(context, DdlOperation::CreateIndex)
    }
    /// DROP DATABASE。
    pub fn executeDropDatabase(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::DropDatabase)
    }
    /// DROP TABLE。
    pub fn executeDropTable(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::DropTable)
    }
    /// DROP VIEW。
    pub fn executeDropView(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::DropView)
    }
    /// DROP SEQUENCE。
    pub fn executeDropSequence(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::DropSequence)
    }
    /// 批量删除本地临时表。
    pub fn dropLocalTemporaryTables(&mut self, tables: Vec<R::TableName>) -> Result<(), R::Error> {
        for table in tables {
            self.runtime.drop_local_temporary_table(&table)?;
        }
        Ok(())
    }
    /// DROP INDEX；拒绝本地临时表。
    pub fn executeDropIndex(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.rejectLocalTemporaryTable("DROP INDEX")?;
        self.executeOperation(context, DdlOperation::DropIndex)
    }
    /// ALTER TABLE；拒绝本地临时表。
    pub fn executeAlterTable(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.rejectLocalTemporaryTable("ALTER TABLE")?;
        self.executeOperation(context, DdlOperation::AlterTable)
    }

    /// 若目标为本地临时表则返回该操作不支持的错误。
    fn rejectLocalTemporaryTable(&mut self, operation: &str) -> Result<(), R::Error> {
        if let Some((schema, table)) = self.runtime.statement_table_name(&self.stmt)
            && self.getLocalTemporaryTable(&schema, &table)?.1
        {
            return Err(self.runtime.local_temporary_ddl_error(operation));
        }
        Ok(())
    }

    /// RECOVER TABLE：按表名或 job id 恢复。
    pub fn executeRecoverTable(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        let (job, table) = if self.runtime.recover_table_uses_name(&self.stmt) {
            self.getRecoverTableByTableName()?
        } else {
            self.getRecoverTableByJobID()?
        };
        self.runtime.execute_recover_table(&self.stmt, job, table)
    }
    /// 按 DDL job id 获取待恢复表信息。
    pub fn getRecoverTableByJobID(&mut self) -> Result<(R::Job, R::TableInfo), R::Error> {
        self.runtime.recover_table_by_job_id(&self.stmt)
    }
    /// 按表名获取待恢复表信息。
    pub fn getRecoverTableByTableName(&mut self) -> Result<(R::Job, R::TableInfo), R::Error> {
        self.runtime.recover_table_by_name(&self.stmt)
    }
    /// FLASHBACK CLUSTER。
    pub fn executeFlashBackCluster(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::FlashbackCluster)
    }
    /// FLASHBACK TABLE：复用 recover 路径。
    pub fn executeFlashbackTable(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        let (job, table) = self.getRecoverTableByTableName()?;
        self.runtime.execute_recover_table(&self.stmt, job, table)
    }
    /// FLASHBACK DATABASE。
    pub fn executeFlashbackDatabase(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        let schema = self.getRecoverDBByName()?;
        self.runtime.execute_recover_database(&self.stmt, schema)
    }
    /// 按库名获取待恢复 schema 信息。
    pub fn getRecoverDBByName(&mut self) -> Result<R::RecoverSchemaInfo, R::Error> {
        self.runtime.recover_database_by_name(&self.stmt)
    }
    /// LOCK TABLES。
    pub fn executeLockTables(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::LockTables)
    }
    /// UNLOCK TABLES。
    pub fn executeUnlockTables(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::UnlockTables)
    }
    /// 清理表锁。
    pub fn executeCleanupTableLock(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CleanupTableLock)
    }
    /// REPAIR TABLE。
    pub fn executeRepairTable(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::RepairTable)
    }
    /// CREATE SEQUENCE。
    pub fn executeCreateSequence(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreateSequence)
    }
    /// ALTER SEQUENCE。
    pub fn executeAlterSequence(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::AlterSequence)
    }
    /// CREATE PLACEMENT POLICY（副本放置策略）。
    pub fn executeCreatePlacementPolicy(
        &mut self,
        context: &mut R::Context,
    ) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreatePlacementPolicy)
    }
    /// CREATE MASKING POLICY（脱敏策略）。
    pub fn executeCreateMaskingPolicy(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreateMaskingPolicy)
    }
    /// DROP PLACEMENT POLICY。
    pub fn executeDropPlacementPolicy(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::DropPlacementPolicy)
    }
    /// ALTER PLACEMENT POLICY。
    pub fn executeAlterPlacementPolicy(
        &mut self,
        context: &mut R::Context,
    ) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::AlterPlacementPolicy)
    }
    /// CREATE RESOURCE GROUP（资源组）。
    pub fn executeCreateResourceGroup(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::CreateResourceGroup)
    }
    /// ALTER RESOURCE GROUP。
    pub fn executeAlterResourceGroup(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::AlterResourceGroup)
    }
    /// DROP RESOURCE GROUP。
    pub fn executeDropResourceGroup(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.executeOperation(context, DdlOperation::DropResourceGroup)
    }
}

/// 从 DDL job 列表中查找 DROP/TRUNCATE 且快照晚于 GC 安全点的表信息，交给回调处理。
/// GC（Garbage Collection）安全点之前的历史可能已被清理，不可再用于恢复。
pub fn GetDropOrTruncateTableInfoFromJobs<R, F>(
    runtime: &mut R,
    jobs: Vec<R::Job>,
    gc_safe_point: u64,
    mut handle: F,
) -> Result<bool, R::Error>
where
    R: DdlRuntime,
    F: FnMut(R::Job, R::TableInfo) -> Result<bool, R::Error>,
{
    for job in jobs {
        if !runtime.job_is_drop_or_truncate_table(&job)
            || !runtime.job_snapshot_is_after_gc(&job, gc_safe_point)
        {
            continue;
        }
        if let Some(table) = runtime.table_at_job_snapshot(&job)?
            && handle(job, table)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
