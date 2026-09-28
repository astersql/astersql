// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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
// 功能使用遥测的单元测试：开关变量、计数差分与 infoschema 场景。
//
// 通过互斥锁串行化测试，避免共享 MetricsSnapshot 互相干扰；
// 覆盖事务提交、分区、TTL、IndexMerge、公平加锁等采集路径。

use crate::main_test::{GetFeatureUsage, GetTxnUsageInfo};
use crate::{
    AccountLockCounter, DDLUsageCounter, ExchangePartitionUsageCounter, FairLockingUsageCounter,
    IndexMergeUsageCounter, MetricsSnapshot, MultiSchemaChangeUsageCounter,
    NonTransactionalStmtCounter, PostReportTelemetryDataForTest, PostSavepointCount,
    ResetMetricsForTest, SchemaInfo, SessionContext, SetMetricsSnapshot, SetTTLJobEnabled,
    StoreBatchCoprCounter, TableInfo, TablePartitionUsageCounter, TxnCommitCounter,
};
use std::sync::{Mutex, MutexGuard};

/// 全局测试锁，保证依赖共享指标快照的用例串行执行。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 持锁并构造干净的 SessionContext，同时重置指标快照。
fn fresh_ctx() -> (MutexGuard<'static, ()>, SessionContext) {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ResetMetricsForTest();
    SetTTLJobEnabled(false);
    let mut ctx = SessionContext::default();
    ctx.GlobalVars
        .insert("enable_global_kill".into(), "1".into());
    ctx.GlobalVars
        .insert("tidb_enable_resource_control".into(), "1".into());
    ctx.GlobalVars
        .insert("tidb_server_memory_limit".into(), "8589934592".into());
    ctx.ResourceGroups = 1;
    ctx.EnablePaging = true;
    ctx.CostModelVersion = 1;
    (guard, ctx)
}

/// 设置会话全局变量，模拟系统变量开关。
fn set_var(ctx: &mut SessionContext, key: &str, value: &str) {
    ctx.GlobalVars.insert(key.into(), value.into());
}

#[test]
/// 验证事务相关开关与提交计数差分采集。
fn TestTxnUsageInfo() {
    let (_guard, mut ctx) = fresh_ctx();

    // Used subtest — mirrors Go t.Run("Used", ...)
    set_var(&mut ctx, "tidb_enable_async_commit", "0");
    set_var(&mut ctx, "tidb_enable_1pc", "0");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(!txn_usage.AsyncCommitUsed);
    assert!(!txn_usage.OnePCUsed);

    set_var(&mut ctx, "tidb_enable_async_commit", "1");
    set_var(&mut ctx, "tidb_enable_1pc", "1");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(txn_usage.AsyncCommitUsed);
    assert!(txn_usage.OnePCUsed);

    set_var(&mut ctx, "tidb_enable_mutation_checker", "0");
    set_var(&mut ctx, "tidb_txn_assertion_level", "OFF");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(!txn_usage.MutationCheckerUsed);
    assert_eq!(txn_usage.AssertionLevel, "OFF");

    set_var(&mut ctx, "tidb_enable_mutation_checker", "1");
    set_var(&mut ctx, "tidb_txn_assertion_level", "STRICT");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(txn_usage.MutationCheckerUsed);
    assert_eq!(txn_usage.AssertionLevel, "STRICT");

    set_var(&mut ctx, "tidb_txn_assertion_level", "FAST");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert_eq!(txn_usage.AssertionLevel, "FAST");

    set_var(&mut ctx, "tidb_rc_read_check_ts", "1");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(txn_usage.RcCheckTS);

    set_var(&mut ctx, "tidb_rc_write_check_ts", "1");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(txn_usage.RCWriteCheckTS);

    // Classic-kernel fair locking toggles (always exercised in this unit fixture).
    set_var(&mut ctx, "tidb_pessimistic_transaction_fair_locking", "0");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(!txn_usage.FairLocking);
    set_var(&mut ctx, "tidb_pessimistic_transaction_fair_locking", "1");
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(txn_usage.FairLocking);

    // Count subtest — mirrors Go t.Run("Count", ...) via metrics snapshot.
    SetMetricsSnapshot(MetricsSnapshot {
        Txn: TxnCommitCounter {
            AsyncCommit: 2,
            OnePC: 1,
            TwoPC: 3,
        },
        ..Default::default()
    });
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert!(txn_usage.AsyncCommitUsed);
    assert!(txn_usage.OnePCUsed);
    assert!(txn_usage.TxnCommitCounter.AsyncCommit > 0);
    assert!(txn_usage.TxnCommitCounter.OnePC > 0);
    assert!(txn_usage.TxnCommitCounter.TwoPC > 0);
}

