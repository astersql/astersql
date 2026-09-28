// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/stream/crr/service/service_test.go`.
//!
//! Network/PD/TiKV/object-store boundaries are mocked with shared in-memory
//! fixtures that preserve Go harness call order, errors, and data shapes.
//! utiltest/crr is not a declared Cargo dependency of this crate, so the
//! fixture mirrors FlushSim/PDSim/replicate semantics locally.
//!
//! 本文件覆盖 CRR service 外层循环、resume 持久化、HTTP 状态端点与 StatusStore 行为。
//! 与 Go `service_test.go` 场景一一对应：成功推进、失败降级、失败不双计、watch 等待、
//! watch 错误恢复、加载旧 resume、Save/Load 重试、关机 flush、端点与 metric、panic 契约。
//! 夹具在本地复刻 FlushSim/PDSim：MemStorage 写 meta/log，SharedPD 推进 checkpoint，
//! SharedDownstream 标记下游存在；`eventually` 轮询 Status 直至谓词成立或超时。
//! 测试只通过公开 API 与包内可见字段组装 Service，不改生产逻辑。
//! `InMemoryResumeStateStore` 可注入 load/save 失败次数，验证重试与 shutdown flush。
//! `WatchErrorPD` 首次 Wait 失败后放行，覆盖 degraded→running 恢复路径。
//! `CancelingFailingDownstreamChecker` 在首次 FileExists 时 cancel，确保 Observed 失败只计一次。
//! HTTP 测试用 `TestMux`/`TestWriter` 代替真实网络，断言状态码与 JSON 子串。
//! 时间敏感断言用短 PollInterval/RetryInterval（毫秒级）加速收敛。
//! 约束：不得把失败注入改成跳过断言；超时即视为回归。
//! 约束：SharedPD.Wait 必须尊重 ctx 取消，否则测试线程泄漏。
//! 约束：pending 列表在 replicate 后应被清空，防止跨用例污染。
//! 约束：meta 路径编码需含 store/flush，供 calculator 解析分组。
//! 约束：Status JSON 断言用 contains，规避 map 键序不稳定。
//! 约束：degraded 时 /readyz 必须 503，与 K8s 探针语义一致。
//! 约束：stopped 后 Live=false，避免探针误判存活。
//! 约束：resume Save 失败文案前缀含 save resume state，便于日志检索。
//! 约束：双计测试依赖 cancel 与失败同一检查点触发。
//! 约束：Watch 空转时 CurrentRound 可增但 SafeCheckpoint 不变。
//! 约束：shutdown flush 使用独立 Background+Timeout context。
//! 约束：Register 路由集合至少包含 /livez /readyz /status。
//! 约束：FileStatistic 的 suffix map 必须深拷贝隔离。
//! 约束：AliveStoreCount 在失败事件中保持上一轮规划值。
//! 约束：GetStatusFileName 返回值变更需同步文档与部署清单。
//! 约束：FailingDownstreamChecker 错误携带文件名便于定位。
//! 约束：InMemoryResumeStateStore 克隆共享同一 inner 状态。
//! 约束：MemStorage.WalkDir 按字典序回调，稳定扫描顺序。
//! 约束：flush_rounds 取最小 checkpoint，模拟多 store 全局水位。
//! 约束：测试任务名 drr_test_task 仅用于隔离 metric 标签。
//! 约束：Service 字段在端点测试中直接填充，绕过 Run 循环。
//! 约束：resume_state_initialized=true 跳过加载，专注 HTTP 路径。
//! 约束：panic 载荷兼容 &str 与 String 两种 catch_unwind 形态。
//! 约束：Thread join 错误表示服务 panic，应直接失败测试。
//! 约束：Statistic 阈值阈值与 flush 内容相关，使用 >= 而非精确相等。
//! 约束：长 RetryInterval 用例依赖 cancel 而非等待休眠结束。
//! 约束：PD store BootAt 固定为 1，避免无关字段干扰。
//! 约束：URI 字符串仅作标识，不发起真实文件系统访问。
//! 约束：EventRoundPlanned 用于写入 AliveStoreCount 基线。
//! 约束：EventCheckpointAdvanced 清零失败计数并置 Ready。
//! 约束：EventCalculationFailed 置 degraded 并累加失败。
//! 约束：require_checkpoint_advanced 同时更新 PD stores 列表。
//! 约束：write_checkpoint_test_meta 的 JSON 字段名对齐 Go fixture。
//! 约束：TestWriter.WriteBody 覆盖而非追加，匹配单次响应。
//! 约束：HandleFunc 后同 path 再注册会覆盖旧 handler。
//! 约束：SharedDownstream 默认 false，避免“未 replicate 却成功”。
//! 约束：load/save 计数在失败分支同样递增，反映重试次数。
//! 约束：copy_u64_map 防止 saved_state 与内部状态别名。
//! 约束：idle_round 采样后 sleep，用于证明 watch 阻塞无空转推进。
//! 约束：成功用例要求 LastSuccessTime 有值，证明事件时间写入。
//! 约束：失败用例要求 LastErrorTime/LastEventTime 均有值。
//! 约束：持久化成功后 ConsecutiveFailures 必须回到 0。
//! 约束：PendingFileCount>0 表明 calculator 已看到未同步文件。
//! 约束：SyncedByStore 键为 store id，值为该 store flush 水位。
//! 约束：HTTP /status 在 degraded 时仍返回 200 与 JSON 主体。
//! 约束：/livez 看 Live，/readyz 看 Ready，语义不可混淆。
//! 约束：new_status_store 初始 State=starting，start 后变 running。
//! 约束：metric 读取使用任务名标签，避免跨测试串扰需独立名。
//! 约束：本文件不启动真实 HTTP server，全部经 TestMux 内存分发。
//! 约束：与 Go 不一致时应修 Rust 实现或注释，而非弱化断言。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};
use std::thread;
use std::time::{Duration, SystemTime};

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    CalculatorDeps, CheckpointCalculatorConfig, CheckpointEvent, Context, Error, EventType,
    FileExistenceChecker, FileStatistic, NewCalculator, PDMetaReader, PersistentState, Store,
    UpstreamStorageReader, WalkOption,
};

use crate::Config;
use crate::http::{
    HttpMux, HttpRequest, HttpResponseWriter, RegisterOrPanic, STATUS_OK,
    STATUS_SERVICE_UNAVAILABLE,
};
use crate::metrics::skipped_store_synced_meta_file_count_metric;
use crate::service::{
    Deps, New, NewExistenceSyncChecker, ResumeStateStore, Service, UpstreamCheckpointWaiter,
};
use crate::status::{
    GetStatusFileName, STATE_DEGRADED, STATE_RUNNING, STATE_STOPPED, new_status_store,
};

