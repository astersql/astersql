// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// SEM 命令字符串常量非空且不等于 UNKNOWN 的抽样测试。
//
// 分别覆盖 SHOW、ADMIN、BRIE（备份恢复）相关命令分类常量。

use crate::sem::*;
use crate::{
    AdminStmt, AdminStmtType, BRIEKind, BRIEStmt, DropTableStmt, InsertStmt, ShowStmt, ShowStmtType,
};

/// 断言命令列表非空且每项都不是 UnknownCommand。
fn known(values: &[&str]) {
    assert!(!values.is_empty());
    for value in values {
        assert_ne!(*value, UnknownCommand);
    }
}

/// SHOW 族命令常量均已定义且非 UNKNOWN。
#[test]
fn test_show_command() {
    known(&[
        ShowCommand,
        ShowCreateTableCommand,
        ShowCreateViewCommand,
        ShowCreateDatabaseCommand,
        ShowCreateUserCommand,
        ShowCreateSequenceCommand,
        ShowCreatePlacementPolicyCommand,
        ShowMaskingPoliciesCommand,
        ShowCreateResourceGroupCommand,
        ShowCreateProcedureCommand,
        ShowDatabasesCommand,
        ShowTableCommand,
        ShowTableStatusCommand,
        ShowColumnsCommand,
        ShowIndexCommand,
        ShowVariablesCommand,
        ShowStatusCommand,
        ShowProcessListCommand,
        ShowEnginesCommand,
        ShowCharsetCommand,
        ShowCollationCommand,
        ShowWarningsCommand,
        ShowErrorsCommand,
        ShowGrantsCommand,
        ShowPrivilegesCommand,
        ShowTriggersCommand,
        ShowProcedureStatusCommand,
        ShowFunctionStatusCommand,
        ShowEventsCommand,
        ShowPluginsCommand,
        ShowProfileCommand,
        ShowProfilesCommand,
        ShowMasterStatusCommand,
        ShowBinaryLogStatusCommand,
        ShowOpenTablesCommand,
        ShowConfigCommand,
        ShowStatsExtendedCommand,
        ShowStatsMetaCommand,
        ShowStatsHistogramsCommand,
        ShowStatsTopNCommand,
        ShowStatsBucketsCommand,
        ShowStatsHealthyCommand,
        ShowStatsLockedCommand,
        ShowHistogramsInFlightCommand,
        ShowColumnStatsUsageCommand,
        ShowBindingsCommand,
        ShowBindingCacheStatusCommand,
        ShowAnalyzeStatusCommand,
        ShowRegionsCommand,
        ShowBuiltinsCommand,
        ShowTableNextRowIdCommand,
        ShowBackupsCommand,
        ShowRestoresCommand,
        ShowImportsCommand,
        ShowCreateImportCommand,
        ShowImportJobsCommand,
        ShowImportGroupsCommand,
        ShowPlacementCommand,
        ShowPlacementForDatabaseCommand,
        ShowPlacementForTableCommand,
        ShowPlacementForPartitionCommand,
        ShowPlacementLabelsCommand,
        ShowSessionStatesCommand,
        ShowDistributionsCommand,
        ShowPlanCommand,
        ShowDistributionJobsCommand,
        ShowAffinityCommand,
    ]);
}