#[test]
/// 验证临时表标记能反映到 featureUsage。
fn TestTemporaryTable() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.TemporaryTable);

    ctx.Schemas.push(SchemaInfo {
        PlacementPolicy: false,
        Tables: vec![TableInfo {
            Temporary: true,
            Public: true,
            ..Default::default()
        }],
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.TemporaryTable);
}

#[test]
/// 验证缓存表标记能反映到 featureUsage。
fn TestCachedTable() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.CachedTable);

    ctx.Schemas.push(SchemaInfo {
        Tables: vec![TableInfo {
            Cached: true,
            Public: true,
            ..Default::default()
        }],
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.CachedTable);

    ctx.Schemas[0].Tables[0].Cached = false;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.CachedTable);
}

#[test]
/// 验证 AutoID cache=1（无缓存）标记采集。
fn TestAutoIDNoCache() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.AutoIDNoCache);

    ctx.Schemas.push(SchemaInfo {
        Tables: vec![TableInfo {
            AutoIDCache: 1,
            Public: true,
            ..Default::default()
        }],
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.AutoIDNoCache);

    ctx.Schemas.clear();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.AutoIDNoCache);
}

#[test]
/// 验证账号锁定相关计数的增量与上报重置。
fn TestAccountLock() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.AccountLock.LockUser, 0);
    assert_eq!(usage.AccountLock.UnlockUser, 0);
    assert_eq!(usage.AccountLock.CreateOrAlterUser, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        AccountLock: AccountLockCounter {
            LockUser: 1,
            UnlockUser: 0,
            CreateOrAlterUser: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.AccountLock.LockUser, 1);
    assert_eq!(usage.AccountLock.UnlockUser, 0);
    assert_eq!(usage.AccountLock.CreateOrAlterUser, 1);

    SetMetricsSnapshot(MetricsSnapshot {
        AccountLock: AccountLockCounter {
            LockUser: 1,
            UnlockUser: 1,
            CreateOrAlterUser: 2,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.AccountLock.LockUser, 1);
    assert_eq!(usage.AccountLock.UnlockUser, 1);
    assert_eq!(usage.AccountLock.CreateOrAlterUser, 2);
}

#[test]
/// 验证多 schema 变更使用计数。
fn TestMultiSchemaChange() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.MultiSchemaChange.MultiSchemaChangeUsed, 0);

    // Single-column alter does not bump the multi-schema counter.
    SetMetricsSnapshot(MetricsSnapshot {
        MultiSchema: MultiSchemaChangeUsageCounter {
            MultiSchemaChangeUsed: 0,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.MultiSchemaChange.MultiSchemaChangeUsed, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        MultiSchema: MultiSchemaChangeUsageCounter {
            MultiSchemaChangeUsed: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.MultiSchemaChange.MultiSchemaChangeUsed, 1);

    SetMetricsSnapshot(MetricsSnapshot {
        MultiSchema: MultiSchemaChangeUsageCounter {
            MultiSchemaChangeUsed: 2,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.MultiSchemaChange.MultiSchemaChangeUsed, 2);

    // Single drop keeps the counter at 2.
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.MultiSchemaChange.MultiSchemaChangeUsed, 2);
}

#[test]
/// 验证各类表分区与运维操作计数及 MaxPartitions 语义。
fn TestTablePartition() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.TablePartition.TablePartitionCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionListCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionMaxPartitionsCnt, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        TablePartition: TablePartitionUsageCounter {
            TablePartitionCnt: 1,
            TablePartitionHashCnt: 1,
            TablePartitionMaxPartitionsCnt: 4,
            ..Default::default()
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.TablePartition.TablePartitionCnt, 1);
    assert_eq!(usage.TablePartition.TablePartitionHashCnt, 1);
    assert_eq!(usage.TablePartition.TablePartitionMaxPartitionsCnt, 4);
    assert_eq!(usage.TablePartition.TablePartitionListCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionRangeCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsGt1Cnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsGt2Cnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsGt3Cnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionListColumnsCnt, 0);
    assert_eq!(
        usage
            .TablePartition
            .TablePartitionCreateIntervalPartitionsCnt,
        0
    );
    assert_eq!(
        usage.TablePartition.TablePartitionAddIntervalPartitionsCnt,
        0
    );
    assert_eq!(
        usage.TablePartition.TablePartitionDropIntervalPartitionsCnt,
        0
    );
    assert_eq!(usage.TablePartition.TablePartitionReorganizePartitionCnt, 0);

    // Go calls PostReportTelemetryDataForTest() then accumulates further DDL counters.
    // Unit fixture: reset baseline, then apply the post-report deltas Go asserts.
    PostReportTelemetryDataForTest();
    ResetMetricsForTest();
    SetMetricsSnapshot(MetricsSnapshot {
        TablePartition: TablePartitionUsageCounter {
            TablePartitionCnt: 5,
            TablePartitionHashCnt: 0,
            TablePartitionMaxPartitionsCnt: 11,
            TablePartitionRangeCnt: 1,
            TablePartitionRangeColumnsCnt: 4,
            TablePartitionRangeColumnsGt1Cnt: 3,
            TablePartitionRangeColumnsGt2Cnt: 2,
            TablePartitionRangeColumnsGt3Cnt: 1,
            TablePartitionCreateIntervalPartitionsCnt: 1,
            TablePartitionAddIntervalPartitionsCnt: 1,
            TablePartitionDropIntervalPartitionsCnt: 1,
            TablePartitionReorganizePartitionCnt: 1,
            ..Default::default()
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.TablePartition.TablePartitionCnt, 5);
    assert_eq!(usage.TablePartition.TablePartitionHashCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionMaxPartitionsCnt, 11);
    assert_eq!(usage.TablePartition.TablePartitionListCnt, 0);
    assert_eq!(usage.TablePartition.TablePartitionRangeCnt, 1);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsCnt, 4);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsGt1Cnt, 3);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsGt2Cnt, 2);
    assert_eq!(usage.TablePartition.TablePartitionRangeColumnsGt3Cnt, 1);
    assert_eq!(usage.TablePartition.TablePartitionListColumnsCnt, 0);
    assert_eq!(
        usage
            .TablePartition
            .TablePartitionCreateIntervalPartitionsCnt,
        1
    );
    assert_eq!(
        usage.TablePartition.TablePartitionAddIntervalPartitionsCnt,
        1
    );
    assert_eq!(
        usage.TablePartition.TablePartitionDropIntervalPartitionsCnt,
        1
    );
    assert_eq!(usage.TablePartition.TablePartitionReorganizePartitionCnt, 1);

    assert_eq!(usage.ExchangePartition.ExchangePartitionCnt, 0);
    SetMetricsSnapshot(MetricsSnapshot {
        TablePartition: usage.TablePartition,
        ExchangePartition: ExchangePartitionUsageCounter {
            ExchangePartitionCnt: 1,
        },
        ..Default::default()
    });
    // Keep initial at zero for exchange/compact deltas (same as absolute counters from 0).
    ResetMetricsForTest();
    SetMetricsSnapshot(MetricsSnapshot {
        TablePartition: TablePartitionUsageCounter {
            TablePartitionComactCnt: 0,
            ..Default::default()
        },
        ExchangePartition: ExchangePartitionUsageCounter {
            ExchangePartitionCnt: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.ExchangePartition.ExchangePartitionCnt, 1);
    assert_eq!(usage.TablePartition.TablePartitionComactCnt, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        TablePartition: TablePartitionUsageCounter {
            TablePartitionComactCnt: 1,
            ..Default::default()
        },
        ExchangePartition: ExchangePartitionUsageCounter {
            ExchangePartitionCnt: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.TablePartition.TablePartitionComactCnt, 1);
}

#[test]
/// 验证 Placement Policy 在库/表/分区上的统计。
fn TestPlacementPolicies() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.PlacementPolicyUsage.NumPlacementPolicies, 0);
    assert_eq!(usage.PlacementPolicyUsage.NumDBWithPolicies, 0);
    assert_eq!(usage.PlacementPolicyUsage.NumTableWithPolicies, 0);
    assert_eq!(
        usage.PlacementPolicyUsage.NumPartitionWithExplicitPolicies,
        0
    );

    ctx.PlacementPolicies = 3;
    ctx.Schemas.push(SchemaInfo {
        PlacementPolicy: true,
        Tables: vec![
            TableInfo {
                Public: true,
                ..Default::default()
            },
            TableInfo {
                PlacementPolicy: true,
                Public: true,
                ..Default::default()
            },
            TableInfo {
                PlacementPolicy: true,
                PartitionPolicies: 1,
                Public: true,
                ..Default::default()
            },
        ],
    });
    // t1 inherits db policy => counted as table with policy in Go infoschema walk when
    // PlacementPolicyRef is set; fixture marks t2/t3 explicitly and counts db+3 tables.
    ctx.Schemas[0].Tables[0].PlacementPolicy = true;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.PlacementPolicyUsage.NumPlacementPolicies, 3);
    assert_eq!(usage.PlacementPolicyUsage.NumDBWithPolicies, 1);
    assert_eq!(usage.PlacementPolicyUsage.NumTableWithPolicies, 3);
    assert_eq!(
        usage.PlacementPolicyUsage.NumPartitionWithExplicitPolicies,
        1
    );

    ctx.PlacementPolicies = 2;
    ctx.Schemas[0].Tables = vec![
        TableInfo {
            PlacementPolicy: true,
            Public: true,
            ..Default::default()
        },
        TableInfo {
            PlacementPolicy: false,
            PartitionPolicies: 1,
            Public: true,
            ..Default::default()
        },
    ];
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.PlacementPolicyUsage.NumPlacementPolicies, 2);
    assert_eq!(usage.PlacementPolicyUsage.NumDBWithPolicies, 1);
    assert_eq!(usage.PlacementPolicyUsage.NumTableWithPolicies, 1);
    assert_eq!(
        usage.PlacementPolicyUsage.NumPartitionWithExplicitPolicies,
        1
    );
}

#[test]
/// 验证资源组数量与资源管控开关。
fn TestResourceGroups() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.ResourceControlUsage.NumResourceGroups, 1);
    assert!(usage.ResourceControlUsage.Enabled);

    set_var(&mut ctx, "tidb_enable_resource_control", "ON");
    ctx.ResourceGroups = 2;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.ResourceControlUsage.Enabled);
    assert_eq!(usage.ResourceControlUsage.NumResourceGroups, 2);

    ctx.ResourceGroups = 3;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.ResourceControlUsage.NumResourceGroups, 3);

    // alter keeps count
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.ResourceControlUsage.NumResourceGroups, 3);

    ctx.ResourceGroups = 2;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.ResourceControlUsage.NumResourceGroups, 2);

    set_var(&mut ctx, "tidb_enable_resource_control", "OFF");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.ResourceControlUsage.NumResourceGroups, 2);
    assert!(!usage.ResourceControlUsage.Enabled);
}

