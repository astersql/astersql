// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

// AST 语句的 SEM（语义）命令分类：将节点映射为稳定的命令字符串。
//
// 用于权限/审计等按“语句种类”归类；常量值与 Go 对齐。
// 完整节点接线启用 `typed_sem_impls`；接线前可用 `SemStatement` 值分类。

// 仅做 AST 元数据分类。
// SEMCommand 对应 Go 中每个语句节点暴露的命令分类方法。
/// 返回该语句节点对应的 SEM 命令字符串（如 `"SELECT"`）。
pub trait SEMCommand {
    fn sem_command(&self) -> &'static str;
}

// 命令字符串逐项保留 Go 常量值；显式常量避免调用方依赖节点格式化结果。
/// 下列常量为各语句 SEM 分类的稳定字符串，值与 Go 逐项一致。
pub const AlterDatabaseCommand: &str = "ALTER DATABASE";
pub const AlterInstanceCommand: &str = "ALTER INSTANCE";
pub const AlterPlacementPolicyCommand: &str = "ALTER PLACEMENT POLICY";
pub const AlterRangeCommand: &str = "ALTER RANGE";
pub const AlterResourceGroupCommand: &str = "ALTER RESOURCE GROUP";
pub const AlterSequenceCommand: &str = "ALTER SEQUENCE";
pub const AlterTableCommand: &str = "ALTER TABLE";
pub const AlterUserCommand: &str = "ALTER USER";
pub const AdminCleanupTableLockCommand: &str = "ADMIN CLEANUP TABLE LOCK";
pub const CreateDatabaseCommand: &str = "CREATE DATABASE";
pub const CreateIndexCommand: &str = "CREATE INDEX";
pub const CreatePlacementPolicyCommand: &str = "CREATE PLACEMENT POLICY";
pub const CreateMaskingPolicyCommand: &str = "CREATE MASKING POLICY";
pub const CreateResourceGroupCommand: &str = "CREATE RESOURCE GROUP";
pub const CreateSequenceCommand: &str = "CREATE SEQUENCE";
pub const CreateTableCommand: &str = "CREATE TABLE";
pub const CreateUserCommand: &str = "CREATE USER";
pub const CreateViewCommand: &str = "CREATE VIEW";
pub const DropDatabaseCommand: &str = "DROP DATABASE";
pub const DropIndexCommand: &str = "DROP INDEX";
pub const DropPlacementPolicyCommand: &str = "DROP PLACEMENT POLICY";
pub const DropResourceGroupCommand: &str = "DROP RESOURCE GROUP";
pub const DropSequenceCommand: &str = "DROP SEQUENCE";
pub const DropTableCommand: &str = "DROP TABLE";
pub const DropViewCommand: &str = "DROP VIEW";
pub const DropUserCommand: &str = "DROP USER";
pub const FlashBackDatabaseCommand: &str = "FLASHBACK DATABASE";
pub const FlashBackTableCommand: &str = "FLASHBACK TABLE";
pub const FlashBackClusterCommand: &str = "FLASHBACK CLUSTER";
pub const LockTablesCommand: &str = "LOCK TABLES";
pub const OptimizeTableCommand: &str = "OPTIMIZE TABLE";
pub const RecoverTableCommand: &str = "RECOVER TABLE";
pub const RenameTableCommand: &str = "RENAME TABLE";
pub const RenameUserCommand: &str = "RENAME USER";
pub const AdminRepairTableCommand: &str = "ADMIN REPAIR TABLE";
pub const TruncateTableCommand: &str = "TRUNCATE TABLE";
pub const UnlockTablesCommand: &str = "UNLOCK TABLES";
pub const CallCommand: &str = "CALL";
pub const DeleteCommand: &str = "DELETE";
pub const DistributeTableCommand: &str = "DISTRIBUTE TABLE";
pub const ImportIntoCommand: &str = "IMPORT INTO";
pub const ReplaceCommand: &str = "REPLACE";
pub const InsertCommand: &str = "INSERT";
pub const LoadDataCommand: &str = "LOAD DATA";
pub const BatchCommand: &str = "BATCH";
pub const SelectCommand: &str = "SELECT";
pub const SplitRegionCommand: &str = "SPLIT REGION";
pub const UpdateCommand: &str = "UPDATE";
pub const ShowCommand: &str = "SHOW";
pub const ShowCreateTableCommand: &str = "SHOW CREATE TABLE";
pub const ShowCreateViewCommand: &str = "SHOW CREATE VIEW";
pub const ShowCreateDatabaseCommand: &str = "SHOW CREATE DATABASE";
pub const ShowCreateUserCommand: &str = "SHOW CREATE USER";
pub const ShowCreateSequenceCommand: &str = "SHOW CREATE SEQUENCE";
pub const ShowCreatePlacementPolicyCommand: &str = "SHOW CREATE PLACEMENT POLICY";
pub const ShowMaskingPoliciesCommand: &str = "SHOW MASKING POLICIES";
pub const ShowCreateResourceGroupCommand: &str = "SHOW CREATE RESOURCE GROUP";
pub const ShowCreateProcedureCommand: &str = "SHOW CREATE PROCEDURE";
pub const ShowDatabasesCommand: &str = "SHOW DATABASES";
pub const ShowTableCommand: &str = "SHOW TABLE";
pub const ShowTableStatusCommand: &str = "SHOW TABLE STATUS";
pub const ShowColumnsCommand: &str = "SHOW COLUMNS";
pub const ShowIndexCommand: &str = "SHOW INDEX";
pub const ShowVariablesCommand: &str = "SHOW VARIABLES";
pub const ShowStatusCommand: &str = "SHOW STATUS";
pub const ShowProcessListCommand: &str = "SHOW PROCESSLIST";
pub const ShowEnginesCommand: &str = "SHOW ENGINES";
pub const ShowCharsetCommand: &str = "SHOW CHARSET";
pub const ShowCollationCommand: &str = "SHOW COLLATION";
pub const ShowWarningsCommand: &str = "SHOW WARNINGS";
pub const ShowErrorsCommand: &str = "SHOW ERRORS";
pub const ShowGrantsCommand: &str = "SHOW GRANTS";
pub const ShowPrivilegesCommand: &str = "SHOW PRIVILEGES";
pub const ShowTriggersCommand: &str = "SHOW TRIGGERS";
pub const ShowProcedureStatusCommand: &str = "SHOW PROCEDURE STATUS";
pub const ShowFunctionStatusCommand: &str = "SHOW FUNCTION STATUS";
pub const ShowEventsCommand: &str = "SHOW EVENTS";
pub const ShowPluginsCommand: &str = "SHOW PLUGINS";
pub const ShowProfileCommand: &str = "SHOW PROFILE";
pub const ShowProfilesCommand: &str = "SHOW PROFILES";
pub const ShowMasterStatusCommand: &str = "SHOW MASTER STATUS";
pub const ShowBinaryLogStatusCommand: &str = "SHOW BINARY LOG STATUS";
pub const ShowOpenTablesCommand: &str = "SHOW OPEN TABLES";
pub const ShowConfigCommand: &str = "SHOW CONFIG";
pub const ShowStatsExtendedCommand: &str = "SHOW STATS_EXTENDED";
pub const ShowStatsMetaCommand: &str = "SHOW STATS_META";
pub const ShowStatsHistogramsCommand: &str = "SHOW STATS_HISTOGRAMS";
pub const ShowStatsTopNCommand: &str = "SHOW STATS_TOPN";
pub const ShowStatsBucketsCommand: &str = "SHOW STATS_BUCKETS";
pub const ShowStatsHealthyCommand: &str = "SHOW STATS_HEALTHY";
pub const ShowStatsLockedCommand: &str = "SHOW STATS_LOCKED";
pub const ShowHistogramsInFlightCommand: &str = "SHOW HISTOGRAMS_IN_FLIGHT";
pub const ShowColumnStatsUsageCommand: &str = "SHOW COLUMN_STATS_USAGE";
pub const ShowBindingsCommand: &str = "SHOW BINDINGS";
pub const ShowBindingCacheStatusCommand: &str = "SHOW BINDING_CACHE STATUS";
pub const ShowAnalyzeStatusCommand: &str = "SHOW ANALYZE STATUS";
pub const ShowRegionsCommand: &str = "SHOW TABLE REGIONS";
pub const ShowBuiltinsCommand: &str = "SHOW BUILTINS";
pub const ShowTableNextRowIdCommand: &str = "SHOW TABLE NEXT_ROW_ID";
pub const ShowBackupsCommand: &str = "SHOW BACKUPS";
pub const ShowRestoresCommand: &str = "SHOW RESTORES";
pub const ShowImportsCommand: &str = "SHOW IMPORTS";
pub const ShowCreateImportCommand: &str = "SHOW CREATE IMPORT";
pub const ShowImportJobsCommand: &str = "SHOW IMPORT JOBS";
pub const ShowImportGroupsCommand: &str = "SHOW IMPORT GROUPS";
pub const ShowPlacementCommand: &str = "SHOW PLACEMENT";
pub const ShowPlacementForDatabaseCommand: &str = "SHOW PLACEMENT FOR DATABASE";
pub const ShowPlacementForTableCommand: &str = "SHOW PLACEMENT FOR TABLE";
pub const ShowPlacementForPartitionCommand: &str = "SHOW PLACEMENT FOR PARTITION";
pub const ShowPlacementLabelsCommand: &str = "SHOW PLACEMENT LABELS";
pub const ShowSessionStatesCommand: &str = "SHOW SESSION_STATES";
pub const ShowDistributionsCommand: &str = "SHOW DISTRIBUTIONS";
pub const ShowPlanCommand: &str = "SHOW PLAN";
pub const ShowDistributionJobsCommand: &str = "SHOW DISTRIBUTION JOB";
pub const ShowAffinityCommand: &str = "SHOW AFFINITY";
pub const ShowStorageClassTransitionsCommand: &str = "SHOW STORAGE_CLASS TRANSITIONS";
pub const AdminShowDDLCommand: &str = "ADMIN SHOW DDL";
pub const AdminCheckTableCommand: &str = "ADMIN CHECK TABLE";
pub const AdminShowDDLJobsCommand: &str = "ADMIN SHOW DDL JOBS";
pub const AdminCancelDDLJobsCommand: &str = "ADMIN CANCEL DDL JOBS";
pub const AdminPauseDDLJobsCommand: &str = "ADMIN PAUSE DDL JOBS";
pub const AdminResumeDDLJobsCommand: &str = "ADMIN RESUME DDL JOBS";
pub const AdminCheckIndexCommand: &str = "ADMIN CHECK INDEX";
pub const AdminRecoverIndexCommand: &str = "ADMIN RECOVER INDEX";
pub const AdminCleanupIndexCommand: &str = "ADMIN CLEANUP INDEX";
pub const AdminCheckIndexRangeCommand: &str = "ADMIN CHECK INDEX RANGE";
pub const AdminShowDDLJobQueriesCommand: &str = "ADMIN SHOW DDL JOB QUERIES";
pub const AdminChecksumTableCommand: &str = "ADMIN CHECKSUM TABLE";
pub const AdminShowSlowCommand: &str = "ADMIN SHOW SLOW";
pub const AdminShowNextRowIDCommand: &str = "ADMIN SHOW NEXT_ROW_ID";
pub const AdminReloadExprPushdownBlacklistCommand: &str = "ADMIN RELOAD EXPR_PUSHDOWN_BLACKLIST";
pub const AdminReloadOptRuleBlacklistCommand: &str = "ADMIN RELOAD OPT_RULE_BLACKLIST";
pub const AdminPluginsDisableCommand: &str = "ADMIN PLUGINS DISABLE";
pub const AdminPluginsEnableCommand: &str = "ADMIN PLUGINS ENABLE";
pub const AdminFlushBindingsCommand: &str = "ADMIN FLUSH BINDINGS";
pub const AdminCaptureBindingsCommand: &str = "ADMIN CAPTURE BINDINGS";
pub const AdminEvolveBindingsCommand: &str = "ADMIN EVOLVE BINDINGS";
pub const AdminReloadBindingsCommand: &str = "ADMIN RELOAD BINDINGS";
pub const AdminReloadStatsExtendedCommand: &str = "ADMIN RELOAD STATS_EXTENDED";
pub const AdminFlushPlanCacheCommand: &str = "ADMIN FLUSH PLAN_CACHE";
pub const AdminSetBDRRoleCommand: &str = "ADMIN SET BDR ROLE";
pub const AdminShowBDRRoleCommand: &str = "ADMIN SHOW BDR ROLE";
pub const AdminUnsetBDRRoleCommand: &str = "ADMIN UNSET BDR ROLE";
pub const AdminAlterDDLJobsCommand: &str = "ADMIN ALTER DDL JOBS";
pub const AdminCreateWorkloadSnapshotCommand: &str = "ADMIN CREATE WORKLOAD SNAPSHOT";
pub const AdminReloadClusterBindingsCommand: &str = "ADMIN RELOAD CLUSTER BINDINGS";
pub const BackupCommand: &str = "BACKUP";
pub const RestoreCommand: &str = "RESTORE";
pub const RestorePITCommand: &str = "RESTORE POINT";
pub const StreamStartCommand: &str = "BACKUP LOGS";
pub const StreamStopCommand: &str = "STOP BACKUP LOGS";
pub const StreamPauseCommand: &str = "PAUSE BACKUP LOGS";
pub const StreamResumeCommand: &str = "RESUME BACKUP LOGS";
pub const StreamStatusCommand: &str = "SHOW BACKUP LOGS STATUS";
pub const StreamMetaDataCommand: &str = "SHOW BACKUP LOGS METADATA";
pub const StreamPurgeCommand: &str = "PURGE BACKUP LOGS";
pub const ShowBRJobCommand: &str = "SHOW BR JOB";
pub const ShowBRJobQueryCommand: &str = "SHOW BR JOB QUERY";
pub const CancelBRJobCommand: &str = "CANCEL BR JOB";
pub const ShowBackupMetaCommand: &str = "SHOW BACKUP META";
pub const AddQueryWatchCommand: &str = "ADD QUERY WATCH";
pub const AnalyzeTableCommand: &str = "ANALYZE TABLE";
pub const BeginCommand: &str = "BEGIN";
pub const BinlogCommand: &str = "BINLOG";
pub const CalibrateResourceCommand: &str = "CALIBRATE RESOURCE";
pub const CancelDistributionJobCommand: &str = "CANCEL DISTRIBUTION JOB";
pub const CommitCommand: &str = "COMMIT";
pub const AlterTableCompactCommand: &str = "ALTER TABLE COMPACT";
pub const CreateBindingCommand: &str = "CREATE BINDING";
pub const CreateStatisticsCommand: &str = "CREATE STATISTICS";
pub const DeallocateCommand: &str = "DEALLOCATE";
pub const DoCommand: &str = "DO";
pub const DropBindingCommand: &str = "DROP BINDING";
pub const DropQueryWatchCommand: &str = "DROP QUERY WATCH";
pub const DropStatisticsCommand: &str = "DROP STATISTICS";
pub const ExecuteCommand: &str = "EXECUTE";
pub const ExplainForConnectionCommand: &str = "EXPLAIN FOR CONNECTION";
pub const ExplainAnalyzeCommand: &str = "EXPLAIN ANALYZE";
pub const ExplainCommand: &str = "EXPLAIN";
pub const FlushCommand: &str = "FLUSH";
pub const GrantCommand: &str = "GRANT";
pub const GrantProxyCommand: &str = "GRANT PROXY";
pub const GrantRoleCommand: &str = "GRANT ROLE";
pub const HelpCommand: &str = "HELP";
pub const CancelImportIntoJobCommand: &str = "CANCEL IMPORT INTO JOB";
pub const KillCommand: &str = "KILL";
pub const PlanReplayerCommand: &str = "PLAN REPLAYER";
pub const PrepareCommand: &str = "PREPARE";
pub const ReleaseSavepointCommand: &str = "RELEASE SAVEPOINT";
pub const RestartCommand: &str = "RESTART";
pub const RevokeCommand: &str = "REVOKE";
pub const RevokeRoleCommand: &str = "REVOKE ROLE";
pub const RollbackCommand: &str = "ROLLBACK";
pub const SavepointCommand: &str = "SAVEPOINT";
pub const SetBindingCommand: &str = "SET BINDING";
pub const SetConfigCommand: &str = "SET CONFIG";
pub const SetDefaultRoleCommand: &str = "SET DEFAULT ROLE";
pub const SetPasswordCommand: &str = "SET PASSWORD";
pub const SetResourceGroupCommand: &str = "SET RESOURCE GROUP";
pub const SetRoleCommand: &str = "SET ROLE";
pub const SetSessionStatesCommand: &str = "SET SESSION_STATES";
pub const SetCommand: &str = "SET";
pub const ShutdownCommand: &str = "SHUTDOWN";
pub const TraceCommand: &str = "TRACE";
pub const TrafficCommand: &str = "TRAFFIC";
pub const UseCommand: &str = "USE";
pub const LoadStatsCommand: &str = "LOAD STATS";
pub const DropStatsCommand: &str = "DROP STATS";
pub const LockStatsCommand: &str = "LOCK STATS";
pub const UnlockStatsCommand: &str = "UNLOCK STATS";
pub const RefreshStatsCommand: &str = "REFRESH STATS";
pub const RecommendIndexCommand: &str = "RECOMMEND INDEX";
pub const ProcedureCommand: &str = "PROCEDURE";
pub const UnknownCommand: &str = "UNKNOWN";
pub const SetOprCommand: &str = "SET OPERATION";