// TestServiceTracksSuccessfulCheckpoint 对应 Go 测试：service 成功推进 checkpoint、保存 resume state，并在 cancel 后进入 stopped 状态。
#[test]
// 背景线程跑 Run；flush 一轮后等待 SafeCheckpoint/SyncedTS/resume 落盘与统计字段。
// cancel 后断言 State=stopped 且 Live/Ready 均为 false。
fn test_service_tracks_successful_checkpoint() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial_checkpoint = h.require_initial_checkpoint();
    let state_store = InMemoryResumeStateStore::default();

    let svc = Arc::new(
        New(
            h.deps(Some(Box::new(state_store.clone()))),
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial_checkpoint, upstream_checkpoint);
    h.require_replicate_all_pending();

    let mut saw_expected_stats = false;
    // 轮询 Status 直至业务条件满足。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        if snapshot.Statistic.UpstreamReadMetaFileCount >= 1
            && snapshot.Statistic.EstimatedSyncLogFileCount >= 1
            && snapshot.Statistic.DownstreamCheckFileCount >= 2
        {
            saw_expected_stats = true;
        }
        snapshot.SafeCheckpoint == upstream_checkpoint
            && snapshot.SyncedTS > 0
            && state_store.saved_state().LastCheckpoint == upstream_checkpoint
            && saw_expected_stats
            && snapshot.ConsecutiveFailures == 0
            && snapshot.LastSuccessTime.is_some()
    });

    cancel();
    // 断言关键字段与 Go 期望一致。
    assert!(done.join().expect("join").is_ok());

    let snapshot = svc.Status();
    // 校验失败路径的状态机迁移。
    assert_eq!(STATE_STOPPED, snapshot.State);
    // 校验成功路径的水位与统计。
    assert!(!snapshot.Live);
    // 夹具步骤：推进上游并同步下游。
    assert!(!snapshot.Ready);
}

// TestServiceTracksFailures 对应 Go 测试：下游检查失败时 service 进入 degraded 并记录失败时间和错误文本。
#[test]
// Sync 换成始终失败的下游检查器，迫使 ComputeNextCheckpoint 失败。
// 期望进入 degraded，且 ConsecutiveFailures/LastError/时间戳均被记录。
fn test_service_tracks_failures() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial_checkpoint = h.require_initial_checkpoint();

    let svc = Arc::new(
        New(
            Deps {
                PD: Box::new(h.pd.clone()),
                Watcher: Box::new(h.pd.clone()),
                Upstream: Box::new(h.upstream.clone()),
                Sync: NewExistenceSyncChecker(FailingDownstreamChecker),
                State: None,
            },
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial_checkpoint, upstream_checkpoint);

    // 轮询 Status 直至业务条件满足。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        snapshot.State == STATE_DEGRADED
            && !snapshot.Ready
            && snapshot.ConsecutiveFailures > 0
            && !snapshot.LastError.is_empty()
            && snapshot.LastErrorTime.is_some()
            && snapshot.LastEventTime.is_some()
    });

    cancel();
    // 断言关键字段与 Go 期望一致。
    assert!(done.join().expect("join").is_ok());
}

// TestServiceDoesNotDoubleCountCalculatorFailure 对应 Go 测试：calculator 已上报的失败不会再被服务层重复计数。
#[test]
// 下游失败同时 cancel，使 Run 退出；ConsecutiveFailures 必须恰好为 1。
// 证明 ObservedCalculator 错误不会再被服务层 record_service_failure。
fn test_service_does_not_double_count_calculator_failure() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial_checkpoint = h.require_initial_checkpoint();

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let cancel: Arc<dyn Fn() + Send + Sync> = Arc::new(cancel);
    let checker = CancelingFailingDownstreamChecker {
        cancel: Arc::clone(&cancel),
        once: Once::new(),
    };

    let svc = Arc::new(
        New(
            Deps {
                PD: Box::new(h.pd.clone()),
                Watcher: Box::new(h.pd.clone()),
                Upstream: Box::new(h.upstream.clone()),
                Sync: NewExistenceSyncChecker(checker),
                State: None,
            },
            default_service_config(),
        )
        .expect("service"),
    );

    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial_checkpoint, upstream_checkpoint);

    // 校验失败路径的状态机迁移。
    assert!(done.join().expect("join").is_ok());

    let snapshot = svc.Status();
    // 校验成功路径的水位与统计。
    assert_eq!(1_u64, snapshot.ConsecutiveFailures);
    // 夹具步骤：推进上游并同步下游。
    assert!(
        snapshot.LastError.contains("boom for"),
        "last error = {}",
        snapshot.LastError
    );
    // 轮询 Status 直至业务条件满足。
    assert!(snapshot.LastErrorTime.is_some());
    // 断言关键字段与 Go 期望一致。
    assert!(snapshot.LastEventTime.is_some());
}

// TestServiceWaitsForCheckpointWatch 对应 Go 测试：checkpoint 不变时服务停在 watch，直到 PD checkpoint 推进才进入下一轮。
#[test]
// 无 flush 时 SafeCheckpoint 停在 initial，CurrentRound 仍可增长（进入 watch）。
// 短暂 sleep 后轮次不变，说明卡在 Wait；flush 后轮次与 checkpoint 一起前进。
fn test_service_waits_for_checkpoint_watch() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial_checkpoint = h.require_initial_checkpoint();
    let state_store = InMemoryResumeStateStore::default();

    let svc = Arc::new(
        New(
            h.deps(Some(Box::new(state_store))),
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    let mut idle_round = 0_u64;
    // 校验失败路径的状态机迁移。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        if snapshot.SafeCheckpoint != initial_checkpoint {
            return false;
        }
        idle_round = snapshot.CurrentRound;
        idle_round >= 2
    });

    thread::sleep(Duration::from_millis(100));
    // 校验成功路径的水位与统计。
    assert_eq!(idle_round, svc.Status().CurrentRound);

    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial_checkpoint, upstream_checkpoint);
    h.require_replicate_all_pending();

    // 夹具步骤：推进上游并同步下游。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        snapshot.SafeCheckpoint == upstream_checkpoint && snapshot.CurrentRound > idle_round
    });

    cancel();
    // 轮询 Status 直至业务条件满足。
    assert!(done.join().expect("join").is_ok());
}