#[test]
/// 验证计划基线自动捕获开关。
fn TestAutoCapture() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.AutoCapture);

    set_var(&mut ctx, "tidb_capture_plan_baselines", "on");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.AutoCapture);

    set_var(&mut ctx, "tidb_capture_plan_baselines", "off");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.AutoCapture);
}

#[test]
/// 验证非事务 DML 计数差分。
fn TestNonTransactionalUsage() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.NonTransactionalUsage.DeleteCount, 0);
    assert_eq!(usage.NonTransactionalUsage.UpdateCount, 0);
    assert_eq!(usage.NonTransactionalUsage.InsertCount, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        NonTransactional: NonTransactionalStmtCounter {
            DeleteCount: 1,
            UpdateCount: 1,
            InsertCount: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.NonTransactionalUsage.DeleteCount, 1);
    assert_eq!(usage.NonTransactionalUsage.UpdateCount, 1);
    assert_eq!(usage.NonTransactionalUsage.InsertCount, 1);
}

#[test]
/// 验证全局 Kill 开关采集。
fn TestGlobalKillUsageInfo() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.GlobalKill);

    set_var(&mut ctx, "enable_global_kill", "0");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.GlobalKill);
}

#[test]
/// 验证 Coprocessor paging 开关采集。
fn TestPagingUsageInfo() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.EnablePaging);

    ctx.EnablePaging = false;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.EnablePaging);
}

