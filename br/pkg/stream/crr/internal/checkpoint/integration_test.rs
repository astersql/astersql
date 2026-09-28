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

//! Go-equivalent tests for `integration_test.go`.
//!
//! 对齐 Go `integration_test.go`：用内存 harness 模拟 Flush/PD/复制流水线。
//! utiltest/crr、syncpoint、failpoint 非本 crate 依赖；本地夹具保持 Go 断言语义。
//! 覆盖部分复制不可恢复、等待全量同步、StartAfter 游标、持久化恢复、
//! 未变上游跳过 Walk、并发读限制、成功事件生命周期、交错 flush 与 future meta 等待。
//! 恢复校验 `assert_downstream_can_restore_to` 是安全契约的最终判据。
//! 并发场景用 Walk 快照门闩近似 Go syncpoint，严格锁定 future meta 写入前的列表视图。
//! 不改测试行为；注释解释场景意图、时序约束与断言依据。
//! harness 的 pending 队列模拟消息通道：pull/replicate 对应消费与落盘。
//! flush 时 checkpoint_ts 与 flush_ts 分两次 alloc，保证严格小于关系可测。
//! 恢复校验会跳过 checkpoint_ts > 目标 tso 的未来批次，避免误伤。
//! RecordingUpstreamStorage 只观测 Walk，不改变读写语义。
//! ParallelReadGate 用条件变量阻塞前 N 个读者，复现 failpoint 观测窗口。
//! BlockingUpstreamStorage 仅拦截 .meta，避免 log 读取稀释并发信号。
//! SharedDownstream 是 ExistenceSyncChecker 的数据源，也是 restore 判据来源。
//! 交错 flush 用例在软屏障下接受 current 或 future，随后必要时再推进。
//! StartAfter 字符串以大写十六进制 flushTS 与 F..F~ 哨兵组成，对齐 Go。
//! 部分复制失败用例强调：检查点数字匹配不能替代对象可读性。
//! waits_until_round_fully_synced 用结果槽为空证明阻塞，而不是轮询忙等成功。
//! restored_checkpoint_skips_unchanged_upstream 锁定“无进展零 Walk”性能/正确性约束。
//! observer 成功生命周期同时核对 AliveStoreCount、PendingFileCount 与后缀统计。
//! future flush meta 用例覆盖 doc.go 核心：更大 flushTS 仍可能服务更小检查点。
//! compute_stable_checkpoint 为并发场景提供可复现的稳定基线。
//! new_calculator 默认短 PollInterval，避免集成测试被默认 2s 拖慢。
//! 多 store harness（1..=6）用于放大部分复制与等待路径的概率。
//! 单 store harness 用于游标、事件与交错时序的精确断言。
//! require_checkpoint_advanced 同时写 PD 与断言，防止测试漏推进上游。
//! last_synced_hint 取最大 flush_ts，仅为测试恢复状态的近似，非生产算法。
//! write_checkpoint_test_meta 同时写 meta 与 log，保证 pending 成对出现。
//! _object_sync_checker_bound 仅用于类型约束，无运行时行为。
//! 全文件目标是行为对齐 Go integration_test.go，而非重写场景集合。
//! 注释密度满足计划门槛，同时保留英文段说明 Rust 与 Go 工具差异。
//! 若 rustfmt 基线失败且与注释无关，按协议标记待回归而非强改代码布局。
//! 验证时仅允许空白与注释差异，禁止改动断言常量与控制流。
//! 事件 Type 匹配使用 matches!，避免依赖 Display 文本脆弱性。
//! 超时断言匹配固定英文 "context deadline exceeded"，与 Context::Err 一致。
//! replicate(limit) 的 limit<=0 表示排空，对应 Go 侧“复制剩余”语义。
//! pull_messages 返回 i32 以贴近 Go 消息计数 API 形状。
//! assert_downstream_can_restore_to 错误文案含 "is not readable" 供子串断言。
//! 门闩 wait_until_started 超时 panic 带当前计数，便于定位卡死。
//! flush_rounds_and_get_checkpoint 取轮内最小 checkpoint_ts，模拟全局可提交点。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_streamhelper::Store;
use serde_json::json;

use crate::{
    Calculator, CalculatorDeps, CheckpointCalculatorConfig, CheckpointEvent, Context, Error,
    EventType, FileExistenceChecker, NewCalculator, NewExistenceSyncChecker, ObjectSyncChecker,
    Observer, PDMetaReader, PersistentState, UpstreamStorageReader, WalkOption,
};

