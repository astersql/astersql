// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DistSQLContext 迁移对齐用的 Aster 单元测试。
//
// 相对 `context_test.rs`，本测试额外覆盖更多共享字段（ExecDetails、
// RunawayChecker、RUConsumptionReporter 等），并校验 Detach 后
// 指针共享/独立拷贝规则与 Go 行为一致。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::contextutil::WarnHandler;
use super::*;

/// Go `int` fields follow the target pointer width and must not be narrowed to `i32`.
#[test]
fn go_int_configuration_fields_use_pointer_width() {
    let value: isize = 1;
    let context = DistSQLContext {
        DistSQLConcurrency: value,
        MinPagingSize: value,
        MaxPagingSize: value,
        PagingSizeBytes: value,
        StoreBatchSize: value,
        ..DistSQLContext::default()
    };
    assert_eq!(context.DistSQLConcurrency, value);
    assert_eq!(context.MinPagingSize, value);
    assert_eq!(context.MaxPagingSize, value);
    assert_eq!(context.PagingSizeBytes, value);
    assert_eq!(context.StoreBatchSize, value);
}

/// 校验 AppendWarning 生效，且 Detach 的指针共享规则对齐 Go。
#[test]
fn append_warning_and_detach_match_go_pointer_rules() {
    // 构造共享依赖与独立 CPU 用量初值，便于后续对比 Detach 拷贝语义。
    let warn_handler = Arc::new(contextutil::NewStaticWarnHandler(5));
    let warn_appender: contextutil::WarnAppenderRef = warn_handler.clone();
    let sql_killer = sqlkiller::SQLKiller::new();
    let cpu_usage = Arc::new(ppcpuusage::SQLCPUUsages::default());
    cpu_usage.SetCPUUsages(ppcpuusage::CPUUsages {
        TidbCPUTime: Duration::from_secs(2),
        TikvCPUTime: Duration::from_secs(3),
    });
    let max_keys_read_counter = Arc::new(AtomicU64::new(17));
    let shared_query_limiter = kv::NewQueryCopStoreLimiter(3).unwrap();
    let shared_mem_tracker: Arc<memory::Tracker> = Arc::from(memory::NewTracker(42, -1));
    let shared_runtime_stats = Arc::new(execdetails::RuntimeStatsColl::default());
    let shared_exec_details = Arc::new(execdetails::SyncExecDetails::default());
    let shared_opaque: SharedContextValue = Arc::new("shared statement value".to_owned());

    // 填充完整 DistSQLContext，含 RunawayChecker / RUConsumptionReporter / ExecDetails。
    let mut context = DistSQLContext {
        WarnHandler: warn_appender.clone(),
        InRestrictedSQL: true,
        EnabledRateLimitAction: true,
        EnableChunkRPC: true,
        OriginalSQL: "select 1".to_owned(),
        KVVars: Some(tikvstore::Variables {
            BackoffLockFast: 1,
            BackOffWeight: 2,
            Killed: &sql_killer.Signal,
        }),
        KvExecCounter: Some(shared_opaque.clone()),
        QueryCopStoreLimiter: Some(shared_query_limiter.clone()),
        SessionMemTracker: Some(shared_mem_tracker.clone()),
        Location: Some(Arc::new(chrono_tz::UTC)),
        RuntimeStatsColl: Some(shared_runtime_stats.clone()),
        SQLKiller: Some(&sql_killer),
        CPUUsage: Some(cpu_usage.clone()),
        ErrCtx: errctx::NewContextWithLevels(
            [errctx::Level::LevelWarn; errctx::errGroupCount],
            warn_appender,
        ),
        TiFlashReplicaRead: tiflash::ClosestAdaptive,
        TiFlashMaxThreads: 1,
        TiFlashMaxBytesBeforeExternalJoin: 2,
        TiFlashMaxBytesBeforeExternalGroupBy: 3,
        TiFlashMaxBytesBeforeExternalSort: 4,
        TiFlashMaxQueryMemoryPerNode: 5,
        TiFlashQuerySpillRatio: 0.5,
        TiFlashHashJoinVersion: "legacy".to_owned(),
        DistSQLConcurrency: 6,
        ReplicaReadType: kv::ReplicaReadType::ReplicaReadFollower,
        WeakConsistency: true,
        RCCheckTS: true,
        NotFillCache: true,
        TaskID: 7,
        Priority: mysql::HighPriority,
        EnablePaging: true,
        MinPagingSize: 8,
        MaxPagingSize: 9,
        PagingSizeBytes: 10,
        RequestSourceType: "internal".to_owned(),
        ExplicitRequestSourceType: "ddl".to_owned(),
        StoreBatchSize: 11,
        ResourceGroupName: "default".to_owned(),
        LoadBasedReplicaReadThreshold: Duration::from_secs(12),
        RunawayChecker: Some(shared_opaque.clone()),
        RUConsumptionReporter: Some(shared_opaque.clone()),
        TiKVClientReadTimeout: 13,
        MaxExecutionTime: 14,
        MaxKeysRead: 15,
        MaxKeysReadCounter: Some(max_keys_read_counter.clone()),
        ReplicaClosestReadThreshold: 16,
        ConnectionID: 17,
        SessionAlias: "session".to_owned(),
        ExecDetails: Some(shared_exec_details.clone()),
        TryCopLiteWorker: std::sync::atomic::AtomicU32::new(1),
        ..DistSQLContext::default()
    };

    context.AppendWarning(errors::New("test warning"));
    assert_eq!(warn_handler.WarningCount(), 1);

    let detached = context.Detach();

    // WarnHandler / SQLKiller 共享；CPUUsage 独立拷贝但初值相同。
    assert!(Arc::ptr_eq(&context.WarnHandler, &detached.WarnHandler));
    assert!(std::ptr::eq(
        context.SQLKiller.expect("original SQL killer"),
        detached.SQLKiller.expect("detached SQL killer"),
    ));
    assert!(!Arc::ptr_eq(
        context.CPUUsage.as_ref().expect("original CPU usage"),
        detached.CPUUsage.as_ref().expect("detached CPU usage"),
    ));
    assert_eq!(
        context.CPUUsage.as_ref().unwrap().GetCPUUsages(),
        detached.CPUUsage.as_ref().unwrap().GetCPUUsages(),
    );

    // KVVars 字段按值拷贝，但 Killed 仍指向 Detach 后 SQLKiller 的 Signal。
    let original_vars = context.KVVars.as_ref().expect("original KV vars");
    let detached_vars = detached.KVVars.as_ref().expect("detached KV vars");
    assert_eq!(detached_vars.BackoffLockFast, original_vars.BackoffLockFast);
    assert_eq!(detached_vars.BackOffWeight, original_vars.BackOffWeight);
    assert!(std::ptr::eq(
        detached_vars.Killed,
        &detached.SQLKiller.expect("detached SQL killer").Signal,
    ));

    // 修改原上下文不应污染独立拷贝的 CPUUsage / Backoff；Killed 信号仍联动。
    context
        .CPUUsage
        .as_ref()
        .unwrap()
        .SetCPUUsages(ppcpuusage::CPUUsages::default());
    assert_eq!(
        detached.CPUUsage.as_ref().unwrap().GetCPUUsages(),
        ppcpuusage::CPUUsages {
            TidbCPUTime: Duration::from_secs(2),
            TikvCPUTime: Duration::from_secs(3),
        },
    );
    context.KVVars.as_mut().unwrap().BackoffLockFast = 99;
    assert_eq!(detached.KVVars.as_ref().unwrap().BackoffLockFast, 1);
    sql_killer.Signal.store(7, Ordering::Relaxed);
    assert_eq!(
        detached
            .KVVars
            .as_ref()
            .unwrap()
            .Killed
            .load(Ordering::Relaxed),
        7
    );

    // MaxKeysReadCounter 独立重建并清零；原计数器保持 17。
    assert_eq!(max_keys_read_counter.load(Ordering::Relaxed), 17);
    let detached_counter = detached
        .MaxKeysReadCounter
        .as_ref()
        .expect("detached max keys read counter");
    assert!(!Arc::ptr_eq(&max_keys_read_counter, detached_counter));
    assert_eq!(detached_counter.load(Ordering::Relaxed), 0);

    // 会话/语句级 Arc 字段保持共享（RU、KV 计数、内存、时区、运行时统计、执行细节等）。
    assert!(Arc::ptr_eq(
        context.QueryCopStoreLimiter.as_ref().unwrap(),
        detached.QueryCopStoreLimiter.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.KvExecCounter.as_ref().unwrap(),
        detached.KvExecCounter.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.SessionMemTracker.as_ref().unwrap(),
        detached.SessionMemTracker.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.Location.as_ref().unwrap(),
        detached.Location.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.RuntimeStatsColl.as_ref().unwrap(),
        detached.RuntimeStatsColl.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.ExecDetails.as_ref().unwrap(),
        detached.ExecDetails.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.RunawayChecker.as_ref().unwrap(),
        detached.RunawayChecker.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        context.RUConsumptionReporter.as_ref().unwrap(),
        detached.RUConsumptionReporter.as_ref().unwrap(),
    ));

    // 标量配置按值完整保留。
    assert!(detached.InRestrictedSQL);
    assert!(detached.EnabledRateLimitAction);
    assert!(detached.EnableChunkRPC);
    assert_eq!(detached.OriginalSQL, "select 1");
    assert_eq!(
        detached.ErrCtx.LevelMap(),
        [errctx::Level::LevelWarn; errctx::errGroupCount],
    );
    assert_eq!(detached.TiFlashReplicaRead, tiflash::ClosestAdaptive);
    assert_eq!(detached.TiFlashMaxThreads, 1);
    assert_eq!(detached.TiFlashMaxBytesBeforeExternalJoin, 2);
    assert_eq!(detached.TiFlashMaxBytesBeforeExternalGroupBy, 3);
    assert_eq!(detached.TiFlashMaxBytesBeforeExternalSort, 4);
    assert_eq!(detached.TiFlashMaxQueryMemoryPerNode, 5);
    assert_eq!(detached.TiFlashQuerySpillRatio, 0.5);
    assert_eq!(detached.TiFlashHashJoinVersion, "legacy");
    assert_eq!(detached.DistSQLConcurrency, 6);
    assert_eq!(
        detached.ReplicaReadType,
        kv::ReplicaReadType::ReplicaReadFollower,
    );
    assert!(detached.WeakConsistency);
    assert!(detached.RCCheckTS);
    assert!(detached.NotFillCache);
    assert_eq!(detached.TaskID, 7);
    assert_eq!(detached.Priority, mysql::HighPriority);
    assert!(detached.EnablePaging);
    assert_eq!(detached.MinPagingSize, 8);
    assert_eq!(detached.MaxPagingSize, 9);
    assert_eq!(detached.PagingSizeBytes, 10);
    assert_eq!(detached.RequestSourceType, "internal");
    assert_eq!(detached.ExplicitRequestSourceType, "ddl");
    assert_eq!(detached.StoreBatchSize, 11);
    assert_eq!(detached.ResourceGroupName, "default");
    assert_eq!(
        detached.LoadBasedReplicaReadThreshold,
        Duration::from_secs(12),
    );
    assert_eq!(detached.TiKVClientReadTimeout, 13);
    assert_eq!(detached.MaxExecutionTime, 14);
    assert_eq!(detached.MaxKeysRead, 15);
    assert_eq!(detached.ReplicaClosestReadThreshold, 16);
    assert_eq!(detached.ConnectionID, 17);
    assert_eq!(detached.SessionAlias, "session");
    assert_eq!(detached.TryCopLiteWorker.load(Ordering::Relaxed), 1);
}