// TestServiceRecoversFromCheckpointWatchError 对应 Go 测试：watcher 首次失败后进入 degraded，随后上游推进可恢复 running。
#[test]
// WatchErrorPD 首次返回 watch boom，随后委托真实 SharedPD。
// 先看到 degraded+错误文案，flush/replicate 后应回到 running 并清空失败。
fn test_service_recovers_from_checkpoint_watch_error() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial_checkpoint = h.require_initial_checkpoint();
    let state_store = InMemoryResumeStateStore::default();
    let pd = WatchErrorPD::new(h.pd.clone(), "watch boom");

    let svc = Arc::new(
        New(
            Deps {
                PD: Box::new(pd.clone()),
                Watcher: Box::new(pd),
                Upstream: Box::new(h.upstream.clone()),
                Sync: NewExistenceSyncChecker(h.downstream.clone()),
                State: Some(Box::new(state_store)),
            },
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    // 断言关键字段与 Go 期望一致。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        snapshot.SafeCheckpoint == initial_checkpoint
            && snapshot.State == STATE_DEGRADED
            && snapshot.LastError == "watch boom"
            && snapshot.ConsecutiveFailures == 1
            && snapshot.LastErrorTime.is_some()
            && snapshot.LastEventTime.is_some()
    });

    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial_checkpoint, upstream_checkpoint);
    h.require_replicate_all_pending();

    // 校验失败路径的状态机迁移。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        snapshot.SafeCheckpoint == upstream_checkpoint
            && snapshot.State == STATE_RUNNING
            && snapshot.ConsecutiveFailures == 0
            && snapshot.LastError.is_empty()
    });

    cancel();
    // 校验成功路径的水位与统计。
    assert!(done.join().expect("join").is_ok());
}

// TestServiceLoadsPersistedResumeState 对应 Go 测试：启动时加载旧状态，从 firstRecord 后继续扫描并最终保存 secondRecord。
#[test]
// 预置 resume=first，磁盘上已有 second 的 pending 文件。
// 启动后先看到 first 水位与 pending>0，replicate 后应保存到 second。
fn test_service_loads_persisted_resume_state() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial = h.require_initial_checkpoint();

    let first = h.flush_store(1);
    h.require_checkpoint_advanced(initial, first.checkpoint_ts);
    h.require_replicate_all_pending();

    let second = h.flush_store(1);
    h.require_checkpoint_advanced(first.checkpoint_ts, second.checkpoint_ts);

    let state_store = InMemoryResumeStateStore::with_state(PersistentState {
        LastCheckpoint: first.checkpoint_ts,
        SyncedTS: first.flush_ts,
        SyncedByStore: HashMap::from([(1_u64, first.flush_ts)]),
    });

    let svc = Arc::new(
        New(
            h.deps(Some(Box::new(state_store.clone()))),
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    // 夹具步骤：推进上游并同步下游。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        snapshot.SafeCheckpoint == first.checkpoint_ts
            && snapshot.SyncedTS == first.flush_ts
            && snapshot.SyncedByStore.get(&1) == Some(&first.flush_ts)
            && snapshot.PendingFileCount > 0
            && snapshot.Statistic.UpstreamReadMetaFileCount == 1
            && snapshot.Statistic.EstimatedSyncLogFileCount == 1
    });

    h.require_replicate_all_pending();
    // 轮询 Status 直至业务条件满足。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        let saved = state_store.saved_state();
        snapshot.SafeCheckpoint == second.checkpoint_ts
            && snapshot.SyncedTS == second.flush_ts
            && snapshot.SyncedByStore.get(&1) == Some(&second.flush_ts)
            && saved.LastCheckpoint == second.checkpoint_ts
            && saved.SyncedTS == second.flush_ts
            && saved.SyncedByStore.get(&1) == Some(&second.flush_ts)
    });

    cancel();
    // 断言关键字段与 Go 期望一致。
    assert!(done.join().expect("join").is_ok());
}

// TestServiceRetriesFailedResumeStatePersist 对应 Go 测试：首次 SaveState 失败后，service 重试并恢复 ready/running。
#[test]
// SaveState 前 N 次失败；save_count>=2 且最终 Ready/running、无残留错误。
fn test_service_retries_failed_resume_state_persist() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial = h.require_initial_checkpoint();
    let state_store = InMemoryResumeStateStore::with_save_failures(1);

    let svc = Arc::new(
        New(
            h.deps(Some(Box::new(state_store.clone()))),
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    let record = h.flush_store(1);
    h.require_checkpoint_advanced(initial, record.checkpoint_ts);
    h.require_replicate_all_pending();

    // 校验失败路径的状态机迁移。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        let saved = state_store.saved_state();
        saved.LastCheckpoint == record.checkpoint_ts
            && saved.SyncedTS == record.flush_ts
            && state_store.save_count() >= 2
            && snapshot.Ready
            && snapshot.State == STATE_RUNNING
            && snapshot.ConsecutiveFailures == 0
            && snapshot.LastError.is_empty()
    });

    cancel();
    // 校验成功路径的水位与统计。
    assert!(done.join().expect("join").is_ok());
}

// TestServiceFlushesPendingResumeStateOnShutdownAfterPersistFailure 对应 Go 测试：持久化失败后即使 retry 很长，shutdown defer 也会 flush pending state。
#[test]
// RetryInterval 设为 1h，避免测试窗口内完成重试休眠。
// cancel 触发 RunStopGuard：即使外层重试未到，shutdown 也应 flush pending。
fn test_service_flushes_pending_resume_state_on_shutdown_after_persist_failure() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let initial = h.require_initial_checkpoint();
    let state_store = InMemoryResumeStateStore::with_state_and_save_failures(
        PersistentState {
            LastCheckpoint: initial,
            SyncedTS: initial,
            SyncedByStore: HashMap::from([(1_u64, initial)]),
        },
        1,
    );

    let svc = Arc::new(
        New(
            h.deps(Some(Box::new(state_store.clone()))),
            Config {
                CalculatorConfig: default_calculator_config(),
                RetryInterval: Duration::from_secs(3600),
            },
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    let record = h.flush_store(1);
    h.require_checkpoint_advanced(initial, record.checkpoint_ts);
    h.require_replicate_all_pending();

    // 夹具步骤：推进上游并同步下游。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        state_store.save_count() == 1
            && snapshot.State == STATE_DEGRADED
            && snapshot.LastError == "save resume state: persist boom"
    });

    cancel();
    // 轮询 Status 直至业务条件满足。
    assert!(done.join().expect("join").is_ok());

    let saved = state_store.saved_state();
    // 断言关键字段与 Go 期望一致。
    assert_eq!(record.checkpoint_ts, saved.LastCheckpoint);
    // 校验失败路径的状态机迁移。
    assert_eq!(record.flush_ts, saved.SyncedTS);
    // 校验成功路径的水位与统计。
    assert_eq!(Some(&record.flush_ts), saved.SyncedByStore.get(&1));
}