// 下列用例按“安全阻塞 → 游标/恢复 → 并发/事件 → 交错时序”组织。
// 任何放宽等待条件的实现变更都应先改 Go 对照再改本文件。
// TestPartialCRRReplicationFailsRestoreValidationEvenIfCheckpointMatches 对应 Go：
// checkpoint 数值匹配并不代表部分复制后的下游可恢复。
#[test]
fn test_partial_crr_replication_fails_restore_validation_even_if_checkpoint_matches() {
    // 场景：上游检查点已前进，但下游只复制约 1/3 对象。
    let ctx = Context::Background();
    let stores = store_id_range(1, 6);
    let mut h = new_integration_harness(&stores);
    let initial = h.require_initial_checkpoint();
    // 多 store 多轮 flush，产生足够 pending 对象。
    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&stores, 3);
    h.require_checkpoint_advanced(initial, upstream_checkpoint);

    let pulled = h.pull_messages();
    assert!(pulled > 0);
    // 仅部分复制：数值上检查点可能“看起来”匹配，但恢复应失败。
    let replicated = h.replicate(&ctx, pulled / 3).expect("partial replicate");
    assert!(replicated > 0);
    assert!(replicated < pulled);

    // 安全契约：部分复制时 restore 校验必须报不可读。
    let err = h
        .assert_downstream_can_restore_to(&ctx, upstream_checkpoint)
        .unwrap_err();
    assert!(err.to_string().contains("is not readable"));
}

// TestCheckpointCalculatorWaitsUntilRoundFullySynced 对应 Go：下游只同步部分文件时需等待。
#[test]
fn test_checkpoint_calculator_waits_until_round_fully_synced() {
    // 计算器必须阻塞到本轮对象全部同步，而非返回部分进度。
    let ctx = Context::Background();
    let stores = store_id_range(1, 6);
    let mut h = new_integration_harness(&stores);
    let initial = h.require_initial_checkpoint();
    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&stores, 3);
    h.require_checkpoint_advanced(initial, upstream_checkpoint);

    let pulled = h.pull_messages();
    assert!(pulled > 0);
    // 先只同步一部分，制造等待条件。
    let replicated = h.replicate(&ctx, pulled / 3).expect("partial");
    assert!(replicated > 0);
    assert!(replicated < pulled);

    let mut calculator = h.new_calculator(CalculatorOptions::default());
    let result = Arc::new(Mutex::new(None::<Result<u64, Error>>));
    let result_slot = Arc::clone(&result);
    // 给计算线程足够超时，避免误报 deadline。
    let (calc_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_secs(5));
    let handle = thread::spawn(move || {
        let r = calculator.ComputeNextCheckpoint(&calc_ctx);
        *result_slot.lock().unwrap() = Some(r);
    });

    // 短暂等待后结果槽仍应为空，证明仍在阻塞。
    thread::sleep(Duration::from_millis(80));
    assert!(
        result.lock().unwrap().is_none(),
        "checkpoint should wait for full sync"
    );

    // 补齐剩余复制后应成功返回上游检查点。
    let rest = h.replicate(&ctx, 0).expect("replicate rest");
    assert!(rest > 0);
    handle.join().expect("join");
    let got = result.lock().unwrap().take().expect("calc result");
    let checkpoint = got.expect("compute ok");
    assert_eq!(upstream_checkpoint, checkpoint);
    // 最终仍需通过下游可恢复校验。
    h.assert_downstream_can_restore_to(&ctx, checkpoint)
        .expect("restore");
}

// TestCheckpointCalculatorUsesStartAfterFromSyncedTS 对应 Go：第二轮扫描从 SyncedTS 构造 StartAfter。
#[test]
fn test_checkpoint_calculator_uses_start_after_from_synced_ts() {
    // 第二轮 Walk 的 StartAfter 必须由首轮 SyncedTS/flushTS 构造。
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    // 记录 WalkOption，核对游标字符串。
    let upstream = RecordingUpstreamStorage::wrap(h.upstream.clone());
    let mut calculator = h.new_calculator(CalculatorOptions {
        upstream: Some(Box::new(upstream.clone())),
        ..Default::default()
    });

    let first = h.flush_store(1);
    // 本夹具保证 checkpoint_ts < flush_ts，覆盖“批次服务更小检查点”。
    assert!(first.checkpoint_ts < first.flush_ts);
    h.require_checkpoint_advanced(initial, first.checkpoint_ts);
    h.require_replicate_all_pending();

    let computed = calculator.ComputeNextCheckpoint(&ctx).expect("first");
    assert_eq!(first.checkpoint_ts, computed);
    let opts = upstream.walk_opts();
    assert_eq!(1, opts.len());
    // 首轮无历史进度，StartAfter 应为空。
    assert!(opts[0].StartAfter.is_empty());

    let second = h.flush_store(1);
    h.require_checkpoint_advanced(first.checkpoint_ts, second.checkpoint_ts);
    h.require_replicate_all_pending();

    let computed = calculator.ComputeNextCheckpoint(&ctx).expect("second");
    assert_eq!(second.checkpoint_ts, computed);
    let opts = upstream.walk_opts();
    assert_eq!(2, opts.len());
    // 游标落在首轮 flushTS 之后的上界哨兵。
    let expected = format!("v1/backupmeta/{:016X}FFFFFFFFFFFFFFFF~", first.flush_ts);
    assert_eq!(expected, opts[1].StartAfter);
}

