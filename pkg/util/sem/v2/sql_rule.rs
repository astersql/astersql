// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// SEM SQL 限制规则与语句命令名映射。
//
// 将 parser AST 节点映射为 SEM 命令名（如 `DROP DATABASE`），并提供命名规则
//（TTL、表 ATTRIBUTES、本地 IMPORT/LOAD DATA、SELECT INTO 等）供配置 `restricted_sql.rule` 引用。

use std::any::Any;
use std::collections::HashMap;
use std::sync::LazyLock;

use ast::sem as ast_sem;

/// 仅携带 SEM 命令名的轻量语句节点，供无完整 AST 的调用方使用。
/// A command-only node for callers that do not need a concrete parser AST payload.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandStatement {
    node_text: ast::base::AstNode,
    command: String,
}

impl CommandStatement {
    /// 构造命令语句：trim 后转大写作为 SEM 命令名。
    pub fn new(command: &str) -> Self {
        Self {
            command: command.trim().to_uppercase(),
            node_text: Default::default(),
        }
    }
}

impl ast::Node for CommandStatement {
    fn node_text(&self) -> &ast::base::AstNode {
        &self.node_text
    }
    fn node_text_mut(&mut self) -> &mut ast::base::AstNode {
        &mut self.node_text
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn accept(&self, visitor: &mut dyn ast::Visitor) -> bool {
        visitor.enter(self);
        visitor.leave(self)
    }
}

/// SQL 规则函数类型：对 AST 返回 true 表示该语句受限。
/// SQLRule decides whether a parser AST statement is restricted.
pub type SQLRule = fn(&dyn ast::Node) -> bool;

/// 表选项中是否包含 TTL / TTL_ENABLE / TTL_JOB_INTERVAL。
fn checkTTLOptions(options: &[ast::TableOption]) -> bool {
    options.iter().any(|option| {
        matches!(
            option.Tp,
            ast::TableOptionType::TTL
                | ast::TableOptionType::TTLEnable
                | ast::TableOptionType::TTLJobInterval
        )
    })
}

#[allow(non_upper_case_globals)]
/// 配置中规则名到实现函数的映射表。
pub static sqlRuleNameMap: LazyLock<HashMap<&'static str, SQLRule>> = LazyLock::new(|| {
    HashMap::from([
        ("time_to_live", TimeToLiveSQLRule as SQLRule),
        (
            "alter_table_attributes",
            AlterTableAttributesRule as SQLRule,
        ),
        (
            "import_with_external_id",
            ImportWithExternalIDRule as SQLRule,
        ),
        ("select_into_file", SelectIntoFileRule as SQLRule),
        ("import_from_local", ImportFromLocalRule as SQLRule),
    ])
});

#[allow(non_snake_case)]
/// 限制 CREATE/ALTER TABLE 上的 TTL 相关选项或 REMOVE TTL。
pub fn TimeToLiveSQLRule(stmt: &dyn ast::Node) -> bool {
    // CREATE TABLE 检查 Options；ALTER TABLE 检查 REMOVE TTL 或 Option 中的 TTL。
    if let Some(create) = stmt.as_any().downcast_ref::<ast::CreateTableStmt>() {
        return checkTTLOptions(&create.Options);
    }
    if let Some(alter) = stmt.as_any().downcast_ref::<ast::AlterTableStmt>() {
        return alter.Specs.iter().any(|spec| {
            spec.Tp == ast::AlterTableType::RemoveTTL
                || (spec.Tp == ast::AlterTableType::Option && checkTTLOptions(&spec.Options))
        });
    }
    false
}

#[allow(non_snake_case)]
/// 限制 ALTER TABLE 的 ATTRIBUTES / PARTITION ATTRIBUTES。
pub fn AlterTableAttributesRule(stmt: &dyn ast::Node) -> bool {
    stmt.as_any()
        .downcast_ref::<ast::AlterTableStmt>()
        .is_some_and(|alter| {
            alter.Specs.iter().any(|spec| {
                matches!(
                    spec.Tp,
                    ast::AlterTableType::Attributes | ast::AlterTableType::PartitionAttributes
                )
            })
        })
}

#[allow(non_snake_case)]
/// 占位规则：当前恒为 false（与 Go 侧未启用逻辑对齐）。
pub fn ImportWithExternalIDRule(_stmt: &dyn ast::Node) -> bool {
    false
}