use crate::{AdminStmt, AdminStmtType, BRIEKind, BRIEStmt, ShowStmt, ShowStmtType};

impl SEMCommand for ShowStmt {
    fn sem_command(&self) -> &'static str {
        match self.Tp {
            ShowStmtType::None => UnknownCommand,
            ShowStmtType::Engines => ShowEnginesCommand,
            ShowStmtType::Databases => ShowDatabasesCommand,
            ShowStmtType::Tables => ShowTableCommand,
            ShowStmtType::TableStatus => ShowTableStatusCommand,
            ShowStmtType::Columns => ShowColumnsCommand,
            ShowStmtType::Warnings => ShowWarningsCommand,
            ShowStmtType::Charset => ShowCharsetCommand,
            ShowStmtType::Variables => ShowVariablesCommand,
            ShowStmtType::Status => ShowStatusCommand,
            ShowStmtType::Collation => ShowCollationCommand,
            ShowStmtType::CreateTable => ShowCreateTableCommand,
            ShowStmtType::CreateView => ShowCreateViewCommand,
            ShowStmtType::CreateUser => ShowCreateUserCommand,
            ShowStmtType::CreateSequence => ShowCreateSequenceCommand,
            ShowStmtType::CreatePlacementPolicy => ShowCreatePlacementPolicyCommand,
            ShowStmtType::Grants => ShowGrantsCommand,
            ShowStmtType::MaskingPolicies => ShowMaskingPoliciesCommand,
            ShowStmtType::Triggers => ShowTriggersCommand,
            ShowStmtType::ProcedureStatus => ShowProcedureStatusCommand,
            ShowStmtType::FunctionStatus => ShowFunctionStatusCommand,
            ShowStmtType::Index => ShowIndexCommand,
            ShowStmtType::ProcessList => ShowProcessListCommand,
            ShowStmtType::CreateDatabase => ShowCreateDatabaseCommand,
            ShowStmtType::Config => ShowConfigCommand,
            ShowStmtType::Events => ShowEventsCommand,
            ShowStmtType::StatsExtended => ShowStatsExtendedCommand,
            ShowStmtType::StatsMeta => ShowStatsMetaCommand,
            ShowStmtType::StatsHistograms => ShowStatsHistogramsCommand,
            ShowStmtType::StatsTopN => ShowStatsTopNCommand,
            ShowStmtType::StatsBuckets => ShowStatsBucketsCommand,
            ShowStmtType::StatsHealthy => ShowStatsHealthyCommand,
            ShowStmtType::StatsLocked => ShowStatsLockedCommand,
            ShowStmtType::HistogramsInFlight => ShowHistogramsInFlightCommand,
            ShowStmtType::ColumnStatsUsage => ShowColumnStatsUsageCommand,
            ShowStmtType::Plugins => ShowPluginsCommand,
            ShowStmtType::Profile => ShowProfileCommand,
            ShowStmtType::Profiles => ShowProfilesCommand,
            ShowStmtType::MasterStatus => ShowMasterStatusCommand,
            ShowStmtType::Privileges => ShowPrivilegesCommand,
            ShowStmtType::Errors => ShowErrorsCommand,
            ShowStmtType::Bindings => ShowBindingsCommand,
            ShowStmtType::BindingCacheStatus => ShowBindingCacheStatusCommand,
            ShowStmtType::OpenTables => ShowOpenTablesCommand,
            ShowStmtType::AnalyzeStatus => ShowAnalyzeStatusCommand,
            ShowStmtType::Regions => ShowRegionsCommand,
            ShowStmtType::Builtins => ShowBuiltinsCommand,
            ShowStmtType::TableNextRowId => ShowTableNextRowIdCommand,
            ShowStmtType::Backups => ShowBackupsCommand,
            ShowStmtType::Restores => ShowRestoresCommand,
            ShowStmtType::Imports => ShowImportsCommand,
            ShowStmtType::CreateImport => ShowCreateImportCommand,
            ShowStmtType::Placement => ShowPlacementCommand,
            ShowStmtType::PlacementForDatabase => ShowPlacementForDatabaseCommand,
            ShowStmtType::PlacementForTable => ShowPlacementForTableCommand,
            ShowStmtType::PlacementForPartition => ShowPlacementForPartitionCommand,
            ShowStmtType::PlacementLabels => ShowPlacementLabelsCommand,
            ShowStmtType::SessionStates => ShowSessionStatesCommand,
            ShowStmtType::CreateResourceGroup => ShowCreateResourceGroupCommand,
            ShowStmtType::ImportJobs => ShowImportJobsCommand,
            ShowStmtType::ImportGroups => ShowImportGroupsCommand,
            ShowStmtType::CreateProcedure => ShowCreateProcedureCommand,
            ShowStmtType::BinlogStatus => ShowBinaryLogStatusCommand,
            ShowStmtType::ReplicaStatus => ShowCommand,
            ShowStmtType::Distributions => ShowDistributionsCommand,
            ShowStmtType::DistributionJobs => ShowDistributionJobsCommand,
            ShowStmtType::Affinity => ShowAffinityCommand,
            ShowStmtType::StorageClassTransitions => ShowStorageClassTransitionsCommand,
        }
    }
}