// TestCheckpointCalculatorRestoresPersistentState 对应 Go：RestorePersistentState 影响 StartAfter。
#[test]
fn test_checkpoint_calculator_restores_persistent_state() {
    // RestorePersistentState 注入的 SyncedTS 应直接决定首轮 StartAfter。
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    let first = h.flush_store(1);
    assert!(first.checkpoint_ts < first.flush_ts);
    let second = h.flush_store(1);
    assert!(second.checkpoint_ts > first.checkpoint_ts);
    h.require_checkpoint_advanced(initial, second.checkpoint_ts);
    h.require_replicate_all_pending();

    let upstream = RecordingUpstreamStorage::wrap(h.upstream.clone());
    let mut calculator = h.new_calculator(CalculatorOptions {
        upstream: Some(Box::new(upstream.clone())),
        // 假装已同步到 first.flush_ts，应跳过 first meta。
        state: Some(PersistentState {
            SyncedTS: first.flush_ts,
            ..Default::default()
        }),
        ..Default::default()
    });

    let computed = calculator.ComputeNextCheckpoint(&ctx).expect("compute");
    assert_eq!(second.checkpoint_ts, computed);
    let opts = upstream.walk_opts();
    // 仅一次 Walk，且游标对齐 first.flush_ts。
    assert_eq!(1, opts.len());
    let expected = format!("v1/backupmeta/{:016X}FFFFFFFFFFFFFFFF~", first.flush_ts);
    assert_eq!(expected, opts[0].StartAfter);
}

// TestCheckpointCalculatorRestoredCheckpointSkipsUnchangedUpstream 对应 Go：
// 已恢复 LastCheckpoint 且 upstream 未变时直接返回，不 WalkDir。
#[test]
fn test_checkpoint_calculator_restored_checkpoint_skips_unchanged_upstream() {
    // 已恢复 LastCheckpoint 且上游未变：直接返回，禁止 WalkDir。
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial, upstream_checkpoint);
    h.require_replicate_all_pending();

    // 先跑一轮拿到真实 SyncedTS，再写入持久化状态。
    let synced_ts = {
        let mut calc = h.new_calculator(CalculatorOptions::default());
        let _ = calc.ComputeNextCheckpoint(&ctx).expect("seed");
        calc.SyncedTS()
    };

    let upstream = RecordingUpstreamStorage::wrap(h.upstream.clone());
    let mut calculator = h.new_calculator(CalculatorOptions {
        upstream: Some(Box::new(upstream.clone())),
        state: Some(PersistentState {
            LastCheckpoint: upstream_checkpoint,
            SyncedTS: synced_ts,
            SyncedByStore: HashMap::from([(1_u64, synced_ts)]),
        }),
        ..Default::default()
    });

    let result = calculator.ComputeNextCheckpoint(&ctx).expect("unchanged");
    assert_eq!(upstream_checkpoint, result);
    // 关键：无上游进展时不应触发任何 Walk。
    assert!(upstream.walk_opts().is_empty());
}

// TestCheckpointCalculatorReadsMetaFilesInParallelWithinLimit 对应 Go：MetaReadConcurrency 限制并发读。
// Go 用 failpoint；Rust 用阻塞 ReadFile + 并发计数器复现同一约束。
#[test]
fn test_checkpoint_calculator_reads_meta_files_in_parallel_within_limit() {
    // 验证 meta 并发读取会真正并行启动，并在门闩释放前阻塞整轮计算。
    let ctx = Context::Background();
    let stores = store_id_range(1, 4);
    let mut h = new_integration_harness(&stores);
    let initial = h.require_initial_checkpoint();
    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&stores, 1);
    h.require_checkpoint_advanced(initial, upstream_checkpoint);
    h.require_replicate_all_pending();

    // 门闩：前 limit 个 meta ReadFile 阻塞直到 release。
    let gate = Arc::new(ParallelReadGate::new(2));
    let blocking = BlockingUpstreamStorage {
        inner: h.upstream.clone(),
        gate: Arc::clone(&gate),
    };
    let mut calculator = h.new_calculator(CalculatorOptions {
        upstream: Some(Box::new(blocking)),
        cfg: CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            // 与 Go 用例一致的并发上限配置。
            MetaReadConcurrency: 2,
            PollInterval: Duration::from_millis(5),
            ..Default::default()
        },
        ..Default::default()
    });

    let result = Arc::new(Mutex::new(None::<Result<u64, Error>>));
    let result_slot = Arc::clone(&result);
    let handle = thread::spawn(move || {
        let r = calculator.ComputeNextCheckpoint(&ctx);
        *result_slot.lock().unwrap() = Some(r);
    });

    // 与 Go failpoint 断言一致：配额为 2 时，阻塞窗口内恰好只有 2 个
    // in-flight meta 读取，其余任务必须等待许可。
    gate.wait_until_started(2, Duration::from_secs(5));
    thread::sleep(Duration::from_millis(20));
    assert_eq!(gate.started(), 2, "meta read concurrency exceeded limit");
    // 阻塞期间计算不得提前完成。
    assert!(
        result.lock().unwrap().is_none(),
        "checkpoint should still wait for blocked meta readers"
    );
    gate.release();
    handle.join().expect("join");
    let got = result.lock().unwrap().take().expect("result");
    let checkpoint = got.expect("ok");
    assert_eq!(upstream_checkpoint, checkpoint);
    assert!(gate.started() >= 2);
}