// TestServiceRetriesFailedResumeStateLoad 对应 Go 测试：首次 LoadState 失败后，服务重试并最终进入 ready/running。
#[test]
// LoadState 首次失败后重试；最终 load_count>=2 且状态 ready/running。
fn test_service_retries_failed_resume_state_load() {
    let ctx = Context::Background();
    let h = new_service_harness();
    let state_store = InMemoryResumeStateStore::with_load_failures(1);

    let svc = Arc::new(
        New(
            h.deps(Some(Box::new(state_store.clone()))),
            default_service_config(),
        )
        .expect("service"),
    );

    let (run_ctx, cancel) = Context::WithCancel(&ctx);
    let run_svc = Arc::clone(&svc);
    let done = thread::spawn(move || run_svc.Run(&run_ctx));

    // 夹具步骤：推进上游并同步下游。
    eventually(Duration::from_secs(5), Duration::from_millis(20), || {
        let snapshot = svc.Status();
        state_store.load_count() >= 2
            && snapshot.State == STATE_RUNNING
            && snapshot.Ready
            && snapshot.ConsecutiveFailures == 0
            && snapshot.LastError.is_empty()
    });

    cancel();
    // 轮询 Status 直至业务条件满足。
    assert!(done.join().expect("join").is_ok());
}

// TestServiceStatusEndpoints 对应 Go 测试：/livez、/readyz 和 /status 根据 statusStore 状态返回 HTTP 码和 JSON 快照。
#[test]
// 手工组装 Service（跳过 New）以便直接操纵 StatusStore。
// 覆盖 /livez 未 start、/readyz 成功与 degraded、/status JSON 字段。
fn test_service_status_endpoints() {
    let (status, observer) = new_status_store("task");
    let svc = Arc::new(Service {
        calc: Mutex::new(
            NewCalculator(
                CalculatorDeps {
                    PD: Box::new(SharedPD::with_checkpoint(1, &[1])),
                    Upstream: Box::new(MemStorage::new("file:///tmp/upstream")),
                    Sync: NewExistenceSyncChecker(SharedDownstream::default()),
                },
                CheckpointCalculatorConfig {
                    TaskName: "task".into(),
                    PollInterval: Duration::from_millis(5),
                    ..Default::default()
                },
                None,
            )
            .expect("calculator"),
        ),
        status: status.clone(),
        observer,
        pd: Box::new(SharedPD::with_checkpoint(1, &[1])),
        state: None,
        cfg: Config {
            CalculatorConfig: CheckpointCalculatorConfig {
                TaskName: "task".into(),
                ..Default::default()
            },
            RetryInterval: Duration::from_millis(5),
        },
        resume_state_initialized: Mutex::new(true),
        pending_resume_state: Mutex::new(None),
    });

    let mut mux = TestMux::default();
    svc.Register(&mut mux);

    let mut live = TestWriter::default();
    mux.serve("/livez", &mut live);
    // 断言关键字段与 Go 期望一致。
    assert_eq!(STATUS_SERVICE_UNAVAILABLE, live.status);

    status.start();
    status.apply_event(CheckpointEvent {
        Type: EventType::EventCheckpointAdvanced,
        Time: Some(SystemTime::now()),
        UpstreamCheckpoint: 42,
        SyncedTS: 42,
        Statistic: Some(FileStatistic {
            UpstreamReadMetaFileCount: 3,
            SkippedStoreSyncedMetaFileCount: 5,
            EstimatedSyncLogFileCount: 7,
            DownstreamCheckFileCount: 11,
            PlannedFileSuffixCounts: HashMap::from([(".log".into(), 7), (".meta".into(), 3)]),
            DownstreamCheckFileSuffixCounts: HashMap::from([
                (".log".into(), 8),
                (".meta".into(), 3),
            ]),
        }),
        ..Default::default()
    });
    status.set_persistent_state(PersistentState {
        LastCheckpoint: 42,
        SyncedTS: 42,
        SyncedByStore: HashMap::from([(1_u64, 42_u64)]),
    });

    let mut ready = TestWriter::default();
    mux.serve("/readyz", &mut ready);
    // 校验失败路径的状态机迁移。
    assert_eq!(STATUS_OK, ready.status);

    status.apply_event(CheckpointEvent {
        Type: EventType::EventCalculationFailed,
        Time: Some(SystemTime::now()),
        Err: Some(Error::new("boom")),
        ..Default::default()
    });
    let mut degraded = TestWriter::default();
    mux.serve("/readyz", &mut degraded);
    // 校验成功路径的水位与统计。
    assert_eq!(STATUS_SERVICE_UNAVAILABLE, degraded.status);

    let mut status_rec = TestWriter::default();
    mux.serve("/status", &mut status_rec);
    // 夹具步骤：推进上游并同步下游。
    assert_eq!(STATUS_OK, status_rec.status);
    let body = String::from_utf8(status_rec.body).expect("utf8");
    // 轮询 Status 直至业务条件满足。
    assert!(body.contains("\"safe_checkpoint\":42"));
    // 断言关键字段与 Go 期望一致。
    assert!(body.contains("\"synced_ts\":42"));
    // 校验失败路径的状态机迁移。
    assert!(body.contains("\"1\":42"));
    // 校验成功路径的水位与统计。
    assert!(body.contains("\"upstream_read_meta_file_count\":3"));
    // 夹具步骤：推进上游并同步下游。
    assert!(body.contains("\"skipped_store_synced_meta_file_count\":5"));
    // 轮询 Status 直至业务条件满足。
    assert!(body.contains("\"estimated_sync_log_file_count\":7"));
    // 断言关键字段与 Go 期望一致。
    assert!(body.contains("\"downstream_check_file_count\":11"));
    // 校验失败路径的状态机迁移。
    assert!(body.contains("\".log\":7"));
    // 校验成功路径的水位与统计。
    assert!(body.contains("\".meta\":3"));
    // 夹具步骤：推进上游并同步下游。
    assert!(body.contains("\".log\":8"));
    assert!(
        body.ends_with('\n'),
        "Go json.Encoder.Encode appends a newline"
    );
}

