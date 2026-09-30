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

use super::super::*;
use super::{Context, Rhs};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AdminRule {
    ResourceGroupOptionListAlt01,
    ResourceGroupOptionListAlt02,
    ResourceGroupOptionListAlt03,
    ResourceGroupPriorityOptionAlt01,
    ResourceGroupPriorityOptionAlt02,
    ResourceGroupPriorityOptionAlt03,
    ResourceGroupRunawayOptionListAlt01,
    ResourceGroupRunawayOptionListAlt02,
    ResourceGroupRunawayOptionListAlt03,
    ResourceGroupRunawayWatchOptionAlt01,
    ResourceGroupRunawayWatchOptionAlt02,
    ResourceGroupRunawayWatchOptionAlt03,
    ResourceGroupRunawayActionOptionAlt01,
    ResourceGroupRunawayActionOptionAlt02,
    ResourceGroupRunawayActionOptionAlt03,
    ResourceGroupRunawayActionOptionAlt04,
    DirectResourceGroupRunawayOptionAlt01,
    DirectResourceGroupRunawayOptionAlt02,
    DirectResourceGroupRunawayOptionAlt03,
    DirectResourceGroupRunawayOptionAlt04,
    DirectResourceGroupRunawayOptionAlt05,
    WatchDurationOptionAlt01,
    WatchDurationOptionAlt02,
    WatchDurationOptionAlt03,
    DirectResourceGroupOptionAlt01,
    DirectResourceGroupOptionAlt02,
    DirectResourceGroupOptionAlt03,
    DirectResourceGroupOptionAlt04,
    DirectResourceGroupOptionAlt05,
    DirectResourceGroupOptionAlt06,
    DirectResourceGroupOptionAlt07,
    DirectResourceGroupOptionAlt08,
    DirectResourceGroupOptionAlt09,
    DirectResourceGroupOptionAlt10,
    DirectResourceGroupOptionAlt11,
    DirectResourceGroupOptionAlt12,
    DirectResourceGroupOptionAlt13,
    ResourceGroupBackgroundOptionListAlt01,
    ResourceGroupBackgroundOptionListAlt02,
    ResourceGroupBackgroundOptionListAlt03,
    DirectResourceGroupBackgroundOptionAlt01,
    DirectResourceGroupBackgroundOptionAlt02,
    AnalyzeTableStmtAlt01,
    AnalyzeTableStmtAlt02,
    AnalyzeTableStmtAlt03,
    AnalyzeTableStmtAlt04,
    AnalyzeTableStmtAlt05,
    AnalyzeTableStmtAlt06,
    AnalyzeTableStmtAlt07,
    AnalyzeTableStmtAlt08,
    AnalyzeTableStmtAlt09,
    AnalyzeTableStmtAlt10,
    AllColumnsOrPredicateColumnsOptAlt01,
    AllColumnsOrPredicateColumnsOptAlt02,
    AllColumnsOrPredicateColumnsOptAlt03,
    AnalyzeOptionListOptAlt01,
    AnalyzeOptionListOptAlt02,
    AnalyzeOptionListAlt01,
    AnalyzeOptionListAlt02,
    AnalyzeOptionListAlt03,
    AnalyzeOptionAlt01,
    AnalyzeOptionAlt02,
    AnalyzeOptionAlt03,
    AnalyzeOptionAlt04,
    AnalyzeOptionAlt05,
    AnalyzeOptionAlt06,
    AnalyzeOptionAlt07,
    AnalyzeOptionAlt08,
    AnalyzeOptionAlt09,
    AnalyzeOptionAlt10,
    AnalyzeOptionAlt11,
    BinlogStmtAlt01,
    IdentListWithParenOptAlt01,
    IdentListWithParenOptAlt02,
    IdentListAlt01,
    IdentListAlt02,
    NotSymAlt02,
    StatsTypeAlt01,
    StatsTypeAlt02,
    StatsTypeAlt03,
    BindingStatusTypeAlt01,
    BindingStatusTypeAlt02,
    CreateStatisticsStmtAlt01,
    DropStatisticsStmtAlt01,
    DropStatsStmtAlt01,
    DropStatsStmtAlt02,
    DropStatsStmtAlt03,
    BRIEStmtAlt01,
    BRIEStmtAlt02,
    BRIEStmtAlt03,
    BRIEStmtAlt04,
    BRIEStmtAlt05,
    BRIEStmtAlt06,
    BRIEStmtAlt07,
    BRIEStmtAlt08,
    BRIEStmtAlt09,
    BRIEStmtAlt10,
    BRIEStmtAlt11,
    BRIEStmtAlt12,
    BRIEStmtAlt13,
    BRIEStmtAlt14,
    BRIETablesAlt01,
    BRIETablesAlt02,
    BRIETablesAlt03,
    DBNameListAlt01,
    DBNameListAlt02,
    BRIEOptionsAlt01,
    BRIEOptionsAlt02,
    BRIEIntegerOptionNameAlt01,
    BRIEIntegerOptionNameAlt02,
    BRIEIntegerOptionNameAlt03,
    BRIEIntegerOptionNameAlt04,
    BRIEBooleanOptionNameAlt01,
    BRIEBooleanOptionNameAlt02,
    BRIEBooleanOptionNameAlt03,
    BRIEBooleanOptionNameAlt04,
    BRIEBooleanOptionNameAlt05,
    BRIEBooleanOptionNameAlt06,
    BRIEBooleanOptionNameAlt07,
    BRIEBooleanOptionNameAlt08,
    BRIEBooleanOptionNameAlt09,
    BRIEBooleanOptionNameAlt10,
    BRIEBooleanOptionNameAlt11,
    BRIEBooleanOptionNameAlt12,
    BRIEStringOptionNameAlt01,
    BRIEStringOptionNameAlt02,
    BRIEStringOptionNameAlt03,
    BRIEStringOptionNameAlt04,
    BRIEStringOptionNameAlt05,
    BRIEStringOptionNameAlt06,
    BRIEStringOptionNameAlt07,
    BRIEKeywordOptionNameAlt01,
    BRIEKeywordOptionNameAlt02,
    BRIEKeywordOptionNameAlt03,
    BRIEOptionAlt01,
    BRIEOptionAlt02,
    BRIEOptionAlt03,
    BRIEOptionAlt04,
    BRIEOptionAlt05,
    BRIEOptionAlt06,
    BRIEOptionAlt07,
    BRIEOptionAlt08,
    BRIEOptionAlt09,
    BRIEOptionAlt10,
    BRIEOptionAlt11,
    BRIEOptionAlt12,
    BRIEOptionAlt13,
    BRIEOptionAlt14,
    BRIEOptionAlt15,
    BRIEOptionAlt16,
    BRIEOptionAlt17,
    BRIEOptionAlt18,
    BRIEOptionAlt19,
    BRIEOptionAlt20,
    BRIEOptionAlt21,
    BooleanAlt01,
    BooleanAlt02,
    BooleanAlt03,
    IfExistsAlt01,
    IfExistsAlt02,
    IfNotExistsAlt01,
    IfNotExistsAlt02,
    IgnoreOptionalAlt01,
    IgnoreOptionalAlt02,
    AlterOrderListAlt01,
    AlterOrderListAlt02,
    AlterOrderItemAlt01,
    ShutdownStmtAlt01,
    RestartStmtAlt01,
    SetStmtAlt01,
    SetStmtAlt02,
    SetStmtAlt03,
    SetStmtAlt04,
    SetStmtAlt05,
    SetStmtAlt06,
    SetStmtAlt07,
    SetStmtAlt08,
    SetStmtAlt09,
    SetStmtAlt10,
    SetStmtAlt11,
    SetStmtAlt12,
    SetExprAlt01,
    SetExprAlt02,
    VariableNameAlt02,
    ConfigItemNameAlt02,
    ConfigItemNameAlt03,
    VariableAssignmentAlt01,
    VariableAssignmentAlt02,
    VariableAssignmentAlt03,
    VariableAssignmentAlt04,
    VariableAssignmentAlt05,
    VariableAssignmentAlt06,
    VariableAssignmentAlt07,
    VariableAssignmentAlt08,
    VariableAssignmentAlt09,
    VariableAssignmentAlt10,
    VariableAssignmentAlt11,
    VariableAssignmentAlt12,
    CharsetNameOrDefaultAlt01,
    CharsetNameOrDefaultAlt02,
    VariableAssignmentListAlt01,
    VariableAssignmentListAlt02,
    AdminStmtLimitOptAlt01,
    AdminStmtLimitOptAlt02,
    AdminStmtLimitOptAlt03,
    AdminStmtAlt01,
    AdminStmtAlt02,
    AdminStmtAlt03,
    AdminStmtAlt04,
    AdminStmtAlt05,
    AdminStmtAlt06,
    AdminStmtAlt07,
    AdminStmtAlt08,
    AdminStmtAlt09,
    AdminStmtAlt10,
    AdminStmtAlt11,
    AdminStmtAlt12,
    AdminStmtAlt13,
    AdminStmtAlt14,
    AdminStmtAlt15,
    AdminStmtAlt16,
    AdminStmtAlt17,
    AdminStmtAlt18,
    AdminStmtAlt19,
    AdminStmtAlt20,
    AdminStmtAlt21,
    AdminStmtAlt22,
    AdminStmtAlt23,
    AdminStmtAlt24,
    AdminStmtAlt25,
    AdminStmtAlt26,
    AdminStmtAlt27,
    AdminStmtAlt28,
    AdminStmtAlt29,
    AdminStmtAlt30,
    AdminStmtAlt31,
    AdminStmtAlt32,
    AdminStmtAlt33,
    AdminStmtAlt34,
    AdminStmtAlt35,
    AlterJobOptionListAlt01,
    AlterJobOptionListAlt02,
    AlterJobOptionAlt01,
    AdminShowSlowAlt01,
    AdminShowSlowAlt02,
    AdminShowSlowAlt03,
    AdminShowSlowAlt04,
    HandleRangeListAlt01,
    HandleRangeListAlt02,
    HandleRangeAlt01,
    NumListAlt01,
    NumListAlt02,
    ShowStmtAlt01,
    ShowStmtAlt02,
    ShowStmtAlt03,
    ShowStmtAlt04,
    ShowStmtAlt05,
    ShowStmtAlt06,
    ShowStmtAlt07,
    ShowStmtAlt08,
    ShowStmtAlt09,
    ShowStmtAlt10,
    ShowStmtAlt11,
    ShowStmtAlt12,
    ShowStmtAlt13,
    ShowStmtAlt14,
    ShowStmtAlt15,
    ShowStmtAlt16,
    ShowStmtAlt17,
    ShowStmtAlt18,
    ShowStmtAlt19,
    ShowStmtAlt20,
    ShowStmtAlt21,
    ShowStmtAlt22,
    ShowStmtAlt23,
    ShowStmtAlt24,
    ShowStmtAlt25,
    ShowStmtAlt26,
    ShowStmtAlt27,
    ShowPlacementTargetAlt01,
    ShowPlacementTargetAlt02,
    ShowPlacementTargetAlt03,
    ShowProfileTypesOptAlt01,
    ShowProfileTypesAlt01,
    ShowProfileTypesAlt02,
    ShowProfileTypeAlt01,
    ShowProfileTypeAlt02,
    ShowProfileTypeAlt03,
    ShowProfileTypeAlt04,
    ShowProfileTypeAlt05,
    ShowProfileTypeAlt06,
    ShowProfileTypeAlt07,
    ShowProfileTypeAlt08,
    ShowProfileTypeAlt09,
    ShowProfileArgsOptAlt01,
    ShowProfileArgsOptAlt02,
    UsingRolesAlt01,
    UsingRolesAlt02,
    ShowTargetFilterableAlt01,
    ShowTargetFilterableAlt02,
    ShowTargetFilterableAlt03,
    ShowTargetFilterableAlt04,
    ShowTargetFilterableAlt05,
    ShowTargetFilterableAlt06,
    ShowTargetFilterableAlt07,
    ShowTargetFilterableAlt08,
    ShowTargetFilterableAlt09,
    ShowTargetFilterableAlt10,
    ShowTargetFilterableAlt11,
    ShowTargetFilterableAlt12,
    ShowTargetFilterableAlt13,
    ShowTargetFilterableAlt14,
    ShowTargetFilterableAlt15,
    ShowTargetFilterableAlt16,
    ShowTargetFilterableAlt17,
    ShowTargetFilterableAlt18,
    ShowTargetFilterableAlt19,
    ShowTargetFilterableAlt20,
    ShowTargetFilterableAlt21,
    ShowTargetFilterableAlt22,
    ShowTargetFilterableAlt23,
    ShowTargetFilterableAlt24,
    ShowTargetFilterableAlt25,
    ShowTargetFilterableAlt26,
    ShowTargetFilterableAlt27,
    ShowTargetFilterableAlt28,
    ShowTargetFilterableAlt29,
    ShowTargetFilterableAlt30,
    ShowTargetFilterableAlt31,
    ShowTargetFilterableAlt32,
    ShowTargetFilterableAlt33,
    ShowTargetFilterableAlt34,
    ShowTargetFilterableAlt35,
    ShowTargetFilterableAlt36,
    ShowTargetFilterableAlt37,
    ShowTargetFilterableAlt38,
    ShowTargetFilterableAlt39,
    ShowTargetFilterableAlt40,
    ShowTargetFilterableAlt41,
    ShowTargetFilterableAlt42,
    ShowTargetFilterableAlt43,
    ShowTargetFilterableAlt44,
    ShowTargetFilterableAlt45,
    ShowTargetFilterableAlt46,
    ShowLikeOrWhereOptAlt01,
    ShowLikeOrWhereOptAlt02,
    ShowLikeOrWhereOptAlt03,
    ShowImportJobTargetAlt01,
    ShowImportJobTargetAlt02,
    ShowImportJobsTargetAlt01,
    ShowImportJobsTargetAlt02,
    GlobalScopeAlt01,
    GlobalScopeAlt02,
    GlobalScopeAlt03,
    StatementScopeAlt01,
    StatementScopeAlt02,
    StatementScopeAlt03,
    StatementScopeAlt04,
    OptFullAlt01,
    OptFullAlt02,
    ShowDatabaseNameOptAlt01,
    ShowDatabaseNameOptAlt02,
    ShowTableAliasOptAlt01,
    FlushStmtAlt01,
    PluginNameListAlt01,
    PluginNameListAlt02,
    FlushOptionAlt01,
    FlushOptionAlt02,
    FlushOptionAlt03,
    FlushOptionAlt04,
    FlushOptionAlt05,
    FlushOptionAlt06,
    FlushOptionAlt07,
    FlushOptionAlt08,
    LogTypeOptAlt01,
    LogTypeOptAlt02,
    LogTypeOptAlt03,
    LogTypeOptAlt04,
    LogTypeOptAlt05,
    LogTypeOptAlt06,
    ClusterOptAlt01,
    ClusterOptAlt02,
    NoWriteToBinLogAliasOptAlt01,
    NoWriteToBinLogAliasOptAlt02,
    NoWriteToBinLogAliasOptAlt03,
    TableNameListOptAlt01,
    WithReadLockOptAlt01,
    WithReadLockOptAlt02,
    AlterInstanceStmtAlt01,
    InstanceOptionAlt01,
    InstanceOptionAlt02,
    HashStringAlt02,
    CreateBindingStmtAlt01,
    CreateBindingStmtAlt02,
    CreateBindingStmtAlt03,
    DropBindingStmtAlt01,
    DropBindingStmtAlt02,
    DropBindingStmtAlt03,
    SetBindingStmtAlt01,
    SetBindingStmtAlt02,
    SetBindingStmtAlt03,
    RecommendIndexStmtAlt01,
    RecommendIndexStmtAlt02,
    RecommendIndexStmtAlt03,
    RecommendIndexStmtAlt04,
    RecommendIndexStmtAlt05,
    RecommendIndexStmtAlt06,
    RecommendIndexOptionListOptAlt01,
    RecommendIndexOptionListOptAlt02,
    RecommendIndexOptionListAlt01,
    RecommendIndexOptionListAlt02,
    RecommendIndexOptionAlt01,
    GrantStmtAlt01,
    RevokeStmtAlt01,
    UnlockTablesStmtAlt01,
    LockTablesStmtAlt01,
    TableLockAlt01,
    LockTypeAlt01,
    LockTypeAlt02,
    LockTypeAlt03,
    LockTypeAlt04,
    TableLockListAlt01,
    TableLockListAlt02,
    OptimizeTableStmtAlt01,
    KillStmtAlt01,
    KillStmtAlt02,
    KillStmtAlt03,
    KillStmtAlt04,
    KillOrKillTiDBAlt01,
    KillOrKillTiDBAlt02,
    LoadStatsStmtAlt01,
    LockStatsStmtAlt01,
    LockStatsStmtAlt02,
    LockStatsStmtAlt03,
    UnlockStatsStmtAlt01,
    UnlockStatsStmtAlt02,
    UnlockStatsStmtAlt03,
    RefreshStatsStmtAlt01,
    StatsObjectListAlt01,
    StatsObjectListAlt02,
    RefreshStatsModeOptAlt01,
    RefreshStatsModeOptAlt02,
    RefreshStatsModeAlt01,
    RefreshStatsModeAlt02,
    RefreshStatsClusterOptAlt01,
    RefreshStatsClusterOptAlt02,
    StatsObjectAlt01,
    StatsObjectAlt02,
    StatsObjectAlt03,
    StatsObjectAlt04,
    CreateResourceGroupStmtAlt01,
    AlterResourceGroupStmtAlt01,
    DropResourceGroupStmtAlt01,
    MaskingPolicyStateOptAlt01,
    MaskingPolicyStateOptAlt02,
    MaskingPolicyStateOptAlt03,
    MaskingPolicyRestrictOnOptAlt01,
    MaskingPolicyRestrictOnOptAlt02,
    MaskingPolicyRestrictOnOptAlt03,
    MaskingPolicyRestrictOperationListAlt01,
    MaskingPolicyRestrictOperationListAlt02,
    MaskingPolicyRestrictOperationAlt01,
    CreateMaskingPolicyStmtAlt01,
    CreateSequenceStmtAlt01,
    CreateSequenceTableOptionListOptAlt01,
    CreateSequenceOptionListOptAlt01,
    SequenceOptionListAlt01,
    SequenceOptionListAlt02,
    SequenceOptionAlt01,
    SequenceOptionAlt02,
    SequenceOptionAlt03,
    SequenceOptionAlt04,
    SequenceOptionAlt05,
    SequenceOptionAlt06,
    SequenceOptionAlt07,
    SequenceOptionAlt08,
    SequenceOptionAlt09,
    SequenceOptionAlt10,
    SequenceOptionAlt11,
    SequenceOptionAlt12,
    SequenceOptionAlt13,
    SequenceOptionAlt14,
    SequenceOptionAlt15,
    SequenceOptionAlt16,
    DropSequenceStmtAlt01,
    AlterSequenceStmtAlt01,
    AlterSequenceOptionListAlt01,
    AlterSequenceOptionListAlt02,
    AlterSequenceOptionAlt02,
    AlterSequenceOptionAlt03,
    AlterSequenceOptionAlt04,
    PlanReplayerStmtAlt01,
    PlanReplayerStmtAlt02,
    PlanReplayerStmtAlt03,
    PlanReplayerStmtAlt04,
    PlanReplayerStmtAlt05,
    PlanReplayerStmtAlt06,
    PlanReplayerStmtAlt07,
    PlanReplayerStmtAlt08,
    PlanReplayerStmtAlt09,
    PlanReplayerStmtAlt10,
    PlanReplayerStmtAlt11,
    PlanReplayerDumpOptAlt01,
    PlanReplayerDumpOptAlt02,
    TrafficStmtAlt01,
    TrafficStmtAlt02,
    TrafficStmtAlt03,
    TrafficStmtAlt04,
    TrafficCaptureOptListAlt01,
    TrafficCaptureOptListAlt02,
    TrafficCaptureOptAlt01,
    TrafficCaptureOptAlt02,
    TrafficCaptureOptAlt03,
    TrafficReplayOptListAlt01,
    TrafficReplayOptListAlt02,
    TrafficReplayOptAlt01,
    TrafficReplayOptAlt02,
    TrafficReplayOptAlt03,
    TrafficReplayOptAlt04,
    OptSpPdparamsAlt01,
    OptSpPdparamsAlt02,
    CalibrateResourceStmtAlt01,
    CalibrateOptionAlt01,
    CalibrateOptionAlt02,
    CalibrateOptionAlt03,
    DynamicCalibrateOptionListAlt01,
    DynamicCalibrateOptionListAlt02,
    DynamicCalibrateOptionListAlt03,
    DynamicCalibrateResourceOptionAlt01,
    DynamicCalibrateResourceOptionAlt02,
    DynamicCalibrateResourceOptionAlt03,
    DynamicCalibrateResourceOptionAlt04,
    CalibrateResourceWorkloadOptionAlt01,
    CalibrateResourceWorkloadOptionAlt02,
    CalibrateResourceWorkloadOptionAlt03,
    CalibrateResourceWorkloadOptionAlt04,
    CalibrateResourceWorkloadOptionAlt05,
    AddQueryWatchStmtAlt01,
    QueryWatchOptionListAlt01,
    QueryWatchOptionListAlt02,
    QueryWatchOptionListAlt03,
    QueryWatchOptionAlt01,
    QueryWatchOptionAlt02,
    QueryWatchOptionAlt03,
    QueryWatchOptionAlt04,
    QueryWatchTextOptionAlt01,
    QueryWatchTextOptionAlt02,
    QueryWatchTextOptionAlt03,
    DropQueryWatchStmtAlt01,
    DropQueryWatchStmtAlt02,
    DropQueryWatchStmtAlt03,
}