#[allow(non_snake_case)]
/// 限制带 SELECT INTO OUTFILE 的查询。
pub fn SelectIntoFileRule(stmt: &dyn ast::Node) -> bool {
    stmt.as_any()
        .downcast_ref::<ast::SelectStmt>()
        .is_some_and(|select| select.SelectIntoOpt.is_some())
}

#[allow(non_snake_case)]
/// 限制从本地路径 IMPORT INTO / 服务端 LOAD DATA（客户端 LOCAL 除外）。
pub fn ImportFromLocalRule(stmt: &dyn ast::Node) -> bool {
    // IMPORT INTO：有 SELECT 子查询则非“本地文件导入”；否则按 Path 是否本地 URL 判定。
    if let Some(import) = stmt.as_any().downcast_ref::<ast::ImportIntoStmt>() {
        if import.Select.is_some() {
            return false;
        }
        return objstore::parse::ParseRawURL(&import.Path)
            .map(|url| objstore::parse::IsLocal(&url))
            .unwrap_or(false);
    }
    // LOAD DATA：客户端 LOCAL 放行；服务端路径再判是否本地。
    if let Some(load) = stmt.as_any().downcast_ref::<ast::LoadDataStmt>() {
        if load.FileLocRef == ast::FileLocRef::Client {
            return false;
        }
        return objstore::parse::ParseRawURL(&load.Path)
            .map(|url| objstore::parse::IsLocal(&url))
            .unwrap_or(false);
    }
    false
}