/// ADMIN 族命令常量均已定义且非 UNKNOWN。
#[test]
fn test_admin_command() {
    known(&[
        AdminShowDDLCommand,
        AdminCheckTableCommand,
        AdminShowDDLJobsCommand,
        AdminCancelDDLJobsCommand,
        AdminPauseDDLJobsCommand,
        AdminResumeDDLJobsCommand,
        AdminCheckIndexCommand,
        AdminRecoverIndexCommand,
        AdminCleanupIndexCommand,
        AdminCheckIndexRangeCommand,
        AdminShowDDLJobQueriesCommand,
        AdminChecksumTableCommand,
        AdminShowSlowCommand,
        AdminShowNextRowIDCommand,
        AdminReloadExprPushdownBlacklistCommand,
        AdminReloadOptRuleBlacklistCommand,
        AdminPluginsDisableCommand,
        AdminPluginsEnableCommand,
        AdminFlushBindingsCommand,
        AdminCaptureBindingsCommand,
        AdminEvolveBindingsCommand,
        AdminReloadBindingsCommand,
        AdminReloadStatsExtendedCommand,
        AdminFlushPlanCacheCommand,
        AdminSetBDRRoleCommand,
        AdminShowBDRRoleCommand,
        AdminUnsetBDRRoleCommand,
        AdminAlterDDLJobsCommand,
        AdminCreateWorkloadSnapshotCommand,
        AdminReloadClusterBindingsCommand,
    ]);
}

/// BRIE（Backup/Restore/Import/Export）相关命令常量。
#[test]
fn test_brie_command() {
    known(&[
        BackupCommand,
        RestoreCommand,
        RestorePITCommand,
        StreamStartCommand,
        StreamStopCommand,
        StreamPauseCommand,
        StreamResumeCommand,
        StreamStatusCommand,
        StreamMetaDataCommand,
        StreamPurgeCommand,
        ShowBRJobCommand,
        ShowBRJobQueryCommand,
        CancelBRJobCommand,
        ShowBackupMetaCommand,
    ]);
}