fn identify(rule_id: RuleId) -> Option<AdminRule> {
    Some(match rule_id.as_str() {
        "addquerywatchstmt_query_watch_add_querywatchopti--c058e6ee3c610c16" => {
            AdminRule::AddQueryWatchStmtAlt01
        }
        "adminshowslow_recent_num--76e56f0d67bb5110" => AdminRule::AdminShowSlowAlt01,
        "adminshowslow_top_all_num--7329660b1373bd2f" => AdminRule::AdminShowSlowAlt04,
        "adminshowslow_top_internal_num--334600dbec3d2c61" => AdminRule::AdminShowSlowAlt03,
        "adminshowslow_top_num--16705840e548e982" => AdminRule::AdminShowSlowAlt02,
        "adminstmt_admin_alter_ddl_jobs_int64num_alterjob--3dee7dd791fba9c5" => {
            AdminRule::AdminStmtAlt35
        }
        "adminstmt_admin_cancel_ddl_jobs_numlist--d6492fe2aa611932" => AdminRule::AdminStmtAlt12,
        "adminstmt_admin_capture_bindings--831e86e10a97a91c" => AdminRule::AdminStmtAlt25,
        "adminstmt_admin_check_index_tablename_identifier--1e1bf606fb07e1de" => {
            AdminRule::AdminStmtAlt06
        }
        "adminstmt_admin_check_index_tablename_identifier--a984e4b7afd42d35" => {
            AdminRule::AdminStmtAlt10
        }
        "adminstmt_admin_check_table_tablenamelist--027ec66fb600a37f" => AdminRule::AdminStmtAlt05,
        "adminstmt_admin_checksum_table_tablenamelist--130b06d1aa71efca" => {
            AdminRule::AdminStmtAlt11
        }
        "adminstmt_admin_cleanup_index_tablename_identifi--37c35d0fd0099f96" => {
            AdminRule::AdminStmtAlt09
        }
        "adminstmt_admin_cleanup_table_lock_tablenamelist--d55c79d969c7af1a" => {
            AdminRule::AdminStmtAlt22
        }
        "adminstmt_admin_create_workload_snapshot--82458851acd52ddd" => AdminRule::AdminStmtAlt08,
        "adminstmt_admin_evolve_bindings--3358db25a93afeb7" => AdminRule::AdminStmtAlt26,
        "adminstmt_admin_flush_bindings--607afce4d0d8dcd0" => AdminRule::AdminStmtAlt24,
        "adminstmt_admin_flush_statementscope_plan_cache--c8335673bf733b85" => {
            AdminRule::AdminStmtAlt31
        }
        "adminstmt_admin_pause_ddl_jobs_numlist--7eb962230022517e" => AdminRule::AdminStmtAlt13,
        "adminstmt_admin_plugins_disable_pluginnamelist--d6bfaf72cf61e97c" => {
            AdminRule::AdminStmtAlt21
        }
        "adminstmt_admin_plugins_enable_pluginnamelist--38d9b262e98b5d4f" => {
            AdminRule::AdminStmtAlt20
        }
        "adminstmt_admin_recover_index_tablename_identifi--1c1170372f30221a" => {
            AdminRule::AdminStmtAlt07
        }
        "adminstmt_admin_reload_bindings--2a6ad8a18176691b" => AdminRule::AdminStmtAlt27,
        "adminstmt_admin_reload_cluster_bindings--59ebc2fcf31d5f79" => AdminRule::AdminStmtAlt28,
        "adminstmt_admin_reload_expr_pushdown_blacklist--30a3616fd663816f" => {
            AdminRule::AdminStmtAlt18
        }
        "adminstmt_admin_reload_opt_rule_blacklist--8fd32ed2ce5b87b1" => AdminRule::AdminStmtAlt19,
        "adminstmt_admin_reload_statistics--97ec4d8217812f60" => AdminRule::AdminStmtAlt30,
        "adminstmt_admin_reload_stats_extended--22ad4baf30d8e4c2" => AdminRule::AdminStmtAlt29,
        "adminstmt_admin_repair_table_tablename_createtab--e77dbbf90ceb2b2a" => {
            AdminRule::AdminStmtAlt23
        }
        "adminstmt_admin_resume_ddl_jobs_numlist--be770a92a3580967" => AdminRule::AdminStmtAlt14,
        "adminstmt_admin_set_bdr_role_bdrrole--c1faed57c75a913c" => AdminRule::AdminStmtAlt32,
        "adminstmt_admin_show_bdr_role--ea0b6b8e471bcbd5" => AdminRule::AdminStmtAlt33,
        "adminstmt_admin_show_ddl--97dad43e86efa973" => AdminRule::AdminStmtAlt01,
        "adminstmt_admin_show_ddl_job_queries_adminstmtli--8cbd83ece271b443" => {
            AdminRule::AdminStmtAlt16
        }
        "adminstmt_admin_show_ddl_job_queries_numlist--0d0f3c8826be994e" => {
            AdminRule::AdminStmtAlt15
        }
        "adminstmt_admin_show_ddl_jobs_int64num_whereclau--6ca3450942dddf40" => {
            AdminRule::AdminStmtAlt03
        }
        "adminstmt_admin_show_ddl_jobs_whereclauseoptiona--1f285afa1228fa27" => {
            AdminRule::AdminStmtAlt02
        }
        "adminstmt_admin_show_slow_adminshowslow--72b6c21f7c57531d" => AdminRule::AdminStmtAlt17,
        "adminstmt_admin_show_tablename_next_row_id--fd14194b59cb1fda" => AdminRule::AdminStmtAlt04,
        "adminstmt_admin_unset_bdr_role--31fc7f4c65a04505" => AdminRule::AdminStmtAlt34,
        "adminstmtlimitopt_limit_lengthnum--acedd0f15af255d8" => AdminRule::AdminStmtLimitOptAlt01,
        "adminstmtlimitopt_limit_lengthnum_lengthnum--4c92aae542c62148" => {
            AdminRule::AdminStmtLimitOptAlt02
        }
        "adminstmtlimitopt_limit_lengthnum_offset_lengthn--72031fe7cc1fb029" => {
            AdminRule::AdminStmtLimitOptAlt03
        }
        "allcolumnsorpredicatecolumnsopt--9afbfb4ff663003f" => {
            AdminRule::AllColumnsOrPredicateColumnsOptAlt01
        }
        "allcolumnsorpredicatecolumnsopt_all_columns--6a197fa4c98257aa" => {
            AdminRule::AllColumnsOrPredicateColumnsOptAlt02
        }
        "allcolumnsorpredicatecolumnsopt_predicate_column--3654a91b8f607b78" => {
            AdminRule::AllColumnsOrPredicateColumnsOptAlt03
        }
        "alterinstancestmt_alter_instance_instanceoption--d06b8d76dc1ca344" => {
            AdminRule::AlterInstanceStmtAlt01
        }
        "alterjoboption_identifier_signedliteral--203c235fc50937cb" => {
            AdminRule::AlterJobOptionAlt01
        }
        "alterjoboptionlist_alterjoboption--8b06b9bf4cb31558" => AdminRule::AlterJobOptionListAlt01,
        "alterjoboptionlist_alterjoboptionlist_alterjobop--2b27affe8702c7c2" => {
            AdminRule::AlterJobOptionListAlt02
        }
        "alterorderitem_columnname_optorder--9eba6999a71f3711" => AdminRule::AlterOrderItemAlt01,
        "alterorderlist_alterorderitem--72a205ad066f9173" => AdminRule::AlterOrderListAlt01,
        "alterorderlist_alterorderlist_alterorderitem--5e37dc977aebd2af" => {
            AdminRule::AlterOrderListAlt02
        }
        "alterresourcegroupstmt_alter_resource_group_ifex--523d8ad7e96aec45" => {
            AdminRule::AlterResourceGroupStmtAlt01
        }
        "altersequenceoption_restart--22ebba0b3654f0a7" => AdminRule::AlterSequenceOptionAlt02,
        "altersequenceoption_restart_eqopt_signednum--aad34abbd56bfa70" => {
            AdminRule::AlterSequenceOptionAlt03
        }
        "altersequenceoption_restart_with_signednum--439fce2523caf68d" => {
            AdminRule::AlterSequenceOptionAlt04
        }
        "altersequenceoptionlist_altersequenceoption--4f50826b9254e30a" => {
            AdminRule::AlterSequenceOptionListAlt01
        }
        "altersequenceoptionlist_altersequenceoptionlist--8e4d10132cb9863e" => {
            AdminRule::AlterSequenceOptionListAlt02
        }
        "altersequencestmt_alter_sequence_ifexists_tablen--9504d6f268428b0a" => {
            AdminRule::AlterSequenceStmtAlt01
        }
        "analyzeoption_num_buckets--3de81053381d8f84" => AdminRule::AnalyzeOptionAlt01,
        "analyzeoption_num_cmsketch_depth--e712e320decf1878" => AdminRule::AnalyzeOptionAlt03,
        "analyzeoption_num_cmsketch_width--5ab3af9cfa71d767" => AdminRule::AnalyzeOptionAlt04,
        "analyzeoption_num_samples--4a132060ade67bd4" => AdminRule::AnalyzeOptionAlt05,
        "analyzeoption_num_topn--8ec91397afdb26fa" => AdminRule::AnalyzeOptionAlt02,
        "analyzeoption_numliteral_ndvrate--74bb35c910b84b4a" => AdminRule::AnalyzeOptionAlt07,
        "analyzeoption_numliteral_samplerate--d6fa25cea0b345a4" => AdminRule::AnalyzeOptionAlt06,
        "analyzeoption_default_buckets--cd85096ebd5babf3" => AdminRule::AnalyzeOptionAlt08,
        "analyzeoption_default_topn--c2d926504f665863" => AdminRule::AnalyzeOptionAlt09,
        "analyzeoption_default_samples--5caa91fa5ed520e7" => AdminRule::AnalyzeOptionAlt10,
        "analyzeoption_default_samplerate--6d162a23401d8fe2" => AdminRule::AnalyzeOptionAlt11,
        "analyzeoptionlist_analyzeoption--7c63d961e87956a8" => AdminRule::AnalyzeOptionListAlt01,
        "analyzeoptionlist_analyzeoptionlist_analyzeoptio--aedb853e042163ed" => {
            AdminRule::AnalyzeOptionListAlt03
        }
        "analyzeoptionlist_analyzeoptionlist_analyzeoptio--d38c2cedb5a15b69" => {
            AdminRule::AnalyzeOptionListAlt02
        }
        "analyzeoptionlistopt--d44153c033c5539b" => AdminRule::AnalyzeOptionListOptAlt01,
        "analyzeoptionlistopt_with_analyzeoptionlist--5cad519f44f3002f" => {
            AdminRule::AnalyzeOptionListOptAlt02
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--03b095576d33aea8" => {
            AdminRule::AnalyzeTableStmtAlt01
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--326705f9c7fbbe84" => {
            AdminRule::AnalyzeTableStmtAlt05
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--327b28f40656c881" => {
            AdminRule::AnalyzeTableStmtAlt03
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--3dec13ba5f2c1d85" => {
            AdminRule::AnalyzeTableStmtAlt09
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--57a11aa1dc09f835" => {
            AdminRule::AnalyzeTableStmtAlt02
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--592e4788c4d4825a" => {
            AdminRule::AnalyzeTableStmtAlt07
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--7bef6cf3f1ebe5c8" => {
            AdminRule::AnalyzeTableStmtAlt10
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--a5e67f7cacdc93a1" => {
            AdminRule::AnalyzeTableStmtAlt04
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--b7b209c710a27be8" => {
            AdminRule::AnalyzeTableStmtAlt06
        }
        "analyzetablestmt_analyze_nowritetobinlogaliasopt--cd55b72b0b102eb0" => {
            AdminRule::AnalyzeTableStmtAlt08
        }
        "bindingstatustype_disabled--fe6a915c43e95ed5" => AdminRule::BindingStatusTypeAlt02,
        "bindingstatustype_enabled--b7503db3035025c4" => AdminRule::BindingStatusTypeAlt01,
        "binlogstmt_binlog_stringlit--43bc6367a27d51e6" => AdminRule::BinlogStmtAlt01,
        "boolean_false--e7453834d71382af" => AdminRule::BooleanAlt02,
        "boolean_num--8c1bf034f6dbee26" => AdminRule::BooleanAlt01,
        "boolean_true--6e1ae6c8526a3992" => AdminRule::BooleanAlt03,
        "briebooleanoptionname_checkpoint--300803464c432e04" => {
            AdminRule::BRIEBooleanOptionNameAlt03
        }
        "briebooleanoptionname_csv_backslash_escape--765969cec6b7de5b" => {
            AdminRule::BRIEBooleanOptionNameAlt07
        }
        "briebooleanoptionname_csv_not_null--d7bcc57466fbdee0" => {
            AdminRule::BRIEBooleanOptionNameAlt06
        }
        "briebooleanoptionname_csv_trim_last_separators--a785694ed1da21a7" => {
            AdminRule::BRIEBooleanOptionNameAlt08
        }
        "briebooleanoptionname_ignore_stats--5aa72bad27183da8" => {
            AdminRule::BRIEBooleanOptionNameAlt11
        }
        "briebooleanoptionname_load_stats--ac4b73b1e916fe6e" => {
            AdminRule::BRIEBooleanOptionNameAlt12
        }
        "briebooleanoptionname_online--7058b64386ceec03" => AdminRule::BRIEBooleanOptionNameAlt02,
        "briebooleanoptionname_send_credentials_to_tikv--fa07a7ac54a13e12" => {
            AdminRule::BRIEBooleanOptionNameAlt01
        }
        "briebooleanoptionname_skip_schema_files--082a707ea8bd3fe3" => {
            AdminRule::BRIEBooleanOptionNameAlt04
        }
        "briebooleanoptionname_strict_format--e31a8eb6446e947d" => {
            AdminRule::BRIEBooleanOptionNameAlt05
        }
        "briebooleanoptionname_wait_tiflash_ready--5383bbd08272eb8d" => {
            AdminRule::BRIEBooleanOptionNameAlt09
        }
        "briebooleanoptionname_with_sys_table--b132342cce9307fd" => {
            AdminRule::BRIEBooleanOptionNameAlt10
        }
        "brieintegeroptionname_checksum_concurrency--7f3925f162e04225" => {
            AdminRule::BRIEIntegerOptionNameAlt03
        }
        "brieintegeroptionname_compression_level--34da9c260f3e8495" => {
            AdminRule::BRIEIntegerOptionNameAlt04
        }
        "brieintegeroptionname_concurrency--2b70f3cc0566aa21" => {
            AdminRule::BRIEIntegerOptionNameAlt01
        }
        "brieintegeroptionname_resume--6985d784b626db59" => AdminRule::BRIEIntegerOptionNameAlt02,
        "briekeywordoptionname_backend--c8fec84d3262848b" => AdminRule::BRIEKeywordOptionNameAlt01,
        "briekeywordoptionname_on_duplicate--06a16766b5c22918" => {
            AdminRule::BRIEKeywordOptionNameAlt02
        }
        "briekeywordoptionname_on_duplicate--f20441954c5b0df5" => {
            AdminRule::BRIEKeywordOptionNameAlt03
        }
        "brieoption_analyze_eqopt_boolean--eac1fc57f744a326" => AdminRule::BRIEOptionAlt15,
        "brieoption_analyze_eqopt_optionlevel--2544a91c18583663" => AdminRule::BRIEOptionAlt16,
        "brieoption_briebooleanoptionname_eqopt_boolean--9c7e194769f2e50e" => {
            AdminRule::BRIEOptionAlt02
        }
        "brieoption_brieintegeroptionname_eqopt_lengthnum--b68a7d0a42adc018" => {
            AdminRule::BRIEOptionAlt01
        }
        "brieoption_briekeywordoptionname_eqopt_stringnam--45df26a2d3a3b65a" => {
            AdminRule::BRIEOptionAlt04
        }
        "brieoption_briestringoptionname_eqopt_stringlit--70b7b68c749414a7" => {
            AdminRule::BRIEOptionAlt03
        }
        "brieoption_checksum_eqopt_boolean--021b00a8df0dea3d" => AdminRule::BRIEOptionAlt13,
        "brieoption_checksum_eqopt_optionlevel--c0f9fc1f31151608" => AdminRule::BRIEOptionAlt14,
        "brieoption_csv_header_eqopt_fieldsorcolumns--8746e357a64978c7" => {
            AdminRule::BRIEOptionAlt11
        }
        "brieoption_csv_header_eqopt_lengthnum--1ca9ae374722139c" => AdminRule::BRIEOptionAlt12,
        "brieoption_full_backup_storage_eqopt_stringlit--d69cc95084b9327a" => {
            AdminRule::BRIEOptionAlt17
        }
        "brieoption_gc_ttl_eqopt_stringlit--127d411a8a23f311" => AdminRule::BRIEOptionAlt21,
        "brieoption_last_backup_eqopt_lengthnum--b7e9881f40f9ed8f" => AdminRule::BRIEOptionAlt09,
        "brieoption_last_backup_eqopt_stringlit--3a938bb5fb2de685" => AdminRule::BRIEOptionAlt08,
        "brieoption_rate_limit_eqopt_lengthnum_mb_second--aae3586034b7f82c" => {
            AdminRule::BRIEOptionAlt10
        }
        "brieoption_restored_ts_eqopt_stringlit--9ab2b499fc5771e2" => AdminRule::BRIEOptionAlt18,
        "brieoption_snapshot_eqopt_lengthnum--ce6dfcd67a65ecc8" => AdminRule::BRIEOptionAlt07,
        "brieoption_snapshot_eqopt_lengthnum_timestampuni--21a0989f7c1cf0d1" => {
            AdminRule::BRIEOptionAlt05
        }
        "brieoption_snapshot_eqopt_stringlit--151c8bf2e3288946" => AdminRule::BRIEOptionAlt06,
        "brieoption_start_ts_eqopt_stringlit--8eaaa00b4d45f9ee" => AdminRule::BRIEOptionAlt19,
        "brieoption_until_ts_eqopt_stringlit--f44766f3118d2ca2" => AdminRule::BRIEOptionAlt20,
        "brieoptions_brieoptions_brieoption--cdd13bd41bcf2105" => AdminRule::BRIEOptionsAlt02,
        "brieoptions_prec_empty--c729889607984289" => AdminRule::BRIEOptionsAlt01,
        "briestmt_backup_brietables_to_stringlit_brieopti--ef497d23b36b9aea" => {
            AdminRule::BRIEStmtAlt01
        }
        "briestmt_backup_logs_to_stringlit_brieoptions--996705c9a444cea8" => {
            AdminRule::BRIEStmtAlt02
        }
        "briestmt_cancel_br_job_int64num--32c24c25ac255958" => AdminRule::BRIEStmtAlt11,
        "briestmt_pause_backup_logs_brieoptions--30a315e8cd813361" => AdminRule::BRIEStmtAlt04,
        "briestmt_purge_backup_logs_from_stringlit_brieop--09c5ca75b5b9ebf0" => {
            AdminRule::BRIEStmtAlt06
        }
        "briestmt_restore_brietables_from_stringlit_brieo--69c323993d75e381" => {
            AdminRule::BRIEStmtAlt13
        }
        "briestmt_restore_point_from_stringlit_brieoption--2ad83fbfb4c1cf72" => {
            AdminRule::BRIEStmtAlt14
        }
        "briestmt_resume_backup_logs--731657ce9aff670e" => AdminRule::BRIEStmtAlt05,
        "briestmt_show_backup_logs_metadata_from_stringli--45d9d4a2ae772dfb" => {
            AdminRule::BRIEStmtAlt08
        }
        "briestmt_show_backup_logs_status--ca49dde69a101de6" => AdminRule::BRIEStmtAlt07,
        "briestmt_show_backup_metadata_from_stringlit--7896862c0cffccc4" => {
            AdminRule::BRIEStmtAlt12
        }
        "briestmt_show_br_job_int64num--f2b6f79c9fd054c1" => AdminRule::BRIEStmtAlt09,
        "briestmt_show_br_job_query_int64num--5ba7a2556e29b749" => AdminRule::BRIEStmtAlt10,
        "briestmt_stop_backup_logs--a1b5df82e6011bef" => AdminRule::BRIEStmtAlt03,
        "briestringoptionname_compression_type--da0987791c75f990" => {
            AdminRule::BRIEStringOptionNameAlt05
        }
        "briestringoptionname_csv_delimiter--dec486e39ae4165d" => {
            AdminRule::BRIEStringOptionNameAlt03
        }
        "briestringoptionname_csv_null--027adab551682899" => AdminRule::BRIEStringOptionNameAlt04,
        "briestringoptionname_csv_separator--d4d74a42a0c284eb" => {
            AdminRule::BRIEStringOptionNameAlt02
        }
        "briestringoptionname_encryption_keyfile--ff2b4595df9e1fc8" => {
            AdminRule::BRIEStringOptionNameAlt07
        }
        "briestringoptionname_encryption_method--c8c0d9e0211ee416" => {
            AdminRule::BRIEStringOptionNameAlt06
        }
        "briestringoptionname_tikv_importer--e43c6bef24fc1514" => {
            AdminRule::BRIEStringOptionNameAlt01
        }
        "brietables_databasesym--ca7f0f1a9232dba3" => AdminRule::BRIETablesAlt01,
        "brietables_databasesym_dbnamelist--6dd2e771f62314ba" => AdminRule::BRIETablesAlt02,
        "brietables_table_tablenamelist--6134039e00bf0b1a" => AdminRule::BRIETablesAlt03,
        "calibrateoption--0c150b3a30734117" => AdminRule::CalibrateOptionAlt01,
        "calibrateoption_calibrateresourceworkloadoption--ca91bc3e5c97926f" => {
            AdminRule::CalibrateOptionAlt03
        }
        "calibrateoption_dynamiccalibrateoptionlist--e2b2ef70a38dcfad" => {
            AdminRule::CalibrateOptionAlt02
        }
        "calibrateresourcestmt_calibrate_resource_calibra--9c3168272c3a0b30" => {
            AdminRule::CalibrateResourceStmtAlt01
        }
        "calibrateresourceworkloadoption_workload_oltp_re--1337eef231f70aa0" => {
            AdminRule::CalibrateResourceWorkloadOptionAlt02
        }
        "calibrateresourceworkloadoption_workload_oltp_re--706831e55714ea99" => {
            AdminRule::CalibrateResourceWorkloadOptionAlt03
        }
        "calibrateresourceworkloadoption_workload_oltp_wr--0236446c4a6d8056" => {
            AdminRule::CalibrateResourceWorkloadOptionAlt04
        }
        "calibrateresourceworkloadoption_workload_tpcc--6537f6897fa25676" => {
            AdminRule::CalibrateResourceWorkloadOptionAlt01
        }
        "calibrateresourceworkloadoption_workload_tpch_10--57b20ee46e7e0123" => {
            AdminRule::CalibrateResourceWorkloadOptionAlt05
        }
        "charsetnameordefault_charsetname--d5f593d9830261ba" => {
            AdminRule::CharsetNameOrDefaultAlt01
        }
        "charsetnameordefault_default--d04f92f2c4128cfa" => AdminRule::CharsetNameOrDefaultAlt02,
        "clusteropt--5add117c81dbe0ba" => AdminRule::ClusterOptAlt01,
        "clusteropt_cluster--9f436b333e7e0cd7" => AdminRule::ClusterOptAlt02,
        "configitemname_identifier_configitemname--27c82cef37de3d5b" => {
            AdminRule::ConfigItemNameAlt02
        }
        "configitemname_identifier_configitemname--6cda795e091b1796" => {
            AdminRule::ConfigItemNameAlt03
        }
        "createbindingstmt_create_globalscope_binding_for--cf6bab27793fbe80" => {
            AdminRule::CreateBindingStmtAlt01
        }
        "createbindingstmt_create_globalscope_binding_fro--12aff2e10dbdba06" => {
            AdminRule::CreateBindingStmtAlt03
        }
        "createbindingstmt_create_globalscope_binding_usi--86ea0ad9c342ef68" => {
            AdminRule::CreateBindingStmtAlt02
        }
        "createmaskingpolicystmt_create_orreplace_masking--a993162791e352a6" => {
            AdminRule::CreateMaskingPolicyStmtAlt01
        }
        "createresourcegroupstmt_create_resource_group_if--0adcf3e3bbd82c7e" => {
            AdminRule::CreateResourceGroupStmtAlt01
        }
        "createsequenceoptionlistopt--ceab03c32575ac12" => {
            AdminRule::CreateSequenceOptionListOptAlt01
        }
        "createsequencestmt_create_sequence_ifnotexists_t--331d99e8957a991b" => {
            AdminRule::CreateSequenceStmtAlt01
        }
        "createsequencetableoptionlistopt_prec_lowerthanc--f120003cb95db109" => {
            AdminRule::CreateSequenceTableOptionListOptAlt01
        }
        "createstatisticsstmt_create_statistics_ifnotexis--3b9375cac716b83b" => {
            AdminRule::CreateStatisticsStmtAlt01
        }
        "dbnamelist_dbname--aa15cec05e481ee4" => AdminRule::DBNameListAlt01,
        "dbnamelist_dbnamelist_dbname--5c8df47fb1b28cc9" => AdminRule::DBNameListAlt02,
        "directresourcegroupbackgroundoption_task_types_e--ec0da036e371b5bf" => {
            AdminRule::DirectResourceGroupBackgroundOptionAlt01
        }
        "directresourcegroupbackgroundoption_utilization--bf655cc41af21496" => {
            AdminRule::DirectResourceGroupBackgroundOptionAlt02
        }
        "directresourcegroupoption_background_eqopt--27426dcfb0c2e2bd" => {
            AdminRule::DirectResourceGroupOptionAlt12
        }
        "directresourcegroupoption_background_eqopt_null--48f0081bf7624c37" => {
            AdminRule::DirectResourceGroupOptionAlt13
        }
        "directresourcegroupoption_background_eqopt_resou--fa7d76d993f55787" => {
            AdminRule::DirectResourceGroupOptionAlt11
        }
        "directresourcegroupoption_burstable--647133ae9396b023" => {
            AdminRule::DirectResourceGroupOptionAlt04
        }
        "directresourcegroupoption_burstable_eqopt_modera--b9b08d94e1959add" => {
            AdminRule::DirectResourceGroupOptionAlt05
        }
        "directresourcegroupoption_burstable_eqopt_off--7a68e8271f4f8c87" => {
            AdminRule::DirectResourceGroupOptionAlt07
        }
        "directresourcegroupoption_burstable_eqopt_unlimi--a339603f7330c999" => {
            AdminRule::DirectResourceGroupOptionAlt06
        }
        "directresourcegroupoption_priority_eqopt_resourc--27aafc4d2dc25856" => {
            AdminRule::DirectResourceGroupOptionAlt03
        }
        "directresourcegroupoption_query_limit_eqopt--7041458c240f0f43" => {
            AdminRule::DirectResourceGroupOptionAlt09
        }
        "directresourcegroupoption_query_limit_eqopt_null--4d62ab96810f18cd" => {
            AdminRule::DirectResourceGroupOptionAlt10
        }
        "directresourcegroupoption_query_limit_eqopt_reso--eb834ffc7d85e088" => {
            AdminRule::DirectResourceGroupOptionAlt08
        }
        "directresourcegroupoption_ru_per_sec_eqopt_lengt--8a9b0d0a5d4ea721" => {
            AdminRule::DirectResourceGroupOptionAlt01
        }
        "directresourcegroupoption_ru_per_sec_eqopt_unlim--01da599a41594624" => {
            AdminRule::DirectResourceGroupOptionAlt02
        }
        "directresourcegrouprunawayoption_action_eqopt_re--e678e11149dadf70" => {
            AdminRule::DirectResourceGroupRunawayOptionAlt04
        }
        "directresourcegrouprunawayoption_exec_elapsed_eq--e1b9e593e50322ed" => {
            AdminRule::DirectResourceGroupRunawayOptionAlt01
        }
        "directresourcegrouprunawayoption_processed_keys--518a953058c3b710" => {
            AdminRule::DirectResourceGroupRunawayOptionAlt02
        }
        "directresourcegrouprunawayoption_ru_eqopt_intlit--0d5b8b8a9b828058" => {
            AdminRule::DirectResourceGroupRunawayOptionAlt03
        }
        "directresourcegrouprunawayoption_watch_eqopt_res--ca6d72ea1ec6509e" => {
            AdminRule::DirectResourceGroupRunawayOptionAlt05
        }
        "dropbindingstmt_drop_globalscope_binding_for_bin--240a8b0808fb5af3" => {
            AdminRule::DropBindingStmtAlt01
        }
        "dropbindingstmt_drop_globalscope_binding_for_bin--7d9464a8d123ef30" => {
            AdminRule::DropBindingStmtAlt02
        }
        "dropbindingstmt_drop_globalscope_binding_for_sql--6ad45040852baf92" => {
            AdminRule::DropBindingStmtAlt03
        }
        "dropquerywatchstmt_query_watch_remove_num--5032dba3296c120f" => {
            AdminRule::DropQueryWatchStmtAlt01
        }
        "dropquerywatchstmt_query_watch_remove_resource_g--3c74f51c402bd317" => {
            AdminRule::DropQueryWatchStmtAlt03
        }
        "dropquerywatchstmt_query_watch_remove_resource_g--b86200d512c2805c" => {
            AdminRule::DropQueryWatchStmtAlt02
        }
        "dropresourcegroupstmt_drop_resource_group_ifexis--8582334d2aff7dd7" => {
            AdminRule::DropResourceGroupStmtAlt01
        }
        "dropsequencestmt_drop_sequence_ifexists_tablenam--15facef1ee9eb2ac" => {
            AdminRule::DropSequenceStmtAlt01
        }
        "dropstatisticsstmt_drop_statistics_identifier--d2755c340bdc8bab" => {
            AdminRule::DropStatisticsStmtAlt01
        }
        "dropstatsstmt_drop_stats_tablename_global--4348aad98498b88e" => {
            AdminRule::DropStatsStmtAlt03
        }
        "dropstatsstmt_drop_stats_tablename_partition_par--6944482848137ab6" => {
            AdminRule::DropStatsStmtAlt02
        }
        "dropstatsstmt_drop_stats_tablenamelist--97ab77958fcb61cd" => AdminRule::DropStatsStmtAlt01,
        "dynamiccalibrateoptionlist_dynamiccalibrateoptio--cb8e2b82f5fd3843" => {
            AdminRule::DynamicCalibrateOptionListAlt02
        }
        "dynamiccalibrateoptionlist_dynamiccalibrateoptio--f5ac5b115ce4c8ef" => {
            AdminRule::DynamicCalibrateOptionListAlt03
        }
        "dynamiccalibrateoptionlist_dynamiccalibrateresou--a3918d2c853ed0d0" => {
            AdminRule::DynamicCalibrateOptionListAlt01
        }
        "dynamiccalibrateresourceoption_duration_eqopt_in--6cedd75352bfeb7e" => {
            AdminRule::DynamicCalibrateResourceOptionAlt04
        }
        "dynamiccalibrateresourceoption_duration_eqopt_st--4a15de0e247a4fd6" => {
            AdminRule::DynamicCalibrateResourceOptionAlt03
        }
        "dynamiccalibrateresourceoption_end_time_eqopt_ex--78afb7a0411f0ec1" => {
            AdminRule::DynamicCalibrateResourceOptionAlt02
        }
        "dynamiccalibrateresourceoption_start_time_eqopt--4ef168a62924c08a" => {
            AdminRule::DynamicCalibrateResourceOptionAlt01
        }
        "flushoption_client_errors_summary--54c7a672bd1ea37b" => AdminRule::FlushOptionAlt07,
        "flushoption_hosts--14868a127e5e0d4a" => AdminRule::FlushOptionAlt04,
        "flushoption_logtypeopt_logs--a04313741e0e88cb" => AdminRule::FlushOptionAlt05,
        "flushoption_privileges--3f48bd592d4d3345" => AdminRule::FlushOptionAlt01,
        "flushoption_stats_delta_statsobjectlist_clustero--d86d75a2dd20a4ec" => {
            AdminRule::FlushOptionAlt08
        }
        "flushoption_status--107cbc18f8ff096f" => AdminRule::FlushOptionAlt02,
        "flushoption_tableortables_tablenamelistopt_withr--32e235b6facd7ff9" => {
            AdminRule::FlushOptionAlt06
        }
        "flushoption_tidb_plugins_pluginnamelist--b88f105f29ab314e" => AdminRule::FlushOptionAlt03,
        "flushstmt_flush_nowritetobinlogaliasopt_flushopt--ea43556a0460feb4" => {
            AdminRule::FlushStmtAlt01
        }
        "globalscope--b55b45846f4cb212" => AdminRule::GlobalScopeAlt01,
        "globalscope_global--5afbef7a702002ea" => AdminRule::GlobalScopeAlt02,
        "globalscope_session--865f8bed528f9139" => AdminRule::GlobalScopeAlt03,
        "grantstmt_grant_roleorprivelemlist_on_objecttype--f21fab35e3d8a620" => {
            AdminRule::GrantStmtAlt01
        }
        "handlerange_int64num_int64num--e26ce18dfd865a50" => AdminRule::HandleRangeAlt01,
        "handlerangelist_handlerange--c84d4b527b52f9a4" => AdminRule::HandleRangeListAlt01,
        "handlerangelist_handlerangelist_handlerange--b3905376bf822dfb" => {
            AdminRule::HandleRangeListAlt02
        }
        "hashstring_hexlit--63ef60cc932ee527" => AdminRule::HashStringAlt02,
        "identlist_identifier--bdfd34ae4e801387" => AdminRule::IdentListAlt01,
        "identlist_identlist_identifier--f796e36816324689" => AdminRule::IdentListAlt02,
        "identlistwithparenopt--6437ca4c81aa1bf8" => AdminRule::IdentListWithParenOptAlt01,
        "identlistwithparenopt_identlist--1cc35b9a62e72a80" => {
            AdminRule::IdentListWithParenOptAlt02
        }
        "ifexists--a3f7d55a18f84496" => AdminRule::IfExistsAlt01,
        "ifexists_if_exists--6482055bbdc63dde" => AdminRule::IfExistsAlt02,
        "ifnotexists--feda7ac9e546b0b3" => AdminRule::IfNotExistsAlt01,
        "ifnotexists_if_notsym_exists--bee5df59e10d77eb" => AdminRule::IfNotExistsAlt02,
        "ignoreoptional--d87fbab597e03ce3" => AdminRule::IgnoreOptionalAlt01,
        "ignoreoptional_ignore--fa9df28d22525450" => AdminRule::IgnoreOptionalAlt02,
        "instanceoption_reload_tls--f19724bf21ba6c76" => AdminRule::InstanceOptionAlt01,
        "instanceoption_reload_tls_no_rollback_on_error--d8f06e1fbd45b0be" => {
            AdminRule::InstanceOptionAlt02
        }
        "killorkilltidb_kill--d595266cc6f943e4" => AdminRule::KillOrKillTiDBAlt01,
        "killorkilltidb_kill_tidb--15c01063c662d417" => AdminRule::KillOrKillTiDBAlt02,
        "killstmt_killorkilltidb_builtinfunction--ad00c7a5ae3f0fad" => AdminRule::KillStmtAlt04,
        "killstmt_killorkilltidb_connection_num--ad12c543b9418d4e" => AdminRule::KillStmtAlt02,
        "killstmt_killorkilltidb_num--d2281ceb350f4a46" => AdminRule::KillStmtAlt01,
        "killstmt_killorkilltidb_query_num--f184cbfb1ddcdd7a" => AdminRule::KillStmtAlt03,
        "loadstatsstmt_load_stats_stringlit--9ce0163b120d99d8" => AdminRule::LoadStatsStmtAlt01,
        "lockstatsstmt_lock_stats_tablename_partition_par--3a5aede3d6788efa" => {
            AdminRule::LockStatsStmtAlt02
        }
        "lockstatsstmt_lock_stats_tablename_partition_par--8ff22e806102d477" => {
            AdminRule::LockStatsStmtAlt03
        }
        "lockstatsstmt_lock_stats_tablenamelist--323aec165d8aaef9" => AdminRule::LockStatsStmtAlt01,
        "locktablesstmt_lock_tablesterminalsym_tablelockl--49dbf3fee19a38a2" => {
            AdminRule::LockTablesStmtAlt01
        }
        "locktype_read--13554a8c6b970e43" => AdminRule::LockTypeAlt01,
        "locktype_read_local--b5583b6f64983ee2" => AdminRule::LockTypeAlt02,
        "locktype_write--2315d3589a312af0" => AdminRule::LockTypeAlt03,
        "locktype_write_local--7724960579ee382d" => AdminRule::LockTypeAlt04,
        "logtypeopt--1e4cd52cd585b3fe" => AdminRule::LogTypeOptAlt01,
        "logtypeopt_binary--a9b2a2551fdc0cbc" => AdminRule::LogTypeOptAlt02,
        "logtypeopt_engine--4587906578ebfd15" => AdminRule::LogTypeOptAlt03,
        "logtypeopt_error--a773e7fbd0325b03" => AdminRule::LogTypeOptAlt04,
        "logtypeopt_general--950f45ccb545ab29" => AdminRule::LogTypeOptAlt05,
        "logtypeopt_slow--8c60aadc2b5e55d8" => AdminRule::LogTypeOptAlt06,
        "maskingpolicyrestrictonopt--813fdaada22f0ead" => {
            AdminRule::MaskingPolicyRestrictOnOptAlt01
        }
        "maskingpolicyrestrictonopt_restrict_on_maskingpo--6a7d15942c4de58f" => {
            AdminRule::MaskingPolicyRestrictOnOptAlt02
        }
        "maskingpolicyrestrictonopt_restrict_on_none--cd2959259a2b41b7" => {
            AdminRule::MaskingPolicyRestrictOnOptAlt03
        }
        "maskingpolicyrestrictoperation_identifier--5c81ff921e0d1abc" => {
            AdminRule::MaskingPolicyRestrictOperationAlt01
        }
        "maskingpolicyrestrictoperationlist_maskingpolicy--0f382ac9cba7f8cb" => {
            AdminRule::MaskingPolicyRestrictOperationListAlt02
        }
        "maskingpolicyrestrictoperationlist_maskingpolicy--851fd7ce44cd24b8" => {
            AdminRule::MaskingPolicyRestrictOperationListAlt01
        }
        "maskingpolicystateopt--1f9404f00fa2da4b" => AdminRule::MaskingPolicyStateOptAlt01,
        "maskingpolicystateopt_disable--0bfc38aefc8853a2" => AdminRule::MaskingPolicyStateOptAlt03,
        "maskingpolicystateopt_enable--6983c88eb9ce3e5f" => AdminRule::MaskingPolicyStateOptAlt02,
        "notsym_not2--2cbff8540fae3bb5" => AdminRule::NotSymAlt02,
        "nowritetobinlogaliasopt_local--c8d5d7655e818a7a" => {
            AdminRule::NoWriteToBinLogAliasOptAlt03
        }
        "nowritetobinlogaliasopt_no_write_to_binlog--24da1fd823267fd8" => {
            AdminRule::NoWriteToBinLogAliasOptAlt02
        }
        "nowritetobinlogaliasopt_prec_lowerthanlocal--4a811a62e1175d5e" => {
            AdminRule::NoWriteToBinLogAliasOptAlt01
        }
        "numlist_int64num--ca9dfc5812dabcbf" => AdminRule::NumListAlt01,
        "numlist_numlist_int64num--49ef498ad6235bfb" => AdminRule::NumListAlt02,
        "optfull--f2e7859da43fbd63" => AdminRule::OptFullAlt01,
        "optfull_full--2543508bc9302f85" => AdminRule::OptFullAlt02,
        "optimizetablestmt_optimize_nowritetobinlogaliaso--dd9af26e4d0c3988" => {
            AdminRule::OptimizeTableStmtAlt01
        }
        "optsppdparams--24f167f0952840a3" => AdminRule::OptSpPdparamsAlt01,
        "optsppdparams_sppdparams--5d148b0b60d0450b" => AdminRule::OptSpPdparamsAlt02,
        "planreplayerdumpopt--4efec3148a4fdc2d" => AdminRule::PlanReplayerDumpOptAlt01,
        "planreplayerdumpopt_with_stats_asofclause--8a3c1c16c91ed34b" => {
            AdminRule::PlanReplayerDumpOptAlt02
        }
        "planreplayerstmt_plan_replayer_capture_remove_st--ca6f9f87918e4eda" => {
            AdminRule::PlanReplayerStmtAlt11
        }
        "planreplayerstmt_plan_replayer_capture_stringlit--f36a490aee210692" => {
            AdminRule::PlanReplayerStmtAlt10
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--012a406955d5b8f7" => {
            AdminRule::PlanReplayerStmtAlt05
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--20f731f1f0945de8" => {
            AdminRule::PlanReplayerStmtAlt02
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--62e93fe653edafdb" => {
            AdminRule::PlanReplayerStmtAlt07
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--831157dcf0827cdb" => {
            AdminRule::PlanReplayerStmtAlt04
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--87c963ddbc6dd51f" => {
            AdminRule::PlanReplayerStmtAlt08
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--8b1f63c591e9b7c4" => {
            AdminRule::PlanReplayerStmtAlt01
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--aa5606c8840f0f93" => {
            AdminRule::PlanReplayerStmtAlt06
        }
        "planreplayerstmt_plan_replayer_dump_planreplayer--d0312b93c9da7fd7" => {
            AdminRule::PlanReplayerStmtAlt03
        }
        "planreplayerstmt_plan_replayer_load_stringlit--3f49ce3606bc45ba" => {
            AdminRule::PlanReplayerStmtAlt09
        }
        "pluginnamelist_identifier--5902fd89b69f1183" => AdminRule::PluginNameListAlt01,
        "pluginnamelist_pluginnamelist_identifier--ac3f97b0a2ed0ebf" => {
            AdminRule::PluginNameListAlt02
        }
        "querywatchoption_action_eqopt_resourcegrouprunaw--0cc7e41801bfb70c" => {
            AdminRule::QueryWatchOptionAlt03
        }
        "querywatchoption_querywatchtextoption--953b292d5b3bc717" => {
            AdminRule::QueryWatchOptionAlt04
        }
        "querywatchoption_resource_group_resourcegroupnam--e07ec04908f01ad3" => {
            AdminRule::QueryWatchOptionAlt01
        }
        "querywatchoption_resource_group_uservariable--fd3950ae90831922" => {
            AdminRule::QueryWatchOptionAlt02
        }
        "querywatchoptionlist_querywatchoption--2fde3e96589e0fe8" => {
            AdminRule::QueryWatchOptionListAlt01
        }
        "querywatchoptionlist_querywatchoptionlist_queryw--a8c56c264fc00872" => {
            AdminRule::QueryWatchOptionListAlt02
        }
        "querywatchoptionlist_querywatchoptionlist_queryw--f62ec7182dc30d2e" => {
            AdminRule::QueryWatchOptionListAlt03
        }
        "querywatchtextoption_plan_digest_simpleexpr--c9ff76d577562bd5" => {
            AdminRule::QueryWatchTextOptionAlt02
        }
        "querywatchtextoption_sql_digest_simpleexpr--0616d44617fdf81a" => {
            AdminRule::QueryWatchTextOptionAlt01
        }
        "querywatchtextoption_sql_text_resourcegrouprunaw--16812f684284f150" => {
            AdminRule::QueryWatchTextOptionAlt03
        }
        "recommendindexoption_identifier_literal--36f46d090166b74a" => {
            AdminRule::RecommendIndexOptionAlt01
        }
        "recommendindexoptionlist_recommendindexoption--6f5af21d82d58218" => {
            AdminRule::RecommendIndexOptionListAlt01
        }
        "recommendindexoptionlist_recommendindexoptionlis--0ef7c46c4dde10cf" => {
            AdminRule::RecommendIndexOptionListAlt02
        }
        "recommendindexoptionlistopt--03b41fd8ccc8c583" => {
            AdminRule::RecommendIndexOptionListOptAlt01
        }
        "recommendindexoptionlistopt_with_recommendindexo--95e947de5dfcc361" => {
            AdminRule::RecommendIndexOptionListOptAlt02
        }
        "recommendindexstmt_recommend_index_apply_num--aa351dea9c931cbe" => {
            AdminRule::RecommendIndexStmtAlt04
        }
        "recommendindexstmt_recommend_index_ignore_num--7ed1f68d408d3040" => {
            AdminRule::RecommendIndexStmtAlt05
        }
        "recommendindexstmt_recommend_index_run_for_strin--d641ccabf44000c2" => {
            AdminRule::RecommendIndexStmtAlt01
        }
        "recommendindexstmt_recommend_index_run_recommend--a569e021ff5e7d71" => {
            AdminRule::RecommendIndexStmtAlt02
        }
        "recommendindexstmt_recommend_index_set_recommend--8c0bc3de6ec72b39" => {
            AdminRule::RecommendIndexStmtAlt06
        }
        "recommendindexstmt_recommend_index_show_option--35ca466f3d9c5b84" => {
            AdminRule::RecommendIndexStmtAlt03
        }
        "refreshstatsclusteropt--a4ca03b56c434a94" => AdminRule::RefreshStatsClusterOptAlt01,
        "refreshstatsclusteropt_cluster--e66b9cbdccc33df5" => {
            AdminRule::RefreshStatsClusterOptAlt02
        }
        "refreshstatsmode_full--1be234b8f972e9ae" => AdminRule::RefreshStatsModeAlt01,
        "refreshstatsmode_lite--2a0bdacc9951959d" => AdminRule::RefreshStatsModeAlt02,
        "refreshstatsmodeopt--ac678adecca49a21" => AdminRule::RefreshStatsModeOptAlt01,
        "refreshstatsmodeopt_refreshstatsmode--e1d14e708fd90b59" => {
            AdminRule::RefreshStatsModeOptAlt02
        }
        "refreshstatsstmt_refresh_stats_statsobjectlist_r--5d6c0fffa53b6103" => {
            AdminRule::RefreshStatsStmtAlt01
        }
        "resourcegroupbackgroundoptionlist_directresource--74d452e39e5045c5" => {
            AdminRule::ResourceGroupBackgroundOptionListAlt01
        }
        "resourcegroupbackgroundoptionlist_resourcegroupb--af12c68880ccab77" => {
            AdminRule::ResourceGroupBackgroundOptionListAlt02
        }
        "resourcegroupbackgroundoptionlist_resourcegroupb--c8cd4e300f898c33" => {
            AdminRule::ResourceGroupBackgroundOptionListAlt03
        }
        "resourcegroupoptionlist_directresourcegroupoptio--1549e8756d86918d" => {
            AdminRule::ResourceGroupOptionListAlt01
        }
        "resourcegroupoptionlist_resourcegroupoptionlist--722061d620847ac7" => {
            AdminRule::ResourceGroupOptionListAlt02
        }
        "resourcegroupoptionlist_resourcegroupoptionlist--76018f1c0488d08b" => {
            AdminRule::ResourceGroupOptionListAlt03
        }
        "resourcegrouppriorityoption_high--cefab38c264a0e2c" => {
            AdminRule::ResourceGroupPriorityOptionAlt03
        }
        "resourcegrouppriorityoption_low--89fcb249490ddb2a" => {
            AdminRule::ResourceGroupPriorityOptionAlt01
        }
        "resourcegrouppriorityoption_medium--7ab0c5b5a1bda1cd" => {
            AdminRule::ResourceGroupPriorityOptionAlt02
        }
        "resourcegrouprunawayactionoption_cooldown--930a7e6418636d46" => {
            AdminRule::ResourceGroupRunawayActionOptionAlt02
        }
        "resourcegrouprunawayactionoption_dryrun--950490ab2a3c8807" => {
            AdminRule::ResourceGroupRunawayActionOptionAlt01
        }
        "resourcegrouprunawayactionoption_kill--3df02edcc498ca39" => {
            AdminRule::ResourceGroupRunawayActionOptionAlt03
        }
        "resourcegrouprunawayactionoption_switch_group_re--0300cdf3fd6e9354" => {
            AdminRule::ResourceGroupRunawayActionOptionAlt04
        }
        "resourcegrouprunawayoptionlist_directresourcegro--7099e9e0cc3b22d1" => {
            AdminRule::ResourceGroupRunawayOptionListAlt01
        }
        "resourcegrouprunawayoptionlist_resourcegroupruna--02913ecc70702866" => {
            AdminRule::ResourceGroupRunawayOptionListAlt02
        }
        "resourcegrouprunawayoptionlist_resourcegroupruna--90249386b64146ba" => {
            AdminRule::ResourceGroupRunawayOptionListAlt03
        }
        "resourcegrouprunawaywatchoption_exact--d89c88d81374a01b" => {
            AdminRule::ResourceGroupRunawayWatchOptionAlt01
        }
        "resourcegrouprunawaywatchoption_plan--82f3177aa41ce233" => {
            AdminRule::ResourceGroupRunawayWatchOptionAlt03
        }
        "resourcegrouprunawaywatchoption_similar--c873c669dcd35b19" => {
            AdminRule::ResourceGroupRunawayWatchOptionAlt02
        }
        "restartstmt_restart--6e3619aa28ec939c" => AdminRule::RestartStmtAlt01,
        "revokestmt_revoke_roleorprivelemlist_on_objectty--86446a029b94172a" => {
            AdminRule::RevokeStmtAlt01
        }
        "sequenceoption_cache_eqopt_signednum--b67857ceabfdc081" => AdminRule::SequenceOptionAlt11,
        "sequenceoption_cycle--9f68eabc75e38b4a" => AdminRule::SequenceOptionAlt14,
        "sequenceoption_increment_by_signednum--cad487f13ffe3a9e" => AdminRule::SequenceOptionAlt02,
        "sequenceoption_increment_eqopt_signednum--a05139c85ba45234" => {
            AdminRule::SequenceOptionAlt01
        }
        "sequenceoption_maxvalue_eqopt_signednum--2df818bbec9bc262" => {
            AdminRule::SequenceOptionAlt08
        }
        "sequenceoption_minvalue_eqopt_signednum--d488e81f45e12e78" => {
            AdminRule::SequenceOptionAlt05
        }
        "sequenceoption_no_cache--b77f278f64d1bfb7" => AdminRule::SequenceOptionAlt13,
        "sequenceoption_no_cycle--1ed5ece62554c7a3" => AdminRule::SequenceOptionAlt16,
        "sequenceoption_no_maxvalue--e110f15e6f501d76" => AdminRule::SequenceOptionAlt10,
        "sequenceoption_no_minvalue--f77833d2e58ec330" => AdminRule::SequenceOptionAlt07,
        "sequenceoption_nocache--213108a4fcba2769" => AdminRule::SequenceOptionAlt12,
        "sequenceoption_nocycle--3f743c77d912c275" => AdminRule::SequenceOptionAlt15,
        "sequenceoption_nomaxvalue--71a7825c1edca3d8" => AdminRule::SequenceOptionAlt09,
        "sequenceoption_nominvalue--6194a9c65c6b76ae" => AdminRule::SequenceOptionAlt06,
        "sequenceoption_start_eqopt_signednum--3a558787b4a6f6f7" => AdminRule::SequenceOptionAlt03,
        "sequenceoption_start_with_signednum--4b3a0bd6c233f9c0" => AdminRule::SequenceOptionAlt04,
        "sequenceoptionlist_sequenceoption--acc6c3f977a2d8fc" => AdminRule::SequenceOptionListAlt01,
        "sequenceoptionlist_sequenceoptionlist_sequenceop--1935ce9b597901a0" => {
            AdminRule::SequenceOptionListAlt02
        }
        "setbindingstmt_set_binding_bindingstatustype_for--7516e6d958167599" => {
            AdminRule::SetBindingStmtAlt01
        }
        "setbindingstmt_set_binding_bindingstatustype_for--d4fef22b58972342" => {
            AdminRule::SetBindingStmtAlt03
        }
        "setbindingstmt_set_binding_bindingstatustype_for--f0bcaa6ee1dcdf86" => {
            AdminRule::SetBindingStmtAlt02
        }
        "setexpr_binary--1d9916d0a012d0d6" => AdminRule::SetExprAlt02,
        "setexpr_on--57eb5c7812905f0a" => AdminRule::SetExprAlt01,
        "setstmt_set_config_identifier_configitemname_eqo--817d91a81eb7e49c" => {
            AdminRule::SetStmtAlt09
        }
        "setstmt_set_config_stringlit_configitemname_eqor--53abd0ccda1462e1" => {
            AdminRule::SetStmtAlt10
        }
        "setstmt_set_global_transaction_transactionchars--6ed770d9d74c1ea2" => {
            AdminRule::SetStmtAlt06
        }
        "setstmt_set_password_eqorassignmenteq_passwordop--34f5d5677c0da0d8" => {
            AdminRule::SetStmtAlt03
        }
        "setstmt_set_password_eqorassignmenteq_passwordop--b61277b6ced02697" => {
            AdminRule::SetStmtAlt02
        }
        "setstmt_set_password_for_username_eqorassignment--48c926560b894f1b" => {
            AdminRule::SetStmtAlt05
        }
        "setstmt_set_password_for_username_eqorassignment--bf436aef79ceedf4" => {
            AdminRule::SetStmtAlt04
        }
        "setstmt_set_resource_group_resourcegroupname--e84882c277c55a7d" => AdminRule::SetStmtAlt12,
        "setstmt_set_session_states_stringlit--619fc6c83a3f8547" => AdminRule::SetStmtAlt11,
        "setstmt_set_session_transaction_transactionchars--3ee8a6eeac882b69" => {
            AdminRule::SetStmtAlt07
        }
        "setstmt_set_transaction_transactionchars--605c5f3903318cb9" => AdminRule::SetStmtAlt08,
        "setstmt_set_variableassignmentlist--746c2351419117e1" => AdminRule::SetStmtAlt01,
        "showdatabasenameopt--8a361e36078a7d4b" => AdminRule::ShowDatabaseNameOptAlt01,
        "showdatabasenameopt_fromorin_dbname--bb31713fe01e89e1" => {
            AdminRule::ShowDatabaseNameOptAlt02
        }
        "showimportjobstarget_import_jobs--650cd1497646a122" => {
            AdminRule::ShowImportJobsTargetAlt01
        }
        "showimportjobstarget_raw_import_jobs--df9523a90b43b724" => {
            AdminRule::ShowImportJobsTargetAlt02
        }
        "showimportjobtarget_import_job--97b5b0709c0bacaa" => AdminRule::ShowImportJobTargetAlt01,
        "showimportjobtarget_raw_import_job--5ea8ead9755ca198" => {
            AdminRule::ShowImportJobTargetAlt02
        }
        "showlikeorwhereopt--e3b4a3ae40a1379a" => AdminRule::ShowLikeOrWhereOptAlt01,
        "showlikeorwhereopt_like_simpleexpr--1f1684ae50799503" => {
            AdminRule::ShowLikeOrWhereOptAlt02
        }
        "showlikeorwhereopt_where_expression--7fc02e40ab9d76d6" => {
            AdminRule::ShowLikeOrWhereOptAlt03
        }
        "showplacementtarget_databasesym_dbname--728380eb66fcce0a" => {
            AdminRule::ShowPlacementTargetAlt01
        }
        "showplacementtarget_table_tablename--995a063eb152d15a" => {
            AdminRule::ShowPlacementTargetAlt02
        }
        "showplacementtarget_table_tablename_partition_id--72f4e3b1212b74e3" => {
            AdminRule::ShowPlacementTargetAlt03
        }
        "showprofileargsopt--de34287beb64c541" => AdminRule::ShowProfileArgsOptAlt01,
        "showprofileargsopt_for_query_int64num--6b3e15ae00c6bc56" => {
            AdminRule::ShowProfileArgsOptAlt02
        }
        "showprofiletype_all--06c408e1c0dad291" => AdminRule::ShowProfileTypeAlt09,
        "showprofiletype_block_io--369ec218b18f09c1" => AdminRule::ShowProfileTypeAlt03,
        "showprofiletype_context_switches--e9196b961cdb24af" => AdminRule::ShowProfileTypeAlt04,
        "showprofiletype_cpu--1e0526cf0177d6c2" => AdminRule::ShowProfileTypeAlt01,
        "showprofiletype_ipc--5d84db225f514226" => AdminRule::ShowProfileTypeAlt06,
        "showprofiletype_memory--e21627ea7c199a0d" => AdminRule::ShowProfileTypeAlt02,
        "showprofiletype_page_faults--0007aabcab406330" => AdminRule::ShowProfileTypeAlt05,
        "showprofiletype_source--1d4b1eba36034537" => AdminRule::ShowProfileTypeAlt08,
        "showprofiletype_swaps--d82d6de1a45e73a0" => AdminRule::ShowProfileTypeAlt07,
        "showprofiletypes_showprofiletype--44003816e74ccceb" => AdminRule::ShowProfileTypesAlt01,
        "showprofiletypes_showprofiletypes_showprofiletyp--fe22efd8c88cb576" => {
            AdminRule::ShowProfileTypesAlt02
        }
        "showprofiletypesopt--399ba47d1886882f" => AdminRule::ShowProfileTypesOptAlt01,
        "showstmt_show_binary_log_status--cb562164948b1a8d" => AdminRule::ShowStmtAlt16,
        "showstmt_show_builtins--9518c5dc9001e86c" => AdminRule::ShowStmtAlt22,
        "showstmt_show_create_database_ifnotexists_dbname--2992c5e38415b058" => {
            AdminRule::ShowStmtAlt04
        }
        "showstmt_show_create_placement_policy_policyname--822071665e219932" => {
            AdminRule::ShowStmtAlt06
        }
        "showstmt_show_create_procedure_tablename--a3c5653986869e54" => AdminRule::ShowStmtAlt26,
        "showstmt_show_create_resource_group_resourcegrou--2d742768cf7919d1" => {
            AdminRule::ShowStmtAlt07
        }
        "showstmt_show_create_sequence_tablename--6890f1f0edb8a8c4" => AdminRule::ShowStmtAlt05,
        "showstmt_show_create_table_tablename--f82ffa83d02a0d17" => AdminRule::ShowStmtAlt02,
        "showstmt_show_create_user_username--c1dfdfa07f199353" => AdminRule::ShowStmtAlt08,
        "showstmt_show_create_view_tablename--b21cd7dcf18b8b94" => AdminRule::ShowStmtAlt03,
        "showstmt_show_distribution_job_int64num--4e3a290d2955a24c" => AdminRule::ShowStmtAlt25,
        "showstmt_show_grants--02f25ed4b9e86e65" => AdminRule::ShowStmtAlt13,
        "showstmt_show_grants_for_username_usingroles--cb2ba4f97d5e3415" => {
            AdminRule::ShowStmtAlt14
        }
        "showstmt_show_masking_policies_for_tablename_whe--e9a3cb3a1d5dc466" => {
            AdminRule::ShowStmtAlt09
        }
        "showstmt_show_master_status--e39dd659dec463f4" => AdminRule::ShowStmtAlt15,
        "showstmt_show_optfull_processlist--587fb85ac59f4133" => AdminRule::ShowStmtAlt18,
        "showstmt_show_placement_for_showplacementtarget--6a9e85a0632bcf89" => {
            AdminRule::ShowStmtAlt23
        }
        "showstmt_show_privileges--a54a898793fcaeb4" => AdminRule::ShowStmtAlt21,
        "showstmt_show_profile_showprofiletypesopt_showpr--d59769d939af6e89" => {
            AdminRule::ShowStmtAlt20
        }
        "showstmt_show_profiles--bd60ed2e117da09e" => AdminRule::ShowStmtAlt19,
        "showstmt_show_replica_status--2677894b11d5a18a" => AdminRule::ShowStmtAlt17,
        "showstmt_show_showimportjobtarget_int64num--89e398f1733ec8b7" => AdminRule::ShowStmtAlt24,
        "showstmt_show_showtargetfilterable_showlikeorwhe--c44224332f737061" => {
            AdminRule::ShowStmtAlt01
        }
        "showstmt_show_table_tablename_next_row_id--72c353b71ccd6179" => AdminRule::ShowStmtAlt11,
        "showstmt_show_table_tablename_partitionnamelisto--d128720d9fdd2540" => {
            AdminRule::ShowStmtAlt27
        }
        "showstmt_show_table_tablename_partitionnamelisto--d87072b00a48d207" => {
            AdminRule::ShowStmtAlt12
        }
        "showstmt_show_table_tablename_partitionnamelisto--f3be36a1c2ec7336" => {
            AdminRule::ShowStmtAlt10
        }
        "showtablealiasopt_fromorin_tablename--2aadda84d0b7a645" => {
            AdminRule::ShowTableAliasOptAlt01
        }
        "showtargetfilterable_affinity--43f9fcb66f0acd12" => AdminRule::ShowTargetFilterableAlt36,
        "showtargetfilterable_analyze_status--f29500a09ba6521c" => {
            AdminRule::ShowTargetFilterableAlt37
        }
        "showtargetfilterable_backups--b36de42e5ca19e1f" => AdminRule::ShowTargetFilterableAlt38,
        "showtargetfilterable_binding_cache_status--930a64ff253a3008" => {
            AdminRule::ShowTargetFilterableAlt21
        }
        "showtargetfilterable_builtincount_errors--90ffd7c1fff0fc60" => {
            AdminRule::ShowTargetFilterableAlt14
        }
        "showtargetfilterable_builtincount_warnings--ec192f7508326246" => {
            AdminRule::ShowTargetFilterableAlt12
        }
        "showtargetfilterable_charsetkw--9439a85e9fc5f978" => AdminRule::ShowTargetFilterableAlt04,
        "showtargetfilterable_collation--c319ea2c45f24133" => AdminRule::ShowTargetFilterableAlt19,
        "showtargetfilterable_column_stats_usage--81977f99057e2c2c" => {
            AdminRule::ShowTargetFilterableAlt35
        }
        "showtargetfilterable_config--ebae373eaa040504" => AdminRule::ShowTargetFilterableAlt03,
        "showtargetfilterable_databases--371e3b7eb9aacfa8" => AdminRule::ShowTargetFilterableAlt02,
        "showtargetfilterable_distribution_jobs--5093905d6dad9e24" => {
            AdminRule::ShowTargetFilterableAlt45
        }
        "showtargetfilterable_storage_class_transitions--44a1c6c4a3e10740" => {
            AdminRule::ShowTargetFilterableAlt46
        }
        "showtargetfilterable_engines--6aeb24d0cec61223" => AdminRule::ShowTargetFilterableAlt01,
        "showtargetfilterable_errors--eae33fdb13df74b7" => AdminRule::ShowTargetFilterableAlt15,
        "showtargetfilterable_events_showdatabasenameopt--3ffd5fa309a49891" => {
            AdminRule::ShowTargetFilterableAlt24
        }
        "showtargetfilterable_extended_optfull_fieldsorco--f4767f30e29a7aac" => {
            AdminRule::ShowTargetFilterableAlt11
        }
        "showtargetfilterable_function_status--9d53b4879d36f058" => {
            AdminRule::ShowTargetFilterableAlt23
        }
        "showtargetfilterable_globalscope_bindings--2e377768501f0237" => {
            AdminRule::ShowTargetFilterableAlt18
        }
        "showtargetfilterable_globalscope_status--73590b3734fcb6b3" => {
            AdminRule::ShowTargetFilterableAlt17
        }
        "showtargetfilterable_globalscope_variables--9ba9b0978100ea64" => {
            AdminRule::ShowTargetFilterableAlt16
        }
        "showtargetfilterable_histograms_in_flight--261357f90afec250" => {
            AdminRule::ShowTargetFilterableAlt34
        }
        "showtargetfilterable_import_group_stringlit--b2ed9e4751c13726" => {
            AdminRule::ShowTargetFilterableAlt43
        }
        "showtargetfilterable_import_groups--1c7795484bb0f927" => {
            AdminRule::ShowTargetFilterableAlt42
        }
        "showtargetfilterable_open_tables_showdatabasenam--cae1e51a61841685" => {
            AdminRule::ShowTargetFilterableAlt06
        }
        "showtargetfilterable_optfull_fieldsorcolumns_sho--eb208563392a06c7" => {
            AdminRule::ShowTargetFilterableAlt10
        }
        "showtargetfilterable_optfull_tables_showdatabase--500663d5de882195" => {
            AdminRule::ShowTargetFilterableAlt05
        }
        "showtargetfilterable_placement--100315d7092d726f" => AdminRule::ShowTargetFilterableAlt40,
        "showtargetfilterable_placement_labels--62fe5d49bfdb4530" => {
            AdminRule::ShowTargetFilterableAlt41
        }
        "showtargetfilterable_plugins--6a293fad7ad8c1c6" => AdminRule::ShowTargetFilterableAlt25,
        "showtargetfilterable_procedure_status--f87f5718fcd42549" => {
            AdminRule::ShowTargetFilterableAlt22
        }
        "showtargetfilterable_restores--ddd5dc52a79a5473" => AdminRule::ShowTargetFilterableAlt39,
        "showtargetfilterable_session_states--94be5175c326969b" => {
            AdminRule::ShowTargetFilterableAlt26
        }
        "showtargetfilterable_showimportjobstarget--1ac11e4ba49561ef" => {
            AdminRule::ShowTargetFilterableAlt44
        }
        "showtargetfilterable_showindexkwd_fromorin_ident--79ca0f47952223d1" => {
            AdminRule::ShowTargetFilterableAlt09
        }
        "showtargetfilterable_showindexkwd_fromorin_table--f7373065b640a6ee" => {
            AdminRule::ShowTargetFilterableAlt08
        }
        "showtargetfilterable_stats_buckets--18d38a911fc4473b" => {
            AdminRule::ShowTargetFilterableAlt31
        }
        "showtargetfilterable_stats_extended--4c63006a77cbb28d" => {
            AdminRule::ShowTargetFilterableAlt27
        }
        "showtargetfilterable_stats_healthy--e32b2797e93c05c9" => {
            AdminRule::ShowTargetFilterableAlt32
        }
        "showtargetfilterable_stats_histograms--fcbef3f3ea640b8f" => {
            AdminRule::ShowTargetFilterableAlt29
        }
        "showtargetfilterable_stats_locked--481644ee97255f38" => {
            AdminRule::ShowTargetFilterableAlt33
        }
        "showtargetfilterable_stats_meta--67c1cd42b79a2dd9" => AdminRule::ShowTargetFilterableAlt28,
        "showtargetfilterable_stats_topn--1f5a89d01d1f638b" => AdminRule::ShowTargetFilterableAlt30,
        "showtargetfilterable_table_status_showdatabasena--95c78070a9074008" => {
            AdminRule::ShowTargetFilterableAlt07
        }
        "showtargetfilterable_triggers_showdatabasenameop--665cc2ac8d5cce35" => {
            AdminRule::ShowTargetFilterableAlt20
        }
        "showtargetfilterable_warnings--a72d976e46d7ce3d" => AdminRule::ShowTargetFilterableAlt13,
        "shutdownstmt_shutdown--20a8bac1e8cc90ae" => AdminRule::ShutdownStmtAlt01,
        "statementscope--e1dd19762f521caa" => AdminRule::StatementScopeAlt01,
        "statementscope_global--65e9c67b9470a602" => AdminRule::StatementScopeAlt02,
        "statementscope_instance--cb0a8e49dde535aa" => AdminRule::StatementScopeAlt03,
        "statementscope_session--c5bff09e372c5da1" => AdminRule::StatementScopeAlt04,
        "statsobject--bcd5d10490a262e0" => AdminRule::StatsObjectAlt01,
        "statsobject_identifier--e160a1694dab826d" => AdminRule::StatsObjectAlt02,
        "statsobject_identifier--ecc8c512eb2e7d89" => AdminRule::StatsObjectAlt04,
        "statsobject_identifier_identifier--1e1edb0ed33eb13e" => AdminRule::StatsObjectAlt03,
        "statsobjectlist_statsobject--acbadb6580bdbf3e" => AdminRule::StatsObjectListAlt01,
        "statsobjectlist_statsobjectlist_statsobject--4985755cef374ed6" => {
            AdminRule::StatsObjectListAlt02
        }
        "statstype_cardinality--ee6d3e8395b57857" => AdminRule::StatsTypeAlt01,
        "statstype_correlation--014926a3015f4027" => AdminRule::StatsTypeAlt03,
        "statstype_dependency--b50a99cd1c5cbf12" => AdminRule::StatsTypeAlt02,
        "tablelock_tablename_locktype--5d2c675200bfa617" => AdminRule::TableLockAlt01,
        "tablelocklist_tablelock--303cfe2dccf49564" => AdminRule::TableLockListAlt01,
        "tablelocklist_tablelocklist_tablelock--ad945683aa461993" => AdminRule::TableLockListAlt02,
        "tablenamelistopt_prec_empty--2acd452fa41aa26f" => AdminRule::TableNameListOptAlt01,
        "trafficcaptureopt_compress_eqopt_boolean--224c422dd643f017" => {
            AdminRule::TrafficCaptureOptAlt03
        }
        "trafficcaptureopt_duration_eqopt_stringlit--86445f66a5422fed" => {
            AdminRule::TrafficCaptureOptAlt01
        }
        "trafficcaptureopt_encryption_method_eqopt_string--93113671d95abfdc" => {
            AdminRule::TrafficCaptureOptAlt02
        }
        "trafficcaptureoptlist_trafficcaptureopt--1ec3308e8f6c4106" => {
            AdminRule::TrafficCaptureOptListAlt01
        }
        "trafficcaptureoptlist_trafficcaptureoptlist_traf--df469889754899d0" => {
            AdminRule::TrafficCaptureOptListAlt02
        }
        "trafficreplayopt_password_eqopt_stringlit--bfe0596c781e542d" => {
            AdminRule::TrafficReplayOptAlt02
        }
        "trafficreplayopt_read_only_eqopt_boolean--fd748ba7aa6acf03" => {
            AdminRule::TrafficReplayOptAlt04
        }
        "trafficreplayopt_speed_eqopt_numliteral--d600d3f639e76af2" => {
            AdminRule::TrafficReplayOptAlt03
        }
        "trafficreplayopt_user_eqopt_stringlit--fc09b406b2514be7" => {
            AdminRule::TrafficReplayOptAlt01
        }
        "trafficreplayoptlist_trafficreplayopt--5a38339650a09540" => {
            AdminRule::TrafficReplayOptListAlt01
        }
        "trafficreplayoptlist_trafficreplayoptlist_traffi--3dff999a27f64b07" => {
            AdminRule::TrafficReplayOptListAlt02
        }
        "trafficstmt_cancel_traffic_jobs--e0aa75bfe1b2b028" => AdminRule::TrafficStmtAlt04,
        "trafficstmt_show_traffic_jobs--f8d5879f4c28ab8b" => AdminRule::TrafficStmtAlt03,
        "trafficstmt_traffic_capture_to_stringlit_traffic--e9a03f043d130311" => {
            AdminRule::TrafficStmtAlt01
        }
        "trafficstmt_traffic_replay_from_stringlit_traffi--2df8a6005e96cd34" => {
            AdminRule::TrafficStmtAlt02
        }
        "unlockstatsstmt_unlock_stats_tablename_partition--ca4bbf5ef29234b3" => {
            AdminRule::UnlockStatsStmtAlt03
        }
        "unlockstatsstmt_unlock_stats_tablename_partition--d77535d57e11b376" => {
            AdminRule::UnlockStatsStmtAlt02
        }
        "unlockstatsstmt_unlock_stats_tablenamelist--a84e4dc2de62c08d" => {
            AdminRule::UnlockStatsStmtAlt01
        }
        "unlocktablesstmt_unlock_tablesterminalsym--98fd591be0972f8b" => {
            AdminRule::UnlockTablesStmtAlt01
        }
        "usingroles--cc734ef0c9538ec8" => AdminRule::UsingRolesAlt01,
        "usingroles_using_rolenamelist--831563cd0dca033a" => AdminRule::UsingRolesAlt02,
        "variableassignment_charsetkw_charsetnameordefaul--081886270960e1a8" => {
            AdminRule::VariableAssignmentAlt12
        }
        "variableassignment_doubleatidentifier_eqorassign--48bd23c6571c4651" => {
            AdminRule::VariableAssignmentAlt06
        }
        "variableassignment_global_variablename_eqorassig--0c698e68e27b481a" => {
            AdminRule::VariableAssignmentAlt02
        }
        "variableassignment_instance_variablename_eqorass--980c06af0dd6ec26" => {
            AdminRule::VariableAssignmentAlt03
        }
        "variableassignment_local_variablename_eqorassign--e332711b52a141c0" => {
            AdminRule::VariableAssignmentAlt05
        }
        "variableassignment_names_charsetname--c51548a055c402a4" => {
            AdminRule::VariableAssignmentAlt08
        }
        "variableassignment_names_charsetname_collate_def--ed8be5a94bba1521" => {
            AdminRule::VariableAssignmentAlt09
        }
        "variableassignment_names_charsetname_collate_str--798ad59035ba0bdc" => {
            AdminRule::VariableAssignmentAlt10
        }
        "variableassignment_names_default--22fc9be28c88c090" => AdminRule::VariableAssignmentAlt11,
        "variableassignment_session_variablename_eqorassi--ae2c9f629950dae1" => {
            AdminRule::VariableAssignmentAlt04
        }
        "variableassignment_singleatidentifier_eqorassign--cbd91a9d266a1e25" => {
            AdminRule::VariableAssignmentAlt07
        }
        "variableassignment_variablename_eqorassignmenteq--f0c5caabf069f1e1" => {
            AdminRule::VariableAssignmentAlt01
        }
        "variableassignmentlist_variableassignment--27ecadb15f032a9c" => {
            AdminRule::VariableAssignmentListAlt01
        }
        "variableassignmentlist_variableassignmentlist_va--eb5da27895ff5469" => {
            AdminRule::VariableAssignmentListAlt02
        }
        "variablename_identifier_identifier--d1f0d40cf78da6e9" => AdminRule::VariableNameAlt02,
        "watchdurationoption--184527ebf389c101" => AdminRule::WatchDurationOptionAlt01,
        "watchdurationoption_duration_eqopt_stringlit--749c56df3df20cc1" => {
            AdminRule::WatchDurationOptionAlt02
        }
        "watchdurationoption_duration_eqopt_unlimited--8dd77561d9695466" => {
            AdminRule::WatchDurationOptionAlt03
        }
        "withreadlockopt--6025ba027643325f" => AdminRule::WithReadLockOptAlt01,
        "withreadlockopt_with_read_lock--23557e07b06c70a7" => AdminRule::WithReadLockOptAlt02,
        _ => return None,
    })
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    identify(rule_id).is_some()
}