#[test]
fn test_status_json_matches_go_time_and_signed_number_encoding() {
    let snapshot = crate::status::StatusSnapshot {
        TaskName: "task".into(),
        AliveStoreCount: -1,
        PendingFileCount: -2,
        Statistic: crate::status::StatusStatistic {
            UpstreamReadMetaFileCount: -3,
            ..Default::default()
        },
        LastSuccessTime: Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(123_456_789)),
        ..Default::default()
    };

    let encoded = crate::status::encode_status_snapshot(&snapshot).expect("json");
    assert!(encoded.contains("\"alive_store_count\":-1"));
    assert!(encoded.contains("\"pending_file_count\":-2"));
    assert!(encoded.contains("\"upstream_read_meta_file_count\":-3"));
    assert!(encoded.contains("\"last_success_time\":\"1970-01-01T00:00:00.123456789Z\""));
    assert!(encoded.contains("\"last_error_time\":\"0001-01-01T00:00:00Z\""));
    assert!(encoded.contains("\"last_event_time\":\"0001-01-01T00:00:00Z\""));
}

// TestStatusStoreTracksFileStatistic 对应 Go 测试：statusStore 复制 FileStatistic map，且 skipped metric label 被更新。
#[test]
// 验证 FileStatistic map 深拷贝：改动 snapshot 副本不影响 store。
// 同时确认 skipped_store_synced_meta_file_count metric 标签更新。
fn test_status_store_tracks_file_statistic() {
    let task_name = "service-test-file-statistic";
    let (status, observer) = new_status_store(task_name);
    status.start();
    status.set_persistent_state(PersistentState {
        LastCheckpoint: 10,
        SyncedTS: 10,
        SyncedByStore: HashMap::from([(1_u64, 10_u64)]),
    });
    observer.BeginCalculationRound();
    status.apply_event(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        Time: Some(SystemTime::now()),
        PendingFileCount: 2,
        Statistic: Some(FileStatistic {
            UpstreamReadMetaFileCount: 1,
            SkippedStoreSyncedMetaFileCount: 3,
            EstimatedSyncLogFileCount: 1,
            PlannedFileSuffixCounts: HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
            ..Default::default()
        }),
        ..Default::default()
    });
    status.apply_event(CheckpointEvent {
        Type: EventType::EventCheckpointAdvanced,
        Time: Some(SystemTime::now()),
        Statistic: Some(FileStatistic {
            UpstreamReadMetaFileCount: 1,
            SkippedStoreSyncedMetaFileCount: 4,
            EstimatedSyncLogFileCount: 1,
            DownstreamCheckFileCount: 2,
            PlannedFileSuffixCounts: HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
            DownstreamCheckFileSuffixCounts: HashMap::from([
                (".log".into(), 1),
                (".meta".into(), 1),
            ]),
        }),
        ..Default::default()
    });

    let mut snapshot = status.snapshot_copy();
    // 轮询 Status 直至业务条件满足。
    assert_eq!(1, snapshot.Statistic.UpstreamReadMetaFileCount);
    // 断言关键字段与 Go 期望一致。
    assert_eq!(4, snapshot.Statistic.SkippedStoreSyncedMetaFileCount);
    // 校验失败路径的状态机迁移。
    assert_eq!(1, snapshot.Statistic.EstimatedSyncLogFileCount);
    // 校验成功路径的水位与统计。
    assert_eq!(2, snapshot.Statistic.DownstreamCheckFileCount);
    // 夹具步骤：推进上游并同步下游。
    assert_eq!(
        HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
        snapshot.Statistic.PlannedFileSuffixCounts
    );
    // 轮询 Status 直至业务条件满足。
    assert_eq!(
        HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
        snapshot.Statistic.DownstreamCheckFileSuffixCounts
    );
    // 断言关键字段与 Go 期望一致。
    assert_eq!(4.0, skipped_store_synced_meta_file_count_metric(task_name));

    snapshot
        .Statistic
        .PlannedFileSuffixCounts
        .insert(".txt".into(), 99);
    snapshot.SyncedByStore.insert(1, 99);
    // 校验失败路径的状态机迁移。
    assert_eq!(
        HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
        status.snapshot_copy().Statistic.PlannedFileSuffixCounts
    );
    // 校验成功路径的水位与统计。
    assert_eq!(
        HashMap::from([(1_u64, 10_u64)]),
        status.snapshot_copy().SyncedByStore
    );
}

#[test]
fn test_status_metrics_are_registered_for_prometheus_gathering() {
    let (status, _) = new_status_store("prometheus_registration_task");
    status.start();

    assert!(prometheus::gather().iter().any(|family| {
        family.name() == "tidb_br_crr_service_live"
            && family.get_metric().iter().any(|metric| {
                metric.get_label().iter().any(|label| {
                    label.name() == "task" && label.value() == "prometheus_registration_task"
                })
            })
    }));
}

// TestStatusStorePreservesFailureStoreCountAndTracksZeroAliveStores 对应 Go 测试：失败事件不覆盖上一轮 alive store 数，下一轮 0 store 可被记录。
#[test]
// 失败事件不得把 AliveStoreCount 清零；随后 Planned(0) 才允许记 0。
fn test_status_store_preserves_failure_store_count_and_tracks_zero_alive_stores() {
    let (status, _) = new_status_store("task");
    status.start();
    status.apply_event(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        Time: Some(SystemTime::now()),
        AliveStoreCount: 3,
        ..Default::default()
    });
    status.apply_event(CheckpointEvent {
        Type: EventType::EventCalculationFailed,
        Time: Some(SystemTime::now()),
        Err: Some(Error::new("boom")),
        ..Default::default()
    });

    let snapshot = status.snapshot_copy();
    // 夹具步骤：推进上游并同步下游。
    assert_eq!(3, snapshot.AliveStoreCount);
    // 轮询 Status 直至业务条件满足。
    assert!(!snapshot.Ready);

    status.apply_event(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        Time: Some(SystemTime::now()),
        AliveStoreCount: 0,
        ..Default::default()
    });
    // 断言关键字段与 Go 期望一致。
    assert_eq!(0, status.snapshot_copy().AliveStoreCount);
}