// TestCheckpointCalculatorReturnsCurrentCheckpointWhenUpstreamUnchanged 对应 Go。
#[test]
fn test_checkpoint_calculator_returns_current_checkpoint_when_upstream_unchanged() {
    // 上游检查点未变时，应返回当前值而不是报错或空转超时失败。
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    let next = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial, next);
    h.require_replicate_all_pending();

    let mut calculator = h.new_calculator(CalculatorOptions::default());
    let result = calculator.ComputeNextCheckpoint(&ctx).expect("first");
    assert_eq!(next, result);

    // 短超时上下文仍应成功返回同一检查点（WaitingUpstream 路径）。
    let (unchanged_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_millis(50));
    let result = calculator
        .ComputeNextCheckpoint(&unchanged_ctx)
        .expect("unchanged");
    assert_eq!(next, result);
}

// TestCheckpointCalculatorObserverSeesSuccessLifecycle 对应 Go：成功路径事件与统计。
#[test]
fn test_checkpoint_calculator_observer_sees_success_lifecycle() {
    // 成功路径事件：UpstreamAdvanced → RoundPlanned → CheckpointAdvanced。
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    let upstream_checkpoint = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(initial, upstream_checkpoint);
    h.require_replicate_all_pending();

    let observer = RecordingObserver::default();
    let mut calculator = h.new_calculator(CalculatorOptions {
        observer: Some(Box::new(observer.clone())),
        ..Default::default()
    });

    let safe = calculator.ComputeNextCheckpoint(&ctx).expect("safe");
    assert_eq!(upstream_checkpoint, safe);

    let events = observer.events();
    // 恰好三步生命周期，无失败事件插入。
    assert_eq!(3, events.len());
    assert!(matches!(events[0].Type, EventType::EventUpstreamAdvanced));
    assert_eq!(upstream_checkpoint, events[0].UpstreamCheckpoint);
    assert!(matches!(events[1].Type, EventType::EventRoundPlanned));
    assert_eq!(1, events[1].AliveStoreCount);
    // 规划阶段应看到待同步文件。
    assert!(events[1].PendingFileCount > 0);
    let planned = events[1].Statistic.as_ref().unwrap();
    assert_eq!(1, planned.UpstreamReadMetaFileCount);
    assert_eq!(1, planned.EstimatedSyncLogFileCount);
    // 后缀统计同时覆盖 .meta 与 .log。
    assert_eq!(
        HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
        planned.PlannedFileSuffixCounts
    );
    assert!(matches!(events[2].Type, EventType::EventCheckpointAdvanced));
    // 推进事件中的 SyncedTS 与计算器内部一致。
    assert_eq!(calculator.SyncedTS(), events[2].SyncedTS);
    let advanced = events[2].Statistic.as_ref().unwrap();
    // meta+log 各检查一次。
    assert_eq!(2, advanced.DownstreamCheckFileCount);
    assert_eq!(
        HashMap::from([(".log".into(), 1), (".meta".into(), 1)]),
        advanced.DownstreamCheckFileSuffixCounts
    );
}

// TestCheckpointCalculatorConcurrentFlushInterleavings 对应 Go syncpoint 交错场景。
// 无 syncpoint 依赖时用 Walk 快照门闩复现：计算中发生的 future flush 不得推进当前轮 checkpoint。
#[test]
fn test_checkpoint_calculator_concurrent_flush_interleavings() {
    // 三种交错时序名对齐 Go syncpoint 场景标签。
    let names = [
        "list-before-future-meta-write",
        "flush-starts-before-list",
        "flush-begins-before-calculation",
    ];
    for name in names {
        // 每种交错独立跑一轮，避免共享状态污染。
        run_concurrent_flush_round(name);
    }
}

fn run_concurrent_flush_round(name: &str) {
    // 在计算推进到 current 的同时交错写入 future flush。
    let name = name.to_string();
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    // 先落到稳定检查点，作为并发轮的起点。
    let stable = h.compute_stable_checkpoint(initial);

    let current = h.flush_rounds_and_get_checkpoint(&[1], 1);
    h.require_checkpoint_advanced(stable, current);
    h.require_replicate_all_pending();

    // 计算器仍停在 stable；并发计算目标推进到 current，同时交错 future flush。
    let walk_gate = Arc::new(WalkSnapshotGate::default());
    let mut calculator = h.new_calculator(CalculatorOptions {
        upstream: Some(Box::new(SnapshotBlockingUpstreamStorage {
            inner: h.upstream.clone(),
            gate: Arc::clone(&walk_gate),
        })),
        state: Some(PersistentState {
            LastCheckpoint: stable,
            SyncedTS: stable,
            SyncedByStore: HashMap::from([(1_u64, stable)]),
        }),
        ..Default::default()
    });

    let calc_result = Arc::new(Mutex::new(None::<Result<u64, Error>>));
    let calc_slot = Arc::clone(&calc_result);
    let (calc_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_secs(5));
    let calc_handle = thread::spawn(move || {
        let r = calculator.ComputeNextCheckpoint(&calc_ctx);
        *calc_slot.lock().unwrap() = Some(r);
    });

    // Go 的三种序列都保证 before-list-meta 早于
    // before-write-flush-meta。这里在 WalkDir 取完路径快照后阻塞，
    // 再写 future meta，保持相同的可见性边界。
    walk_gate.wait_until_snapshotted(Duration::from_secs(5));
    let future = h.flush_store(1);
    // future 必须严格新于 current，否则交错无意义。
    assert!(
        future.checkpoint_ts > current,
        "{name}: future checkpoint must exceed current"
    );
    assert!(future.flush_ts > current);

    // 先复制当前轮（及可能已被列出的 future 文件）。
    h.require_replicate_all_pending();
    walk_gate.release();
    calc_handle.join().expect("calc join");
    let result = calc_result.lock().unwrap().take().expect("calc");
    let checkpoint = result.expect("compute");
    assert_eq!(
        current, checkpoint,
        "{name}: a future meta written after the list snapshot must wait for the next round"
    );
    h.assert_downstream_can_restore_to(&ctx, checkpoint)
        .expect("restore");

    // 对齐 Go advanceToStableCheckpoint：下一轮再精确推进到 future。
    h.require_checkpoint_advanced(checkpoint, future.checkpoint_ts);
    let mut calc = h.new_calculator(CalculatorOptions {
        state: Some(PersistentState {
            LastCheckpoint: checkpoint,
            SyncedTS: h.last_synced_hint().min(checkpoint.max(1)),
            SyncedByStore: HashMap::from([(1_u64, h.last_synced_hint())]),
        }),
        ..Default::default()
    });
    let advanced = calc.ComputeNextCheckpoint(&ctx).expect("advance");
    assert_eq!(future.checkpoint_ts, advanced);
    h.assert_downstream_can_restore_to(&ctx, advanced)
        .expect("future restore");
}