impl SEMCommand for AdminStmt {
    fn sem_command(&self) -> &'static str {
        match self.statement_type {
            AdminStmtType::ShowDdl => AdminShowDDLCommand,
            AdminStmtType::ShowDdlJobs => AdminShowDDLJobsCommand,
            AdminStmtType::ShowSlow => AdminShowSlowCommand,
            AdminStmtType::CaptureBindings => AdminCaptureBindingsCommand,
            AdminStmtType::ShowNextRowId => AdminShowNextRowIDCommand,
            AdminStmtType::ShowDdlJobQueries | AdminStmtType::ShowDdlJobQueriesWithRange => {
                AdminShowDDLJobQueriesCommand
            }
            AdminStmtType::CheckTable => AdminCheckTableCommand,
            AdminStmtType::WorkloadRepoCreate => AdminCreateWorkloadSnapshotCommand,
            AdminStmtType::ReloadExprPushdownBlacklist => AdminReloadExprPushdownBlacklistCommand,
            AdminStmtType::ReloadOptRuleBlacklist => AdminReloadOptRuleBlacklistCommand,
            AdminStmtType::FlushBindings => AdminFlushBindingsCommand,
            AdminStmtType::EvolveBindings => AdminEvolveBindingsCommand,
            AdminStmtType::ReloadBindings => AdminReloadBindingsCommand,
            AdminStmtType::ReloadClusterBindings => AdminReloadClusterBindingsCommand,
            AdminStmtType::ReloadStatistics => AdminReloadStatsExtendedCommand,
            AdminStmtType::ShowBdrRole => AdminShowBDRRoleCommand,
            AdminStmtType::UnsetBdrRole => AdminUnsetBDRRoleCommand,
            AdminStmtType::CancelDdlJobs => AdminCancelDDLJobsCommand,
            AdminStmtType::PauseDdlJobs => AdminPauseDDLJobsCommand,
            AdminStmtType::ResumeDdlJobs => AdminResumeDDLJobsCommand,
            AdminStmtType::CheckIndex => AdminCheckIndexCommand,
            AdminStmtType::RecoverIndex => AdminRecoverIndexCommand,
            AdminStmtType::CleanupIndex => AdminCleanupIndexCommand,
            AdminStmtType::ChecksumTable => AdminChecksumTableCommand,
            AdminStmtType::CheckIndexRange => AdminCheckIndexRangeCommand,
            AdminStmtType::PluginEnable => AdminPluginsEnableCommand,
            AdminStmtType::PluginDisable => AdminPluginsDisableCommand,
            AdminStmtType::FlushPlanCache => AdminFlushPlanCacheCommand,
            AdminStmtType::SetBdrRole => AdminSetBDRRoleCommand,
            AdminStmtType::AlterDdlJob => AdminAlterDDLJobsCommand,
        }
    }
}