/// 将 AST 节点映射为 SEM 规范命令名字符串，供受限 SQL 命令列表匹配。
pub(crate) fn semCommand(stmt: &dyn ast::Node) -> String {
    if let Some(command) = stmt.as_any().downcast_ref::<CommandStatement>() {
        return command.command.clone();
    }
    if let Some(drop) = stmt.as_any().downcast_ref::<ast::DropTableStmt>() {
        return if drop.IsView {
            ast_sem::DropViewCommand
        } else {
            ast_sem::DropTableCommand
        }
        .to_owned();
    }
    if let Some(insert) = stmt.as_any().downcast_ref::<ast::InsertStmt>() {
        return if insert.IsReplace {
            ast_sem::ReplaceCommand
        } else {
            ast_sem::InsertCommand
        }
        .to_owned();
    }
    if let Some(explain) = stmt.as_any().downcast_ref::<ast::ExplainStmt>() {
        return if explain.analyze {
            ast_sem::ExplainAnalyzeCommand
        } else {
            ast_sem::ExplainCommand
        }
        .to_owned();
    }
    if let Some(show) = stmt.as_any().downcast_ref::<ast::ShowStmt>() {
        return showCommand(show.Tp).to_owned();
    }
    if let Some(admin) = stmt.as_any().downcast_ref::<ast::AdminStmt>() {
        return adminCommand(admin.statement_type).to_owned();
    }
    if let Some(brie) = stmt.as_any().downcast_ref::<ast::BRIEStmt>() {
        return brieCommand(brie.Kind).to_owned();
    }

    // 其余语句类型一对一映射到固定命令名；未命中则 UnknownCommand。
    macro_rules! fixed_command {
        ($($type:ty => $command:expr),+ $(,)?) => {
            $(if stmt.as_any().is::<$type>() { return $command.to_owned(); })+
        };
    }
    fixed_command! {
        ast::AlterDatabaseStmt => ast_sem::AlterDatabaseCommand,
        ast::AlterInstanceStmt => ast_sem::AlterInstanceCommand,
        ast::AlterPlacementPolicyStmt => ast_sem::AlterPlacementPolicyCommand,
        ast::AlterRangeStmt => ast_sem::AlterRangeCommand,
        ast::AlterResourceGroupStmt => ast_sem::AlterResourceGroupCommand,
        ast::AlterSequenceStmt => ast_sem::AlterSequenceCommand,
        ast::AlterTableStmt => ast_sem::AlterTableCommand,
        ast::AlterUserStmt => ast_sem::AlterUserCommand,
        ast::CleanupTableLockStmt => ast_sem::AdminCleanupTableLockCommand,
        ast::CreateDatabaseStmt => ast_sem::CreateDatabaseCommand,
        ast::CreateIndexStmt => ast_sem::CreateIndexCommand,
        ast::CreatePlacementPolicyStmt => ast_sem::CreatePlacementPolicyCommand,
        ast::CreateMaskingPolicyStmt => ast_sem::CreateMaskingPolicyCommand,
        ast::CreateResourceGroupStmt => ast_sem::CreateResourceGroupCommand,
        ast::CreateSequenceStmt => ast_sem::CreateSequenceCommand,
        ast::CreateTableStmt => ast_sem::CreateTableCommand,
        ast::CreateUserStmt => ast_sem::CreateUserCommand,
        ast::CreateViewStmt => ast_sem::CreateViewCommand,
        ast::DropDatabaseStmt => ast_sem::DropDatabaseCommand,
        ast::DropIndexStmt => ast_sem::DropIndexCommand,
        ast::DropPlacementPolicyStmt => ast_sem::DropPlacementPolicyCommand,
        ast::DropResourceGroupStmt => ast_sem::DropResourceGroupCommand,
        ast::DropSequenceStmt => ast_sem::DropSequenceCommand,
        ast::DropUserStmt => ast_sem::DropUserCommand,
        ast::FlashBackDatabaseStmt => ast_sem::FlashBackDatabaseCommand,
        ast::FlashBackTableStmt => ast_sem::FlashBackTableCommand,
        ast::FlashBackToTimestampStmt => ast_sem::FlashBackClusterCommand,
        ast::LockTablesStmt => ast_sem::LockTablesCommand,
        ast::OptimizeTableStmt => ast_sem::OptimizeTableCommand,
        ast::RecoverTableStmt => ast_sem::RecoverTableCommand,
        ast::RenameTableStmt => ast_sem::RenameTableCommand,
        ast::RenameUserStmt => ast_sem::RenameUserCommand,
        ast::RepairTableStmt => ast_sem::AdminRepairTableCommand,
        ast::TruncateTableStmt => ast_sem::TruncateTableCommand,
        ast::UnlockTablesStmt => ast_sem::UnlockTablesCommand,
        ast::CallStmt => ast_sem::CallCommand,
        ast::DeleteStmt => ast_sem::DeleteCommand,
        ast::DistributeTableStmt => ast_sem::DistributeTableCommand,
        ast::ImportIntoStmt => ast_sem::ImportIntoCommand,
        ast::LoadDataStmt => ast_sem::LoadDataCommand,
        ast::NonTransactionalDMLStmt => ast_sem::BatchCommand,
        ast::SelectStmt => ast_sem::SelectCommand,
        ast::SetOprStmt => ast_sem::SetOprCommand,
        ast::SplitRegionStmt => ast_sem::SplitRegionCommand,
        ast::UpdateStmt => ast_sem::UpdateCommand,
        ast::AddQueryWatchStmt => ast_sem::AddQueryWatchCommand,
        ast::AnalyzeTableStmt => ast_sem::AnalyzeTableCommand,
        ast::BeginStmt => ast_sem::BeginCommand,
        ast::BinlogStmt => ast_sem::BinlogCommand,
        ast::CalibrateResourceStmt => ast_sem::CalibrateResourceCommand,
        ast::CancelDistributionJobStmt => ast_sem::CancelDistributionJobCommand,
        ast::CommitStmt => ast_sem::CommitCommand,
        ast::CompactTableStmt => ast_sem::AlterTableCompactCommand,
        ast::CreateBindingStmt => ast_sem::CreateBindingCommand,
        ast::CreateStatisticsStmt => ast_sem::CreateStatisticsCommand,
        ast::DeallocateStmt => ast_sem::DeallocateCommand,
        ast::DoStmt => ast_sem::DoCommand,
        ast::DropBindingStmt => ast_sem::DropBindingCommand,
        ast::DropQueryWatchStmt => ast_sem::DropQueryWatchCommand,
        ast::DropStatisticsStmt => ast_sem::DropStatisticsCommand,
        ast::ExecuteStmt => ast_sem::ExecuteCommand,
        ast::ExplainForStmt => ast_sem::ExplainForConnectionCommand,
        ast::FlushStmt => ast_sem::FlushCommand,
        ast::GrantStmt => ast_sem::GrantCommand,
        ast::GrantProxyStmt => ast_sem::GrantProxyCommand,
        ast::GrantRoleStmt => ast_sem::GrantRoleCommand,
        ast::HelpStmt => ast_sem::HelpCommand,
        ast::ImportIntoActionStmt => ast_sem::CancelImportIntoJobCommand,
        ast::KillStmt => ast_sem::KillCommand,
        ast::PlanReplayerStmt => ast_sem::PlanReplayerCommand,
        ast::PrepareStmt => ast_sem::PrepareCommand,
        ast::ReleaseSavepointStmt => ast_sem::ReleaseSavepointCommand,
        ast::RestartStmt => ast_sem::RestartCommand,
        ast::RevokeStmt => ast_sem::RevokeCommand,
        ast::RevokeRoleStmt => ast_sem::RevokeRoleCommand,
        ast::RollbackStmt => ast_sem::RollbackCommand,
        ast::SavepointStmt => ast_sem::SavepointCommand,
        ast::SetBindingStmt => ast_sem::SetBindingCommand,
        ast::SetConfigStmt => ast_sem::SetConfigCommand,
        ast::SetDefaultRoleStmt => ast_sem::SetDefaultRoleCommand,
        ast::SetPwdStmt => ast_sem::SetPasswordCommand,
        ast::SetResourceGroupStmt => ast_sem::SetResourceGroupCommand,
        ast::SetRoleStmt => ast_sem::SetRoleCommand,
        ast::SetSessionStatesStmt => ast_sem::SetSessionStatesCommand,
        ast::SetStmt => ast_sem::SetCommand,
        ast::ShutdownStmt => ast_sem::ShutdownCommand,
        ast::TraceStmt => ast_sem::TraceCommand,
        ast::TrafficStmt => ast_sem::TrafficCommand,
        ast::UseStmt => ast_sem::UseCommand,
        ast::RecommendIndexStmt => ast_sem::RecommendIndexCommand,
        ast::LoadStatsStmt => ast_sem::LoadStatsCommand,
        ast::DropStatsStmt => ast_sem::DropStatsCommand,
        ast::LockStatsStmt => ast_sem::LockStatsCommand,
        ast::UnlockStatsStmt => ast_sem::UnlockStatsCommand,
        ast::RefreshStatsStmt => ast_sem::RefreshStatsCommand,
        ast::ProcedureBlock => ast_sem::ProcedureCommand,
        ast::ProcedureInfo => ast_sem::ProcedureCommand,
        ast::DropProcedureStmt => ast_sem::ProcedureCommand,
        ast::ProcedureIfInfo => ast_sem::ProcedureCommand,
        ast::ProcedureElseIfBlock => ast_sem::ProcedureCommand,
        ast::ProcedureElseBlock => ast_sem::ProcedureCommand,
        ast::ProcedureIfBlock => ast_sem::ProcedureCommand,
        ast::SimpleWhenThenStmt => ast_sem::ProcedureCommand,
        ast::SimpleCaseStmt => ast_sem::ProcedureCommand,
        ast::SearchWhenThenStmt => ast_sem::ProcedureCommand,
        ast::SearchCaseStmt => ast_sem::ProcedureCommand,
        ast::ProcedureRepeatStmt => ast_sem::ProcedureCommand,
        ast::ProcedureWhileStmt => ast_sem::ProcedureCommand,
        ast::ProcedureOpenCur => ast_sem::ProcedureCommand,
        ast::ProcedureCloseCur => ast_sem::ProcedureCommand,
        ast::ProcedureFetchInto => ast_sem::ProcedureCommand,
        ast::ProcedureLabelBlock => ast_sem::ProcedureCommand,
        ast::ProcedureLabelLoop => ast_sem::ProcedureCommand,
        ast::ProcedureJump => ast_sem::ProcedureCommand,
        ast::ProcedureErrorCon => ast_sem::ProcedureCommand,
        ast::ProcedureErrorVal => ast_sem::ProcedureCommand,
        ast::ProcedureErrorState => ast_sem::ProcedureCommand,
    }
    ast_sem::UnknownCommand.to_owned()
}