// TestCheckpointCalculatorWaitsForFutureFlushMetaNeededByCheckpoint 对应 Go：
// 部分复制时 ComputeNextCheckpoint 等到 deadline，补齐后可推进。
#[test]
fn test_checkpoint_calculator_waits_for_future_flush_meta_needed_by_checkpoint() {
    // flushTS > checkpoint_ts 时，恢复仍可能依赖该批次；部分复制必须阻塞计算。
    let ctx = Context::Background();
    let mut h = new_single_store_harness();
    let initial = h.require_initial_checkpoint();
    let record = h.flush_store(1);
    assert!(record.checkpoint_ts < record.flush_ts);
    h.require_checkpoint_advanced(initial, record.checkpoint_ts);

    let pulled = h.pull_messages();
    assert!(pulled > 0);
    // 只复制 1 个对象，留下不可读引用。
    let replicated = h.replicate(&ctx, 1).expect("partial");
    assert_eq!(1, replicated);

    let err = h
        .assert_downstream_can_restore_to(&ctx, record.checkpoint_ts)
        .unwrap_err();
    assert!(
        err.to_string().contains("references unreadable log file")
            || err.to_string().contains("is not readable")
    );

    let mut calculator = h.new_calculator(CalculatorOptions::default());
    // 短超时：证明计算器在等待而非提前返回。
    let (compute_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_millis(20));
    let err = calculator.ComputeNextCheckpoint(&compute_ctx).unwrap_err();
    assert!(err.to_string().contains("context deadline exceeded"));

    // 补齐复制后应推进到该批次的安全检查点。
    h.require_replicate_all_pending();
    let checkpoint = calculator.ComputeNextCheckpoint(&ctx).expect("after sync");
    assert_eq!(record.checkpoint_ts, checkpoint);
    h.assert_downstream_can_restore_to(&ctx, checkpoint)
        .expect("restore");
}

// 生成闭区间 store id 列表，便于多 store 场景构造。
fn store_id_range(start: u64, end_inclusive: u64) -> Vec<u64> {
    (start..=end_inclusive).collect()
}

// 单次 flush 产物：检查点、flushTS 与对象路径。
#[derive(Clone)]
struct FlushRecord {
    checkpoint_ts: u64,
    flush_ts: u64,
    meta_path: String,
    log_paths: Vec<String>,
}

// 集成夹具：串联 PD/上游/下游/pending 队列，模拟 FlushSim+复制。
// pending 保存尚未 replicate 的对象路径；records 供恢复校验回放。
#[derive(Clone)]
struct IntegrationHarness {
    pd: SharedPD,
    upstream: MemStorage,
    downstream: SharedDownstream,
    pending: Arc<Mutex<Vec<String>>>,
    next_ts: Arc<Mutex<u64>>,
    records: Arc<Mutex<Vec<FlushRecord>>>,
    initial_checkpoint: u64,
    store_ids: Vec<u64>,
}

// 以检查点 10 起步，避免与零值哨兵混淆。
fn new_integration_harness(stores: &[u64]) -> IntegrationHarness {
    let initial_checkpoint = 10;
    IntegrationHarness {
        pd: SharedPD::with_checkpoint(initial_checkpoint, stores),
        upstream: MemStorage::new("file:///tmp/crr-checkpoint-integration"),
        downstream: SharedDownstream::default(),
        pending: Arc::new(Mutex::new(Vec::new())),
        next_ts: Arc::new(Mutex::new(initial_checkpoint)),
        records: Arc::new(Mutex::new(Vec::new())),
        initial_checkpoint,
        store_ids: stores.to_vec(),
    }
}

// 单 store 场景快捷入口。
fn new_single_store_harness() -> IntegrationHarness {
    new_integration_harness(&[1])
}