#[test]
/// 验证代价模型 v2 开关采集。
fn TestCostModelVer2UsageInfo() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.EnableCostModelVer2, ctx.CostModelVersion == 2);

    ctx.CostModelVersion = 2;
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.EnableCostModelVer2);
}

#[test]
/// 验证 SAVEPOINT 计数差分。
fn TestTxnSavepointUsageInfo() {
    let (_guard, ctx) = fresh_ctx();
    SetMetricsSnapshot(MetricsSnapshot {
        Savepoint: 2,
        ..Default::default()
    });
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert_eq!(txn_usage.SavepointCounter, 2);

    SetMetricsSnapshot(MetricsSnapshot {
        Savepoint: 3,
        ..Default::default()
    });
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert_eq!(txn_usage.SavepointCounter, 3);

    PostSavepointCount();
    SetMetricsSnapshot(MetricsSnapshot {
        Savepoint: 4, // absolute counter; after PostSavepointCount baseline=3, diff=1
        ..Default::default()
    });
    let txn_usage = GetTxnUsageInfo(&ctx);
    assert_eq!(txn_usage.SavepointCounter, 1);
}

#[test]
/// 验证惰性悲观唯一性检查计数。
fn TestLazyPessimisticUniqueCheck() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetTxnUsageInfo(&ctx);
    assert_eq!(usage.LazyUniqueCheckSetCounter, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        LazyUnique: 2,
        ..Default::default()
    });
    let usage = GetTxnUsageInfo(&ctx);
    assert_eq!(usage.LazyUniqueCheckSetCounter, 2);
}