impl SEMCommand for BRIEStmt {
    fn sem_command(&self) -> &'static str {
        match self.Kind {
            BRIEKind::Backup => BackupCommand,
            BRIEKind::CancelJob => CancelBRJobCommand,
            BRIEKind::StreamStart => StreamStartCommand,
            BRIEKind::StreamMetaData => StreamMetaDataCommand,
            BRIEKind::StreamStatus => StreamStatusCommand,
            BRIEKind::StreamPause => StreamPauseCommand,
            BRIEKind::StreamResume => StreamResumeCommand,
            BRIEKind::StreamStop => StreamStopCommand,
            BRIEKind::StreamPurge => StreamPurgeCommand,
            BRIEKind::Restore => RestoreCommand,
            BRIEKind::RestorePIT => RestorePITCommand,
            BRIEKind::ShowJob => ShowBRJobCommand,
            BRIEKind::ShowQuery => ShowBRJobQueryCommand,
            BRIEKind::ShowBackupMeta => ShowBackupMetaCommand,
        }
    }
}

// 大多数节点无条件返回固定分类；宏只消除重复 impl，不隐藏原有一对一映射。
// 固定分类直接接到对应 AST 节点；宏只消除重复的一行实现。
mod typed_sem_impls {
    use super::*;
    use crate::*;
    macro_rules! fixed_sem_command {
    ($($ty:ty => $command:expr),+ $(,)?) => { $(
        impl SEMCommand for $ty {
            fn sem_command(&self) -> &'static str { $command }
        }
    )+ };
}