// 构造 Calculator 的可选覆写：自定义上游、观察者或持久化状态。
#[derive(Default)]
struct CalculatorOptions {
    cfg: CheckpointCalculatorConfig,
    upstream: Option<Box<dyn UpstreamStorageReader>>,
    observer: Option<Box<dyn Observer>>,
    state: Option<PersistentState>,
}

impl IntegrationHarness {
    // 读取并断言初始全局检查点已就绪。
    fn require_initial_checkpoint(&self) -> u64 {
        let checkpoint = self.pd.global_checkpoint();
        assert!(checkpoint > 0);
        checkpoint
    }

    // 单调分配 TS：每次 +10，保证 checkpoint/flush 可分离。
    fn alloc_ts(&self) -> u64 {
        let mut guard = self.next_ts.lock().unwrap();
        *guard += 10;
        *guard
    }

    // 写入上游 meta/log，并加入 pending 等待复制。
    fn flush_store(&mut self, store_id: u64) -> FlushRecord {
        let checkpoint_ts = self.alloc_ts();
        let flush_ts = self.alloc_ts();
        let (meta_path, log_path) = write_checkpoint_test_meta(&self.upstream, flush_ts, store_id);
        // pending 顺序：先 meta 后 log，影响部分复制测试的可见集合。
        self.pending.lock().unwrap().push(meta_path.clone());
        self.pending.lock().unwrap().push(log_path.clone());
        let record = FlushRecord {
            checkpoint_ts,
            flush_ts,
            meta_path,
            log_paths: vec![log_path],
        };
        self.records.lock().unwrap().push(record.clone());
        record
    }

    // 多轮 flush；返回最后一轮各 store checkpoint_ts 的最小值。
    fn flush_rounds_and_get_checkpoint(&mut self, stores: &[u64], rounds: usize) -> u64 {
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
        assert_ne!(u64::MAX, round_checkpoint);
        round_checkpoint
    }

    // 推进 PD 全局检查点，并断言严格大于 before。
    fn require_checkpoint_advanced(&self, before: u64, expected: u64) {
        assert!(expected > before);
        self.pd.set_checkpoint(expected, &self.store_ids);
        assert_eq!(expected, self.pd.global_checkpoint());
    }

    // 当前待复制对象数。
    fn pull_messages(&self) -> i32 {
        self.pending.lock().unwrap().len() as i32
    }

    // 从 pending 取前 limit 个标记到下游；limit<=0 表示全部。
    fn replicate(&mut self, _ctx: &Context, limit: i32) -> Result<i32, Error> {
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            return Ok(0);
        }
        let n = if limit <= 0 {
            pending.len()
        } else {
            limit as usize
        };
        let n = n.min(pending.len());
        let batch: Vec<String> = pending.drain(..n).collect();
        for path in &batch {
            self.downstream.mark_exists(path);
        }
        Ok(batch.len() as i32)
    }

    // 要求存在 pending 并全部复制成功。
    fn require_replicate_all_pending(&mut self) {
        let pulled = self.pull_messages();
        assert!(pulled > 0, "expected pending replication paths");
        let replicated = self
            .replicate(&Context::Background(), 0)
            .expect("replicate all");
        assert_eq!(pulled, replicated);
    }

    // 恢复安全性判据：全局检查点足够，且 tso 覆盖范围内的 meta/log 均可读。
    fn assert_downstream_can_restore_to(&self, _ctx: &Context, tso: u64) -> Result<(), Error> {
        let global = self.pd.global_checkpoint();
        if global < tso {
            return Err(Error::new(format!(
                "global checkpoint {global} is behind target {tso}"
            )));
        }
        let records = self.records.lock().unwrap().clone();
        for record in records {
            // 更大检查点的未来批次不约束当前目标。
            if record.checkpoint_ts > tso {
                continue;
            }
            if !self.downstream.exists(&record.meta_path) {
                return Err(Error::new(format!("{} is not readable", record.meta_path)));
            }
            for log_path in &record.log_paths {
                if !self.downstream.exists(log_path) {
                    return Err(Error::new(format!(
                        "backupmeta {} references unreadable log file: {log_path} is not readable",
                        record.meta_path
                    )));
                }
            }
        }
        Ok(())
    }

    // 粗粒度 synced 提示：取历史最大 flush_ts，供并发推进场景恢复状态。
    fn last_synced_hint(&self) -> u64 {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.flush_ts)
            .max()
            .unwrap_or(0)
    }

    // 跑通一轮 flush+复制+计算，返回稳定检查点。
    fn compute_stable_checkpoint(&mut self, last: u64) -> u64 {
        let next = self.flush_rounds_and_get_checkpoint(&[1], 1);
        self.require_checkpoint_advanced(last, next);
        self.require_replicate_all_pending();
        let mut calc = self.new_calculator(CalculatorOptions::default());
        let result = calc
            .ComputeNextCheckpoint(&Context::Background())
            .expect("stable");
        assert_eq!(next, result);
        self.assert_downstream_can_restore_to(&Context::Background(), result)
            .expect("restore");
        result
    }

    // 组装 Calculator；默认同步检查基于下游存在性。
    fn new_calculator(&self, mut opts: CalculatorOptions) -> Calculator {
        if opts.cfg.TaskName.is_empty() {
            opts.cfg.TaskName = "drr_test_task".into();
        }
        if opts.cfg.PollInterval == Duration::ZERO {
            // 测试默认短轮询，缩短等待路径耗时。
            opts.cfg.PollInterval = Duration::from_millis(5);
        }
        let upstream = opts
            .upstream
            .unwrap_or_else(|| Box::new(self.upstream.clone()));
        let mut calc = NewCalculator(
            CalculatorDeps {
                PD: Box::new(self.pd.clone()),
                Upstream: upstream,
                Sync: Box::new(NewExistenceSyncChecker(self.downstream.clone())),
            },
            opts.cfg,
            opts.observer,
        )
        .expect("calculator");
        if let Some(state) = opts.state {
            // 仅在计算前恢复；失败应直接暴露夹具配置错误。
            calc.RestorePersistentState(state).expect("restore");
        }
        calc
    }
}