#[allow(non_snake_case)]
/// SHOW 语句子类型到 SEM 命令名。
fn showCommand(statement_type: ast::ShowStmtType) -> &'static str {
    use ast::ShowStmtType::*;
    match statement_type {
        CreateTable => ast_sem::ShowCreateTableCommand,
        CreateView => ast_sem::ShowCreateViewCommand,
        CreateDatabase => ast_sem::ShowCreateDatabaseCommand,
        CreateUser => ast_sem::ShowCreateUserCommand,
        CreateSequence => ast_sem::ShowCreateSequenceCommand,
        CreatePlacementPolicy => ast_sem::ShowCreatePlacementPolicyCommand,
        MaskingPolicies => ast_sem::ShowMaskingPoliciesCommand,
        CreateResourceGroup => ast_sem::ShowCreateResourceGroupCommand,
        CreateProcedure => ast_sem::ShowCreateProcedureCommand,
        Databases => ast_sem::ShowDatabasesCommand,
        Tables => ast_sem::ShowTableCommand,
        TableStatus => ast_sem::ShowTableStatusCommand,
        Columns => ast_sem::ShowColumnsCommand,
        Index => ast_sem::ShowIndexCommand,
        Variables => ast_sem::ShowVariablesCommand,
        Status => ast_sem::ShowStatusCommand,
        ProcessList => ast_sem::ShowProcessListCommand,
        Engines => ast_sem::ShowEnginesCommand,
        Charset => ast_sem::ShowCharsetCommand,
        Collation => ast_sem::ShowCollationCommand,
        Warnings => ast_sem::ShowWarningsCommand,
        Errors => ast_sem::ShowErrorsCommand,
        Grants => ast_sem::ShowGrantsCommand,
        Privileges => ast_sem::ShowPrivilegesCommand,
        Triggers => ast_sem::ShowTriggersCommand,
        ProcedureStatus => ast_sem::ShowProcedureStatusCommand,
        FunctionStatus => ast_sem::ShowFunctionStatusCommand,
        Events => ast_sem::ShowEventsCommand,
        Plugins => ast_sem::ShowPluginsCommand,
        Profile => ast_sem::ShowProfileCommand,
        Profiles => ast_sem::ShowProfilesCommand,
        MasterStatus => ast_sem::ShowMasterStatusCommand,
        BinlogStatus => ast_sem::ShowBinaryLogStatusCommand,
        ReplicaStatus => ast_sem::ShowCommand,
        OpenTables => ast_sem::ShowOpenTablesCommand,
        Config => ast_sem::ShowConfigCommand,
        StatsExtended => ast_sem::ShowStatsExtendedCommand,
        StatsMeta => ast_sem::ShowStatsMetaCommand,
        StatsHistograms => ast_sem::ShowStatsHistogramsCommand,
        StatsTopN => ast_sem::ShowStatsTopNCommand,
        StatsBuckets => ast_sem::ShowStatsBucketsCommand,
        StatsHealthy => ast_sem::ShowStatsHealthyCommand,
        StatsLocked => ast_sem::ShowStatsLockedCommand,
        HistogramsInFlight => ast_sem::ShowHistogramsInFlightCommand,
        ColumnStatsUsage => ast_sem::ShowColumnStatsUsageCommand,
        Bindings => ast_sem::ShowBindingsCommand,
        BindingCacheStatus => ast_sem::ShowBindingCacheStatusCommand,
        AnalyzeStatus => ast_sem::ShowAnalyzeStatusCommand,
        Regions => ast_sem::ShowRegionsCommand,
        Builtins => ast_sem::ShowBuiltinsCommand,
        TableNextRowId => ast_sem::ShowTableNextRowIdCommand,
        Backups => ast_sem::ShowBackupsCommand,
        Restores => ast_sem::ShowRestoresCommand,
        Imports => ast_sem::ShowImportsCommand,
        CreateImport => ast_sem::ShowCreateImportCommand,
        ImportJobs => ast_sem::ShowImportJobsCommand,
        ImportGroups => ast_sem::ShowImportGroupsCommand,
        Placement => ast_sem::ShowPlacementCommand,
        PlacementForDatabase => ast_sem::ShowPlacementForDatabaseCommand,
        PlacementForTable => ast_sem::ShowPlacementForTableCommand,
        PlacementForPartition => ast_sem::ShowPlacementForPartitionCommand,
        PlacementLabels => ast_sem::ShowPlacementLabelsCommand,
        SessionStates => ast_sem::ShowSessionStatesCommand,
        Distributions => ast_sem::ShowDistributionsCommand,
        DistributionJobs => ast_sem::ShowDistributionJobsCommand,
        Affinity => ast_sem::ShowAffinityCommand,
        None => ast_sem::UnknownCommand,
    }
}