#[test]
fn test_dynamic_sem_commands_match_go() {
    assert_eq!(DropTableStmt::default().sem_command(), DropTableCommand);
    assert_eq!(
        DropTableStmt {
            IsView: true,
            ..Default::default()
        }
        .sem_command(),
        DropViewCommand
    );
    assert_eq!(InsertStmt::default().sem_command(), InsertCommand);
    assert_eq!(
        InsertStmt {
            IsReplace: true,
            ..Default::default()
        }
        .sem_command(),
        ReplaceCommand
    );

    let show_cases = [
        (ShowStmtType::None, UnknownCommand),
        (ShowStmtType::Engines, ShowEnginesCommand),
        (ShowStmtType::Databases, ShowDatabasesCommand),
        (ShowStmtType::Tables, ShowTableCommand),
        (ShowStmtType::TableStatus, ShowTableStatusCommand),
        (ShowStmtType::Columns, ShowColumnsCommand),
        (ShowStmtType::Warnings, ShowWarningsCommand),
        (ShowStmtType::Charset, ShowCharsetCommand),
        (ShowStmtType::Variables, ShowVariablesCommand),
        (ShowStmtType::Status, ShowStatusCommand),
        (ShowStmtType::Collation, ShowCollationCommand),
        (ShowStmtType::CreateTable, ShowCreateTableCommand),
        (ShowStmtType::CreateView, ShowCreateViewCommand),
        (ShowStmtType::CreateUser, ShowCreateUserCommand),
        (ShowStmtType::CreateSequence, ShowCreateSequenceCommand),
        (
            ShowStmtType::CreatePlacementPolicy,
            ShowCreatePlacementPolicyCommand,
        ),
        (ShowStmtType::Grants, ShowGrantsCommand),
        (ShowStmtType::MaskingPolicies, ShowMaskingPoliciesCommand),
        (ShowStmtType::Triggers, ShowTriggersCommand),
        (ShowStmtType::ProcedureStatus, ShowProcedureStatusCommand),
        (ShowStmtType::FunctionStatus, ShowFunctionStatusCommand),
        (ShowStmtType::Index, ShowIndexCommand),
        (ShowStmtType::ProcessList, ShowProcessListCommand),
        (ShowStmtType::CreateDatabase, ShowCreateDatabaseCommand),
        (ShowStmtType::Config, ShowConfigCommand),
        (ShowStmtType::Events, ShowEventsCommand),
        (ShowStmtType::StatsExtended, ShowStatsExtendedCommand),
        (ShowStmtType::StatsMeta, ShowStatsMetaCommand),
        (ShowStmtType::StatsHistograms, ShowStatsHistogramsCommand),
        (ShowStmtType::StatsTopN, ShowStatsTopNCommand),
        (ShowStmtType::StatsBuckets, ShowStatsBucketsCommand),
        (ShowStmtType::StatsHealthy, ShowStatsHealthyCommand),
        (ShowStmtType::StatsLocked, ShowStatsLockedCommand),
        (
            ShowStmtType::HistogramsInFlight,
            ShowHistogramsInFlightCommand,
        ),
        (ShowStmtType::ColumnStatsUsage, ShowColumnStatsUsageCommand),
        (ShowStmtType::Plugins, ShowPluginsCommand),
        (ShowStmtType::Profile, ShowProfileCommand),
        (ShowStmtType::Profiles, ShowProfilesCommand),
        (ShowStmtType::MasterStatus, ShowMasterStatusCommand),
        (ShowStmtType::Privileges, ShowPrivilegesCommand),
        (ShowStmtType::Errors, ShowErrorsCommand),
        (ShowStmtType::Bindings, ShowBindingsCommand),
        (
            ShowStmtType::BindingCacheStatus,
            ShowBindingCacheStatusCommand,
        ),
        (ShowStmtType::OpenTables, ShowOpenTablesCommand),
        (ShowStmtType::AnalyzeStatus, ShowAnalyzeStatusCommand),
        (ShowStmtType::Regions, ShowRegionsCommand),
        (ShowStmtType::Builtins, ShowBuiltinsCommand),
        (ShowStmtType::TableNextRowId, ShowTableNextRowIdCommand),
        (ShowStmtType::Backups, ShowBackupsCommand),
        (ShowStmtType::Restores, ShowRestoresCommand),
        (ShowStmtType::Imports, ShowImportsCommand),
        (ShowStmtType::CreateImport, ShowCreateImportCommand),
        (ShowStmtType::Placement, ShowPlacementCommand),
        (
            ShowStmtType::PlacementForDatabase,
            ShowPlacementForDatabaseCommand,
        ),
        (
            ShowStmtType::PlacementForTable,
            ShowPlacementForTableCommand,
        ),
        (
            ShowStmtType::PlacementForPartition,
            ShowPlacementForPartitionCommand,
        ),
        (ShowStmtType::PlacementLabels, ShowPlacementLabelsCommand),
        (ShowStmtType::SessionStates, ShowSessionStatesCommand),
        (
            ShowStmtType::CreateResourceGroup,
            ShowCreateResourceGroupCommand,
        ),
        (ShowStmtType::ImportJobs, ShowImportJobsCommand),
        (ShowStmtType::ImportGroups, ShowImportGroupsCommand),
        (ShowStmtType::CreateProcedure, ShowCreateProcedureCommand),
        (ShowStmtType::BinlogStatus, ShowBinaryLogStatusCommand),
        (ShowStmtType::ReplicaStatus, ShowCommand),
        (ShowStmtType::Distributions, ShowDistributionsCommand),
        (ShowStmtType::DistributionJobs, ShowDistributionJobsCommand),
        (ShowStmtType::Affinity, ShowAffinityCommand),
    ];
    for (kind, command) in show_cases {
        assert_eq!(
            ShowStmt {
                Tp: kind,
                ..Default::default()
            }
            .sem_command(),
            command
        );
    }

    let admin_cases = [
        (AdminStmtType::ShowDdl, AdminShowDDLCommand),
        (AdminStmtType::ShowDdlJobs, AdminShowDDLJobsCommand),
        (AdminStmtType::ShowSlow, AdminShowSlowCommand),
        (AdminStmtType::CaptureBindings, AdminCaptureBindingsCommand),
        (AdminStmtType::ShowNextRowId, AdminShowNextRowIDCommand),
        (
            AdminStmtType::ShowDdlJobQueries,
            AdminShowDDLJobQueriesCommand,
        ),
        (
            AdminStmtType::ShowDdlJobQueriesWithRange,
            AdminShowDDLJobQueriesCommand,
        ),
        (AdminStmtType::CheckTable, AdminCheckTableCommand),
        (
            AdminStmtType::WorkloadRepoCreate,
            AdminCreateWorkloadSnapshotCommand,
        ),
        (
            AdminStmtType::ReloadExprPushdownBlacklist,
            AdminReloadExprPushdownBlacklistCommand,
        ),
        (
            AdminStmtType::ReloadOptRuleBlacklist,
            AdminReloadOptRuleBlacklistCommand,
        ),
        (AdminStmtType::FlushBindings, AdminFlushBindingsCommand),
        (AdminStmtType::EvolveBindings, AdminEvolveBindingsCommand),
        (AdminStmtType::ReloadBindings, AdminReloadBindingsCommand),
        (
            AdminStmtType::ReloadClusterBindings,
            AdminReloadClusterBindingsCommand,
        ),
        (
            AdminStmtType::ReloadStatistics,
            AdminReloadStatsExtendedCommand,
        ),
        (AdminStmtType::ShowBdrRole, AdminShowBDRRoleCommand),
        (AdminStmtType::UnsetBdrRole, AdminUnsetBDRRoleCommand),
        (AdminStmtType::CancelDdlJobs, AdminCancelDDLJobsCommand),
        (AdminStmtType::PauseDdlJobs, AdminPauseDDLJobsCommand),
        (AdminStmtType::ResumeDdlJobs, AdminResumeDDLJobsCommand),
        (AdminStmtType::CheckIndex, AdminCheckIndexCommand),
        (AdminStmtType::RecoverIndex, AdminRecoverIndexCommand),
        (AdminStmtType::CleanupIndex, AdminCleanupIndexCommand),
        (AdminStmtType::ChecksumTable, AdminChecksumTableCommand),
        (AdminStmtType::CheckIndexRange, AdminCheckIndexRangeCommand),
        (AdminStmtType::PluginEnable, AdminPluginsEnableCommand),
        (AdminStmtType::PluginDisable, AdminPluginsDisableCommand),
        (AdminStmtType::FlushPlanCache, AdminFlushPlanCacheCommand),
        (AdminStmtType::SetBdrRole, AdminSetBDRRoleCommand),
        (AdminStmtType::AlterDdlJob, AdminAlterDDLJobsCommand),
    ];
    for (kind, command) in admin_cases {
        assert_eq!(AdminStmt::new(kind).sem_command(), command);
    }

    let brie_cases = [
        (BRIEKind::Backup, BackupCommand),
        (BRIEKind::CancelJob, CancelBRJobCommand),
        (BRIEKind::StreamStart, StreamStartCommand),
        (BRIEKind::StreamMetaData, StreamMetaDataCommand),
        (BRIEKind::StreamStatus, StreamStatusCommand),
        (BRIEKind::StreamPause, StreamPauseCommand),
        (BRIEKind::StreamResume, StreamResumeCommand),
        (BRIEKind::StreamStop, StreamStopCommand),
        (BRIEKind::StreamPurge, StreamPurgeCommand),
        (BRIEKind::Restore, RestoreCommand),
        (BRIEKind::RestorePIT, RestorePITCommand),
        (BRIEKind::ShowJob, ShowBRJobCommand),
        (BRIEKind::ShowQuery, ShowBRJobQueryCommand),
        (BRIEKind::ShowBackupMeta, ShowBackupMetaCommand),
    ];
    for (kind, command) in brie_cases {
        assert_eq!(
            BRIEStmt {
                Kind: kind,
                ..Default::default()
            }
            .sem_command(),
            command
        );
    }
}