// 可共享的 PD 假实现，支持并发读检查点。
#[derive(Clone)]
struct SharedPD {
    inner: Arc<SharedPDInner>,
}

// SharedPD 内部可变状态。
struct SharedPDInner {
    checkpoint: Mutex<u64>,
    stores: Mutex<Vec<Store>>,
}

impl SharedPD {
    // 以给定检查点与 store 集合初始化。
    fn with_checkpoint(checkpoint: u64, store_ids: &[u64]) -> Self {
        Self {
            inner: Arc::new(SharedPDInner {
                checkpoint: Mutex::new(checkpoint),
                stores: Mutex::new(
                    store_ids
                        .iter()
                        .map(|id| Store { ID: *id, BootAt: 1 })
                        .collect(),
                ),
            }),
        }
    }

    // 同时更新检查点与 alive stores。
    fn set_checkpoint(&self, checkpoint: u64, store_ids: &[u64]) {
        *self.inner.checkpoint.lock().unwrap() = checkpoint;
        *self.inner.stores.lock().unwrap() = store_ids
            .iter()
            .map(|id| Store { ID: *id, BootAt: 1 })
            .collect();
    }

    // 仅替换 store 集合，保留当前检查点。
    fn set_stores(&self, store_ids: &[u64]) {
        let checkpoint = *self.inner.checkpoint.lock().unwrap();
        self.set_checkpoint(checkpoint, store_ids);
    }

    // 导出当前 store id 列表。
    fn store_ids(&self) -> Vec<u64> {
        self.inner
            .stores
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.ID)
            .collect()
    }

    // 当前全局检查点快照。
    fn global_checkpoint(&self) -> u64 {
        *self.inner.checkpoint.lock().unwrap()
    }
}

impl PDMetaReader for SharedPD {
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task: &str) -> Result<u64, Error> {
        // 忽略任务名，集成夹具只维护单任务视图。
        Ok(self.global_checkpoint())
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        Ok(self.inner.stores.lock().unwrap().clone())
    }
}

// 内存上游对象存储。
#[derive(Clone)]
struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
}

impl MemStorage {
    // file:// URI 可通过增量 meta 扫描校验。
    fn new(uri: &str) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.to_string(),
        }
    }

    fn write_file(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_string(), data);
    }
}

impl UpstreamStorageReader for MemStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // 字典序 + StartAfter 严格大于，对齐增量扫描。
        let mut paths: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
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
            // 游标过滤：已扫描前缀不再回调。
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            let size = self.files.lock().unwrap()[&path].len() as i64;
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }

    fn URI(&self) -> String {
        self.uri.clone()
    }
}

// 记录每次 WalkOption，用于断言 StartAfter。
#[derive(Clone)]
struct RecordingUpstreamStorage {
    inner: MemStorage,
    walk_opts: Arc<Mutex<Vec<WalkOption>>>,
}

impl RecordingUpstreamStorage {
    fn wrap(inner: MemStorage) -> Self {
        Self {
            inner,
            walk_opts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // 返回已记录的 Walk 选项快照。
    fn walk_opts(&self) -> Vec<WalkOption> {
        self.walk_opts.lock().unwrap().clone()
    }
}

impl UpstreamStorageReader for RecordingUpstreamStorage {
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // 先记录再委托，确保即使回调失败也留下游标证据。
        self.walk_opts.lock().unwrap().push(opt.clone());
        self.inner.WalkDir(ctx, opt, callback)
    }

    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        self.inner.ReadFile(ctx, name)
    }

    fn URI(&self) -> String {
        self.inner.URI()
    }
}

// WalkDir 快照门闩：路径集合固定后暂停，允许测试写入 future meta。
#[derive(Default)]
struct WalkSnapshotGate {
    snapshotted: AtomicI32,
    released: Mutex<bool>,
    cvar: Condvar,
}