#[allow(non_snake_case)]
/// ADMIN 语句子类型到 SEM 命令名。
fn adminCommand(statement_type: ast::AdminStmtType) -> &'static str {
    use ast::AdminStmtType::*;
    match statement_type {
        ShowDdl => ast_sem::AdminShowDDLCommand,
        CheckTable => ast_sem::AdminCheckTableCommand,
        ShowDdlJobs => ast_sem::AdminShowDDLJobsCommand,
        CancelDdlJobs => ast_sem::AdminCancelDDLJobsCommand,
        PauseDdlJobs => ast_sem::AdminPauseDDLJobsCommand,
        ResumeDdlJobs => ast_sem::AdminResumeDDLJobsCommand,
        CheckIndex => ast_sem::AdminCheckIndexCommand,
        RecoverIndex => ast_sem::AdminRecoverIndexCommand,
        CleanupIndex => ast_sem::AdminCleanupIndexCommand,
        CheckIndexRange => ast_sem::AdminCheckIndexRangeCommand,
        ShowDdlJobQueries | ShowDdlJobQueriesWithRange => ast_sem::AdminShowDDLJobQueriesCommand,
        ChecksumTable => ast_sem::AdminChecksumTableCommand,
        ShowSlow => ast_sem::AdminShowSlowCommand,
        ShowNextRowId => ast_sem::AdminShowNextRowIDCommand,
        ReloadExprPushdownBlacklist => ast_sem::AdminReloadExprPushdownBlacklistCommand,
        ReloadOptRuleBlacklist => ast_sem::AdminReloadOptRuleBlacklistCommand,
        PluginDisable => ast_sem::AdminPluginsDisableCommand,
        PluginEnable => ast_sem::AdminPluginsEnableCommand,
        FlushBindings => ast_sem::AdminFlushBindingsCommand,
        CaptureBindings => ast_sem::AdminCaptureBindingsCommand,
        EvolveBindings => ast_sem::AdminEvolveBindingsCommand,
        ReloadBindings => ast_sem::AdminReloadBindingsCommand,
        ReloadStatistics => ast_sem::AdminReloadStatsExtendedCommand,
        FlushPlanCache => ast_sem::AdminFlushPlanCacheCommand,
        SetBdrRole => ast_sem::AdminSetBDRRoleCommand,
        ShowBdrRole => ast_sem::AdminShowBDRRoleCommand,
        UnsetBdrRole => ast_sem::AdminUnsetBDRRoleCommand,
        AlterDdlJob => ast_sem::AdminAlterDDLJobsCommand,
        WorkloadRepoCreate => ast_sem::AdminCreateWorkloadSnapshotCommand,
        ReloadClusterBindings => ast_sem::AdminReloadClusterBindingsCommand,
    }
}

#[allow(non_snake_case)]
/// BRIE（Backup/Restore 等）语句种类到 SEM 命令名。
fn brieCommand(kind: ast::BRIEKind) -> &'static str {
    use ast::BRIEKind::*;
    match kind {
        Backup => ast_sem::BackupCommand,
        Restore => ast_sem::RestoreCommand,
        RestorePIT => ast_sem::RestorePITCommand,
        StreamStart => ast_sem::StreamStartCommand,
        StreamStop => ast_sem::StreamStopCommand,
        StreamPause => ast_sem::StreamPauseCommand,
        StreamResume => ast_sem::StreamResumeCommand,
        StreamStatus => ast_sem::StreamStatusCommand,
        StreamMetaData => ast_sem::StreamMetaDataCommand,
        StreamPurge => ast_sem::StreamPurgeCommand,
        ShowJob => ast_sem::ShowBRJobCommand,
        ShowQuery => ast_sem::ShowBRJobQueryCommand,
        CancelJob => ast_sem::CancelBRJobCommand,
        ShowBackupMeta => ast_sem::ShowBackupMetaCommand,
    }
}