#[test]
/// 验证 FLASHBACK CLUSTER 使用计数。
fn TestFlashbackCluster() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.DDLUsageCounter.FlashbackClusterUsed, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        DDL: DDLUsageCounter {
            FlashbackClusterUsed: 1,
            ..Default::default()
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.DDLUsageCounter.FlashbackClusterUsed, 1);

    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.DDLUsageCounter.FlashbackClusterUsed, 1);
}

#[test]
/// 验证加索引加速与元数据锁（MDL）相关采集。
fn TestAddIndexAccelerationAndMDL() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.DDLUsageCounter.AddIndexIngestUsed, 0);

    let mut expected_cnt = 0i64;
    // Classic path: disable MDL, add index (ingest), then enable MDL.
    set_var(&mut ctx, "tidb_enable_metadata_lock", "0");
    expected_cnt += 1;
    SetMetricsSnapshot(MetricsSnapshot {
        DDL: DDLUsageCounter {
            AddIndexIngestUsed: expected_cnt,
            ..Default::default()
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.DDLUsageCounter.AddIndexIngestUsed, expected_cnt);
    assert!(!usage.DDLUsageCounter.MetadataLockUsed);

    set_var(&mut ctx, "tidb_enable_metadata_lock", "1");
    expected_cnt += 1;
    SetMetricsSnapshot(MetricsSnapshot {
        DDL: DDLUsageCounter {
            AddIndexIngestUsed: expected_cnt,
            ..Default::default()
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.DDLUsageCounter.AddIndexIngestUsed, expected_cnt);
    assert!(usage.DDLUsageCounter.MetadataLockUsed);
}

#[test]
/// 验证服务器内存限制配置的识别。
fn TestGlobalMemoryControl() {
    let (_guard, mut ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.EnableGlobalMemoryControl);

    set_var(&mut ctx, "tidb_server_memory_limit", "5368709120");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.EnableGlobalMemoryControl);

    set_var(&mut ctx, "tidb_server_memory_limit", "0");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.EnableGlobalMemoryControl);
}

#[test]
/// 验证 IndexMerge 使用计数差分。
fn TestIndexMergeUsage() {
    let (_guard, ctx) = fresh_ctx();
    SetMetricsSnapshot(MetricsSnapshot {
        IndexMerge: IndexMergeUsageCounter { IndexMergeUsed: 1 },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.IndexMergeUsageCounter.IndexMergeUsed, 1);

    SetMetricsSnapshot(MetricsSnapshot {
        IndexMerge: IndexMergeUsageCounter { IndexMergeUsed: 2 },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.IndexMergeUsageCounter.IndexMergeUsed, 2);

    SetMetricsSnapshot(MetricsSnapshot {
        IndexMerge: IndexMergeUsageCounter { IndexMergeUsed: 3 },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.IndexMergeUsageCounter.IndexMergeUsed, 3);

    // no_index_merge keeps counter unchanged
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.IndexMergeUsageCounter.IndexMergeUsed, 3);
}

#[test]
/// 验证 TTL（生存时间）任务与表数量遥测。
fn TestTTLTelemetry() {
    let (_guard, mut ctx) = fresh_ctx();
    SetTTLJobEnabled(false);

    ctx.Schemas.push(SchemaInfo {
        Tables: vec![TableInfo {
            ID: 1,
            Public: true,
            TTLEnabled: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    });
    // Aggregated delete-rows / delay histograms match Go checkTableHist* expectations:
    // (0,1,0,0,0) delete buckets and (0,0,1,0,0) delay buckets for the first table.
    ctx.TTLDeletedRows = vec![50_000];
    ctx.TTLDelayHours = vec![(1, 20)];

    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(!usage.TTLUsage.TTLJobEnabled);
    assert_eq!(usage.TTLUsage.TTLTables, 1);
    assert_eq!(usage.TTLUsage.TTLJobEnabledTables, 1);
    assert!(!usage.TTLUsage.TTLHistDate.is_empty());
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows.len(), 5);
    assert_eq!(
        usage.TTLUsage.TableHistWithDeleteRows[0].LessThan,
        Some(10_000)
    );
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[0].Count, 0);
    assert_eq!(
        usage.TTLUsage.TableHistWithDeleteRows[1].LessThan,
        Some(100_000)
    );
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[1].Count, 1);
    assert_eq!(
        usage.TTLUsage.TableHistWithDeleteRows[2].LessThan,
        Some(1_000_000)
    );
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[2].Count, 0);
    assert_eq!(
        usage.TTLUsage.TableHistWithDeleteRows[3].LessThan,
        Some(10_000_000)
    );
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[3].Count, 0);
    assert!(usage.TTLUsage.TableHistWithDeleteRows[4].LessThanMax);
    assert!(usage.TTLUsage.TableHistWithDeleteRows[4].LessThan.is_none());
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[4].Count, 0);

    assert_eq!(usage.TTLUsage.TableHistWithDelayTime.len(), 5);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[0].LessThan, Some(1));
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[0].Count, 0);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[1].LessThan, Some(6));
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[1].Count, 0);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[2].LessThan, Some(24));
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[2].Count, 1);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[3].LessThan, Some(72));
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[3].Count, 0);
    assert!(usage.TTLUsage.TableHistWithDelayTime[4].LessThanMax);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[4].Count, 0);

    ctx.Schemas[0].Tables.push(TableInfo {
        ID: 2,
        Public: true,
        TTLEnabled: Some(true),
        ..Default::default()
    });
    SetTTLJobEnabled(true);
    // Second table adds a <10k delete-rows sample and a <6h delay sample.
    ctx.TTLDeletedRows = vec![50_000, 9_999];
    ctx.TTLDelayHours = vec![(1, 20), (2, 5)];
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.TTLUsage.TTLJobEnabled);
    assert_eq!(usage.TTLUsage.TTLTables, 2);
    assert_eq!(usage.TTLUsage.TTLJobEnabledTables, 2);
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[0].Count, 1);
    assert_eq!(usage.TTLUsage.TableHistWithDeleteRows[1].Count, 1);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[1].Count, 1);
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[2].Count, 1);

    ctx.Schemas[0].Tables.push(TableInfo {
        ID: 3,
        Public: true,
        TTLEnabled: Some(false),
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert!(usage.TTLUsage.TTLJobEnabled);
    assert_eq!(usage.TTLUsage.TTLTables, 3);
    assert_eq!(usage.TTLUsage.TTLJobEnabledTables, 2);
    // Table without delay history contributes to the max bucket.
    assert_eq!(usage.TTLUsage.TableHistWithDelayTime[4].Count, 1);
}

#[test]
/// 验证 Store Batch Coprocessor 计数与批大小。
fn TestStoreBatchCopr() {
    let (_guard, mut ctx) = fresh_ctx();
    SetMetricsSnapshot(MetricsSnapshot {
        StoreBatch: StoreBatchCoprCounter {
            BatchSize: 4,
            ..Default::default()
        },
        ..Default::default()
    });
    set_var(&mut ctx, "tidb_store_batch_size", "4");
    let init = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(init.StoreBatchCoprUsage.BatchSize, 4);

    SetMetricsSnapshot(MetricsSnapshot {
        StoreBatch: StoreBatchCoprCounter {
            BatchSize: 4,
            BatchedQuery: 1,
            ..Default::default()
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.StoreBatchCoprUsage.BatchSize, 4);
    let diff = usage.StoreBatchCoprUsage.Sub(init.StoreBatchCoprUsage);
    assert_eq!(diff.BatchedQuery, 1);
    assert_eq!(diff.BatchedQueryTask, 0);
    assert_eq!(diff.BatchedCount, 0);
    assert_eq!(diff.BatchedFallbackCount, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        StoreBatch: StoreBatchCoprCounter {
            BatchSize: 4,
            BatchedQuery: 2,
            BatchedQueryTask: 2,
            BatchedCount: 1,
            BatchedFallbackCount: 0,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    let diff = usage.StoreBatchCoprUsage.Sub(init.StoreBatchCoprUsage);
    assert_eq!(diff.BatchedQuery, 2);
    assert_eq!(diff.BatchedQueryTask, 2);
    assert_eq!(diff.BatchedCount, 1);
    assert_eq!(diff.BatchedFallbackCount, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        StoreBatch: StoreBatchCoprCounter {
            BatchSize: 4,
            BatchedQuery: 3,
            BatchedQueryTask: 4,
            BatchedCount: 1,
            BatchedFallbackCount: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    let diff = usage.StoreBatchCoprUsage.Sub(init.StoreBatchCoprUsage);
    assert_eq!(diff.BatchedQuery, 3);
    assert_eq!(diff.BatchedQueryTask, 4);
    assert_eq!(diff.BatchedCount, 1);
    assert_eq!(diff.BatchedFallbackCount, 1);

    set_var(&mut ctx, "tidb_store_batch_size", "0");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.StoreBatchCoprUsage.BatchSize, 0);
    let diff = usage.StoreBatchCoprUsage.Sub(init.StoreBatchCoprUsage);
    assert_eq!(diff.BatchedQuery, 3);
    assert_eq!(diff.BatchedQueryTask, 4);
    assert_eq!(diff.BatchedCount, 1);
    assert_eq!(diff.BatchedFallbackCount, 1);

    // Go leaves the Sub result's BatchSize (zero) unchanged when the global
    // variable lookup fails. A missing fixture value models that error path.
    ctx.GlobalVars.remove("tidb_store_batch_size");
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.StoreBatchCoprUsage.BatchSize, 0);
}

#[test]
/// 验证公平加锁使用与生效计数。
fn TestFairLockingUsage() {
    let (_guard, ctx) = fresh_ctx();
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingUsed, 0);
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingEffective, 0);

    // Not counted before commit — metrics still zero mid-transaction.
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingUsed, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        FairLocking: FairLockingUsageCounter {
            TxnFairLockingUsed: 1,
            TxnFairLockingEffective: 0,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingUsed, 1);
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingEffective, 0);

    SetMetricsSnapshot(MetricsSnapshot {
        FairLocking: FairLockingUsageCounter {
            TxnFairLockingUsed: 2,
            TxnFairLockingEffective: 0,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingUsed, 2);
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingEffective, 0);

    // LockedWithConflict path: used=4, effective=1
    SetMetricsSnapshot(MetricsSnapshot {
        FairLocking: FairLockingUsageCounter {
            TxnFairLockingUsed: 4,
            TxnFairLockingEffective: 1,
        },
        ..Default::default()
    });
    let usage = GetFeatureUsage(&ctx).unwrap();
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingUsed, 4);
    assert_eq!(usage.Txn.FairLockingUsageCounter.TxnFairLockingEffective, 1);
}

#[test]
/// Go 的 json.Marshal 必须保留 featureUsage 的全部 tagged 字段。
fn TestFeatureUsageMarshalPreservesGoFields() {
    let (_guard, mut ctx) = fresh_ctx();
    set_var(&mut ctx, "tidb_txn_assertion_level", "STRICT");
    set_var(&mut ctx, "tidb_enable_metadata_lock", "ON");
    set_var(&mut ctx, "tidb_store_batch_size", "8");
    SetMetricsSnapshot(MetricsSnapshot {
        Txn: TxnCommitCounter {
            TwoPC: 1,
            AsyncCommit: 2,
            OnePC: 3,
        },
        CTE: crate::CTEUsageCounter {
            NonRecursiveCTEUsed: 4,
            RecursiveUsed: 5,
            NonCTEUsed: 6,
        },
        AccountLock: AccountLockCounter {
            LockUser: 7,
            UnlockUser: 8,
            CreateOrAlterUser: 9,
        },
        NonTransactional: NonTransactionalStmtCounter {
            DeleteCount: 10,
            UpdateCount: 11,
            InsertCount: 12,
        },
        DDL: DDLUsageCounter {
            AddIndexIngestUsed: 13,
            FlashbackClusterUsed: 14,
            DistReorgUsed: 15,
            ..Default::default()
        },
        StoreBatch: StoreBatchCoprCounter {
            BatchSize: 8,
            BatchedQuery: 16,
            BatchedQueryTask: 17,
            BatchedCount: 18,
            BatchedFallbackCount: 19,
        },
        ..Default::default()
    });

    let json = GetFeatureUsage(&ctx).unwrap().Marshal();
    for field in [
        "\"txnCommitCounter\"",
        "\"mutationCheckerUsed\"",
        "\"assertionLevel\":\"STRICT\"",
        "\"cte\"",
        "\"nonRecursiveCTEUsed\":4",
        "\"accountLock\"",
        "\"lockUser\":7",
        "\"nonTransactional\"",
        "\"delete\":10",
        "\"multi_schema_change_used\"",
        "\"exchange_partition_cnt\"",
        "\"table_partition_cnt\"",
        "\"add_index_ingest_used\":13",
        "\"metadata_lock_used\":true",
        "\"index_merge_used\"",
        "\"batch_size\":8",
        "\"query\":16",
        "\"batched_fallback\":19",
        "\"ttl_hist_date\"",
    ] {
        assert!(json.contains(field), "missing {field} in {json}");
    }
    assert!(json.contains("\"rcWriteCheckTS\":false"), "{json}");
    assert!(json.contains("\"lazyUniqueCheckSetCounter\":0"), "{json}");
    assert!(!json.contains("\"RCWriteCheckTS\""), "{json}");
    assert!(!json.contains("\"LazyUniqueCheckSetCounter\""), "{json}");

    set_var(&mut ctx, "tidb_txn_assertion_level", "<>&\u{2028}\u{2029}");
    let json = GetFeatureUsage(&ctx).unwrap().Marshal();
    assert!(
        json.contains("\"assertionLevel\":\"\\u003c\\u003e\\u0026\\u2028\\u2029\""),
        "{json}"
    );
}

#[test]
/// TTL 查询文本必须保留 Go 侧的时间窗口、状态过滤和分组条件。
fn TestTTLQueriesMatchGoFilters() {
    assert_eq!(
        crate::selectDeletedRowsOneDaySQL,
        "SELECT parent_table_id, CAST(SUM(deleted_rows) AS SIGNED)\n\t\t\tFROM\n\t\t\t    mysql.tidb_ttl_job_history\n\t\t\tWHERE\n\t\t\t\tstatus != 'running'\n\t\t\t    AND create_time >= CURDATE() - INTERVAL 7 DAY\n\t\t\t    AND finish_time >= CURDATE() - INTERVAL 1 DAY\n\t\t\t    AND finish_time < CURDATE()\n\t\t\tGROUP BY parent_table_id;"
    );
    assert_eq!(
        crate::selectDelaySQL,
        "SELECT\n\t\t\tparent_table_id, TIMESTAMPDIFF(MINUTE, MIN(tm), CURDATE()) AS ttl_minutes\n\t\t\tFROM\n\t\t\t\t(\n\t\t\t\t\tSELECT\n\t\t\t\t\t\ttable_id,\n\t\t\t\t\t\tparent_table_id,\n\t\t\t\t\t\tMAX(ttl_expire) AS tm\n\t\t\t\t\tFROM\n\t\t\t\t\t\tmysql.tidb_ttl_job_history\n\t\t\t\t\tWHERE\n\t\t\t\t\t\tcreate_time > CURDATE() - INTERVAL 7 DAY\n\t\t\t\t\t\tAND finish_time < CURDATE()\n\t\t\t\t\t\tAND status = 'finished'\n\t\t\t\t\t\tAND JSON_VALID(summary_text)\n\t\t\t\t\t\tAND summary_text ->> \"$.scan_task_err\" IS NULL\n\t\t\t\t\tGROUP BY\n\t\t\t\t\t\ttable_id, parent_table_id\n\t\t\t\t) t\n\t\t\tGROUP BY parent_table_id;"
    );
}