#[test]
fn test_checkpoint_advanced_with_empty_store_progress_clears_old_progress() {
    let (status, _) = new_status_store("task");
    status.set_persistent_state(PersistentState {
        LastCheckpoint: 10,
        SyncedTS: 10,
        SyncedByStore: HashMap::from([(1, 10)]),
    });

    status.apply_event(CheckpointEvent {
        Type: EventType::EventCheckpointAdvanced,
        SyncedByStore: HashMap::new(),
        SyncedByStoreSet: true,
        ..Default::default()
    });

    assert!(status.snapshot_copy().SyncedByStore.is_empty());
}

// TestGetStatusFileName 对应 Go 测试：resume state 文件名保持稳定。
#[test]
// 契约测试：路径字符串变更会破坏运维脚本与文档。
fn test_get_status_file_name() {
    // 校验失败路径的状态机迁移。
    assert_eq!("crr-checkpoint/resume-state.json", GetStatusFileName());
}

// TestServiceRegisterPanicsOnNilMux 对应 Go 测试：Register(nil) 必须 panic，避免静默丢失 health handler。
#[test]
// RegisterOrPanic(None) 必须 panic，文案固定为 service: nil mux。
fn test_service_register_panics_on_nil_mux() {
    let svc = Arc::new(
        New(
            Deps {
                PD: Box::new(SharedPD::with_checkpoint(1, &[1])),
                Watcher: Box::new(SharedPD::with_checkpoint(1, &[1])),
                Upstream: Box::new(MemStorage::new("file:///tmp/upstream")),
                Sync: NewExistenceSyncChecker(SharedDownstream::default()),
                State: None,
            },
            default_service_config(),
        )
        .expect("service"),
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RegisterOrPanic::<TestMux>(&svc, None);
    }));
    let payload = result.expect_err("expected panic");
    let msg = payload
        .downcast_ref::<&str>()
        .copied()
        .map(str::to_string)
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .expect("panic payload");
    // 校验成功路径的水位与统计。
    assert_eq!("service: nil mux", msg);
}

// 测试用短轮询间隔，加速 calculator 收敛。
fn default_calculator_config() -> CheckpointCalculatorConfig {
    CheckpointCalculatorConfig {
        TaskName: "drr_test_task".into(),
        PollInterval: Duration::from_millis(5),
        ..Default::default()
    }
}

// 外层重试同样压到毫秒级，缩短失败注入场景等待。
fn default_service_config() -> Config {
    Config {
        CalculatorConfig: default_calculator_config(),
        RetryInterval: Duration::from_millis(5),
    }
}

// 轮询谓词直至超时；超时 panic，对齐 Go require.Eventually。
fn eventually(timeout: Duration, interval: Duration, mut pred: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return;
        }
        thread::sleep(interval);
    }
    panic!("condition not met within {timeout:?}");
}

// 单次 flush 产生的 checkpoint/flush 时间戳对。
struct FlushRecord {
    checkpoint_ts: u64,
    flush_ts: u64,
}

// 聚合 PD/上游/下游/pending 路径与时间戳分配器的集成测试夹具。
struct ServiceHarness {
    pd: SharedPD,
    upstream: MemStorage,
    downstream: SharedDownstream,
    pending: Arc<Mutex<Vec<String>>>,
    next_ts: Arc<Mutex<u64>>,
    initial_checkpoint: u64,
}

// 初始 checkpoint=10、单 store id=1，与多数用例假设一致。
fn new_service_harness() -> ServiceHarness {
    let initial_checkpoint = 10;
    let pd = SharedPD::with_checkpoint(initial_checkpoint, &[1]);
    ServiceHarness {
        pd,
        upstream: MemStorage::new("file:///tmp/crr-service-test"),
        downstream: SharedDownstream::default(),
        pending: Arc::new(Mutex::new(Vec::new())),
        next_ts: Arc::new(Mutex::new(initial_checkpoint)),
        initial_checkpoint,
    }
}

// ServiceHarness 方法语义对齐 Go 测试夹具辅助函数。
impl ServiceHarness {
    // 组装 Deps；State 由调用方注入以便覆盖有/无 resume 两类场景。
    fn deps(&self, state: Option<Box<dyn ResumeStateStore>>) -> Deps {
        Deps {
            PD: Box::new(self.pd.clone()),
            Watcher: Box::new(self.pd.clone()),
            Upstream: Box::new(self.upstream.clone()),
            Sync: NewExistenceSyncChecker(self.downstream.clone()),
            State: state,
        }
    }

    // 读取并断言 PD 全局 checkpoint 已就绪。
    fn require_initial_checkpoint(&self) -> u64 {
        let checkpoint = self.pd.global_checkpoint();
        // 夹具步骤：推进上游并同步下游。
        assert!(checkpoint > 0);
        checkpoint
    }

    // 单调分配时间戳，步进 10，避免与 initial 冲突。
    fn alloc_ts(&self) -> u64 {
        let mut guard = self.next_ts.lock().expect("ts lock");
        *guard += 10;
        *guard
    }

    // 写入一对 meta/log，并登记到 pending，供后续 replicate。
    fn flush_store(&self, store_id: u64) -> FlushRecord {
        let checkpoint_ts = self.alloc_ts();
        let flush_ts = self.alloc_ts();
        let (meta_path, log_path) =
            write_checkpoint_test_meta(&self.upstream, flush_ts, store_id, checkpoint_ts);
        self.pending.lock().expect("pending lock").push(meta_path);
        self.pending.lock().expect("pending lock").push(log_path);
        FlushRecord {
            checkpoint_ts,
            flush_ts,
        }
    }

    // 多 store/多轮 flush，返回本轮最小 checkpoint_ts 作为全局水位候选。
    fn flush_rounds_and_get_checkpoint(&self, stores: &[u64], rounds: usize) -> u64 {
        let mut round_checkpoint = u64::MAX;
        for _ in 0..rounds {
            round_checkpoint = u64::MAX;
            for store_id in stores {
                let record = self.flush_store(*store_id);
                if record.checkpoint_ts < round_checkpoint {
                    round_checkpoint = record.checkpoint_ts;
                }
            }
        }
        // 轮询 Status 直至业务条件满足。
        assert_ne!(u64::MAX, round_checkpoint);
        round_checkpoint
    }

    // 断言推进合法后写回 PD，供服务 watch/计算观测。
    fn require_checkpoint_advanced(&self, before: u64, expected: u64) {
        // 断言关键字段与 Go 期望一致。
        assert!(expected > before);
        self.pd.set_checkpoint(expected, &[1]);
        // 校验失败路径的状态机迁移。
        assert_eq!(expected, self.pd.global_checkpoint());
    }