    fixed_sem_command! {
        AlterDatabaseStmt => AlterDatabaseCommand,
        AlterInstanceStmt => AlterInstanceCommand,
        AlterPlacementPolicyStmt => AlterPlacementPolicyCommand,
        AlterRangeStmt => AlterRangeCommand,
        AlterResourceGroupStmt => AlterResourceGroupCommand,
        AlterSequenceStmt => AlterSequenceCommand,
        AlterTableStmt => AlterTableCommand,
        AlterUserStmt => AlterUserCommand,
        CleanupTableLockStmt => AdminCleanupTableLockCommand,
        CreateDatabaseStmt => CreateDatabaseCommand,
        CreateIndexStmt => CreateIndexCommand,
        CreatePlacementPolicyStmt => CreatePlacementPolicyCommand,
        CreateMaskingPolicyStmt => CreateMaskingPolicyCommand,
        CreateResourceGroupStmt => CreateResourceGroupCommand,
        CreateSequenceStmt => CreateSequenceCommand,
        CreateTableStmt => CreateTableCommand,
        CreateUserStmt => CreateUserCommand,
        CreateViewStmt => CreateViewCommand,
        DropDatabaseStmt => DropDatabaseCommand,
        DropIndexStmt => DropIndexCommand,
        DropPlacementPolicyStmt => DropPlacementPolicyCommand,
        DropResourceGroupStmt => DropResourceGroupCommand,
        DropSequenceStmt => DropSequenceCommand,
        DropUserStmt => DropUserCommand,
        FlashBackDatabaseStmt => FlashBackDatabaseCommand,
        FlashBackTableStmt => FlashBackTableCommand,
        FlashBackToTimestampStmt => FlashBackClusterCommand,
        LockTablesStmt => LockTablesCommand,
        OptimizeTableStmt => OptimizeTableCommand,
        RecoverTableStmt => RecoverTableCommand,
        RenameTableStmt => RenameTableCommand,
        RenameUserStmt => RenameUserCommand,
        RepairTableStmt => AdminRepairTableCommand,
        TruncateTableStmt => TruncateTableCommand,
        UnlockTablesStmt => UnlockTablesCommand,
        CallStmt => CallCommand,
        DeleteStmt => DeleteCommand,
        DistributeTableStmt => DistributeTableCommand,
        ImportIntoStmt => ImportIntoCommand,
        LoadDataStmt => LoadDataCommand,
        NonTransactionalDMLStmt => BatchCommand,
        SelectStmt => SelectCommand,
        SetOprStmt => SetOprCommand,
        SplitRegionStmt => SplitRegionCommand,
        UpdateStmt => UpdateCommand,
        AddQueryWatchStmt => AddQueryWatchCommand,
        AnalyzeTableStmt => AnalyzeTableCommand,
        BeginStmt => BeginCommand,
        BinlogStmt => BinlogCommand,
        CalibrateResourceStmt => CalibrateResourceCommand,
        CancelDistributionJobStmt => CancelDistributionJobCommand,
        CommitStmt => CommitCommand,
        CompactTableStmt => AlterTableCompactCommand,
        CreateBindingStmt => CreateBindingCommand,
        CreateStatisticsStmt => CreateStatisticsCommand,
        DeallocateStmt => DeallocateCommand,
        DoStmt => DoCommand,
        DropBindingStmt => DropBindingCommand,
        DropQueryWatchStmt => DropQueryWatchCommand,
        DropStatisticsStmt => DropStatisticsCommand,
        ExecuteStmt => ExecuteCommand,
        ExplainForStmt => ExplainForConnectionCommand,
        FlushStmt => FlushCommand,
        GrantStmt => GrantCommand,
        GrantProxyStmt => GrantProxyCommand,
        GrantRoleStmt => GrantRoleCommand,
        HelpStmt => HelpCommand,
        ImportIntoActionStmt => CancelImportIntoJobCommand,
        KillStmt => KillCommand,
        PlanReplayerStmt => PlanReplayerCommand,
        PrepareStmt => PrepareCommand,
        ReleaseSavepointStmt => ReleaseSavepointCommand,
        RestartStmt => RestartCommand,
        RevokeStmt => RevokeCommand,
        RevokeRoleStmt => RevokeRoleCommand,
        RollbackStmt => RollbackCommand,
        SavepointStmt => SavepointCommand,
        SetBindingStmt => SetBindingCommand,
        SetConfigStmt => SetConfigCommand,
        SetDefaultRoleStmt => SetDefaultRoleCommand,
        SetPwdStmt => SetPasswordCommand,
        SetResourceGroupStmt => SetResourceGroupCommand,
        SetRoleStmt => SetRoleCommand,
        SetSessionStatesStmt => SetSessionStatesCommand,
        SetStmt => SetCommand,
        ShutdownStmt => ShutdownCommand,
        TraceStmt => TraceCommand,
        TrafficStmt => TrafficCommand,
        UseStmt => UseCommand,
        RecommendIndexStmt => RecommendIndexCommand,
        LoadStatsStmt => LoadStatsCommand,
        DropStatsStmt => DropStatsCommand,
        LockStatsStmt => LockStatsCommand,
        UnlockStatsStmt => UnlockStatsCommand,
        RefreshStatsStmt => RefreshStatsCommand,
        ProcedureBlock => ProcedureCommand,
        ProcedureInfo => ProcedureCommand,
        DropProcedureStmt => ProcedureCommand,
        ProcedureIfInfo => ProcedureCommand,
        ProcedureElseIfBlock => ProcedureCommand,
        ProcedureElseBlock => ProcedureCommand,
        ProcedureIfBlock => ProcedureCommand,
        SimpleWhenThenStmt => ProcedureCommand,
        SimpleCaseStmt => ProcedureCommand,
        SearchWhenThenStmt => ProcedureCommand,
        SearchCaseStmt => ProcedureCommand,
        ProcedureRepeatStmt => ProcedureCommand,
        ProcedureWhileStmt => ProcedureCommand,
        ProcedureOpenCur => ProcedureCommand,
        ProcedureCloseCur => ProcedureCommand,
        ProcedureFetchInto => ProcedureCommand,
        ProcedureLabelBlock => ProcedureCommand,
        ProcedureLabelLoop => ProcedureCommand,
        ProcedureJump => ProcedureCommand,
        ProcedureErrorCon => ProcedureCommand,
        ProcedureErrorVal => ProcedureCommand,
        ProcedureErrorState => ProcedureCommand,
    }