pub(super) fn apply(
    rule_id: RuleId,
    rhs: Rhs<'_>,
    context: Context<'_>,
) -> Option<Result<bool, isize>> {
    Some(apply_rule(identify(rule_id)?, rhs, context))
}

fn apply_rule(rule: AdminRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        AdminRule::ResourceGroupOptionListAlt01
        | AdminRule::ResourceGroupOptionListAlt02
        | AdminRule::ResourceGroupOptionListAlt03 => {
            let list_back = if rule == AdminRule::ResourceGroupOptionListAlt01 {
                None
            } else if rule == AdminRule::ResourceGroupOptionListAlt02 {
                Some(1)
            } else {
                Some(2)
            };
            let mut values = list_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::ResourceGroupOption>>()
                        })
                        .cloned()
                })
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ResourceGroupOption>())
            {
                if values.iter().any(|old| old.Tp == value.Tp) {
                    yylex.AppendError(yylex.Errorf("Dupliated options specified", &[]));
                    return Err(1);
                }
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::ResourceGroupPriorityOptionAlt01
        | AdminRule::ResourceGroupPriorityOptionAlt02
        | AdminRule::ResourceGroupPriorityOptionAlt03 => {
            out.item = Some(Box::new(match rule {
                AdminRule::ResourceGroupPriorityOptionAlt01 => 1u64,
                AdminRule::ResourceGroupPriorityOptionAlt02 => 8u64,
                _ => 16u64,
            }))
        }
        AdminRule::ResourceGroupRunawayOptionListAlt01
        | AdminRule::ResourceGroupRunawayOptionListAlt02
        | AdminRule::ResourceGroupRunawayOptionListAlt03 => {
            let list_back = if rule == AdminRule::ResourceGroupRunawayOptionListAlt01 {
                None
            } else if rule == AdminRule::ResourceGroupRunawayOptionListAlt02 {
                Some(1)
            } else {
                Some(2)
            };
            let mut values = list_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::ResourceGroupRunawayOption>>()
                        })
                        .cloned()
                })
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ResourceGroupRunawayOption>())
            {
                if value.Tp != parser_ast::ResourceGroupRunawayOptionType::Rule
                    && values.iter().any(|old| old.Tp == value.Tp)
                {
                    yylex.AppendError(yylex.Errorf("Dupliated runaway options specified", &[]));
                    return Err(1);
                }
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::ResourceGroupRunawayWatchOptionAlt01
        | AdminRule::ResourceGroupRunawayWatchOptionAlt02
        | AdminRule::ResourceGroupRunawayWatchOptionAlt03 => {
            out.item = Some(Box::new(match rule {
                AdminRule::ResourceGroupRunawayWatchOptionAlt01 => {
                    parser_ast::RunawayWatchType::Exact
                }
                AdminRule::ResourceGroupRunawayWatchOptionAlt02 => {
                    parser_ast::RunawayWatchType::Similar
                }
                _ => parser_ast::RunawayWatchType::Plan,
            }))
        }
        AdminRule::ResourceGroupRunawayActionOptionAlt01
        | AdminRule::ResourceGroupRunawayActionOptionAlt02
        | AdminRule::ResourceGroupRunawayActionOptionAlt03
        | AdminRule::ResourceGroupRunawayActionOptionAlt04 => {
            out.item = Some(Box::new(parser_ast::ResourceGroupRunawayActionOption {
                Type: match rule {
                    AdminRule::ResourceGroupRunawayActionOptionAlt01 => {
                        parser_ast::RunawayActionType::DryRun
                    }
                    AdminRule::ResourceGroupRunawayActionOptionAlt02 => {
                        parser_ast::RunawayActionType::Cooldown
                    }
                    AdminRule::ResourceGroupRunawayActionOptionAlt03 => {
                        parser_ast::RunawayActionType::Kill
                    }
                    _ => parser_ast::RunawayActionType::SwitchGroup,
                },
                SwitchGroupName: if rule == AdminRule::ResourceGroupRunawayActionOptionAlt04 {
                    parser_ast::NewCIStr(&rhs[rhs_len - (1)].ident)
                } else {
                    parser_ast::CIStr::default()
                },
            }))
        }
        AdminRule::DirectResourceGroupRunawayOptionAlt01
        | AdminRule::DirectResourceGroupRunawayOptionAlt02
        | AdminRule::DirectResourceGroupRunawayOptionAlt03
        | AdminRule::DirectResourceGroupRunawayOptionAlt04
        | AdminRule::DirectResourceGroupRunawayOptionAlt05 => {
            let mut option = parser_ast::ResourceGroupRunawayOption::default();
            match rule {
                AdminRule::DirectResourceGroupRunawayOptionAlt01 => {
                    let value = rhs[rhs_len - (0)].ident.clone();
                    if let Err(error) = validate_go_duration(&value) {
                        yylex.AppendError(yylex.Errorf(
                            &format!("The EXEC_ELAPSED option is not a valid duration: {error}"),
                            &[],
                        ));
                        return Err(1);
                    }
                    option.Tp = parser_ast::ResourceGroupRunawayOptionType::Rule;
                    option.RuleOption = Some(parser_ast::ResourceGroupRunawayRuleOption {
                        Tp: parser_ast::RunawayRuleType::ExecElapsed,
                        ExecElapsed: value,
                        ..Default::default()
                    });
                }
                AdminRule::DirectResourceGroupRunawayOptionAlt02
                | AdminRule::DirectResourceGroupRunawayOptionAlt03 => {
                    option.Tp = parser_ast::ResourceGroupRunawayOptionType::Rule;
                    let value = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default() as i64;
                    option.RuleOption = Some(parser_ast::ResourceGroupRunawayRuleOption {
                        Tp: if rule == AdminRule::DirectResourceGroupRunawayOptionAlt02 {
                            parser_ast::RunawayRuleType::ProcessedKeys
                        } else {
                            parser_ast::RunawayRuleType::RequestUnit
                        },
                        ProcessedKeys: if rule == AdminRule::DirectResourceGroupRunawayOptionAlt02 {
                            value
                        } else {
                            0
                        },
                        RequestUnit: if rule == AdminRule::DirectResourceGroupRunawayOptionAlt03 {
                            value
                        } else {
                            0
                        },
                        ..Default::default()
                    });
                }
                AdminRule::DirectResourceGroupRunawayOptionAlt04 => {
                    option.Tp = parser_ast::ResourceGroupRunawayOptionType::Action;
                    option.ActionOption = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<parser_ast::ResourceGroupRunawayActionOption>()
                        })
                        .cloned();
                }
                _ => {
                    let mut duration = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .map(semantic_value_text)
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if duration == "unlimited" {
                        duration.clear();
                    }
                    if !duration.is_empty()
                        && let Err(error) = validate_go_duration(&duration)
                    {
                        yylex.AppendError(yylex.Errorf(
                            &format!("The WATCH DURATION option is not a valid duration: {error}"),
                            &[],
                        ));
                        return Err(1);
                    }
                    option.Tp = parser_ast::ResourceGroupRunawayOptionType::Watch;
                    option.WatchOption = Some(parser_ast::ResourceGroupRunawayWatchOption {
                        Type: rhs[rhs_len - (1)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<parser_ast::RunawayWatchType>())
                            .copied()
                            .unwrap_or_default(),
                        Duration: duration,
                    });
                }
            }
            out.item = Some(Box::new(option));
        }
        AdminRule::WatchDurationOptionAlt01 | AdminRule::WatchDurationOptionAlt03 => {
            out.item = Some(Box::new(String::new()))
        }
        AdminRule::WatchDurationOptionAlt02 => {
            out.item = Some(Box::new(rhs[rhs_len - (0)].ident.clone()))
        }
        AdminRule::DirectResourceGroupOptionAlt01
        | AdminRule::DirectResourceGroupOptionAlt02
        | AdminRule::DirectResourceGroupOptionAlt03
        | AdminRule::DirectResourceGroupOptionAlt04
        | AdminRule::DirectResourceGroupOptionAlt05
        | AdminRule::DirectResourceGroupOptionAlt06
        | AdminRule::DirectResourceGroupOptionAlt07
        | AdminRule::DirectResourceGroupOptionAlt08
        | AdminRule::DirectResourceGroupOptionAlt09
        | AdminRule::DirectResourceGroupOptionAlt10
        | AdminRule::DirectResourceGroupOptionAlt11
        | AdminRule::DirectResourceGroupOptionAlt12
        | AdminRule::DirectResourceGroupOptionAlt13 => {
            let mut option = parser_ast::ResourceGroupOption::default();
            match rule {
                AdminRule::DirectResourceGroupOptionAlt01 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::RURate;
                    option.UintValue = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                        .max(0) as u64;
                }
                AdminRule::DirectResourceGroupOptionAlt02 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::RURate;
                    option.Burstable = parser_ast::BurstableType::Unlimited;
                }
                AdminRule::DirectResourceGroupOptionAlt03 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Priority;
                    option.UintValue = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                        .max(0) as u64;
                }
                AdminRule::DirectResourceGroupOptionAlt04
                | AdminRule::DirectResourceGroupOptionAlt05 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Burstable;
                    option.Burstable = parser_ast::BurstableType::Moderated;
                }
                AdminRule::DirectResourceGroupOptionAlt06 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Burstable;
                    option.Burstable = parser_ast::BurstableType::Unlimited;
                }
                AdminRule::DirectResourceGroupOptionAlt07 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Burstable;
                    option.Burstable = parser_ast::BurstableType::Disable;
                }
                AdminRule::DirectResourceGroupOptionAlt08 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Runaway;
                    option.RunawayOptionList = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::ResourceGroupRunawayOption>>()
                        })
                        .cloned()
                        .unwrap_or_default();
                }
                AdminRule::DirectResourceGroupOptionAlt09
                | AdminRule::DirectResourceGroupOptionAlt10 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Runaway
                }
                AdminRule::DirectResourceGroupOptionAlt11 => {
                    option.Tp = parser_ast::ResourceGroupOptionType::Background;
                    option.BackgroundOptions = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::ResourceGroupBackgroundOption>>()
                        })
                        .cloned()
                        .unwrap_or_default();
                }
                _ => option.Tp = parser_ast::ResourceGroupOptionType::Background,
            }
            out.item = Some(Box::new(option));
        }
        AdminRule::ResourceGroupBackgroundOptionListAlt01
        | AdminRule::ResourceGroupBackgroundOptionListAlt02
        | AdminRule::ResourceGroupBackgroundOptionListAlt03 => {
            let list_back = if rule == AdminRule::ResourceGroupBackgroundOptionListAlt01 {
                None
            } else if rule == AdminRule::ResourceGroupBackgroundOptionListAlt02 {
                Some(1)
            } else {
                Some(2)
            };
            let mut values = list_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::ResourceGroupBackgroundOption>>()
                        })
                        .cloned()
                })
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ResourceGroupBackgroundOption>())
            {
                if values.iter().any(|old| old.Type == value.Type) {
                    yylex.AppendError(yylex.Errorf("Dupliated background options specified", &[]));
                    return Err(1);
                }
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::DirectResourceGroupBackgroundOptionAlt01
        | AdminRule::DirectResourceGroupBackgroundOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::ResourceGroupBackgroundOption {
                Type: if rule == AdminRule::DirectResourceGroupBackgroundOptionAlt01 {
                    parser_ast::BackgroundOptionType::TaskNames
                } else {
                    parser_ast::BackgroundOptionType::UtilizationLimit
                },
                StrValue: if rule == AdminRule::DirectResourceGroupBackgroundOptionAlt01 {
                    rhs[rhs_len - (0)].ident.clone()
                } else {
                    String::new()
                },
                UintValue: if rule == AdminRule::DirectResourceGroupBackgroundOptionAlt02 {
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                        .max(0) as u64
                } else {
                    0
                },
            }))
        }
        AdminRule::AnalyzeTableStmtAlt01
        | AdminRule::AnalyzeTableStmtAlt02
        | AdminRule::AnalyzeTableStmtAlt03
        | AdminRule::AnalyzeTableStmtAlt04
        | AdminRule::AnalyzeTableStmtAlt05
        | AdminRule::AnalyzeTableStmtAlt06
        | AdminRule::AnalyzeTableStmtAlt07
        | AdminRule::AnalyzeTableStmtAlt08
        | AdminRule::AnalyzeTableStmtAlt09
        | AdminRule::AnalyzeTableStmtAlt10 => {
            let table_back = match rule {
                AdminRule::AnalyzeTableStmtAlt01 => 2,
                AdminRule::AnalyzeTableStmtAlt02 | AdminRule::AnalyzeTableStmtAlt03 => 3,
                AdminRule::AnalyzeTableStmtAlt04 | AdminRule::AnalyzeTableStmtAlt08 => 4,
                AdminRule::AnalyzeTableStmtAlt05
                | AdminRule::AnalyzeTableStmtAlt06
                | AdminRule::AnalyzeTableStmtAlt07
                | AdminRule::AnalyzeTableStmtAlt10 => 5,
                AdminRule::AnalyzeTableStmtAlt09 => 3,
                _ => 2,
            };
            let mut tables = if rule == AdminRule::AnalyzeTableStmtAlt01 {
                rhs[rhs_len - (table_back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                rhs[rhs_len - (table_back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .into_iter()
                    .collect()
            };
            if tables.is_empty() {
                return Ok(false);
            }
            let no_write_back = match rule {
                AdminRule::AnalyzeTableStmtAlt01 => 4,
                AdminRule::AnalyzeTableStmtAlt02 => 5,
                AdminRule::AnalyzeTableStmtAlt03 => 6,
                AdminRule::AnalyzeTableStmtAlt04 => 6,
                AdminRule::AnalyzeTableStmtAlt05 => 7,
                AdminRule::AnalyzeTableStmtAlt06 => 8,
                AdminRule::AnalyzeTableStmtAlt07 => 7,
                AdminRule::AnalyzeTableStmtAlt08 => 6,
                AdminRule::AnalyzeTableStmtAlt09 => 5,
                AdminRule::AnalyzeTableStmtAlt10 => 7,
                _ => 0,
            };
            let no_write = rhs[rhs_len - (no_write_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            let mut statement = parser_ast::AnalyzeTableStmt {
                TableNames: std::mem::take(&mut tables),
                NoWriteToBinLog: no_write,
                ..Default::default()
            };
            match rule {
                AdminRule::AnalyzeTableStmtAlt01 => {
                    statement.ColumnChoice = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ColumnChoice>())
                        .copied()
                        .unwrap_or_default();
                    statement.AnalyzeOpts = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                        .cloned()
                        .unwrap_or_default();
                }
                AdminRule::AnalyzeTableStmtAlt02 | AdminRule::AnalyzeTableStmtAlt03 => {
                    statement.IndexNames = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.IndexFlag = true;
                    statement.Incremental = rule == AdminRule::AnalyzeTableStmtAlt03;
                    statement.AnalyzeOpts = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                        .cloned()
                        .unwrap_or_default();
                }
                AdminRule::AnalyzeTableStmtAlt04 => {
                    statement.PartitionNames = rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.ColumnChoice = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ColumnChoice>())
                        .copied()
                        .unwrap_or_default();
                    statement.AnalyzeOpts = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                        .cloned()
                        .unwrap_or_default();
                }
                AdminRule::AnalyzeTableStmtAlt05 | AdminRule::AnalyzeTableStmtAlt06 => {
                    statement.PartitionNames = rhs[rhs_len - (3)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.IndexNames = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.IndexFlag = true;
                    statement.Incremental = rule == AdminRule::AnalyzeTableStmtAlt06;
                    statement.AnalyzeOpts = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                        .cloned()
                        .unwrap_or_default();
                }
                AdminRule::AnalyzeTableStmtAlt07 | AdminRule::AnalyzeTableStmtAlt08 => {
                    statement.ColumnNames = rhs[rhs_len
                        - (if rule == AdminRule::AnalyzeTableStmtAlt07 {
                            1
                        } else {
                            0
                        })]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default();
                    statement.HistogramOperation = if rule == AdminRule::AnalyzeTableStmtAlt07 {
                        parser_ast::HistogramOperationType::Update
                    } else {
                        parser_ast::HistogramOperationType::Drop
                    };
                    if rule == AdminRule::AnalyzeTableStmtAlt07 {
                        statement.AnalyzeOpts = rhs[rhs_len - (0)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                            .cloned()
                            .unwrap_or_default();
                    }
                }
                AdminRule::AnalyzeTableStmtAlt09 | AdminRule::AnalyzeTableStmtAlt10 => {
                    if rule == AdminRule::AnalyzeTableStmtAlt10 {
                        statement.PartitionNames = rhs[rhs_len - (3)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                            .cloned()
                            .unwrap_or_default();
                    }
                    statement.ColumnNames = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.ColumnChoice = parser_ast::ColumnChoice::List;
                    statement.AnalyzeOpts = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                        .cloned()
                        .unwrap_or_default();
                }
                _ => {}
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AllColumnsOrPredicateColumnsOptAlt01
        | AdminRule::AllColumnsOrPredicateColumnsOptAlt02
        | AdminRule::AllColumnsOrPredicateColumnsOptAlt03 => {
            out.item = Some(Box::new(match rule {
                AdminRule::AllColumnsOrPredicateColumnsOptAlt01 => {
                    parser_ast::ColumnChoice::Default
                }
                AdminRule::AllColumnsOrPredicateColumnsOptAlt02 => parser_ast::ColumnChoice::All,
                _ => parser_ast::ColumnChoice::Predicate,
            }))
        }
        AdminRule::AnalyzeOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::AnalyzeOpt>::new()))
        }
        AdminRule::AnalyzeOptionListOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::AnalyzeOptionListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AnalyzeOpt>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        AdminRule::AnalyzeOptionListAlt02 | AdminRule::AnalyzeOptionListAlt03 => {
            let prior_back = if rule == AdminRule::AnalyzeOptionListAlt02 {
                2
            } else {
                1
            };
            let mut values = rhs[rhs_len - (prior_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AnalyzeOpt>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::AnalyzeOptionAlt01
        | AdminRule::AnalyzeOptionAlt02
        | AdminRule::AnalyzeOptionAlt03
        | AdminRule::AnalyzeOptionAlt04
        | AdminRule::AnalyzeOptionAlt05
        | AdminRule::AnalyzeOptionAlt06
        | AdminRule::AnalyzeOptionAlt07 => {
            let value_back = if matches!(
                rule,
                AdminRule::AnalyzeOptionAlt03 | AdminRule::AnalyzeOptionAlt04
            ) {
                2
            } else {
                1
            };
            let text = rhs[rhs_len - (value_back)]
                .item
                .as_deref()
                .map(semantic_value_text)
                .unwrap_or_default();
            let kind = match rule {
                AdminRule::AnalyzeOptionAlt01 => parser_ast::AnalyzeOptionType::NumBuckets,
                AdminRule::AnalyzeOptionAlt02 => parser_ast::AnalyzeOptionType::NumTopN,
                AdminRule::AnalyzeOptionAlt03 => parser_ast::AnalyzeOptionType::CMSketchDepth,
                AdminRule::AnalyzeOptionAlt04 => parser_ast::AnalyzeOptionType::CMSketchWidth,
                AdminRule::AnalyzeOptionAlt05 => parser_ast::AnalyzeOptionType::NumSamples,
                AdminRule::AnalyzeOptionAlt06 => parser_ast::AnalyzeOptionType::SampleRate,
                _ => parser_ast::AnalyzeOptionType::NDVRate,
            };
            out.item = Some(Box::new(parser_ast::AnalyzeOpt {
                Type: kind,
                Value: Some(parser_ast::ExprNode::Value(text)),
            }));
        }
        AdminRule::AnalyzeOptionAlt08
        | AdminRule::AnalyzeOptionAlt09
        | AdminRule::AnalyzeOptionAlt10
        | AdminRule::AnalyzeOptionAlt11 => {
            out.item = Some(Box::new(parser_ast::AnalyzeOpt {
                Type: match rule {
                    AdminRule::AnalyzeOptionAlt08 => parser_ast::AnalyzeOptionType::NumBuckets,
                    AdminRule::AnalyzeOptionAlt09 => parser_ast::AnalyzeOptionType::NumTopN,
                    AdminRule::AnalyzeOptionAlt10 => parser_ast::AnalyzeOptionType::NumSamples,
                    _ => parser_ast::AnalyzeOptionType::SampleRate,
                },
                Value: None,
            }));
        }
        AdminRule::BinlogStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::BinlogStmt {
                node_text: Default::default(),
                Str: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        AdminRule::IdentListWithParenOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::CIStr>::new()))
        }
        AdminRule::IdentListWithParenOptAlt02 => {
            let values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(values));
        }
        AdminRule::IdentListAlt01 => {
            out.item = Some(Box::new(vec![parser_ast::NewCIStr(
                &rhs[rhs_len - (0)].ident,
            )]))
        }
        AdminRule::IdentListAlt02 => {
            let mut names = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            names.push(parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident));
            out.item = Some(Box::new(names));
        }
        AdminRule::NotSymAlt02 => out.ident = "NOT".to_owned(),
        AdminRule::StatsTypeAlt01 => out.item = Some(Box::new(0u8)),
        AdminRule::StatsTypeAlt02 => out.item = Some(Box::new(1u8)),
        AdminRule::StatsTypeAlt03 => out.item = Some(Box::new(2u8)),
        AdminRule::BindingStatusTypeAlt01 => out.item = Some(Box::new(0u8)),
        AdminRule::BindingStatusTypeAlt02 => out.item = Some(Box::new(1u8)),
        AdminRule::CreateStatisticsStmtAlt01 => {
            let Some(table) = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let columns = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::CreateStatisticsStmt {
                node_text: Default::default(),
                IfNotExists: rhs[rhs_len - (9)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                StatsName: rhs[rhs_len - (8)].ident.clone(),
                StatsType: rhs[rhs_len - (6)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<u8>())
                    .copied()
                    .unwrap_or_default(),
                Table: table,
                Columns: columns,
            }));
        }
        AdminRule::DropStatisticsStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropStatisticsStmt {
                node_text: Default::default(),
                StatsName: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        AdminRule::BooleanAlt01 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(value != 0));
        }
        AdminRule::BooleanAlt02 => out.item = Some(Box::new(false)),
        AdminRule::BooleanAlt03 => out.item = Some(Box::new(true)),
        AdminRule::DropStatsStmtAlt01 => {
            let tables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::DropStatsStmt {
                Tables: tables,
                ..Default::default()
            }));
        }
        AdminRule::DropStatsStmtAlt02 | AdminRule::DropStatsStmtAlt03 => {
            let table_back = if rule == AdminRule::DropStatsStmtAlt02 {
                2
            } else {
                1
            };
            let Some(table) = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::DropStatsStmt {
                node_text: Default::default(),
                Tables: vec![table],
                PartitionNames: if rule == AdminRule::DropStatsStmtAlt02 {
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                IsGlobalStats: rule == AdminRule::DropStatsStmtAlt03,
            }));
        }
        AdminRule::BRIEStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len - (3)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::BRIEStmt>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            statement.Kind = parser_ast::BRIEKind::Backup;
            statement.Storage = rhs[rhs_len - (1)].ident.clone();
            statement.Options = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::BRIEOption>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::BRIEStmtAlt02
        | AdminRule::BRIEStmtAlt03
        | AdminRule::BRIEStmtAlt04
        | AdminRule::BRIEStmtAlt05
        | AdminRule::BRIEStmtAlt06
        | AdminRule::BRIEStmtAlt07
        | AdminRule::BRIEStmtAlt08
        | AdminRule::BRIEStmtAlt09
        | AdminRule::BRIEStmtAlt10
        | AdminRule::BRIEStmtAlt11
        | AdminRule::BRIEStmtAlt12
        | AdminRule::BRIEStmtAlt13
        | AdminRule::BRIEStmtAlt14 => {
            let mut statement = if matches!(rule, AdminRule::BRIEStmtAlt13) {
                let Some(statement) = rhs[rhs_len - (3)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<parser_ast::BRIEStmt>().ok())
                    .map(|item| *item)
                else {
                    return Ok(false);
                };
                statement
            } else {
                parser_ast::BRIEStmt::default()
            };
            statement.Kind = match rule {
                AdminRule::BRIEStmtAlt02 => parser_ast::BRIEKind::StreamStart,
                AdminRule::BRIEStmtAlt03 => parser_ast::BRIEKind::StreamStop,
                AdminRule::BRIEStmtAlt04 => parser_ast::BRIEKind::StreamPause,
                AdminRule::BRIEStmtAlt05 => parser_ast::BRIEKind::StreamResume,
                AdminRule::BRIEStmtAlt06 => parser_ast::BRIEKind::StreamPurge,
                AdminRule::BRIEStmtAlt07 => parser_ast::BRIEKind::StreamStatus,
                AdminRule::BRIEStmtAlt08 => parser_ast::BRIEKind::StreamMetaData,
                AdminRule::BRIEStmtAlt09 => parser_ast::BRIEKind::ShowJob,
                AdminRule::BRIEStmtAlt10 => parser_ast::BRIEKind::ShowQuery,
                AdminRule::BRIEStmtAlt11 => parser_ast::BRIEKind::CancelJob,
                AdminRule::BRIEStmtAlt12 => parser_ast::BRIEKind::ShowBackupMeta,
                AdminRule::BRIEStmtAlt13 => parser_ast::BRIEKind::Restore,
                _ => parser_ast::BRIEKind::RestorePIT,
            };
            let storage_back = match rule {
                AdminRule::BRIEStmtAlt02
                | AdminRule::BRIEStmtAlt06
                | AdminRule::BRIEStmtAlt13
                | AdminRule::BRIEStmtAlt14 => Some(1),
                AdminRule::BRIEStmtAlt08 | AdminRule::BRIEStmtAlt12 => Some(0),
                _ => None,
            };
            if let Some(back) = storage_back {
                statement.Storage = rhs[rhs_len - (back)].ident.clone();
            }
            if matches!(
                rule,
                AdminRule::BRIEStmtAlt02
                    | AdminRule::BRIEStmtAlt04
                    | AdminRule::BRIEStmtAlt06
                    | AdminRule::BRIEStmtAlt13
                    | AdminRule::BRIEStmtAlt14
            ) {
                statement.Options = rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::BRIEOption>>())
                    .cloned()
                    .unwrap_or_default();
            }
            if matches!(
                rule,
                AdminRule::BRIEStmtAlt09 | AdminRule::BRIEStmtAlt10 | AdminRule::BRIEStmtAlt11
            ) {
                statement.JobID = rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|value| {
                        value
                            .downcast_ref::<i64>()
                            .copied()
                            .or_else(|| value.downcast_ref::<u64>().map(|value| *value as i64))
                    })
                    .unwrap_or_default();
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::BRIETablesAlt01 => out.item = Some(Box::new(parser_ast::BRIEStmt::default())),
        AdminRule::BRIETablesAlt02 => {
            out.item = Some(Box::new(parser_ast::BRIEStmt {
                Schemas: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<String>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::BRIETablesAlt03 => {
            out.item = Some(Box::new(parser_ast::BRIEStmt {
                Tables: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::DBNameListAlt01 => {
            out.item = Some(Box::new(vec![rhs[rhs_len - (0)].ident.clone()]))
        }
        AdminRule::DBNameListAlt02 => {
            let mut schemas = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<String>>())
                .cloned()
                .unwrap_or_default();
            schemas.push(rhs[rhs_len - (0)].ident.clone());
            out.item = Some(Box::new(schemas));
        }
        AdminRule::BRIEOptionsAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::BRIEOption>::new()))
        }
        AdminRule::BRIEOptionsAlt02 => {
            let mut options = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::BRIEOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(option) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::BRIEOption>())
            {
                options.push(option.clone());
            }
            out.item = Some(Box::new(options));
        }
        AdminRule::BRIEIntegerOptionNameAlt01
        | AdminRule::BRIEIntegerOptionNameAlt02
        | AdminRule::BRIEIntegerOptionNameAlt03
        | AdminRule::BRIEIntegerOptionNameAlt04
        | AdminRule::BRIEBooleanOptionNameAlt01
        | AdminRule::BRIEBooleanOptionNameAlt02
        | AdminRule::BRIEBooleanOptionNameAlt03
        | AdminRule::BRIEBooleanOptionNameAlt04
        | AdminRule::BRIEBooleanOptionNameAlt05
        | AdminRule::BRIEBooleanOptionNameAlt06
        | AdminRule::BRIEBooleanOptionNameAlt07
        | AdminRule::BRIEBooleanOptionNameAlt08
        | AdminRule::BRIEBooleanOptionNameAlt09
        | AdminRule::BRIEBooleanOptionNameAlt10
        | AdminRule::BRIEBooleanOptionNameAlt11
        | AdminRule::BRIEBooleanOptionNameAlt12
        | AdminRule::BRIEStringOptionNameAlt01
        | AdminRule::BRIEStringOptionNameAlt02
        | AdminRule::BRIEStringOptionNameAlt03
        | AdminRule::BRIEStringOptionNameAlt04
        | AdminRule::BRIEStringOptionNameAlt05
        | AdminRule::BRIEStringOptionNameAlt06
        | AdminRule::BRIEStringOptionNameAlt07
        | AdminRule::BRIEKeywordOptionNameAlt01
        | AdminRule::BRIEKeywordOptionNameAlt02
        | AdminRule::BRIEKeywordOptionNameAlt03 => {
            let option_type = match rule {
                AdminRule::BRIEIntegerOptionNameAlt01 => parser_ast::BRIEOptionConcurrency,
                AdminRule::BRIEIntegerOptionNameAlt02 => parser_ast::BRIEOptionResume,
                AdminRule::BRIEIntegerOptionNameAlt03 => parser_ast::BRIEOptionChecksumConcurrency,
                AdminRule::BRIEIntegerOptionNameAlt04 => parser_ast::BRIEOptionCompressionLevel,
                AdminRule::BRIEBooleanOptionNameAlt01 => parser_ast::BRIEOptionSendCreds,
                AdminRule::BRIEBooleanOptionNameAlt02 => parser_ast::BRIEOptionOnline,
                AdminRule::BRIEBooleanOptionNameAlt03 => parser_ast::BRIEOptionCheckpoint,
                AdminRule::BRIEBooleanOptionNameAlt04 => parser_ast::BRIEOptionSkipSchemaFiles,
                AdminRule::BRIEBooleanOptionNameAlt05 => parser_ast::BRIEOptionStrictFormat,
                AdminRule::BRIEBooleanOptionNameAlt06 => parser_ast::BRIEOptionCSVNotNull,
                AdminRule::BRIEBooleanOptionNameAlt07 => parser_ast::BRIEOptionCSVBackslashEscape,
                AdminRule::BRIEBooleanOptionNameAlt08 => {
                    parser_ast::BRIEOptionCSVTrimLastSeparators
                }
                AdminRule::BRIEBooleanOptionNameAlt09 => parser_ast::BRIEOptionWaitTiflashReady,
                AdminRule::BRIEBooleanOptionNameAlt10 => parser_ast::BRIEOptionWithSysTable,
                AdminRule::BRIEBooleanOptionNameAlt11 => parser_ast::BRIEOptionIgnoreStats,
                AdminRule::BRIEBooleanOptionNameAlt12 => parser_ast::BRIEOptionLoadStats,
                AdminRule::BRIEStringOptionNameAlt01 => parser_ast::BRIEOptionTiKVImporter,
                AdminRule::BRIEStringOptionNameAlt02 => parser_ast::BRIEOptionCSVSeparator,
                AdminRule::BRIEStringOptionNameAlt03 => parser_ast::BRIEOptionCSVDelimiter,
                AdminRule::BRIEStringOptionNameAlt04 => parser_ast::BRIEOptionCSVNull,
                AdminRule::BRIEStringOptionNameAlt05 => parser_ast::BRIEOptionCompression,
                AdminRule::BRIEStringOptionNameAlt06 => parser_ast::BRIEOptionEncryptionMethod,
                AdminRule::BRIEStringOptionNameAlt07 => parser_ast::BRIEOptionEncryptionKeyFile,
                AdminRule::BRIEKeywordOptionNameAlt01 => parser_ast::BRIEOptionBackend,
                AdminRule::BRIEKeywordOptionNameAlt02 | AdminRule::BRIEKeywordOptionNameAlt03 => {
                    parser_ast::BRIEOptionOnDuplicate
                }
                _ => unreachable!(),
            };
            out.item = Some(Box::new(option_type));
        }
        AdminRule::BRIEOptionAlt01 => {
            let option_type = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u16>())
                .copied()
                .unwrap_or_default();
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: option_type,
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt02 => {
            let option_type = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u16>())
                .copied()
                .unwrap_or_default();
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false) as u64;
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: option_type,
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt03 | AdminRule::BRIEOptionAlt04 => {
            let option_type = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u16>())
                .copied()
                .unwrap_or_default();
            let mut value = rhs[rhs_len - (0)].ident.clone();
            if rule == AdminRule::BRIEOptionAlt04 {
                value.make_ascii_lowercase();
            }
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: option_type,
                StrValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt05 => {
            let unit = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                .copied()
                .unwrap_or_default();
            let Some(duration) = unit.duration_nanos() else {
                yylex.AppendError(yylex.Errorf("time unit is not a fixed time interval", &[]));
                return Err(1);
            };
            let value = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: parser_ast::BRIEOptionBackupTimeAgo,
                UintValue: value.wrapping_mul(duration),
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt06
        | AdminRule::BRIEOptionAlt08
        | AdminRule::BRIEOptionAlt17
        | AdminRule::BRIEOptionAlt18
        | AdminRule::BRIEOptionAlt19
        | AdminRule::BRIEOptionAlt20
        | AdminRule::BRIEOptionAlt21 => {
            let option_type = match rule {
                AdminRule::BRIEOptionAlt06 => parser_ast::BRIEOptionBackupTS,
                AdminRule::BRIEOptionAlt08 => parser_ast::BRIEOptionLastBackupTS,
                AdminRule::BRIEOptionAlt17 => parser_ast::BRIEOptionFullBackupStorage,
                AdminRule::BRIEOptionAlt18 => parser_ast::BRIEOptionRestoredTS,
                AdminRule::BRIEOptionAlt19 => parser_ast::BRIEOptionStartTS,
                AdminRule::BRIEOptionAlt20 => parser_ast::BRIEOptionUntilTS,
                _ => parser_ast::BRIEOptionGCTTL,
            };
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: option_type,
                StrValue: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt07 | AdminRule::BRIEOptionAlt09 => {
            let option_type = if rule == AdminRule::BRIEOptionAlt07 {
                parser_ast::BRIEOptionBackupTSO
            } else {
                parser_ast::BRIEOptionLastBackupTSO
            };
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: option_type,
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt10 => {
            let value = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default()
                .saturating_mul(1_048_576);
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: parser_ast::BRIEOptionRateLimit,
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt11 => {
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: parser_ast::BRIEOptionCSVHeader,
                UintValue: parser_ast::BRIECSVHeaderIsColumns,
                ..Default::default()
            }))
        }
        AdminRule::BRIEOptionAlt12 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: parser_ast::BRIEOptionCSVHeader,
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt13 | AdminRule::BRIEOptionAlt15 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false) as u64;
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: if rule == AdminRule::BRIEOptionAlt13 {
                    parser_ast::BRIEOptionChecksum
                } else {
                    parser_ast::BRIEOptionAnalyze
                },
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::BRIEOptionAlt14 | AdminRule::BRIEOptionAlt16 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::BRIEOption {
                Tp: if rule == AdminRule::BRIEOptionAlt14 {
                    parser_ast::BRIEOptionChecksum
                } else {
                    parser_ast::BRIEOptionAnalyze
                },
                UintValue: value,
                ..Default::default()
            }));
        }
        AdminRule::RecommendIndexStmtAlt03 => {
            out.statement = Some(Box::new(parser_ast::RecommendIndexStmt {
                Action: "show".to_owned(),
                ..Default::default()
            }))
        }
        AdminRule::AlterInstanceStmtAlt01 => {
            out.statement = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::AlterInstanceStmt>().ok())
                .map(|item| item as Box<dyn parser_ast::Node>)
        }
        AdminRule::InstanceOptionAlt01 | AdminRule::InstanceOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::AlterInstanceStmt {
                node_text: Default::default(),
                ReloadTLS: true,
                NoRollbackOnError: rule == AdminRule::InstanceOptionAlt02,
            }))
        }
        AdminRule::RecommendIndexStmtAlt04 | AdminRule::RecommendIndexStmtAlt05 => {
            out.statement = Some(Box::new(parser_ast::RecommendIndexStmt {
                Action: if rule == AdminRule::RecommendIndexStmtAlt04 {
                    "apply"
                } else {
                    "ignore"
                }
                .to_owned(),
                ID: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i64>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::RecommendIndexStmtAlt01
        | AdminRule::RecommendIndexStmtAlt02
        | AdminRule::RecommendIndexStmtAlt06 => {
            let options = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::RecommendIndexOption>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::RecommendIndexStmt {
                Action: if rule == AdminRule::RecommendIndexStmtAlt06 {
                    "set"
                } else {
                    "run"
                }
                .to_owned(),
                SQL: if rule == AdminRule::RecommendIndexStmtAlt01 {
                    rhs[rhs_len - (1)].ident.clone()
                } else {
                    String::new()
                },
                Options: options,
                ..Default::default()
            }));
        }
        AdminRule::RecommendIndexOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::RecommendIndexOption>::new()))
        }
        AdminRule::RecommendIndexOptionListOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::RecommendIndexOptionListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::RecommendIndexOption>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        AdminRule::RecommendIndexOptionListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::RecommendIndexOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::RecommendIndexOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::RecommendIndexOptionAlt01 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::RecommendIndexOption {
                Option: rhs[rhs_len - (2)].ident.clone(),
                Value: value,
            }));
        }
        AdminRule::CalibrateResourceStmtAlt01 => {
            out.statement = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::CalibrateResourceStmt>().ok())
                .map(|item| item as Box<dyn parser_ast::Node>)
        }
        AdminRule::CalibrateOptionAlt01 => {
            out.item = Some(Box::new(parser_ast::CalibrateResourceStmt::default()))
        }
        AdminRule::CalibrateOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::CalibrateResourceStmt {
                DynamicCalibrateResourceOptionList: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| {
                        item.downcast_ref::<Vec<parser_ast::DynamicCalibrateResourceOption>>()
                    })
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::CalibrateOptionAlt03 => {
            out.item = Some(Box::new(parser_ast::CalibrateResourceStmt {
                Tp: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::CalibrateResourceType>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::DynamicCalibrateOptionListAlt01
        | AdminRule::DynamicCalibrateOptionListAlt02
        | AdminRule::DynamicCalibrateOptionListAlt03 => {
            let list_back = if rule == AdminRule::DynamicCalibrateOptionListAlt01 {
                None
            } else if rule == AdminRule::DynamicCalibrateOptionListAlt02 {
                Some(1)
            } else {
                Some(2)
            };
            let mut values = list_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::DynamicCalibrateResourceOption>>()
                        })
                        .cloned()
                })
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::DynamicCalibrateResourceOption>())
            {
                if values.iter().any(|old| old.Tp == value.Tp) {
                    yylex.AppendError(yylex.Errorf("Dupliated options specified", &[]));
                    return Err(1);
                }
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::DynamicCalibrateResourceOptionAlt01
        | AdminRule::DynamicCalibrateResourceOptionAlt02
        | AdminRule::DynamicCalibrateResourceOptionAlt03
        | AdminRule::DynamicCalibrateResourceOptionAlt04 => {
            let tp = match rule {
                AdminRule::DynamicCalibrateResourceOptionAlt01 => {
                    parser_ast::DynamicCalibrateResourceOptionType::StartTime
                }
                AdminRule::DynamicCalibrateResourceOptionAlt02 => {
                    parser_ast::DynamicCalibrateResourceOptionType::EndTime
                }
                _ => parser_ast::DynamicCalibrateResourceOptionType::Duration,
            };
            let mut option = parser_ast::DynamicCalibrateResourceOption {
                Tp: tp,
                ..Default::default()
            };
            match rule {
                AdminRule::DynamicCalibrateResourceOptionAlt01
                | AdminRule::DynamicCalibrateResourceOptionAlt02 => {
                    option.Ts = rhs[rhs_len - (0)].expr.clone()
                }
                AdminRule::DynamicCalibrateResourceOptionAlt03 => {
                    option.StrValue = rhs[rhs_len - (0)].ident.clone();
                    if let Err(error) = parser_duration::ParseDuration(&option.StrValue) {
                        yylex.AppendError(yylex.Errorf(
                            &format!("The DURATION option is not a valid duration: {error}"),
                            &[],
                        ));
                        return Err(1);
                    }
                }
                _ => {
                    option.Ts = rhs[rhs_len - (1)].expr.clone();
                    option.Unit = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                        .copied()
                        .unwrap_or_default();
                }
            }
            out.item = Some(Box::new(option));
        }
        AdminRule::CalibrateResourceWorkloadOptionAlt01
        | AdminRule::CalibrateResourceWorkloadOptionAlt02
        | AdminRule::CalibrateResourceWorkloadOptionAlt03
        | AdminRule::CalibrateResourceWorkloadOptionAlt04
        | AdminRule::CalibrateResourceWorkloadOptionAlt05 => {
            out.item = Some(Box::new(match rule {
                AdminRule::CalibrateResourceWorkloadOptionAlt01 => {
                    parser_ast::CalibrateResourceType::TPCC
                }
                AdminRule::CalibrateResourceWorkloadOptionAlt02 => {
                    parser_ast::CalibrateResourceType::OLTPReadWrite
                }
                AdminRule::CalibrateResourceWorkloadOptionAlt03 => {
                    parser_ast::CalibrateResourceType::OLTPReadOnly
                }
                AdminRule::CalibrateResourceWorkloadOptionAlt04 => {
                    parser_ast::CalibrateResourceType::OLTPWriteOnly
                }
                _ => parser_ast::CalibrateResourceType::TPCH10,
            }))
        }
        AdminRule::AddQueryWatchStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::AddQueryWatchStmt {
                node_text: Default::default(),
                QueryWatchOptionList: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::QueryWatchOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        AdminRule::QueryWatchOptionListAlt01
        | AdminRule::QueryWatchOptionListAlt02
        | AdminRule::QueryWatchOptionListAlt03 => {
            let list_back = if rule == AdminRule::QueryWatchOptionListAlt01 {
                None
            } else if rule == AdminRule::QueryWatchOptionListAlt02 {
                Some(1)
            } else {
                Some(2)
            };
            let mut values = list_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::QueryWatchOption>>())
                        .cloned()
                })
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::QueryWatchOption>())
            {
                if values.iter().any(|old| old.Tp == value.Tp) {
                    yylex.AppendError(yylex.Errorf("Dupliated options specified", &[]));
                    return Err(1);
                }
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::QueryWatchOptionAlt01
        | AdminRule::QueryWatchOptionAlt02
        | AdminRule::QueryWatchOptionAlt03
        | AdminRule::QueryWatchOptionAlt04 => {
            let mut option = parser_ast::QueryWatchOption::default();
            match rule {
                AdminRule::QueryWatchOptionAlt01 => {
                    option.Tp = parser_ast::QueryWatchOptionType::ResourceGroup;
                    option.ResourceGroupOption = Some(parser_ast::QueryWatchResourceGroupOption {
                        GroupNameStr: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                        GroupNameExpr: None,
                    });
                }
                AdminRule::QueryWatchOptionAlt02 => {
                    option.Tp = parser_ast::QueryWatchOptionType::ResourceGroup;
                    option.ResourceGroupOption = Some(parser_ast::QueryWatchResourceGroupOption {
                        GroupNameExpr: rhs[rhs_len - (0)].expr.clone(),
                        ..Default::default()
                    });
                }
                AdminRule::QueryWatchOptionAlt03 => {
                    option.Tp = parser_ast::QueryWatchOptionType::Action;
                    option.ActionOption = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<parser_ast::ResourceGroupRunawayActionOption>()
                        })
                        .cloned();
                }
                _ => {
                    option.Tp = parser_ast::QueryWatchOptionType::Type;
                    option.TextOption = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::QueryWatchTextOption>())
                        .cloned();
                }
            }
            out.item = Some(Box::new(option));
        }
        AdminRule::QueryWatchTextOptionAlt01
        | AdminRule::QueryWatchTextOptionAlt02
        | AdminRule::QueryWatchTextOptionAlt03 => {
            let Some(pattern) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let watch_type = match rule {
                AdminRule::QueryWatchTextOptionAlt01 => parser_ast::RunawayWatchType::Similar,
                AdminRule::QueryWatchTextOptionAlt02 => parser_ast::RunawayWatchType::Plan,
                _ => rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::RunawayWatchType>())
                    .copied()
                    .unwrap_or_default(),
            };
            out.item = Some(Box::new(parser_ast::QueryWatchTextOption {
                Type: watch_type,
                PatternExpr: pattern,
                TypeSpecified: rule == AdminRule::QueryWatchTextOptionAlt03,
            }));
        }
        AdminRule::ShutdownStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ShutdownStmt::default()))
        }
        AdminRule::RestartStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::RestartStmt::default()))
        }
        AdminRule::SetStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::SetPwdStmt {
                Password: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }));
        }
        AdminRule::SetStmtAlt03 | AdminRule::SetStmtAlt04 | AdminRule::SetStmtAlt05 => {
            let (user_back, password_back, retain) = match rule {
                AdminRule::SetStmtAlt03 => (None, 3, true),
                AdminRule::SetStmtAlt04 => (Some(2), 0, false),
                _ => (Some(5), 3, true),
            };
            let user = user_back.and_then(|back| {
                rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| {
                        item.downcast_ref::<parser_auth::parser::auth::auth::UserIdentity>()
                    })
                    .cloned()
            });
            out.statement = Some(Box::new(parser_ast::SetPwdStmt {
                node_text: Default::default(),
                User: user,
                Password: rhs[rhs_len - (password_back)].ident.clone(),
                RetainCurrentPassword: retain,
            }));
        }
        AdminRule::SetStmtAlt01 | AdminRule::SetStmtAlt07 => {
            let variables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::SetStmt {
                node_text: Default::default(),
                Variables: variables,
            }));
        }
        AdminRule::SetStmtAlt06 => {
            let mut variables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
                .cloned()
                .unwrap_or_default();
            for variable in &mut variables {
                variable.IsGlobal = true;
            }
            out.statement = Some(Box::new(parser_ast::SetStmt {
                node_text: Default::default(),
                Variables: variables,
            }));
        }
        AdminRule::SetStmtAlt08 => {
            let mut variables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
                .cloned()
                .unwrap_or_default();
            for variable in &mut variables {
                if variable.Name == "tx_isolation" {
                    variable.Name = "tx_isolation_one_shot".to_owned();
                }
            }
            out.statement = Some(Box::new(parser_ast::SetStmt {
                node_text: Default::default(),
                Variables: variables,
            }));
        }
        AdminRule::SetStmtAlt11 => {
            out.statement = Some(Box::new(parser_ast::SetSessionStatesStmt {
                node_text: Default::default(),
                SessionStates: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        AdminRule::SetStmtAlt09 | AdminRule::SetStmtAlt10 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::SetConfigStmt {
                node_text: Default::default(),
                Type: if rule == AdminRule::SetStmtAlt09 {
                    rhs[rhs_len - (3)].ident.to_ascii_lowercase()
                } else {
                    String::new()
                },
                Instance: if rule == AdminRule::SetStmtAlt10 {
                    rhs[rhs_len - (3)].ident.clone()
                } else {
                    String::new()
                },
                Name: rhs[rhs_len - (2)].ident.clone(),
                Value: value,
            }));
        }
        AdminRule::SetStmtAlt12 => {
            out.statement = Some(Box::new(parser_ast::SetResourceGroupStmt {
                node_text: Default::default(),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
            }))
        }
        AdminRule::AdminStmtLimitOptAlt01
        | AdminRule::AdminStmtLimitOptAlt02
        | AdminRule::AdminStmtLimitOptAlt03 => {
            let number = |back: usize| {
                rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64
            };
            out.item = Some(Box::new(match rule {
                AdminRule::AdminStmtLimitOptAlt01 => parser_ast::LimitSimple {
                    Offset: 0,
                    Count: number(0),
                },
                AdminRule::AdminStmtLimitOptAlt02 => parser_ast::LimitSimple {
                    Offset: number(2),
                    Count: number(0),
                },
                _ => parser_ast::LimitSimple {
                    Offset: number(0),
                    Count: number(2),
                },
            }));
        }
        AdminRule::AdminStmtAlt03 => {
            let mut statement = parser_ast::AdminStmt::new(parser_ast::AdminStmtType::ShowDdlJobs);
            statement.job_number = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(semantic_numeric_isize)
                .unwrap_or_default() as i64;
            statement.where_expr = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt10 => {
            let mut statement =
                parser_ast::AdminStmt::new(parser_ast::AdminStmtType::CheckIndexRange);
            if let Some(table) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
            {
                statement.tables.push(table.clone());
            }
            statement.index = rhs[rhs_len - (1)].ident.clone();
            statement.handle_ranges = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::HandleRange>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt16 => {
            let mut statement =
                parser_ast::AdminStmt::new(parser_ast::AdminStmtType::ShowDdlJobQueriesWithRange);
            statement.limit_simple = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::LimitSimple>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt17 => {
            let mut statement = parser_ast::AdminStmt::new(parser_ast::AdminStmtType::ShowSlow);
            statement.show_slow = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ShowSlow>())
                .cloned();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt20 | AdminRule::AdminStmtAlt21 => {
            let mut statement = parser_ast::AdminStmt::new(if rule == AdminRule::AdminStmtAlt20 {
                parser_ast::AdminStmtType::PluginEnable
            } else {
                parser_ast::AdminStmtType::PluginDisable
            });
            statement.plugins = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<String>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt22 => {
            out.statement = Some(Box::new(parser_ast::CleanupTableLockStmt {
                node_text: Default::default(),
                Tables: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        AdminRule::AdminStmtAlt23 => {
            let Some(table) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let Some(create) = rhs[rhs_len - (0)].statement.take().and_then(|node| {
                node.into_any()
                    .downcast::<parser_ast::CreateTableStmt>()
                    .ok()
            }) else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::RepairTableStmt {
                node_text: Default::default(),
                Table: table,
                CreateStmt: create,
            }));
        }
        AdminRule::AdminStmtAlt31 => {
            let mut statement =
                parser_ast::AdminStmt::new(parser_ast::AdminStmtType::FlushPlanCache);
            statement.statement_scope = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::StatementScope>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt32 => {
            let mut statement = parser_ast::AdminStmt::new(parser_ast::AdminStmtType::SetBdrRole);
            statement.bdr_role = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::BDRRole>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt35 => {
            let mut statement = parser_ast::AdminStmt::new(parser_ast::AdminStmtType::AlterDdlJob);
            statement.job_number = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(semantic_numeric_isize)
                .unwrap_or_default() as i64;
            statement.alter_job_options = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AlterJobOption>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AlterJobOptionListAlt01 | AdminRule::AlterJobOptionListAlt02 => {
            let mut values = if rule == AdminRule::AlterJobOptionListAlt02 {
                rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::AlterJobOption>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AlterJobOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::AlterJobOptionAlt01 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::AlterJobOption {
                Name: rhs[rhs_len - (2)].ident.to_ascii_lowercase(),
                Value: value,
            }));
        }
        AdminRule::AdminShowSlowAlt01
        | AdminRule::AdminShowSlowAlt02
        | AdminRule::AdminShowSlowAlt03
        | AdminRule::AdminShowSlowAlt04 => {
            out.item = Some(Box::new(parser_ast::ShowSlow {
                Tp: if rule == AdminRule::AdminShowSlowAlt01 {
                    parser_ast::ShowSlowType::Recent
                } else {
                    parser_ast::ShowSlowType::Top
                },
                Kind: match rule {
                    AdminRule::AdminShowSlowAlt03 => parser_ast::ShowSlowKind::Internal,
                    AdminRule::AdminShowSlowAlt04 => parser_ast::ShowSlowKind::All,
                    _ => parser_ast::ShowSlowKind::Default,
                },
                Count: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64,
            }))
        }
        AdminRule::HandleRangeListAlt01 | AdminRule::HandleRangeListAlt02 => {
            let mut values = if rule == AdminRule::HandleRangeListAlt02 {
                rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::HandleRange>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::HandleRange>())
            {
                values.push(*value);
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::HandleRangeAlt01 => {
            out.item = Some(Box::new(parser_ast::HandleRange {
                Begin: rhs[rhs_len - (3)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i64,
                End: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i64,
            }))
        }
        AdminRule::ShowStmtAlt08 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::CreateUser,
                User: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| {
                        item.downcast_ref::<parser_auth::parser::auth::auth::UserIdentity>()
                    })
                    .cloned(),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt09 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::MaskingPolicies,
                Table: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                Where: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt10 | AdminRule::ShowStmtAlt12 => {
            let table_back = if rule == AdminRule::ShowStmtAlt10 {
                3
            } else {
                5
            };
            let mut table = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            table.PartitionNames = rhs[rhs_len
                - (if rule == AdminRule::ShowStmtAlt10 {
                    2
                } else {
                    4
                })]
            .item
            .as_deref()
            .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
            .cloned()
            .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::Regions,
                Table: Some(table),
                IndexName: if rule == AdminRule::ShowStmtAlt12 {
                    parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident)
                } else {
                    parser_ast::CIStr::default()
                },
                Where: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                ..Default::default()
            }));
        }
        AdminRule::ShowStmtAlt11 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::TableNextRowId,
                Table: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt14 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::Grants,
                User: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| {
                        item.downcast_ref::<parser_auth::parser::auth::auth::UserIdentity>()
                    })
                    .cloned(),
                Roles: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| {
                        item.downcast_ref::<Vec<parser_auth::parser::auth::auth::RoleIdentity>>()
                    })
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt20 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::Profile,
                ShowProfileTypes: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ProfileType>>())
                    .cloned()
                    .unwrap_or_default(),
                ShowProfileArgs: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i64>())
                    .copied(),
                ShowProfileLimit: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                    .cloned(),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt23 => {
            out.statement = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::ShowStmt>().ok())
                .map(|item| item as Box<dyn parser_ast::Node>)
        }
        AdminRule::ShowStmtAlt24 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::ImportJobs,
                ImportJobRaw: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ImportJobID: Some(
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default() as i64,
                ),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt25 => {
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::DistributionJobs,
                DistributionJobID: Some(
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default() as i64,
                ),
                ..Default::default()
            }))
        }
        AdminRule::ShowStmtAlt27 => {
            let mut table = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            table.PartitionNames = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::Distributions,
                Table: Some(table),
                Where: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                ..Default::default()
            }));
        }
        AdminRule::ShowPlacementTargetAlt01 => {
            out.item = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::PlacementForDatabase,
                DBName: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }))
        }
        AdminRule::ShowPlacementTargetAlt02 => {
            out.item = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::PlacementForTable,
                Table: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                ..Default::default()
            }))
        }
        AdminRule::ShowPlacementTargetAlt03 => {
            out.item = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::PlacementForPartition,
                Table: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                Partition: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }))
        }
        AdminRule::ShowProfileTypesOptAlt01 => out.item = None,
        AdminRule::ShowProfileTypesAlt01 | AdminRule::ShowProfileTypesAlt02 => {
            let mut values = if rule == AdminRule::ShowProfileTypesAlt02 {
                rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ProfileType>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ProfileType>())
            {
                values.push(*value);
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::ShowProfileTypeAlt01
        | AdminRule::ShowProfileTypeAlt02
        | AdminRule::ShowProfileTypeAlt03
        | AdminRule::ShowProfileTypeAlt04
        | AdminRule::ShowProfileTypeAlt05
        | AdminRule::ShowProfileTypeAlt06
        | AdminRule::ShowProfileTypeAlt07
        | AdminRule::ShowProfileTypeAlt08
        | AdminRule::ShowProfileTypeAlt09 => {
            out.item = Some(Box::new(match rule {
                AdminRule::ShowProfileTypeAlt01 => parser_ast::ProfileType::Cpu,
                AdminRule::ShowProfileTypeAlt02 => parser_ast::ProfileType::Memory,
                AdminRule::ShowProfileTypeAlt03 => parser_ast::ProfileType::BlockIo,
                AdminRule::ShowProfileTypeAlt04 => parser_ast::ProfileType::ContextSwitch,
                AdminRule::ShowProfileTypeAlt05 => parser_ast::ProfileType::PageFaults,
                AdminRule::ShowProfileTypeAlt06 => parser_ast::ProfileType::Ipc,
                AdminRule::ShowProfileTypeAlt07 => parser_ast::ProfileType::Swaps,
                AdminRule::ShowProfileTypeAlt08 => parser_ast::ProfileType::Source,
                _ => parser_ast::ProfileType::All,
            }))
        }
        AdminRule::ShowProfileArgsOptAlt01 | AdminRule::UsingRolesAlt01 => out.item = None,
        AdminRule::ShowProfileArgsOptAlt02 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i64,
            ))
        }
        AdminRule::UsingRolesAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::StatementScopeAlt01
        | AdminRule::StatementScopeAlt02
        | AdminRule::StatementScopeAlt03
        | AdminRule::StatementScopeAlt04 => {
            out.item = Some(Box::new(match rule {
                AdminRule::StatementScopeAlt02 => parser_ast::StatementScope::Global,
                AdminRule::StatementScopeAlt03 => parser_ast::StatementScope::Instance,
                _ => parser_ast::StatementScope::Session,
            }))
        }
        AdminRule::ShowTableAliasOptAlt01 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::FlushOptionAlt08 => {
            out.item = Some(Box::new(parser_ast::FlushStmt {
                Tp: parser_ast::FlushStmtType::StatsDelta,
                FlushObjects: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::StatsObject>>())
                    .cloned()
                    .unwrap_or_default(),
                IsCluster: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        AdminRule::SetExprAlt01 => out.expr = Some(parser_ast::ExprNode::Value("ON".to_owned())),
        AdminRule::SetExprAlt02 => {
            out.expr = Some(parser_ast::ExprNode::Value("BINARY".to_owned()))
        }
        AdminRule::VariableNameAlt02 | AdminRule::ConfigItemNameAlt02 => {
            out.ident = format!("{}.{}", rhs[rhs_len - (2)].ident, rhs[rhs_len - (0)].ident)
        }
        AdminRule::ConfigItemNameAlt03 => {
            out.ident = format!("{}-{}", rhs[rhs_len - (2)].ident, rhs[rhs_len - (0)].ident)
        }
        AdminRule::VariableAssignmentAlt01
        | AdminRule::VariableAssignmentAlt02
        | AdminRule::VariableAssignmentAlt03
        | AdminRule::VariableAssignmentAlt04
        | AdminRule::VariableAssignmentAlt05 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::VariableAssignment {
                Name: rhs[rhs_len - (2)].ident.clone(),
                Value: value,
                IsGlobal: rule == AdminRule::VariableAssignmentAlt02,
                IsInstance: rule == AdminRule::VariableAssignmentAlt03,
                IsSystem: true,
                ..Default::default()
            }));
        }
        AdminRule::VariableAssignmentAlt06 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let mut name = rhs[rhs_len - (2)].ident.to_lowercase();
            let is_global = name.starts_with("@@global.");
            let is_instance = name.starts_with("@@instance.");
            for prefix in ["@@global.", "@@instance.", "@@session.", "@@local.", "@@"] {
                if let Some(value) = name.strip_prefix(prefix) {
                    name = value.to_owned();
                    break;
                }
            }
            out.item = Some(Box::new(parser_ast::VariableAssignment {
                Name: name,
                Value: value,
                IsGlobal: is_global,
                IsInstance: is_instance,
                IsSystem: true,
                ..Default::default()
            }));
        }
        AdminRule::VariableAssignmentAlt07 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::VariableAssignment {
                Name: rhs[rhs_len - (2)].ident.trim_start_matches('@').to_owned(),
                Value: value,
                ..Default::default()
            }));
        }
        AdminRule::VariableAssignmentAlt08
        | AdminRule::VariableAssignmentAlt09
        | AdminRule::VariableAssignmentAlt10
        | AdminRule::VariableAssignmentAlt11
        | AdminRule::VariableAssignmentAlt12 => {
            let (name, value_back, extend) = match rule {
                AdminRule::VariableAssignmentAlt08 => (parser_ast::SetNames, 0, None),
                AdminRule::VariableAssignmentAlt09 => (parser_ast::SetNames, 2, None),
                AdminRule::VariableAssignmentAlt10 => (parser_ast::SetNames, 2, Some(0)),
                AdminRule::VariableAssignmentAlt11 => (parser_ast::SetNames, 0, None),
                _ => (parser_ast::SetCharset, 0, None),
            };
            let value = if rule == AdminRule::VariableAssignmentAlt11 {
                parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::DefaultValue,
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                }
            } else if rule == AdminRule::VariableAssignmentAlt12 {
                rhs[rhs_len - (value_back)].expr.clone().unwrap_or_default()
            } else {
                parser_ast::ExprNode::Value(rhs[rhs_len - (value_back)].ident.clone())
            };
            let extend_value =
                extend.map(|back| parser_ast::ExprNode::Value(rhs[rhs_len - (back)].ident.clone()));
            out.item = Some(Box::new(parser_ast::VariableAssignment {
                Name: name.to_owned(),
                Value: value,
                ExtendValue: extend_value,
                ..Default::default()
            }));
        }
        AdminRule::CharsetNameOrDefaultAlt01 => {
            out.expr = Some(parser_ast::ExprNode::Value(
                rhs[rhs_len - (0)].ident.clone(),
            ))
        }
        AdminRule::CharsetNameOrDefaultAlt02 => {
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::DefaultValue,
                OriginTextPosition: 0,
                Flag: Default::default(),
            })
        }
        AdminRule::VariableAssignmentListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::VariableAssignment>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        AdminRule::VariableAssignmentListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::VariableAssignment>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::AlterOrderListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AlterOrderItem>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        AdminRule::AlterOrderListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AlterOrderItem>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AlterOrderItem>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::AlterOrderItemAlt01 => {
            let column = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
                .unwrap_or_default();
            let desc = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.item = Some(Box::new(parser_ast::AlterOrderItem {
                Column: column,
                Desc: desc,
            }));
        }
        AdminRule::HashStringAlt02 => {
            out.ident = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(semantic_value_text)
                .unwrap_or_default()
        }
        AdminRule::IfExistsAlt01 | AdminRule::IfNotExistsAlt01 | AdminRule::IgnoreOptionalAlt01 => {
            out.item = Some(Box::new(false))
        }
        AdminRule::IfExistsAlt02 | AdminRule::IfNotExistsAlt02 | AdminRule::IgnoreOptionalAlt02 => {
            out.item = Some(Box::new(true))
        }
        AdminRule::AdminStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ShowDdl,
            )))
        }
        AdminRule::AdminStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ShowDdlJobs,
            )))
        }
        AdminRule::AdminStmtAlt04
        | AdminRule::AdminStmtAlt06
        | AdminRule::AdminStmtAlt07
        | AdminRule::AdminStmtAlt09 => {
            let table_back = 1;
            let table = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            let statement_type = match rule {
                AdminRule::AdminStmtAlt04 => parser_ast::AdminStmtType::ShowNextRowId,
                AdminRule::AdminStmtAlt06 => parser_ast::AdminStmtType::CheckIndex,
                AdminRule::AdminStmtAlt07 => parser_ast::AdminStmtType::RecoverIndex,
                _ => parser_ast::AdminStmtType::CleanupIndex,
            };
            let mut statement = parser_ast::AdminStmt::new(statement_type);
            statement.tables.push(table);
            if rule != AdminRule::AdminStmtAlt04 {
                statement.index = rhs[rhs_len - (0)].ident.clone();
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt05 | AdminRule::AdminStmtAlt11 => {
            let tables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                .cloned()
                .unwrap_or_default();
            let statement_type = if rule == AdminRule::AdminStmtAlt05 {
                parser_ast::AdminStmtType::CheckTable
            } else {
                parser_ast::AdminStmtType::ChecksumTable
            };
            let mut statement = parser_ast::AdminStmt::new(statement_type);
            statement.tables = tables;
            out.statement = Some(Box::new(statement));
        }
        AdminRule::AdminStmtAlt08 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::WorkloadRepoCreate,
            )))
        }
        AdminRule::AdminStmtAlt18 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ReloadExprPushdownBlacklist,
            )))
        }
        AdminRule::AdminStmtAlt19 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ReloadOptRuleBlacklist,
            )))
        }
        AdminRule::AdminStmtAlt24 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::FlushBindings,
            )))
        }
        AdminRule::AdminStmtAlt25 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::CaptureBindings,
            )))
        }
        AdminRule::AdminStmtAlt26 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::EvolveBindings,
            )))
        }
        AdminRule::AdminStmtAlt27 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ReloadBindings,
            )))
        }
        AdminRule::AdminStmtAlt28 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ReloadClusterBindings,
            )))
        }
        AdminRule::AdminStmtAlt29 | AdminRule::AdminStmtAlt30 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ReloadStatistics,
            )))
        }
        AdminRule::AdminStmtAlt33 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::ShowBdrRole,
            )))
        }
        AdminRule::AdminStmtAlt34 => {
            out.statement = Some(Box::new(parser_ast::AdminStmt::new(
                parser_ast::AdminStmtType::UnsetBdrRole,
            )))
        }
        AdminRule::AdminStmtAlt12
        | AdminRule::AdminStmtAlt13
        | AdminRule::AdminStmtAlt14
        | AdminRule::AdminStmtAlt15 => {
            let job_ids = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<i64>>())
                .cloned()
                .unwrap_or_default();
            let statement_type = match rule {
                AdminRule::AdminStmtAlt12 => parser_ast::AdminStmtType::CancelDdlJobs,
                AdminRule::AdminStmtAlt13 => parser_ast::AdminStmtType::PauseDdlJobs,
                AdminRule::AdminStmtAlt14 => parser_ast::AdminStmtType::ResumeDdlJobs,
                _ => parser_ast::AdminStmtType::ShowDdlJobQueries,
            };
            let mut statement = parser_ast::AdminStmt::new(statement_type);
            statement.job_ids = job_ids;
            out.statement = Some(Box::new(statement));
        }
        AdminRule::NumListAlt01 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(vec![value]));
        }
        AdminRule::NumListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<i64>>())
                .cloned()
                .unwrap_or_default();
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            values.push(value);
            out.item = Some(Box::new(values));
        }
        AdminRule::ShowStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ShowStmt>())
                .cloned()
            else {
                return Ok(false);
            };
            let filter = rhs[rhs_len - (0)].item.as_deref();
            if let Some(pattern) = filter.and_then(|item| item.downcast_ref::<ShowLikeSemantic>()) {
                statement.Pattern = Some(pattern.0.clone());
            } else {
                statement.Where = filter
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned();
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::ShowStmtAlt02
        | AdminRule::ShowStmtAlt03
        | AdminRule::ShowStmtAlt04
        | AdminRule::ShowStmtAlt05
        | AdminRule::ShowStmtAlt06
        | AdminRule::ShowStmtAlt07 => {
            let mut statement = parser_ast::ShowStmt::default();
            statement.Tp = match rule {
                AdminRule::ShowStmtAlt02 => parser_ast::ShowStmtType::CreateTable,
                AdminRule::ShowStmtAlt03 => parser_ast::ShowStmtType::CreateView,
                AdminRule::ShowStmtAlt04 => parser_ast::ShowStmtType::CreateDatabase,
                AdminRule::ShowStmtAlt05 => parser_ast::ShowStmtType::CreateSequence,
                AdminRule::ShowStmtAlt06 => parser_ast::ShowStmtType::CreatePlacementPolicy,
                _ => parser_ast::ShowStmtType::CreateResourceGroup,
            };
            if matches!(
                rule,
                AdminRule::ShowStmtAlt02 | AdminRule::ShowStmtAlt03 | AdminRule::ShowStmtAlt05
            ) {
                statement.Table = rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned();
            } else if rule == AdminRule::ShowStmtAlt04 {
                statement.IfNotExists = rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
                statement.DBName = rhs[rhs_len - (0)].ident.clone();
            } else if rule == AdminRule::ShowStmtAlt07 {
                statement.ResourceGroupName = rhs[rhs_len - (0)].ident.clone();
            } else {
                statement.DBName = rhs[rhs_len - (0)].ident.clone();
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::ShowStmtAlt13
        | AdminRule::ShowStmtAlt15
        | AdminRule::ShowStmtAlt16
        | AdminRule::ShowStmtAlt17
        | AdminRule::ShowStmtAlt18
        | AdminRule::ShowStmtAlt19
        | AdminRule::ShowStmtAlt21
        | AdminRule::ShowStmtAlt22 => {
            let statement_type = match rule {
                AdminRule::ShowStmtAlt13 => parser_ast::ShowStmtType::Grants,
                AdminRule::ShowStmtAlt15 => parser_ast::ShowStmtType::MasterStatus,
                AdminRule::ShowStmtAlt16 => parser_ast::ShowStmtType::BinlogStatus,
                AdminRule::ShowStmtAlt17 => parser_ast::ShowStmtType::ReplicaStatus,
                AdminRule::ShowStmtAlt18 => parser_ast::ShowStmtType::ProcessList,
                AdminRule::ShowStmtAlt19 => parser_ast::ShowStmtType::Profiles,
                AdminRule::ShowStmtAlt21 => parser_ast::ShowStmtType::Privileges,
                _ => parser_ast::ShowStmtType::Builtins,
            };
            let mut statement = parser_ast::ShowStmt {
                Tp: statement_type,
                ..Default::default()
            };
            if rule == AdminRule::ShowStmtAlt18 {
                statement.Full = rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::ShowStmtAlt26 => {
            let procedure = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned();
            out.statement = Some(Box::new(parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::CreateProcedure,
                Procedure: procedure,
                ..Default::default()
            }));
        }
        AdminRule::ShowTargetFilterableAlt01
        | AdminRule::ShowTargetFilterableAlt02
        | AdminRule::ShowTargetFilterableAlt03
        | AdminRule::ShowTargetFilterableAlt04
        | AdminRule::ShowTargetFilterableAlt05
        | AdminRule::ShowTargetFilterableAlt06
        | AdminRule::ShowTargetFilterableAlt07
        | AdminRule::ShowTargetFilterableAlt08
        | AdminRule::ShowTargetFilterableAlt09
        | AdminRule::ShowTargetFilterableAlt10
        | AdminRule::ShowTargetFilterableAlt11
        | AdminRule::ShowTargetFilterableAlt12
        | AdminRule::ShowTargetFilterableAlt13
        | AdminRule::ShowTargetFilterableAlt14
        | AdminRule::ShowTargetFilterableAlt15
        | AdminRule::ShowTargetFilterableAlt16
        | AdminRule::ShowTargetFilterableAlt17
        | AdminRule::ShowTargetFilterableAlt18
        | AdminRule::ShowTargetFilterableAlt19
        | AdminRule::ShowTargetFilterableAlt20
        | AdminRule::ShowTargetFilterableAlt21
        | AdminRule::ShowTargetFilterableAlt22
        | AdminRule::ShowTargetFilterableAlt23
        | AdminRule::ShowTargetFilterableAlt24
        | AdminRule::ShowTargetFilterableAlt25
        | AdminRule::ShowTargetFilterableAlt26
        | AdminRule::ShowTargetFilterableAlt27
        | AdminRule::ShowTargetFilterableAlt28
        | AdminRule::ShowTargetFilterableAlt29
        | AdminRule::ShowTargetFilterableAlt30
        | AdminRule::ShowTargetFilterableAlt31
        | AdminRule::ShowTargetFilterableAlt32
        | AdminRule::ShowTargetFilterableAlt33
        | AdminRule::ShowTargetFilterableAlt34
        | AdminRule::ShowTargetFilterableAlt35
        | AdminRule::ShowTargetFilterableAlt36
        | AdminRule::ShowTargetFilterableAlt37
        | AdminRule::ShowTargetFilterableAlt38
        | AdminRule::ShowTargetFilterableAlt39
        | AdminRule::ShowTargetFilterableAlt40
        | AdminRule::ShowTargetFilterableAlt41
        | AdminRule::ShowTargetFilterableAlt42
        | AdminRule::ShowTargetFilterableAlt43
        | AdminRule::ShowTargetFilterableAlt44
        | AdminRule::ShowTargetFilterableAlt45
        | AdminRule::ShowTargetFilterableAlt46 => {
            let statement_type = match rule {
                AdminRule::ShowTargetFilterableAlt01 => parser_ast::ShowStmtType::Engines,
                AdminRule::ShowTargetFilterableAlt02 => parser_ast::ShowStmtType::Databases,
                AdminRule::ShowTargetFilterableAlt03 => parser_ast::ShowStmtType::Config,
                AdminRule::ShowTargetFilterableAlt04 => parser_ast::ShowStmtType::Charset,
                AdminRule::ShowTargetFilterableAlt05 => parser_ast::ShowStmtType::Tables,
                AdminRule::ShowTargetFilterableAlt06 => parser_ast::ShowStmtType::OpenTables,
                AdminRule::ShowTargetFilterableAlt07 => parser_ast::ShowStmtType::TableStatus,
                AdminRule::ShowTargetFilterableAlt08 | AdminRule::ShowTargetFilterableAlt09 => {
                    parser_ast::ShowStmtType::Index
                }
                AdminRule::ShowTargetFilterableAlt10 | AdminRule::ShowTargetFilterableAlt11 => {
                    parser_ast::ShowStmtType::Columns
                }
                AdminRule::ShowTargetFilterableAlt12 | AdminRule::ShowTargetFilterableAlt13 => {
                    parser_ast::ShowStmtType::Warnings
                }
                AdminRule::ShowTargetFilterableAlt14 | AdminRule::ShowTargetFilterableAlt15 => {
                    parser_ast::ShowStmtType::Errors
                }
                AdminRule::ShowTargetFilterableAlt16 => parser_ast::ShowStmtType::Variables,
                AdminRule::ShowTargetFilterableAlt17 => parser_ast::ShowStmtType::Status,
                AdminRule::ShowTargetFilterableAlt18 => parser_ast::ShowStmtType::Bindings,
                AdminRule::ShowTargetFilterableAlt19 => parser_ast::ShowStmtType::Collation,
                AdminRule::ShowTargetFilterableAlt20 => parser_ast::ShowStmtType::Triggers,
                AdminRule::ShowTargetFilterableAlt21 => {
                    parser_ast::ShowStmtType::BindingCacheStatus
                }
                AdminRule::ShowTargetFilterableAlt22 => parser_ast::ShowStmtType::ProcedureStatus,
                AdminRule::ShowTargetFilterableAlt23 => parser_ast::ShowStmtType::FunctionStatus,
                AdminRule::ShowTargetFilterableAlt24 => parser_ast::ShowStmtType::Events,
                AdminRule::ShowTargetFilterableAlt25 => parser_ast::ShowStmtType::Plugins,
                AdminRule::ShowTargetFilterableAlt26 => parser_ast::ShowStmtType::SessionStates,
                AdminRule::ShowTargetFilterableAlt27 => parser_ast::ShowStmtType::StatsExtended,
                AdminRule::ShowTargetFilterableAlt28 => parser_ast::ShowStmtType::StatsMeta,
                AdminRule::ShowTargetFilterableAlt29 => parser_ast::ShowStmtType::StatsHistograms,
                AdminRule::ShowTargetFilterableAlt30 => parser_ast::ShowStmtType::StatsTopN,
                AdminRule::ShowTargetFilterableAlt31 => parser_ast::ShowStmtType::StatsBuckets,
                AdminRule::ShowTargetFilterableAlt32 => parser_ast::ShowStmtType::StatsHealthy,
                AdminRule::ShowTargetFilterableAlt33 => parser_ast::ShowStmtType::StatsLocked,
                AdminRule::ShowTargetFilterableAlt34 => {
                    parser_ast::ShowStmtType::HistogramsInFlight
                }
                AdminRule::ShowTargetFilterableAlt35 => parser_ast::ShowStmtType::ColumnStatsUsage,
                AdminRule::ShowTargetFilterableAlt36 => parser_ast::ShowStmtType::Affinity,
                AdminRule::ShowTargetFilterableAlt37 => parser_ast::ShowStmtType::AnalyzeStatus,
                AdminRule::ShowTargetFilterableAlt38 => parser_ast::ShowStmtType::Backups,
                AdminRule::ShowTargetFilterableAlt39 => parser_ast::ShowStmtType::Restores,
                AdminRule::ShowTargetFilterableAlt40 => parser_ast::ShowStmtType::Placement,
                AdminRule::ShowTargetFilterableAlt41 => parser_ast::ShowStmtType::PlacementLabels,
                AdminRule::ShowTargetFilterableAlt42 | AdminRule::ShowTargetFilterableAlt43 => {
                    parser_ast::ShowStmtType::ImportGroups
                }
                AdminRule::ShowTargetFilterableAlt44 => parser_ast::ShowStmtType::ImportJobs,
                AdminRule::ShowTargetFilterableAlt46 => {
                    parser_ast::ShowStmtType::StorageClassTransitions
                }
                _ => parser_ast::ShowStmtType::DistributionJobs,
            };
            let mut statement = parser_ast::ShowStmt {
                Tp: statement_type,
                ..Default::default()
            };
            match rule {
                AdminRule::ShowTargetFilterableAlt05 => {
                    statement.DBName = rhs[rhs_len - (0)].ident.clone();
                    statement.Full = rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false);
                }
                AdminRule::ShowTargetFilterableAlt06
                | AdminRule::ShowTargetFilterableAlt07
                | AdminRule::ShowTargetFilterableAlt20
                | AdminRule::ShowTargetFilterableAlt24 => {
                    statement.DBName = rhs[rhs_len - (0)].ident.clone()
                }
                AdminRule::ShowTargetFilterableAlt08 => {
                    statement.Table = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                        .cloned()
                }
                AdminRule::ShowTargetFilterableAlt09 => {
                    statement.Table = Some(parser_ast::TableName {
                        Name: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                        Schema: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                        ..Default::default()
                    })
                }
                AdminRule::ShowTargetFilterableAlt10 | AdminRule::ShowTargetFilterableAlt11 => {
                    statement.Table = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                        .cloned();
                    statement.DBName = rhs[rhs_len - (0)].ident.clone();
                    statement.Full = rhs[rhs_len - (3)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false);
                    statement.Extended = rule == AdminRule::ShowTargetFilterableAlt11;
                }
                AdminRule::ShowTargetFilterableAlt12 | AdminRule::ShowTargetFilterableAlt14 => {
                    statement.CountWarningsOrErrors = true
                }
                AdminRule::ShowTargetFilterableAlt16
                | AdminRule::ShowTargetFilterableAlt17
                | AdminRule::ShowTargetFilterableAlt18 => {
                    statement.GlobalScope = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false)
                }
                AdminRule::ShowTargetFilterableAlt43 => {
                    statement.ShowGroupKey = rhs[rhs_len - (0)].ident.clone()
                }
                AdminRule::ShowTargetFilterableAlt44 => {
                    statement.ImportJobRaw = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false)
                }
                _ => {}
            }
            out.item = Some(Box::new(statement));
        }
        AdminRule::ShowLikeOrWhereOptAlt01 => out.item = None,
        AdminRule::ShowLikeOrWhereOptAlt02 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(ShowLikeSemantic(expr)) as Box<dyn Any>)
        }
        AdminRule::ShowLikeOrWhereOptAlt03 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>)
        }
        AdminRule::ShowImportJobTargetAlt01
        | AdminRule::ShowImportJobsTargetAlt01
        | AdminRule::GlobalScopeAlt01
        | AdminRule::GlobalScopeAlt03
        | AdminRule::OptFullAlt01 => out.item = Some(Box::new(false)),
        AdminRule::ShowImportJobTargetAlt02
        | AdminRule::ShowImportJobsTargetAlt02
        | AdminRule::GlobalScopeAlt02
        | AdminRule::OptFullAlt02 => out.item = Some(Box::new(true)),
        AdminRule::ShowDatabaseNameOptAlt01 => out.ident.clear(),
        AdminRule::ShowDatabaseNameOptAlt02 => out.ident = rhs[rhs_len - (0)].ident.clone(),
        AdminRule::FlushStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FlushStmt>())
                .cloned()
            else {
                return Ok(false);
            };
            statement.NoWriteToBinLog = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.statement = Some(Box::new(statement));
        }
        AdminRule::PluginNameListAlt01 => {
            out.item = Some(Box::new(vec![rhs[rhs_len - (0)].ident.clone()]))
        }
        AdminRule::PluginNameListAlt02 => {
            let mut names = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<String>>())
                .cloned()
                .unwrap_or_default();
            names.push(rhs[rhs_len - (0)].ident.clone());
            out.item = Some(Box::new(names));
        }
        AdminRule::FlushOptionAlt01
        | AdminRule::FlushOptionAlt02
        | AdminRule::FlushOptionAlt03
        | AdminRule::FlushOptionAlt04
        | AdminRule::FlushOptionAlt05
        | AdminRule::FlushOptionAlt06
        | AdminRule::FlushOptionAlt07 => {
            let mut statement = parser_ast::FlushStmt {
                Tp: match rule {
                    AdminRule::FlushOptionAlt01 => parser_ast::FlushStmtType::Privileges,
                    AdminRule::FlushOptionAlt02 => parser_ast::FlushStmtType::Status,
                    AdminRule::FlushOptionAlt03 => parser_ast::FlushStmtType::TiDBPlugin,
                    AdminRule::FlushOptionAlt04 => parser_ast::FlushStmtType::Hosts,
                    AdminRule::FlushOptionAlt05 => parser_ast::FlushStmtType::Logs,
                    AdminRule::FlushOptionAlt06 => parser_ast::FlushStmtType::Tables,
                    _ => parser_ast::FlushStmtType::ClientErrorsSummary,
                },
                ..Default::default()
            };
            match rule {
                AdminRule::FlushOptionAlt03 => {
                    statement.Plugins = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<String>>())
                        .cloned()
                        .unwrap_or_default()
                }
                AdminRule::FlushOptionAlt05 => {
                    statement.LogType = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::LogType>())
                        .copied()
                        .unwrap_or_default()
                }
                AdminRule::FlushOptionAlt06 => {
                    statement.Tables = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.ReadLock = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false);
                }
                _ => {}
            }
            out.item = Some(Box::new(statement));
        }
        AdminRule::LogTypeOptAlt01
        | AdminRule::LogTypeOptAlt02
        | AdminRule::LogTypeOptAlt03
        | AdminRule::LogTypeOptAlt04
        | AdminRule::LogTypeOptAlt05
        | AdminRule::LogTypeOptAlt06 => {
            out.item = Some(Box::new(match rule {
                AdminRule::LogTypeOptAlt01 => parser_ast::LogType::Default,
                AdminRule::LogTypeOptAlt02 => parser_ast::LogType::Binary,
                AdminRule::LogTypeOptAlt03 => parser_ast::LogType::Engine,
                AdminRule::LogTypeOptAlt04 => parser_ast::LogType::Error,
                AdminRule::LogTypeOptAlt05 => parser_ast::LogType::General,
                _ => parser_ast::LogType::Slow,
            }))
        }
        AdminRule::ClusterOptAlt01 => out.item = Some(Box::new(false)),
        AdminRule::ClusterOptAlt02 => out.item = Some(Box::new(true)),
        AdminRule::NoWriteToBinLogAliasOptAlt01 => out.item = Some(Box::new(false)),
        AdminRule::NoWriteToBinLogAliasOptAlt02 => out.item = Some(Box::new(true)),
        AdminRule::NoWriteToBinLogAliasOptAlt03 => out.item = Some(Box::new(true)),
        AdminRule::TableNameListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::TableName>::new()))
        }
        AdminRule::WithReadLockOptAlt01 => out.item = Some(Box::new(false)),
        AdminRule::WithReadLockOptAlt02 => out.item = Some(Box::new(true)),
        AdminRule::CreateBindingStmtAlt01 | AdminRule::CreateBindingStmtAlt02 => {
            let (origin, hinted) = if rule == AdminRule::CreateBindingStmtAlt01 {
                (
                    rhs[rhs_len - (2)].statement.take(),
                    rhs[rhs_len - (0)].statement.take(),
                )
            } else {
                let hinted = rhs[rhs_len - (0)].statement.take();
                (None, hinted)
            };
            let global_back = if rule == AdminRule::CreateBindingStmtAlt01 {
                5
            } else {
                3
            };
            out.statement = Some(Box::new(parser_ast::CreateBindingStmt {
                OriginNode: origin,
                HintedNode: hinted,
                GlobalScope: rhs[rhs_len - (global_back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }));
        }
        AdminRule::CreateBindingStmtAlt03 => {
            out.statement = Some(Box::new(parser_ast::CreateBindingStmt {
                GlobalScope: rhs[rhs_len - (7)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                PlanDigests: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::StringOrUserVar>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::DropBindingStmtAlt01 | AdminRule::DropBindingStmtAlt02 => {
            let (origin, hinted) = if rule == AdminRule::DropBindingStmtAlt01 {
                (rhs[rhs_len - (0)].statement.take(), None)
            } else {
                (
                    rhs[rhs_len - (2)].statement.take(),
                    rhs[rhs_len - (0)].statement.take(),
                )
            };
            out.statement = Some(Box::new(parser_ast::DropBindingStmt {
                OriginNode: origin,
                HintedNode: hinted,
                GlobalScope: rhs[rhs_len
                    - (if rule == AdminRule::DropBindingStmtAlt01 {
                        3
                    } else {
                        5
                    })]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false),
                ..Default::default()
            }));
        }
        AdminRule::DropBindingStmtAlt03 => {
            out.statement = Some(Box::new(parser_ast::DropBindingStmt {
                GlobalScope: rhs[rhs_len - (5)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                SQLDigests: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::StringOrUserVar>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::SetBindingStmtAlt01 | AdminRule::SetBindingStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::SetBindingStmt {
                BindingStatusType: rhs[rhs_len
                    - (if rule == AdminRule::SetBindingStmtAlt01 {
                        2
                    } else {
                        4
                    })]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::BindingStatusType>())
                .copied()
                .unwrap_or_default(),
                OriginNode: rhs[rhs_len
                    - (if rule == AdminRule::SetBindingStmtAlt01 {
                        0
                    } else {
                        2
                    })]
                .statement
                .take(),
                HintedNode: if rule == AdminRule::SetBindingStmtAlt02 {
                    rhs[rhs_len - (0)].statement.take()
                } else {
                    None
                },
                ..Default::default()
            }))
        }
        AdminRule::SetBindingStmtAlt03 => {
            out.statement = Some(Box::new(parser_ast::SetBindingStmt {
                BindingStatusType: rhs[rhs_len - (4)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::BindingStatusType>())
                    .copied()
                    .unwrap_or_default(),
                SQLDigest: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }))
        }
        AdminRule::GrantStmtAlt01 | AdminRule::RevokeStmtAlt01 => {
            let list_back = if rule == AdminRule::GrantStmtAlt01 {
                7
            } else {
                5
            };
            let values = rhs[rhs_len - (list_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<RoleOrPrivSemantic>>())
                .cloned()
                .unwrap_or_default();
            let mut privileges = Vec::new();
            for value in values {
                match value {
                    RoleOrPrivSemantic::Priv(value) => privileges.push(value),
                    RoleOrPrivSemantic::Dynamic(name) => privileges.push(parser_ast::PrivElem {
                        Name: name,
                        ..Default::default()
                    }),
                    RoleOrPrivSemantic::Role(_) => {
                        yylex.AppendError(yylex.Errorf("expected privilege", &[]));
                        return Err(1);
                    }
                }
            }
            if rule == AdminRule::GrantStmtAlt01 {
                out.statement = Some(Box::new(parser_ast::GrantStmt {
                    node_text: Default::default(),
                    Privs: privileges,
                    ObjectType: rhs[rhs_len - (5)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ObjectTypeType>())
                        .copied()
                        .unwrap_or_default(),
                    Level: rhs[rhs_len - (4)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::GrantLevel>())
                        .cloned()
                        .unwrap_or_default(),
                    Users: rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserSpec>>())
                        .cloned()
                        .unwrap_or_default(),
                    AuthTokenOrTLSOptions: rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::AuthTokenOrTLSOption>>()
                        })
                        .cloned()
                        .unwrap_or_default(),
                    WithGrant: rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                }));
            } else {
                out.statement = Some(Box::new(parser_ast::RevokeStmt {
                    node_text: Default::default(),
                    Privs: privileges,
                    ObjectType: rhs[rhs_len - (3)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ObjectTypeType>())
                        .copied()
                        .unwrap_or_default(),
                    Level: rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::GrantLevel>())
                        .cloned()
                        .unwrap_or_default(),
                    Users: rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserSpec>>())
                        .cloned()
                        .unwrap_or_default(),
                }));
            }
        }
        AdminRule::CreateMaskingPolicyStmtAlt01 => {
            let or_replace = rhs[rhs_len - (13)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            let if_not_exists = rhs[rhs_len - (10)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            if or_replace && if_not_exists {
                yylex.AppendError(yylex.Errorf(
                    "'OR REPLACE' and 'IF NOT EXISTS' are mutually exclusive",
                    &[],
                ));
                return Err(1);
            }
            out.statement = Some(Box::new(parser_ast::CreateMaskingPolicyStmt {
                node_text: Default::default(),
                OrReplace: or_replace,
                IfNotExists: if_not_exists,
                PolicyName: parser_ast::NewCIStr(&rhs[rhs_len - (9)].ident),
                Table: rhs[rhs_len - (7)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                Column: parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr(&rhs[rhs_len - (5)].ident),
                    ..Default::default()
                },
                Expr: rhs[rhs_len - (2)].expr.clone(),
                RestrictOps: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::MaskingPolicyRestrictOps>())
                    .copied()
                    .unwrap_or_default(),
                MaskingPolicyState: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::MaskingPolicyState>())
                    .copied()
                    .unwrap_or_default(),
            }));
        }
        AdminRule::UnlockTablesStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::UnlockTablesStmt::default()))
        }
        AdminRule::LockTablesStmtAlt01 => {
            let locks = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableLock>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::LockTablesStmt {
                node_text: Default::default(),
                TableLocks: locks,
            }));
        }
        AdminRule::TableLockAlt01 => {
            let Some(table) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let lock_type = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableLockType>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::TableLock {
                Table: table,
                Type: lock_type,
            }));
        }
        AdminRule::LockTypeAlt01
        | AdminRule::LockTypeAlt02
        | AdminRule::LockTypeAlt03
        | AdminRule::LockTypeAlt04 => {
            out.item = Some(Box::new(match rule {
                AdminRule::LockTypeAlt01 => parser_ast::TableLockType::Read,
                AdminRule::LockTypeAlt02 => parser_ast::TableLockType::ReadLocal,
                AdminRule::LockTypeAlt03 => parser_ast::TableLockType::Write,
                _ => parser_ast::TableLockType::WriteLocal,
            }))
        }
        AdminRule::TableLockListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableLock>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        AdminRule::TableLockListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableLock>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableLock>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::OptimizeTableStmtAlt01 => {
            let tables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                .cloned()
                .unwrap_or_default();
            let no_write = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.statement = Some(Box::new(parser_ast::OptimizeTableStmt {
                node_text: Default::default(),
                NoWriteToBinLog: no_write,
                Tables: tables,
            }));
        }
        AdminRule::CreateResourceGroupStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::CreateResourceGroupStmt {
                node_text: Default::default(),
                IfNotExists: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ResourceGroupName: parser_ast::NewCIStr(&rhs[rhs_len - (1)].ident),
                ResourceGroupOptionList: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ResourceGroupOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        AdminRule::AlterResourceGroupStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::AlterResourceGroupStmt {
                node_text: Default::default(),
                IfExists: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ResourceGroupName: parser_ast::NewCIStr(&rhs[rhs_len - (1)].ident),
                ResourceGroupOptionList: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ResourceGroupOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        AdminRule::MaskingPolicyStateOptAlt01
        | AdminRule::MaskingPolicyStateOptAlt02
        | AdminRule::MaskingPolicyStateOptAlt03 => {
            out.item = Some(Box::new(parser_ast::MaskingPolicyState {
                Enabled: rule != AdminRule::MaskingPolicyStateOptAlt03,
                Explicit: rule != AdminRule::MaskingPolicyStateOptAlt01,
            }))
        }
        AdminRule::MaskingPolicyRestrictOnOptAlt01 | AdminRule::MaskingPolicyRestrictOnOptAlt03 => {
            out.item = Some(Box::new(parser_ast::MaskingPolicyRestrictOpNone))
        }
        AdminRule::MaskingPolicyRestrictOnOptAlt02 => out.item = rhs[rhs_len - (1)].item.take(),
        AdminRule::MaskingPolicyRestrictOperationListAlt01 => {
            out.item = rhs[rhs_len - (0)].item.take()
        }
        AdminRule::MaskingPolicyRestrictOperationListAlt02 => {
            let left = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::MaskingPolicyRestrictOps>())
                .copied()
                .unwrap_or_default();
            let right = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::MaskingPolicyRestrictOps>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(left | right));
        }
        AdminRule::MaskingPolicyRestrictOperationAlt01 => {
            let op = match rhs[rhs_len - (0)].ident.to_ascii_uppercase().as_str() {
                "INSERT_INTO_SELECT" => parser_ast::MaskingPolicyRestrictOpInsertIntoSelect,
                "UPDATE_SELECT" => parser_ast::MaskingPolicyRestrictOpUpdateSelect,
                "DELETE_SELECT" => parser_ast::MaskingPolicyRestrictOpDeleteSelect,
                "CTAS" => parser_ast::MaskingPolicyRestrictOpCTAS,
                _ => {
                    yylex.AppendError(
                        yylex.Errorf("unsupported masking policy restrict operation", &[]),
                    );
                    return Err(1);
                }
            };
            out.item = Some(Box::new(op));
        }
        AdminRule::KillStmtAlt01
        | AdminRule::KillStmtAlt02
        | AdminRule::KillStmtAlt03
        | AdminRule::KillStmtAlt04 => {
            let extension_back = match rule {
                AdminRule::KillStmtAlt01 => 1,
                AdminRule::KillStmtAlt02 | AdminRule::KillStmtAlt03 => 2,
                _ => 1,
            };
            let extension = rhs[rhs_len - (extension_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            let expression = if rule == AdminRule::KillStmtAlt04 {
                rhs[rhs_len - (0)].expr.clone()
            } else {
                None
            };
            let connection_id = if rule == AdminRule::KillStmtAlt04 {
                0
            } else {
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .map(getUint64FromNUM)
                    .unwrap_or_default()
            };
            out.statement = Some(Box::new(parser_ast::KillStmt {
                node_text: Default::default(),
                Query: rule == AdminRule::KillStmtAlt03,
                ConnectionID: connection_id,
                TiDBExtension: extension,
                Expr: expression,
            }));
        }
        AdminRule::KillOrKillTiDBAlt01 => out.item = Some(Box::new(false)),
        AdminRule::KillOrKillTiDBAlt02 => out.item = Some(Box::new(true)),
        AdminRule::LoadStatsStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::LoadStatsStmt {
                node_text: Default::default(),
                Path: rhs[rhs_len - (0)].ident.clone(),
            }))
        }
        AdminRule::LockStatsStmtAlt01 | AdminRule::UnlockStatsStmtAlt01 => {
            let tables = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(if rule == AdminRule::LockStatsStmtAlt01 {
                Box::new(parser_ast::LockStatsStmt {
                    node_text: Default::default(),
                    Tables: tables,
                }) as Box<dyn parser_ast::Node>
            } else {
                Box::new(parser_ast::UnlockStatsStmt {
                    node_text: Default::default(),
                    Tables: tables,
                }) as Box<dyn parser_ast::Node>
            });
        }
        AdminRule::LockStatsStmtAlt02
        | AdminRule::LockStatsStmtAlt03
        | AdminRule::UnlockStatsStmtAlt02
        | AdminRule::UnlockStatsStmtAlt03 => {
            let (table_back, partition_back) = if matches!(
                rule,
                AdminRule::LockStatsStmtAlt02 | AdminRule::UnlockStatsStmtAlt02
            ) {
                (2, 0)
            } else {
                (4, 1)
            };
            let Some(mut table) = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            table.PartitionNames = rhs[rhs_len - (partition_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(
                if matches!(
                    rule,
                    AdminRule::LockStatsStmtAlt02 | AdminRule::LockStatsStmtAlt03
                ) {
                    Box::new(parser_ast::LockStatsStmt {
                        node_text: Default::default(),
                        Tables: vec![table],
                    }) as Box<dyn parser_ast::Node>
                } else {
                    Box::new(parser_ast::UnlockStatsStmt {
                        node_text: Default::default(),
                        Tables: vec![table],
                    }) as Box<dyn parser_ast::Node>
                },
            );
        }
        AdminRule::CreateSequenceStmtAlt01 => {
            let Some(name) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::CreateSequenceStmt {
                node_text: Default::default(),
                IfNotExists: rhs[rhs_len - (3)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Name: name,
                SeqOptions: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SequenceOption>>())
                    .cloned()
                    .unwrap_or_default(),
                TblOptions: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        AdminRule::CreateSequenceTableOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::TableOption>::new()));
        }
        AdminRule::CreateSequenceOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::SequenceOption>::new()))
        }
        AdminRule::SequenceOptionListAlt01 | AdminRule::AlterSequenceOptionListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SequenceOption>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        AdminRule::SequenceOptionListAlt02 | AdminRule::AlterSequenceOptionListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::SequenceOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SequenceOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::SequenceOptionAlt01
        | AdminRule::SequenceOptionAlt02
        | AdminRule::SequenceOptionAlt03
        | AdminRule::SequenceOptionAlt04
        | AdminRule::SequenceOptionAlt05
        | AdminRule::SequenceOptionAlt06
        | AdminRule::SequenceOptionAlt07
        | AdminRule::SequenceOptionAlt08
        | AdminRule::SequenceOptionAlt09
        | AdminRule::SequenceOptionAlt10
        | AdminRule::SequenceOptionAlt11
        | AdminRule::SequenceOptionAlt12
        | AdminRule::SequenceOptionAlt13
        | AdminRule::SequenceOptionAlt14
        | AdminRule::SequenceOptionAlt15
        | AdminRule::SequenceOptionAlt16 => {
            let option_type = match rule {
                AdminRule::SequenceOptionAlt01 | AdminRule::SequenceOptionAlt02 => {
                    parser_ast::SequenceOptionType::IncrementBy
                }
                AdminRule::SequenceOptionAlt03 | AdminRule::SequenceOptionAlt04 => {
                    parser_ast::SequenceOptionType::StartWith
                }
                AdminRule::SequenceOptionAlt05 => parser_ast::SequenceOptionType::MinValue,
                AdminRule::SequenceOptionAlt06 | AdminRule::SequenceOptionAlt07 => {
                    parser_ast::SequenceOptionType::NoMinValue
                }
                AdminRule::SequenceOptionAlt08 => parser_ast::SequenceOptionType::MaxValue,
                AdminRule::SequenceOptionAlt09 | AdminRule::SequenceOptionAlt10 => {
                    parser_ast::SequenceOptionType::NoMaxValue
                }
                AdminRule::SequenceOptionAlt11 => parser_ast::SequenceOptionType::Cache,
                AdminRule::SequenceOptionAlt12 | AdminRule::SequenceOptionAlt13 => {
                    parser_ast::SequenceOptionType::NoCache
                }
                AdminRule::SequenceOptionAlt14 => parser_ast::SequenceOptionType::Cycle,
                _ => parser_ast::SequenceOptionType::NoCycle,
            };
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::SequenceOption {
                Tp: option_type,
                IntValue: value,
            }));
        }
        AdminRule::DropSequenceStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropSequenceStmt {
                node_text: Default::default(),
                IfExists: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Sequences: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        AdminRule::AlterSequenceStmtAlt01 => {
            let Some(name) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::AlterSequenceStmt {
                node_text: Default::default(),
                IfExists: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Name: name,
                SeqOptions: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SequenceOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        AdminRule::AlterSequenceOptionAlt02
        | AdminRule::AlterSequenceOptionAlt03
        | AdminRule::AlterSequenceOptionAlt04 => {
            let option_type = if rule == AdminRule::AlterSequenceOptionAlt02 {
                parser_ast::SequenceOptionType::Restart
            } else {
                parser_ast::SequenceOptionType::RestartWith
            };
            let value = if rule == AdminRule::AlterSequenceOptionAlt02 {
                0
            } else {
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i64>())
                    .copied()
                    .unwrap_or_default()
            };
            out.item = Some(Box::new(parser_ast::SequenceOption {
                Tp: option_type,
                IntValue: value,
            }));
        }
        AdminRule::PlanReplayerStmtAlt01
        | AdminRule::PlanReplayerStmtAlt02
        | AdminRule::PlanReplayerStmtAlt03
        | AdminRule::PlanReplayerStmtAlt04
        | AdminRule::PlanReplayerStmtAlt05
        | AdminRule::PlanReplayerStmtAlt06
        | AdminRule::PlanReplayerStmtAlt07
        | AdminRule::PlanReplayerStmtAlt08 => {
            let mut statement = parser_ast::PlanReplayerStmt {
                Analyze: matches!(
                    rule,
                    AdminRule::PlanReplayerStmtAlt02
                        | AdminRule::PlanReplayerStmtAlt04
                        | AdminRule::PlanReplayerStmtAlt06
                        | AdminRule::PlanReplayerStmtAlt08
                ),
                ..Default::default()
            };
            let history_back = match rule {
                AdminRule::PlanReplayerStmtAlt01 | AdminRule::PlanReplayerStmtAlt05 => 2,
                AdminRule::PlanReplayerStmtAlt02 | AdminRule::PlanReplayerStmtAlt06 => 3,
                AdminRule::PlanReplayerStmtAlt03 => 6,
                AdminRule::PlanReplayerStmtAlt04 => 7,
                AdminRule::PlanReplayerStmtAlt07 => 4,
                _ => 5,
            };
            statement.HistoricalStatsInfo = rhs[rhs_len - (history_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AsOfClause>())
                .cloned();
            match rule {
                AdminRule::PlanReplayerStmtAlt01 | AdminRule::PlanReplayerStmtAlt02 => {
                    statement.Stmt = rhs[rhs_len - (0)].statement.take()
                }
                AdminRule::PlanReplayerStmtAlt03 | AdminRule::PlanReplayerStmtAlt04 => {
                    statement.Where = rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                        .cloned();
                    statement.OrderBy = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                        .cloned()
                        .unwrap_or_default();
                    statement.Limit = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                        .cloned();
                }
                AdminRule::PlanReplayerStmtAlt05 | AdminRule::PlanReplayerStmtAlt06 => {
                    statement.File = rhs[rhs_len - (0)].ident.clone()
                }
                AdminRule::PlanReplayerStmtAlt07 | AdminRule::PlanReplayerStmtAlt08 => {
                    statement.StmtList = rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<String>>())
                        .cloned()
                        .unwrap_or_default()
                }
                _ => {}
            }
            out.statement = Some(Box::new(statement));
        }
        AdminRule::PlanReplayerStmtAlt09 => {
            out.statement = Some(Box::new(parser_ast::PlanReplayerStmt {
                Load: true,
                File: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }))
        }
        AdminRule::PlanReplayerStmtAlt10 | AdminRule::PlanReplayerStmtAlt11 => {
            out.statement = Some(Box::new(parser_ast::PlanReplayerStmt {
                Capture: rule == AdminRule::PlanReplayerStmtAlt10,
                Remove: rule == AdminRule::PlanReplayerStmtAlt11,
                SQLDigest: rhs[rhs_len - (1)].ident.clone(),
                PlanDigest: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }))
        }
        AdminRule::PlanReplayerDumpOptAlt01 => out.item = None,
        AdminRule::PlanReplayerDumpOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::TrafficStmtAlt01
        | AdminRule::TrafficStmtAlt02
        | AdminRule::TrafficStmtAlt03
        | AdminRule::TrafficStmtAlt04 => {
            let op_type = match rule {
                AdminRule::TrafficStmtAlt01 => parser_ast::TrafficOpType::Capture,
                AdminRule::TrafficStmtAlt02 => parser_ast::TrafficOpType::Replay,
                AdminRule::TrafficStmtAlt03 => parser_ast::TrafficOpType::Show,
                _ => parser_ast::TrafficOpType::Cancel,
            };
            let (dir, options) = if rule <= AdminRule::TrafficStmtAlt02 {
                (
                    rhs[rhs_len - (1)].ident.clone(),
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::TrafficOption>>())
                        .cloned()
                        .unwrap_or_default(),
                )
            } else {
                (String::new(), Vec::new())
            };
            out.statement = Some(Box::new(parser_ast::TrafficStmt {
                node_text: Default::default(),
                OpType: op_type,
                Dir: dir,
                Options: options,
            }));
        }
        AdminRule::TrafficCaptureOptListAlt01 | AdminRule::TrafficReplayOptListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TrafficOption>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        AdminRule::TrafficCaptureOptListAlt02 | AdminRule::TrafficReplayOptListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TrafficOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TrafficOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::TrafficCaptureOptAlt01 => {
            let value = rhs[rhs_len - (0)].ident.clone();
            if let Err(error) = validate_go_duration(&value) {
                yylex.AppendError(yylex.Errorf(
                    &format!("The DURATION option is not a valid duration: {error}"),
                    &[],
                ));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::TrafficOption {
                OptionType: parser_ast::TrafficOptionType::Duration,
                StrValue: value,
                ..Default::default()
            }));
        }
        AdminRule::TrafficCaptureOptAlt02
        | AdminRule::TrafficReplayOptAlt01
        | AdminRule::TrafficReplayOptAlt02 => {
            out.item = Some(Box::new(parser_ast::TrafficOption {
                OptionType: match rule {
                    AdminRule::TrafficCaptureOptAlt02 => {
                        parser_ast::TrafficOptionType::EncryptionMethod
                    }
                    AdminRule::TrafficReplayOptAlt01 => parser_ast::TrafficOptionType::Username,
                    _ => parser_ast::TrafficOptionType::Password,
                },
                StrValue: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }))
        }
        AdminRule::TrafficCaptureOptAlt03 | AdminRule::TrafficReplayOptAlt04 => {
            out.item = Some(Box::new(parser_ast::TrafficOption {
                OptionType: if rule == AdminRule::TrafficCaptureOptAlt03 {
                    parser_ast::TrafficOptionType::Compress
                } else {
                    parser_ast::TrafficOptionType::ReadOnly
                },
                BoolValue: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        AdminRule::TrafficReplayOptAlt03 => {
            out.item = Some(Box::new(parser_ast::TrafficOption {
                OptionType: parser_ast::TrafficOptionType::Speed,
                FloatValue: Some(parser_ast::ExprNode::Value(
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .map(semantic_value_text)
                        .unwrap_or_default(),
                )),
                ..Default::default()
            }))
        }
        AdminRule::OptSpPdparamsAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::StoreParameter>::new()))
        }
        AdminRule::OptSpPdparamsAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::RefreshStatsStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::RefreshStatsStmt {
                node_text: Default::default(),
                RefreshObjects: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::StatsObject>>())
                    .cloned()
                    .unwrap_or_default(),
                RefreshMode: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::RefreshStatsMode>())
                    .copied(),
                IsClusterWide: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
            }))
        }
        AdminRule::StatsObjectListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::StatsObject>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        AdminRule::StatsObjectListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::StatsObject>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::StatsObject>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        AdminRule::RefreshStatsModeOptAlt01 => out.item = None,
        AdminRule::RefreshStatsModeOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        AdminRule::RefreshStatsModeAlt01 | AdminRule::RefreshStatsModeAlt02 => {
            out.item = Some(Box::new(if rule == AdminRule::RefreshStatsModeAlt01 {
                parser_ast::RefreshStatsMode::Full
            } else {
                parser_ast::RefreshStatsMode::Lite
            }))
        }
        AdminRule::RefreshStatsClusterOptAlt01 => out.item = Some(Box::new(false)),
        AdminRule::RefreshStatsClusterOptAlt02 => out.item = Some(Box::new(true)),
        AdminRule::StatsObjectAlt01
        | AdminRule::StatsObjectAlt02
        | AdminRule::StatsObjectAlt03
        | AdminRule::StatsObjectAlt04 => {
            let mut object = parser_ast::StatsObject::default();
            match rule {
                AdminRule::StatsObjectAlt01 => {
                    object.StatsObjectScope = parser_ast::StatsObjectScope::Global
                }
                AdminRule::StatsObjectAlt02 => {
                    object.StatsObjectScope = parser_ast::StatsObjectScope::Database;
                    object.DBName = parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident);
                }
                AdminRule::StatsObjectAlt03 => {
                    object.StatsObjectScope = parser_ast::StatsObjectScope::Table;
                    object.DBName = parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident);
                    object.TableName = parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident);
                }
                _ => {
                    object.StatsObjectScope = parser_ast::StatsObjectScope::Table;
                    object.TableName = parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident);
                }
            }
            out.item = Some(Box::new(object));
        }
        AdminRule::DropResourceGroupStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropResourceGroupStmt {
                node_text: Default::default(),
                IfExists: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ResourceGroupName: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
            }))
        }
        AdminRule::DropQueryWatchStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropQueryWatchStmt {
                IntValue: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i64>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        AdminRule::DropQueryWatchStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::DropQueryWatchStmt {
                GroupNameStr: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }))
        }
        AdminRule::DropQueryWatchStmtAlt03 => {
            out.statement = Some(Box::new(parser_ast::DropQueryWatchStmt {
                GroupNameExpr: rhs[rhs_len - (0)].expr.clone(),
                ..Default::default()
            }))
        }
    }
    Ok(true)
}