    // 把 pending 路径全部 mark_exists，模拟下游同步完成。
    fn require_replicate_all_pending(&self) {
        let mut pending = self.pending.lock().expect("pending lock");
        // 校验成功路径的水位与统计。
        assert!(!pending.is_empty(), "expected pending replication paths");
        for path in pending.drain(..) {
            self.downstream.mark_exists(&path);
        }
    }
}

#[derive(Clone)]
// 内存对象存储：实现 UpstreamStorageReader 的 WalkDir/ReadFile/URI。
struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
}

// 内存存储写入与构造。
impl MemStorage {
    fn new(uri: &str) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.to_string(),
        }
    }

    fn write_file(&self, path: &str, data: Vec<u8>) {
        self.files
            .lock()
            .expect("files lock")
            .insert(path.to_string(), data);
    }
}

// 按前缀与 StartAfter 过滤目录遍历，对齐对象存储 Walk 语义。
impl UpstreamStorageReader for MemStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut paths: Vec<String> = self
            .files
            .lock()
            .expect("files lock")
            .keys()
            .cloned()
            .collect();
        paths.sort();
        let prefix = if opt.SubDir.is_empty() {
            String::new()
        } else {
            format!("{}/", opt.SubDir.trim_end_matches('/'))
        };
        for path in paths {
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            let size = self.files.lock().expect("files lock")[&path].len() as i64;
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        self.files
            .lock()
            .expect("files lock")
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }

    fn URI(&self) -> String {
        self.uri.clone()
    }
}

#[derive(Clone)]
// 可共享的 PD 桩：同时充当 PDMetaReader 与 UpstreamCheckpointWaiter。
struct SharedPD {
    inner: Arc<SharedPDInner>,
}

// checkpoint 与 store 列表的互斥保护状态。
struct SharedPDInner {
    checkpoint: Mutex<u64>,
    stores: Mutex<Vec<Store>>,
}

// PD 桩的构造与水位更新。
impl SharedPD {
    // 按给定水位与 store id 列表构造 SharedPD。
    fn with_checkpoint(checkpoint: u64, store_ids: &[u64]) -> Self {
        let pd = Self {
            inner: Arc::new(SharedPDInner {
                checkpoint: Mutex::new(checkpoint),
                stores: Mutex::new(
                    store_ids
                        .iter()
                        .map(|id| Store { ID: *id, BootAt: 1 })
                        .collect(),
                ),
            }),
        };
        pd
    }

    // 原子更新全局 checkpoint 与存活 store 集合。
    fn set_checkpoint(&self, checkpoint: u64, store_ids: &[u64]) {
        *self.inner.checkpoint.lock().expect("pd lock") = checkpoint;
        *self.inner.stores.lock().expect("stores lock") = store_ids
            .iter()
            .map(|id| Store { ID: *id, BootAt: 1 })
            .collect();
    }

    // 读取当前全局 checkpoint。
    fn global_checkpoint(&self) -> u64 {
        *self.inner.checkpoint.lock().expect("pd lock")
    }
}

// 元数据读取直接返回内存水位与 store 列表。
impl PDMetaReader for SharedPD {
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task: &str) -> Result<u64, Error> {
        Ok(self.global_checkpoint())
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        Ok(self.inner.stores.lock().expect("stores lock").clone())
    }
}

// 忙等至 checkpoint>current 或 ctx 取消。
impl UpstreamCheckpointWaiter for SharedPD {
    fn WaitGlobalCheckpointAdvance(
        &self,
        ctx: &Context,
        _task_name: &str,
        current: u64,
    ) -> Result<(), Error> {
        loop {
            if let Some(err) = ctx.Err() {
                return Err(err);
            }
            if self.global_checkpoint() > current {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[derive(Clone)]
// 包装 SharedPD：首次 Wait 失败一次，之后透传。
struct WatchErrorPD {
    inner: SharedPD,
    fail_next: Arc<Mutex<bool>>,
    error_text: String,
}

// 构造时 fail_next=true，确保首次 Wait 失败。
impl WatchErrorPD {
    fn new(inner: SharedPD, error_text: &str) -> Self {
        Self {
            inner,
            fail_next: Arc::new(Mutex::new(true)),
            error_text: error_text.to_string(),
        }
    }
}

// 元数据读取完全委托 inner SharedPD。
impl PDMetaReader for WatchErrorPD {
    fn GetGlobalCheckpointForTask(&self, ctx: &Context, task: &str) -> Result<u64, Error> {
        self.inner.GetGlobalCheckpointForTask(ctx, task)
    }

    fn Stores(&self, ctx: &Context) -> Result<Vec<Store>, Error> {
        self.inner.Stores(ctx)
    }
}

// 首次失败后清除标志并透传 Wait。
impl UpstreamCheckpointWaiter for WatchErrorPD {
    fn WaitGlobalCheckpointAdvance(
        &self,
        ctx: &Context,
        task_name: &str,
        current: u64,
    ) -> Result<(), Error> {
        {
            let mut fail_next = self.fail_next.lock().expect("fail lock");
            if *fail_next {
                *fail_next = false;
                return Err(Error::new(self.error_text.clone()));
            }
        }
        self.inner
            .WaitGlobalCheckpointAdvance(ctx, task_name, current)
    }
}

#[derive(Clone, Default)]
// 下游文件存在性集合；默认不存在，需 mark_exists。
struct SharedDownstream {
    files: Arc<Mutex<HashMap<String, bool>>>,
}

// 下游存在性标记辅助。
impl SharedDownstream {
    // 标记路径已同步，供 FileExists 返回 true。
    fn mark_exists(&self, name: &str) {
        self.files
            .lock()
            .expect("downstream lock")
            .insert(name.to_string(), true);
    }
}

// 查表返回是否已 mark_exists。
impl FileExistenceChecker for SharedDownstream {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        Ok(*self
            .files
            .lock()
            .expect("downstream lock")
            .get(name)
            .unwrap_or(&false))
    }
}

struct FailingDownstreamChecker;

// 恒失败，错误信息包含文件名。
impl FileExistenceChecker for FailingDownstreamChecker {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        Err(Error::new(format!("boom for {name}")))
    }
}

// 首次检查时调用 cancel，并返回 boom，用于双计回归。
struct CancelingFailingDownstreamChecker {
    once: Once,
    cancel: Arc<dyn Fn() + Send + Sync>,
}

// Once 保证 cancel 只触发一次。
impl FileExistenceChecker for CancelingFailingDownstreamChecker {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        self.once.call_once(|| (self.cancel)());
        Err(Error::new(format!("boom for {name}")))
    }
}

#[derive(Clone, Default)]
// 可注入 load/save 失败次数的内存 ResumeStateStore。
struct InMemoryResumeStateStore {
    inner: Arc<Mutex<InMemoryResumeStateStoreInner>>,
}

#[derive(Default)]
// 实际状态与失败计数器；经 Mutex 共享给克隆的 store。
struct InMemoryResumeStateStoreInner {
    state: Option<PersistentState>,
    load_failures_left: i32,
    loads: i32,
    save_failures_left: i32,
    saves: i32,
}

// 工厂与观测方法：预置状态、失败次数、计数器。
impl InMemoryResumeStateStore {
    // 预置 PersistentState，模拟进程重启前已落盘。
    fn with_state(state: PersistentState) -> Self {
        let store = Self::default();
        store.inner.lock().expect("lock").state = Some(state);
        store
    }

