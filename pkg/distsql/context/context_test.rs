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

// DistSQLContext::Detach 行为的单元测试。
//
// DistSQL（分布式 SQL）上下文在语句结束后可能被“剥离”以便异步收尾：
// 部分字段与会话共享（浅拷贝/共享指针），部分字段需独立副本以免互相干扰。
// 本测试构造一个字段齐全的上下文，验证 Detach 后的共享关系、独立拷贝与标量保留。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use super::contextutil::WarnHandler;
use super::*;

// test_context_detach corresponds to Go's TestContextDetach.
/// 验证 Detach 对共享引用、独立拷贝与标量字段的复制规则与 Go 一致。
#[test]
fn test_context_detach() {
    // 准备会话侧依赖：SQLKiller（可中断查询的信号）、CPU 用量、告警处理器等。
    let sql_killer = sqlkiller::SQLKiller::new();
    sql_killer.Signal.store(1, Ordering::Relaxed);
    let cpu_usage = Arc::new(ppcpuusage::SQLCPUUsages::default());
    let warn_handler = Arc::new(contextutil::NewStaticWarnHandler(5));
    let warn_appender: contextutil::WarnAppenderRef = warn_handler.clone();
    let kv_exec_counter: SharedContextValue = Arc::new(AtomicU64::new(0));
    let ru_metrics = Arc::new(execdetails::RUV2Metrics::default());
    let mem_tracker: Arc<memory::Tracker> = Arc::from(memory::NewTracker(0, -1));
    let location = Arc::new(chrono_tz::UTC);
    let runtime_stats = Arc::new(execdetails::RuntimeStatsColl::default());
    let max_keys_read_counter = Arc::new(AtomicU64::new(0));
    let shared_statement_value: SharedContextValue = Arc::new("statement-owned".to_owned());
    let exec_details = Arc::new(execdetails::SyncExecDetails::default());

    // 填充 DistSQL 运行时选项：副本读、分页、TiFlash（列存加速引擎）配额、资源组等。
    let mut obj = DistSQLContext {
        WarnHandler: warn_appender.clone(),
        InRestrictedSQL: true,
        EnabledRateLimitAction: true,
        EnableChunkRPC: true,
        OriginalSQL: "a".to_owned(),
        KVVars: Some(tikvstore::Variables {
            BackoffLockFast: 1,
            BackOffWeight: 2,
            Killed: &sql_killer.Signal,
        }),
        KvExecCounter: Some(kv_exec_counter.clone()),
        RUV2Metrics: Some(ru_metrics.clone()),
        SessionMemTracker: Some(mem_tracker.clone()),
        Location: Some(location.clone()),
        RuntimeStatsColl: Some(runtime_stats.clone()),
        SQLKiller: Some(&sql_killer),
        CPUUsage: Some(cpu_usage.clone()),
        ErrCtx: errctx::NewContextWithLevels(
            [errctx::Level::LevelWarn; errctx::errGroupCount],
            warn_appender,
        ),
        TiFlashReplicaRead: tiflash::ClosestAdaptive,
        TiFlashMaxThreads: 1,
        TiFlashMaxBytesBeforeExternalJoin: 1,
        TiFlashMaxBytesBeforeExternalGroupBy: 1,
        TiFlashMaxBytesBeforeExternalSort: 1,
        TiFlashMaxQueryMemoryPerNode: 1,
        TiFlashQuerySpillRatio: 1.0,
        TiFlashHashJoinVersion: "legacy".to_owned(),
        DistSQLConcurrency: 1,
        ReplicaReadType: kv::ReplicaReadType::ReplicaReadFollower,
        WeakConsistency: true,
        RCCheckTS: true,
        NotFillCache: true,
        TaskID: 1,
        Priority: mysql::HighPriority,
        EnablePaging: true,
        MinPagingSize: 1,
        MaxPagingSize: 1,
        PagingSizeBytes: 1,
        RequestSourceType: "a".to_owned(),
        ExplicitRequestSourceType: "b".to_owned(),
        StoreBatchSize: 1,
        ResourceGroupName: "c".to_owned(),
        LoadBasedReplicaReadThreshold: Duration::from_secs(1),
        RunawayChecker: Some(shared_statement_value.clone()),
        RUConsumptionReporter: Some(shared_statement_value.clone()),
        TiKVClientReadTimeout: 1,
        MaxExecutionTime: 1,
        MaxKeysRead: 1,
        MaxKeysReadCounter: Some(max_keys_read_counter.clone()),
        ReplicaClosestReadThreshold: 1,
        ConnectionID: 1,
        SessionAlias: "c".to_owned(),
        ExecDetails: Some(exec_details.clone()),
        TryCopLiteWorker: AtomicU32::new(0),
        ..DistSQLContext::default()
    };
    obj.TryCopLiteWorker.store(1, Ordering::Relaxed);

    // AppendWarning 应写入共享的 WarnHandler，计数在 Detach 前后可见。
    obj.AppendWarning(errors::New("test warning"));
    assert_eq!(warn_handler.WarningCount(), 1);

    let detached = obj.Detach();

    // Go Detach shallow-copies these session/statement-owned values.
    // 浅拷贝：WarnHandler / SQLKiller / RU 指标 / 内存 Tracker 等仍指向同一对象。
    assert!(Arc::ptr_eq(&obj.WarnHandler, &detached.WarnHandler));
    assert!(std::ptr::eq(
        obj.SQLKiller.expect("original SQL killer"),
        detached.SQLKiller.expect("detached SQL killer"),
    ));
    assert!(Arc::ptr_eq(
        obj.RUV2Metrics.as_ref().unwrap(),
        detached.RUV2Metrics.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        obj.KvExecCounter.as_ref().unwrap(),
        detached.KvExecCounter.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        obj.SessionMemTracker.as_ref().unwrap(),
        detached.SessionMemTracker.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        obj.Location.as_ref().unwrap(),
        detached.Location.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        obj.RuntimeStatsColl.as_ref().unwrap(),
        detached.RuntimeStatsColl.as_ref().unwrap(),
    ));
    // Go Detach 对语句侧对象同样只做浅拷贝；非空输入必须保持同一指针。
    // Client 未在本测试构造，因为它是完整 KV RPC 接口；其 Option<Arc<_>> 使用与下列
    // 字段相同的 Clone 路径，且生产实现逐字段映射已覆盖该分支。
    assert!(detached.Client.is_none());
    assert!(detached.ResourceGroupTagger.is_none());
    assert!(Arc::ptr_eq(
        obj.RunawayChecker.as_ref().unwrap(),
        detached.RunawayChecker.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        obj.RUConsumptionReporter.as_ref().unwrap(),
        detached.RUConsumptionReporter.as_ref().unwrap(),
    ));
    assert!(Arc::ptr_eq(
        obj.ExecDetails.as_ref().unwrap(),
        detached.ExecDetails.as_ref().unwrap(),
    ));

    // CPU usage and KV variables are independent copies, while Killed follows SQLKiller.
    // CPUUsage / KVVars 为独立副本；但 Killed 指针仍指向同一 SQLKiller.Signal。
    assert!(!Arc::ptr_eq(
        obj.CPUUsage.as_ref().unwrap(),
        detached.CPUUsage.as_ref().unwrap(),
    ));
    assert_eq!(
        obj.CPUUsage.as_ref().unwrap().GetCPUUsages(),
        detached.CPUUsage.as_ref().unwrap().GetCPUUsages(),
    );
    obj.CPUUsage
        .as_ref()
        .unwrap()
        .SetCPUUsages(ppcpuusage::CPUUsages {
            TidbCPUTime: Duration::from_secs(2),
            TikvCPUTime: Duration::from_secs(3),
        });
    assert_eq!(
        detached.CPUUsage.as_ref().unwrap().GetCPUUsages(),
        ppcpuusage::CPUUsages::default(),
    );

    let detached_vars = detached.KVVars.as_ref().unwrap();
    assert_eq!(detached_vars.BackoffLockFast, 1);
    assert_eq!(detached_vars.BackOffWeight, 2);
    assert!(std::ptr::eq(detached_vars.Killed, &sql_killer.Signal));
    obj.KVVars.as_mut().unwrap().BackoffLockFast = 99;
    assert_eq!(detached_vars.BackoffLockFast, 1);
    sql_killer.Signal.store(7, Ordering::Relaxed);
    assert_eq!(detached_vars.Killed.load(Ordering::Relaxed), 7);

    // MaxKeysReadCounter 独立重建并清零，避免继承原语句已读 key 计数。
    let detached_counter = detached.MaxKeysReadCounter.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&max_keys_read_counter, detached_counter));
    assert_eq!(detached_counter.load(Ordering::Relaxed), 0);

    // The remaining populated fields retain their values exactly.
    // 其余标量/枚举配置按值保留，供 Detach 后的收尾逻辑继续使用。
    assert!(detached.InRestrictedSQL);
    assert!(detached.EnabledRateLimitAction);
    assert!(detached.EnableChunkRPC);
    assert_eq!(detached.OriginalSQL, "a");
    assert_eq!(
        detached.ErrCtx.LevelMap(),
        [errctx::Level::LevelWarn; errctx::errGroupCount],
    );
    assert_eq!(detached.TiFlashReplicaRead, tiflash::ClosestAdaptive);
    assert_eq!(detached.TiFlashMaxThreads, 1);
    assert_eq!(detached.TiFlashMaxBytesBeforeExternalJoin, 1);
    assert_eq!(detached.TiFlashMaxBytesBeforeExternalGroupBy, 1);
    assert_eq!(detached.TiFlashMaxBytesBeforeExternalSort, 1);
    assert_eq!(detached.TiFlashMaxQueryMemoryPerNode, 1);
    assert_eq!(detached.TiFlashQuerySpillRatio, 1.0);
    assert_eq!(detached.TiFlashHashJoinVersion, "legacy");
    assert_eq!(detached.DistSQLConcurrency, 1);
    assert_eq!(
        detached.ReplicaReadType,
        kv::ReplicaReadType::ReplicaReadFollower
    );
    assert!(detached.WeakConsistency);
    assert!(detached.RCCheckTS);
    assert!(detached.NotFillCache);
    assert_eq!(detached.TaskID, 1);
    assert_eq!(detached.Priority, mysql::HighPriority);
    assert!(detached.EnablePaging);
    assert_eq!(detached.MinPagingSize, 1);
    assert_eq!(detached.MaxPagingSize, 1);
    assert_eq!(detached.PagingSizeBytes, 1);
    assert_eq!(detached.RequestSourceType, "a");
    assert_eq!(detached.ExplicitRequestSourceType, "b");
    assert_eq!(detached.StoreBatchSize, 1);
    assert_eq!(detached.ResourceGroupName, "c");
    assert_eq!(
        detached.LoadBasedReplicaReadThreshold,
        Duration::from_secs(1)
    );
    assert_eq!(detached.TiKVClientReadTimeout, 1);
    assert_eq!(detached.MaxExecutionTime, 1);
    assert_eq!(detached.MaxKeysRead, 1);
    assert_eq!(detached.ReplicaClosestReadThreshold, 1);
    assert_eq!(detached.ConnectionID, 1);
    assert_eq!(detached.SessionAlias, "c");
    assert_eq!(detached.TryCopLiteWorker.load(Ordering::Relaxed), 1);
}