    // DropTableStmt 保留 Go 中依赖节点字段的动态命令分类分支。
    impl SEMCommand for DropTableStmt {
        fn sem_command(&self) -> &'static str {
            // IsView 改变 SQL 动词，必须先于默认命令判断。
            if self.IsView {
                DropViewCommand
            } else {
                DropTableCommand
            }
        }
    }

    // InsertStmt 保留 Go 中依赖节点字段的动态命令分类分支。
    impl SEMCommand for InsertStmt {
        fn sem_command(&self) -> &'static str {
            // IsReplace 改变 SQL 动词，必须先于默认命令判断。
            if self.IsReplace {
                ReplaceCommand
            } else {
                InsertCommand
            }
        }
    }

    /* Show/Admin/BRIE use the crate's canonical enums and are implemented above.
    impl SEMCommand for ShowStmt {
        fn sem_command(&self) -> &'static str {
            match self.tp {
                ShowCreateTable => ShowCreateTableCommand,
                ShowCreateView => ShowCreateViewCommand,
                ShowCreateDatabase => ShowCreateDatabaseCommand,
                ShowCreateUser => ShowCreateUserCommand,
                ShowCreateSequence => ShowCreateSequenceCommand,
                ShowCreatePlacementPolicy => ShowCreatePlacementPolicyCommand,
                ShowMaskingPolicies => ShowMaskingPoliciesCommand,
                ShowCreateResourceGroup => ShowCreateResourceGroupCommand,
                ShowCreateProcedure => ShowCreateProcedureCommand,
                ShowDatabases => ShowDatabasesCommand,
                ShowTables => ShowTableCommand,
                ShowTableStatus => ShowTableStatusCommand,
                ShowColumns => ShowColumnsCommand,
                ShowIndex => ShowIndexCommand,
                ShowVariables => ShowVariablesCommand,
                ShowStatus => ShowStatusCommand,
                ShowProcessList => ShowProcessListCommand,
                ShowEngines => ShowEnginesCommand,
                ShowCharset => ShowCharsetCommand,
                ShowCollation => ShowCollationCommand,
                ShowWarnings => ShowWarningsCommand,
                ShowErrors => ShowErrorsCommand,
                ShowGrants => ShowGrantsCommand,
                ShowPrivileges => ShowPrivilegesCommand,
                ShowTriggers => ShowTriggersCommand,
                ShowProcedureStatus => ShowProcedureStatusCommand,
                ShowFunctionStatus => ShowFunctionStatusCommand,
                ShowEvents => ShowEventsCommand,
                ShowPlugins => ShowPluginsCommand,
                ShowProfile => ShowProfileCommand,
                ShowProfiles => ShowProfilesCommand,
                ShowMasterStatus => ShowMasterStatusCommand,
                ShowBinlogStatus => ShowBinaryLogStatusCommand,
                ShowReplicaStatus => ShowCommand,
                ShowOpenTables => ShowOpenTablesCommand,
                ShowConfig => ShowConfigCommand,
                ShowStatsExtended => ShowStatsExtendedCommand,
                ShowStatsMeta => ShowStatsMetaCommand,
                ShowStatsHistograms => ShowStatsHistogramsCommand,
                ShowStatsTopN => ShowStatsTopNCommand,
                ShowStatsBuckets => ShowStatsBucketsCommand,
                ShowStatsHealthy => ShowStatsHealthyCommand,
                ShowStatsLocked => ShowStatsLockedCommand,
                ShowHistogramsInFlight => ShowHistogramsInFlightCommand,
                ShowColumnStatsUsage => ShowColumnStatsUsageCommand,
                ShowBindings => ShowBindingsCommand,
                ShowBindingCacheStatus => ShowBindingCacheStatusCommand,
                ShowAnalyzeStatus => ShowAnalyzeStatusCommand,
                ShowRegions => ShowRegionsCommand,
                ShowBuiltins => ShowBuiltinsCommand,
                ShowTableNextRowId => ShowTableNextRowIdCommand,
                ShowBackups => ShowBackupsCommand,
                ShowRestores => ShowRestoresCommand,
                ShowImports => ShowImportsCommand,
                ShowCreateImport => ShowCreateImportCommand,
                ShowImportJobs => ShowImportJobsCommand,
                ShowImportGroups => ShowImportGroupsCommand,
                ShowPlacement => ShowPlacementCommand,
                ShowPlacementForDatabase => ShowPlacementForDatabaseCommand,
                ShowPlacementForTable => ShowPlacementForTableCommand,
                ShowPlacementForPartition => ShowPlacementForPartitionCommand,
                ShowPlacementLabels => ShowPlacementLabelsCommand,
                ShowSessionStates => ShowSessionStatesCommand,
                ShowDistributions => ShowDistributionsCommand,
                ShowDistributionJobs => ShowDistributionJobsCommand,
                ShowAffinity => ShowAffinityCommand,
                _ => UnknownCommand,
            }
        }
    }

    // AdminStmt 保留 Go 中依赖节点字段的动态命令分类分支。
    impl SEMCommand for AdminStmt {
        fn sem_command(&self) -> &'static str {
            // 未识别的枚举值与 Go default 一致归入 UNKNOWN。
            match self.tp {
                AdminShowDDL => AdminShowDDLCommand,
                AdminCheckTable => AdminCheckTableCommand,
                AdminShowDDLJobs => AdminShowDDLJobsCommand,
                AdminCancelDDLJobs => AdminCancelDDLJobsCommand,
                AdminPauseDDLJobs => AdminPauseDDLJobsCommand,
                AdminResumeDDLJobs => AdminResumeDDLJobsCommand,
                AdminCheckIndex => AdminCheckIndexCommand,
                AdminRecoverIndex => AdminRecoverIndexCommand,
                AdminCleanupIndex => AdminCleanupIndexCommand,
                AdminCheckIndexRange => AdminCheckIndexRangeCommand,
                AdminShowDDLJobQueries => AdminShowDDLJobQueriesCommand,
                AdminShowDDLJobQueriesWithRange => AdminShowDDLJobQueriesCommand,
                AdminChecksumTable => AdminChecksumTableCommand,
                AdminShowSlow => AdminShowSlowCommand,
                AdminShowNextRowID => AdminShowNextRowIDCommand,
                AdminReloadExprPushdownBlacklist => AdminReloadExprPushdownBlacklistCommand,
                AdminReloadOptRuleBlacklist => AdminReloadOptRuleBlacklistCommand,
                AdminPluginDisable => AdminPluginsDisableCommand,
                AdminPluginEnable => AdminPluginsEnableCommand,
                AdminFlushBindings => AdminFlushBindingsCommand,
                AdminCaptureBindings => AdminCaptureBindingsCommand,
                AdminEvolveBindings => AdminEvolveBindingsCommand,
                AdminReloadBindings => AdminReloadBindingsCommand,
                AdminReloadStatistics => AdminReloadStatsExtendedCommand,
                AdminFlushPlanCache => AdminFlushPlanCacheCommand,
                AdminSetBDRRole => AdminSetBDRRoleCommand,
                AdminShowBDRRole => AdminShowBDRRoleCommand,
                AdminUnsetBDRRole => AdminUnsetBDRRoleCommand,
                AdminAlterDDLJob => AdminAlterDDLJobsCommand,
                AdminWorkloadRepoCreate => AdminCreateWorkloadSnapshotCommand,
                AdminReloadClusterBindings => AdminReloadClusterBindingsCommand,
                _ => UnknownCommand,
            }
        }
    }

    // BRIEStmt 保留 Go 中依赖节点字段的动态命令分类分支。
    impl SEMCommand for BRIEStmt {
        fn sem_command(&self) -> &'static str {
            // 未识别的枚举值与 Go default 一致归入 UNKNOWN。
            match self.kind {
                BRIEKindBackup => BackupCommand,
                BRIEKindRestore => RestoreCommand,
                BRIEKindRestorePIT => RestorePITCommand,
                BRIEKindStreamStart => StreamStartCommand,
                BRIEKindStreamStop => StreamStopCommand,
                BRIEKindStreamPause => StreamPauseCommand,
                BRIEKindStreamResume => StreamResumeCommand,
                BRIEKindStreamStatus => StreamStatusCommand,
                BRIEKindStreamMetaData => StreamMetaDataCommand,
                BRIEKindStreamPurge => StreamPurgeCommand,
                BRIEKindShowJob => ShowBRJobCommand,
                BRIEKindShowQuery => ShowBRJobQueryCommand,
                BRIEKindCancelJob => CancelBRJobCommand,
                BRIEKindShowBackupMeta => ShowBackupMetaCommand,
                _ => UnknownCommand,
            }
        }
    }

    */

    // ExplainStmt 保留 Go 中依赖节点字段的动态命令分类分支。
    impl SEMCommand for ExplainStmt {
        fn sem_command(&self) -> &'static str {
            // Analyze 改变 SQL 动词，必须先于默认命令判断。
            if self.analyze {
                ExplainAnalyzeCommand
            } else {
                ExplainCommand
            }
        }
    }
}

/// A standalone SEM classification value used before all AST node modules are wired together.
/// 接线前的独立 SEM 分类值：固定命令，或带动态字段的 Drop/Insert/Explain。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemStatement {
    /// 固定命令字符串。
    Fixed(&'static str),
    /// DROP TABLE；view=true 时分类为 DROP VIEW。
    DropTable { view: bool },
    /// INSERT；replace=true 时分类为 REPLACE。
    Insert { replace: bool },
    /// EXPLAIN；analyze=true 时分类为 EXPLAIN ANALYZE。
    Explain { analyze: bool },
}

impl SEMCommand for SemStatement {
    fn sem_command(&self) -> &'static str {
        // 动态变体按字段选择命令，固定变体直接返回。
        match *self {
            SemStatement::Fixed(command) => command,
            SemStatement::DropTable { view: true } => DropViewCommand,
            SemStatement::DropTable { view: false } => DropTableCommand,
            SemStatement::Insert { replace: true } => ReplaceCommand,
            SemStatement::Insert { replace: false } => InsertCommand,
            SemStatement::Explain { analyze: true } => ExplainAnalyzeCommand,
            SemStatement::Explain { analyze: false } => ExplainCommand,
        }
    }
}
