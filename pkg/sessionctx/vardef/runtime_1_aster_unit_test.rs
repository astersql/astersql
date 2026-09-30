// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// `runtime` 与部分 `sysvar` 常量的 Aster 迁移单元测试。
//
// 校验租约默认值与往返设置、NextGen 只读变量名大小写无关匹配，
// 以及 SET NAMES/CHARSET 相关变量分组顺序与 Go 一致。

use astersql_sessionctx_vardef::*;
use std::time::Duration;

struct LeaseRestore {
    schema: Duration,
    stats: Duration,
    plan_replayer_gc: Duration,
}

impl LeaseRestore {
    fn capture() -> Self {
        Self {
            schema: GetSchemaLease(),
            stats: GetStatsLease(),
            plan_replayer_gc: GetPlanReplayerGCLease(),
        }
    }
}

impl Drop for LeaseRestore {
    fn drop(&mut self) {
        SetSchemaLease(self.schema);
        SetStatsLease(self.stats);
        SetPlanReplayerGCLease(self.plan_replayer_gc);
    }
}

#[test]
/// 验证 Schema/Stats/PlanReplayer 租约默认值，并确认 setter/getter 往返一致。
fn runtime_leases_keep_go_defaults_and_round_trip() {
    // These setters mutate process-global atomics. Restore all three values even
    // if an assertion panics so this test cannot leak state into sibling tests.
    let restore = LeaseRestore::capture();

    assert_eq!(GetSchemaLease(), Duration::from_secs(1));
    assert_eq!(GetStatsLease(), Duration::from_secs(3));
    assert_eq!(GetPlanReplayerGCLease(), Duration::from_secs(10 * 60));

    SetSchemaLease(Duration::from_millis(1250));
    SetStatsLease(Duration::ZERO);
    SetPlanReplayerGCLease(Duration::from_micros(42));

    assert_eq!(GetSchemaLease(), Duration::from_millis(1250));
    assert_eq!(GetStatsLease(), Duration::ZERO);
    assert_eq!(GetPlanReplayerGCLease(), Duration::from_micros(42));

    let original = (restore.schema, restore.stats, restore.plan_replayer_gc);
    drop(restore);
    assert_eq!(GetSchemaLease(), original.0);
    assert_eq!(GetStatsLease(), original.1);
    assert_eq!(GetPlanReplayerGCLease(), original.2);
}

#[test]
fn go_merge_43_plan_replayer_file_retention_tracks_runtime_changes() {
    let original = GetPlanReplayerFileRetentionTime();
    assert_eq!(original, Duration::from_secs(7 * 24 * 60 * 60));
    SetPlanReplayerFileRetentionTime(Duration::from_secs(60));
    assert_eq!(GetPlanReplayerFileRetentionTime(), Duration::from_secs(60));
    SetPlanReplayerFileRetentionTime(original);
}

#[test]
/// 验证 NextGen 只读变量列表及大小写不敏感匹配；无关名称应返回 false。
fn next_gen_read_only_variables_match_go_case_insensitively() {
    for name in [
        TiDBEnableMDL,
        TiDBMaxDistTaskNodes,
        TiDBDDLReorgMaxWriteSpeed,
        TiDBDDLDiskQuota,
        TiDBEnableDistTask,
        TiDBDDLEnableFastReorg,
    ] {
        assert!(IsReadOnlyVarInNextGen(name));
        assert!(IsReadOnlyVarInNextGen(&name.to_ascii_uppercase()));
    }

    assert!(!IsReadOnlyVarInNextGen("abc"));
    assert!(!IsReadOnlyVarInNextGen(" tidb_enable_metadata_lock"));
}

#[test]
/// 验证 SET NAMES/CHARSET 变量数组顺序，以及事务模式与密码掩码常量。
fn set_statement_variable_groups_match_go_order() {
    assert_eq!(
        SetNamesVariables,
        [
            CharacterSetClient,
            CharacterSetConnection,
            CharacterSetResults,
        ]
    );
    assert_eq!(
        SetCharsetVariables,
        [CharacterSetClient, CharacterSetResults]
    );
    assert_eq!(MaskPwd, "******");
    assert_eq!(PessimisticTxnMode, "pessimistic");
    assert_eq!(OptimisticTxnMode, "optimistic");
}