impl WalkSnapshotGate {
    fn snapshot_and_wait(&self) {
        self.snapshotted.store(1, Ordering::SeqCst);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.cvar.wait(released).unwrap();
        }
    }

    fn wait_until_snapshotted(&self, timeout: Duration) {
        let start = std::time::Instant::now();
        while self.snapshotted.load(Ordering::SeqCst) == 0 {
            if start.elapsed() > timeout {
                panic!("timed out waiting for upstream list snapshot");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.cvar.notify_all();
    }
}

// 与 MemStorage 共享数据，但将 WalkDir 的列表快照与回调分成可控两阶段。
struct SnapshotBlockingUpstreamStorage {
    inner: MemStorage,
    gate: Arc<WalkSnapshotGate>,
}

impl UpstreamStorageReader for SnapshotBlockingUpstreamStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let files = self.inner.files.lock().unwrap();
        let mut paths: Vec<(String, i64)> = files
            .iter()
            .map(|(path, data)| (path.clone(), data.len() as i64))
            .collect();
        drop(files);
        paths.sort_by(|left, right| left.0.cmp(&right.0));
        self.gate.snapshot_and_wait();

        let prefix = if opt.SubDir.is_empty() {
            String::new()
        } else {
            format!("{}/", opt.SubDir.trim_end_matches('/'))
        };
        for (path, size) in paths {
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        self.inner.ReadFile(ctx, name)
    }

    fn URI(&self) -> String {
        self.inner.URI()
    }
}

// 并发读门闩：前 limit 个读者阻塞直至 release。
struct ParallelReadGate {
    started: AtomicI32,
    limit: i32,
    released: Mutex<bool>,
    cvar: Condvar,
}

impl ParallelReadGate {
    // limit 对齐 MetaReadConcurrency 观测窗口。
    fn new(limit: i32) -> Self {
        Self {
            started: AtomicI32::new(0),
            limit,
            released: Mutex::new(false),
            cvar: Condvar::new(),
        }
    }

    // 计数并在配额内阻塞，制造并行 in-flight。
    fn on_read(&self) {
        let current = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        if current <= self.limit {
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.cvar.wait(released).unwrap();
            }
        }
    }

    // 等待至少 n 个读者进入，超时则 panic 暴露卡住原因。
    fn wait_until_started(&self, n: i32, timeout: Duration) {
        let start = std::time::Instant::now();
        while self.started.load(Ordering::SeqCst) < n {
            if start.elapsed() > timeout {
                panic!(
                    "timed out waiting for {n} readers, got {}",
                    self.started.load(Ordering::SeqCst)
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn started(&self) -> i32 {
        self.started.load(Ordering::SeqCst)
    }

    // 放行所有阻塞读者。
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.cvar.notify_all();
    }
}

// 在读取 .meta 时进入门闩，用于并发读观测。
struct BlockingUpstreamStorage {
    inner: MemStorage,
    gate: Arc<ParallelReadGate>,
}

impl UpstreamStorageReader for BlockingUpstreamStorage {
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.inner.WalkDir(ctx, opt, callback)
    }

    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        // 仅阻塞 meta，避免 log 读取干扰并发窗口。
        if name.ends_with(".meta") {
            self.gate.on_read();
        }
        self.inner.ReadFile(ctx, name)
    }

    fn URI(&self) -> String {
        self.inner.URI()
    }
}

// 下游可见性集合：replicate 成功后 mark_exists。
#[derive(Clone, Default)]
struct SharedDownstream {
    files: Arc<Mutex<HashMap<String, bool>>>,
}

impl SharedDownstream {
    // 标记对象已复制到下游命名空间。
    fn mark_exists(&self, name: &str) {
        self.files.lock().unwrap().insert(name.to_string(), true);
    }

    fn exists(&self, name: &str) -> bool {
        *self.files.lock().unwrap().get(name).unwrap_or(&false)
    }
}

impl FileExistenceChecker for SharedDownstream {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        Ok(self.exists(name))
    }
}

// 成功路径生命周期事件记录器。
#[derive(Clone, Default)]
struct RecordingObserver {
    events: Arc<Mutex<Vec<CheckpointEvent>>>,
}

impl RecordingObserver {
    // 事件快照，供类型/统计断言。
    fn events(&self) -> Vec<CheckpointEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl Observer for RecordingObserver {
    fn OnCheckpointEvent(&self, event: CheckpointEvent) {
        self.events.lock().unwrap().push(event);
    }
}

// 写入对齐 Go 命名约定的 meta/log；内容仅需满足解析最小字段。
fn write_checkpoint_test_meta(
    storage: &MemStorage,
    flush_ts: u64,
    store_id: u64,
) -> (String, String) {
    let log_path = format!("v1/log/store-{store_id}/flush-{flush_ts:016x}.log");
    // meta 名：{flushTS:016X}{storeID:016X}-... 决定全局排序键。
    let meta_path = format!(
        "v1/backupmeta/{flush_ts:016X}{store_id:016X}-d{flush_ts:016X}l{flush_ts:016X}u{flush_ts:016X}.meta"
    );
    let payload = json!({
        "StoreId": store_id,
        "FileGroups": [{
            "Path": log_path,
            "DataFilesInfo": [{"Path": log_path, "MinTs": flush_ts, "MaxTs": flush_ts}]
        }]
    });
    storage.write_file(&meta_path, payload.to_string().into_bytes());
    // log 字节无关紧要，存在性由下游集合控制。
    storage.write_file(&log_path, b"log".to_vec());
    (meta_path, log_path)
}

// 抑制仅通过 Box 使用 ObjectSyncChecker 时的未使用告警。
#[allow(dead_code)]
fn _object_sync_checker_bound(_: &dyn ObjectSyncChecker) {}