    // 前 n 次 SaveState 返回 persist boom。
    fn with_save_failures(n: i32) -> Self {
        let store = Self::default();
        store.inner.lock().expect("lock").save_failures_left = n;
        store
    }

    // 前 n 次 LoadState 返回 load boom。
    fn with_load_failures(n: i32) -> Self {
        let store = Self::default();
        store.inner.lock().expect("lock").load_failures_left = n;
        store
    }

    // 同时预置状态与 save 失败次数，服务关机 flush 场景使用。
    fn with_state_and_save_failures(state: PersistentState, n: i32) -> Self {
        let store = Self::with_state(state);
        store.inner.lock().expect("lock").save_failures_left = n;
        store
    }

    // 深拷贝当前已保存状态，供断言 LastCheckpoint/Synced*。
    fn saved_state(&self) -> PersistentState {
        let inner = self.inner.lock().expect("lock");
        inner
            .state
            .as_ref()
            .map(|state| PersistentState {
                SyncedByStore: copy_u64_map(&state.SyncedByStore),
                ..state.clone()
            })
            .unwrap_or_default()
    }

    // SaveState 调用次数（含失败尝试）。
    fn save_count(&self) -> i32 {
        self.inner.lock().expect("lock").saves
    }

    // LoadState 调用次数（含失败尝试）。
    fn load_count(&self) -> i32 {
        self.inner.lock().expect("lock").loads
    }
}

// 按剩余失败次数注入错误，成功路径深拷贝状态。
impl ResumeStateStore for InMemoryResumeStateStore {
    fn LoadState(&self, _ctx: &Context) -> Result<Option<PersistentState>, Error> {
        let mut inner = self.inner.lock().expect("lock");
        inner.loads += 1;
        if inner.load_failures_left > 0 {
            inner.load_failures_left -= 1;
            return Err(Error::new("load boom"));
        }
        Ok(inner.state.as_ref().map(|state| PersistentState {
            SyncedByStore: copy_u64_map(&state.SyncedByStore),
            ..state.clone()
        }))
    }

    fn SaveState(&self, _ctx: &Context, state: PersistentState) -> Result<(), Error> {
        let mut inner = self.inner.lock().expect("lock");
        inner.saves += 1;
        if inner.save_failures_left > 0 {
            inner.save_failures_left -= 1;
            return Err(Error::new("persist boom"));
        }
        inner.state = Some(PersistentState {
            SyncedByStore: copy_u64_map(&state.SyncedByStore),
            ..state
        });
        Ok(())
    }
}

// 浅拷贝 u64 map，避免测试间共享可变引用。
fn copy_u64_map(input: &HashMap<u64, u64>) -> HashMap<u64, u64> {
    input.iter().map(|(k, v)| (*k, *v)).collect()
}

// 按 calculator 期望的路径约定写入 meta JSON 与占位 log。
// log 内容不被读取，存在性/同步状态才影响推进。
fn write_checkpoint_test_meta(
    storage: &MemStorage,
    flush_ts: u64,
    store_id: u64,
    _checkpoint_ts: u64,
) -> (String, String) {
    let log_path = format!("v1/log/store-{store_id}/flush-{flush_ts:016x}.log");
    let meta_path = format!(
        "v1/backupmeta/{flush_ts:016X}{store_id:016X}-d{flush_ts:016X}l{flush_ts:016X}u{flush_ts:016X}.meta"
    );
    let payload = format!(
        r#"{{"StoreId":{store_id},"FileGroups":[{{"Path":"{log_path}","DataFilesInfo":[{{"Path":"{log_path}","MinTs":{flush_ts},"MaxTs":{flush_ts}}}]}}]}}"#
    );
    storage.write_file(&meta_path, payload.into_bytes());
    // Log content is not read by the calculator; only existence/sync matters.
    storage.write_file(&log_path, b"log".to_vec());
    (meta_path, log_path)
}

#[derive(Default)]
// 简易 HttpMux：按 path 注册 handler，供 Register 测试。
struct TestMux {
    routes: HashMap<String, crate::http::HttpHandler>,
}

// 注册路由表项。
impl HttpMux for TestMux {
    fn HandleFunc(&mut self, path: &str, handler: crate::http::HttpHandler) {
        self.routes.insert(path.to_string(), handler);
    }
}

// 测试侧触发已注册 handler。
impl TestMux {
    // 查找路由并调用 handler；缺失路由直接 panic。
    fn serve(&self, path: &str, writer: &mut TestWriter) {
        let handler = self
            .routes
            .get(path)
            .unwrap_or_else(|| panic!("missing route {path}"));
        handler(
            &HttpRequest {
                Method: "GET".into(),
                Path: path.into(),
            },
            writer,
        );
    }
}

#[derive(Default)]
// 捕获状态码/头/体，便于断言 HTTP 响应。
struct TestWriter {
    headers: HashMap<String, String>,
    status: u16,
    body: Vec<u8>,
}

// 记录响应头、状态码与正文。
impl HttpResponseWriter for TestWriter {
    fn Header(&mut self) -> &mut HashMap<String, String> {
        &mut self.headers
    }

    fn WriteHeader(&mut self, status: u16) {
        self.status = status;
    }

    fn WriteBody(&mut self, body: &[u8]) {
        self.body = body.to_vec();
    }
}
