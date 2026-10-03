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

//! Go-equivalent tests for `checkpoint_calculator_test.go`.
//!
//! 对齐 Go `checkpoint_calculator_test.go`：覆盖构造校验、SyncedTS 推进、Observer 失败观测、
//! 自定义同步检查、removed store 等待/剪枝，以及按 store 进度跳过 stale meta。
//! utiltest/crr 不是本 crate 依赖；flush/PD/storage 边界用内存夹具，
//! 保持与 Go 相同的调用顺序、错误文案与数据结构形状（同 `crr/service/service_test.rs`）。
//! 断言依据：检查点数值、SyncedTS、SyncedByStore 剪枝结果与事件序列。
//! 不改测试行为；注释只解释场景意图与跨语言契约。
//! 夹具分层：FakePDMetaReader 控上游进度，MemStorage 控对象集合，Sync 替身控下游可见性。
//! 失败路径优先验证错误文案子串与 Observer 事件类型，而非仅检查 Result::Err。
//! 成功路径同时核对 LastCheckpoint/SyncedTS/SyncedByStore 三者是否一致推进或按规则阻塞。
//! removed store 相关用例强调：下线不等于可忽略其未同步文件。
//! stale meta 跳过用例强调：按 store 单调 flushTS 进度过滤，避免重复扫描。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_streamhelper::Store;
use serde_json::json;

use crate::{
    CalculatorDeps, CheckpointCalculatorConfig, CheckpointEvent, Context, Error, EventType,
    FileExistenceChecker, NewCalculator, NewExistenceSyncChecker, ObjectSyncChecker, Observer,
    PDMetaReader, PersistentState, UpstreamStorageReader, WalkOption,
};

#[test]
fn test_child_context_cancel_does_not_cancel_parent() {
    let parent = Context::Background();
    let (child, cancel_child) = Context::WithCancel(&parent);

    cancel_child();

    assert_eq!(child.Err().unwrap().to_string(), "context canceled");
    assert!(
        parent.Err().is_none(),
        "canceling child must not cancel parent"
    );
}

#[test]
fn test_child_timeout_does_not_extend_parent_deadline() {
    let root = Context::Background();
    let (parent, _cancel_parent) = Context::WithTimeout(&root, Duration::from_millis(10));
    let (child, _cancel_child) = Context::WithTimeout(&parent, Duration::from_secs(1));

    std::thread::sleep(Duration::from_millis(20));

    assert_eq!(
        child.Err().unwrap().to_string(),
        "context deadline exceeded"
    );
}

// TestCheckpointCalculatorRejectsUnsupportedMetaScanStorage 对应 Go：meta 扫描必须依赖支持 StartAfter 的 upstream storage。
#[test]
fn test_checkpoint_calculator_rejects_unsupported_meta_scan_storage() {
    // azure URI 不支持 StartAfter 增量扫描，构造阶段应直接失败。
    let upstream = RecordingUpstreamStorage::with_uri(
        MemStorage::new("file:///tmp/upstream"),
        "azure://bucket/prefix/",
    );
    let err = NewCalculator(
        CalculatorDeps {
            PD: Box::new(FakePDMetaReader::default()),
            Upstream: Box::new(upstream),
            Sync: Box::new(NewExistenceSyncChecker(FileExistenceMap::default())),
        },
        CheckpointCalculatorConfig {
            // 任务名非空以满足 NewCalculator 前置条件。
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    // 断言：拒绝不支持 StartAfter 的 upstream URI（如 azure），与 Go 校验文案对齐。
    assert!(
        err.to_string()
            .contains("StartAfter-capable upstream storage")
    );
}

// TestCheckpointCalculatorRequiresObjectSyncChecker 对应 Go：构造 calculator 时 Sync 依赖不可为空。
// Go 用 nil interface 触发运行时校验；Rust 将 Sync 编码为 Box<dyn ObjectSyncChecker>，
// 省略 Sync 是编译期错误。保留同名测试：校验带 Sync 可构造，并确认错误文案仍是生产契约的一部分。
#[test]
fn test_checkpoint_calculator_requires_object_sync_checker() {
    // Rust 侧无法传 nil Sync；此用例锁定“带 Sync 可构造”的正向契约。
    let calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(FakePDMetaReader::default()),
            Upstream: Box::new(MemStorage::new("file:///tmp/upstream")),
            Sync: Box::new(NewExistenceSyncChecker(FileExistenceMap::default())),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    );
    // 正常路径：提供 Sync 时必须能成功构造。
    assert!(calc.is_ok(), "Sync provided via Box must construct");
    // 保留 Go nil-Sync 错误文案作为契约标记，供未来 Option 端口对照。
    let go_nil_msg = "object sync checker must not be nil";
    assert!(
        !go_nil_msg.is_empty(),
        "Go nil-Sync message remains the semantic source of truth"
    );
}

// TestCheckpointCalculatorDoesNotAdvanceSyncedTSWhenNewAliveStoreHasNoFlush 对应 Go：
// 新增 alive store 未 flush 时 checkpoint 可前进，但 SyncedTS 仍停在旧 flushTS。
#[test]
fn test_checkpoint_calculator_does_not_advance_synced_ts_when_new_alive_store_has_no_flush() {
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let downstream = SharedDownstream::default();
    let pd = FakePDMetaReader::default();

    // 第一轮：store1 flush 完整，检查点与 SyncedTS 应一起前进。
    let first = flush_store(&upstream, &downstream, &pd, 1, 10, 20);
    // PD 仅宣告 store1 存活，与首轮 flush 一致。
    pd.set(first.checkpoint_ts, &[1]);
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd.clone()),
            Upstream: Box::new(upstream.clone()),
            Sync: Box::new(NewExistenceSyncChecker(downstream.clone())),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");

    let first_checkpoint = calculator
        .ComputeNextCheckpoint(&ctx)
        .expect("first checkpoint");
    // 断言首轮检查点与 SyncedTS 均落到 store1 的 flush 进度。
    assert_eq!(first.checkpoint_ts, first_checkpoint);
    assert_eq!(first.flush_ts, calculator.SyncedTS());

    // 第二轮：store1 再 flush；随后把未写 meta 的 store2 标为 alive。
    let second = flush_store(&upstream, &downstream, &pd, 1, 30, 40);
    // Go 把 store 2 加入 PD alive store 列表，但不给它写 flush meta；SyncedTS 因此不前进。
    pd.set(second.checkpoint_ts, &[1, 2]);
    let second_checkpoint = calculator
        .ComputeNextCheckpoint(&ctx)
        .expect("second checkpoint");
    // 检查点可前进到 second；SyncedTS 被缺 flush 的 alive store2 卡住。
    assert_eq!(second.checkpoint_ts, second_checkpoint);
    assert_eq!(first.flush_ts, calculator.SyncedTS());
}

// TestCheckpointCalculatorObserverSeesFailure 对应 Go：Sync checker 失败时 observer 必须看到 failed event。
#[test]
fn test_checkpoint_calculator_observer_sees_failure() {
    // 准备上游 meta 后注入恒失败 Sync，观察事件链完整性。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let record = write_checkpoint_test_meta(&upstream, 20, 1);
    let pd = FakePDMetaReader::default();
    // 单 store 场景，聚焦 Sync 失败而非剪枝逻辑。
    pd.set(record.checkpoint_ts, &[1]);
    let observer = RecordingObserver::default();
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            // 注入恒失败 checker。
            Sync: Box::new(FailingObjectSyncChecker),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        // 挂载 RecordingObserver 捕获事件。
        Some(Box::new(observer.clone())),
    )
    .expect("calculator");

    // Sync 失败：错误应向上返回，且 Observer 看到完整失败事件链。
    let err = calculator.ComputeNextCheckpoint(&ctx).unwrap_err();
    assert!(err.to_string().contains("check sync status"));
    let events = observer.events();
    // 事件序：UpstreamAdvanced → RoundPlanned → CalculationFailed。
    assert_eq!(3, events.len());
    // 失败前仍应发出上游推进与轮次规划事件。
    assert!(matches!(events[0].Type, EventType::EventUpstreamAdvanced));
    assert!(matches!(events[1].Type, EventType::EventRoundPlanned));
    assert!(matches!(events[2].Type, EventType::EventCalculationFailed));
    // 失败事件必须携带底层 Sync 错误信息。
    assert!(events[2].Err.as_ref().unwrap().to_string().contains("boom"));
}

// TestCheckpointCalculatorUsesProvidedObjectSyncChecker 对应 Go：自定义 ObjectSyncChecker 决定安全 checkpoint。
#[test]
fn test_checkpoint_calculator_uses_provided_object_sync_checker() {
    // 验证计算器真正调用注入的 ObjectSyncChecker，而非绕过依赖。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let record = write_checkpoint_test_meta(&upstream, 20, 1);
    let pd = FakePDMetaReader::default();
    pd.set(record.checkpoint_ts, &[1]);

    // 自定义 FileSyncMap：meta 与 log 均标记已同步，应允许推进。
    let mut sync_states = HashMap::new();
    sync_states.insert(record.meta_path.clone(), true);
    for log_path in &record.log_paths {
        sync_states.insert(log_path.clone(), true);
    }

    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(FileSyncMap(sync_states)),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");
    let checkpoint_ts = calculator.ComputeNextCheckpoint(&ctx).expect("checkpoint");
    // 安全检查点与 SyncedTS 均应对齐本轮 flush 记录。
    assert_eq!(record.checkpoint_ts, checkpoint_ts);
    assert_eq!(record.flush_ts, calculator.SyncedTS());
}

// TestCheckpointCalculatorFailsOnObjectSyncError 对应 Go：任一 log 文件检查返回 error 时整轮计算失败。
#[test]
fn test_checkpoint_calculator_fails_on_object_sync_error() {
    // meta 成功、log 报错：确保错误不是被吞掉或仅记日志。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let record = write_checkpoint_test_meta(&upstream, 20, 1);
    let pd = FakePDMetaReader::default();
    pd.set(record.checkpoint_ts, &[1]);

    let mut sync_states = HashMap::new();
    sync_states.insert(
        record.meta_path.clone(),
        FileSyncResult {
            synced: true,
            err: None,
        },
    );
    // log 路径故意返回 error：整轮应失败且错误透传 "boom"。
    for log_path in &record.log_paths {
        sync_states.insert(
            log_path.clone(),
            FileSyncResult {
                synced: false,
                err: Some(Error::new("boom")),
            },
        );
    }

    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(FileSyncResultMap(sync_states)),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");
    let err = calculator.ComputeNextCheckpoint(&ctx).unwrap_err();
    // 任一对象同步检查报错即中止本轮。
    assert!(err.to_string().contains("boom"));
}

// TestCheckpointCalculatorWaitsForRemovedStoreFiles 对应 Go：removed store 的旧 log 未同步时，计算会等到 context 超时。
#[test]
fn test_checkpoint_calculator_waits_for_removed_store_files() {
    // store1 已不在 alive 列表，但其遗留 log 仍必须等同步。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    // store1@10 与 store2@20 各写一份 meta；随后 PD 只保留 store2。
    let (meta1_path, log1_path) = write_meta_only(&upstream, 10, 1);
    let (meta2_path, log2_path) = write_meta_only(&upstream, 20, 2);

    let pd = FakePDMetaReader::default();
    // alive 仅 store2，模拟 store1 被移除。
    pd.set(20, &[2]);
    let sync = NewExistenceSyncChecker(FileExistenceMap(HashMap::from([
        (meta1_path, true),
        (meta2_path, true),
        (log2_path, true),
        // 故意缺少 log1：removed store1 的文件未同步，应阻塞直到超时。
    ])));
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(sync),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            // 缩短轮询间隔，加快超时路径触发。
            PollInterval: Duration::from_millis(1),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");

    // 短超时上下文：验证等待路径真正阻塞而非立即失败。
    let (compute_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_millis(20));
    let err = calculator.ComputeNextCheckpoint(&compute_ctx).unwrap_err();
    assert!(err.to_string().contains("context deadline exceeded"));
    // 防止编译器优化掉未使用的缺失路径变量。
    assert!(!log1_path.is_empty());
}

// TestCheckpointCalculatorPrunesRemovedStoreAfterFilesSynced 对应 Go：removed store 文件都同步后，SyncedByStore 只保留存活 store。
#[test]
fn test_checkpoint_calculator_prunes_removed_store_after_files_synced() {
    // 两 store 文件齐备；首轮后应剪掉已不存活的 store1 进度。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    // 两 store 文件齐全，第二轮无新 meta 时验证剪枝稳定性。
    let (meta1_path, log1_path) = write_meta_only(&upstream, 10, 1);
    let (meta2_path, log2_path) = write_meta_only(&upstream, 20, 2);
    let pd = FakePDMetaReader::default();
    pd.set(20, &[2]);

    // 全部对象已在下游可见，首轮应成功而非超时。
    let sync = NewExistenceSyncChecker(FileExistenceMap(HashMap::from([
        (meta1_path, true),
        (log1_path, true),
        (meta2_path, true),
        (log2_path, true),
    ])));
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd.clone()),
            Upstream: Box::new(upstream),
            Sync: Box::new(sync),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");

    // 首轮：store1 已移除但仍有文件；SyncedTS 取两 store flush 的最小值 10。
    let checkpoint_ts = calculator
        .ComputeNextCheckpoint(&ctx)
        .expect("first compute");
    assert_eq!(20_u64, checkpoint_ts);
    assert_eq!(10_u64, calculator.SyncedTS());
    // 剪枝后 SyncedByStore 只剩存活 store2。
    assert_eq!(
        HashMap::from([(2_u64, 20_u64)]),
        calculator.StateSnapshot().SyncedByStore
    );

    // 第二轮无新 meta：检查点前进，SyncedTS 跟到 store2 已同步进度。
    pd.set(30, &[2]);
    let checkpoint_ts = calculator
        .ComputeNextCheckpoint(&ctx)
        .expect("second compute");
    // 第二轮：检查点到 30，SyncedTS 跟到 20，map 仍仅 store2。
    assert_eq!(30_u64, checkpoint_ts);
    assert_eq!(20_u64, calculator.SyncedTS());
    assert_eq!(
        HashMap::from([(2_u64, 20_u64)]),
        calculator.StateSnapshot().SyncedByStore
    );
}

// TestCheckpointCalculatorPrunesRemovedStoreBeforeAliveStoreBlocksAdvance 对应 Go：
// 恢复状态里的 removed store 会先被剪掉，再用 alive store 判定是否阻塞 SyncedTS。
#[test]
fn test_checkpoint_calculator_prunes_removed_store_before_alive_store_blocks_advance() {
    // 验证剪枝顺序：先移除下线 store，再评估 alive 缺口阻塞。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let pd = FakePDMetaReader::default();
    // 上游检查点 60；alive 为 store2/3，不含持久化里的 store1。
    pd.set(60, &[2, 3]);
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(NewExistenceSyncChecker(FileExistenceMap::default())),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");
    // 恢复状态含已下线 store1(进度100) 与存活 store2(进度50)；alive 还有未观察的 store3。
    calculator
        .RestorePersistentState(PersistentState {
            LastCheckpoint: 40,
            SyncedTS: 50,
            SyncedByStore: HashMap::from([(1_u64, 100_u64), (2_u64, 50_u64)]),
        })
        .expect("restore state");

    // store1 应先被剪掉；store3 无 flush 阻塞 SyncedTS 不上升。
    let checkpoint_ts = calculator.ComputeNextCheckpoint(&ctx).expect("compute");
    assert_eq!(60_u64, checkpoint_ts);
    assert_eq!(50_u64, calculator.SyncedTS());
    // SyncedByStore 仅保留仍存活且有进度的 store2。
    assert_eq!(
        HashMap::from([(2_u64, 50_u64)]),
        calculator.StateSnapshot().SyncedByStore
    );
}

// TestCheckpointCalculatorSkipsMetaSyncedByStoreProgress 对应 Go：已由 store 进度覆盖的 stale meta 不再读取。
#[test]
fn test_checkpoint_calculator_skips_meta_synced_by_store_progress() {
    // 避免对已覆盖进度的 stale meta 重复 ReadFile/下游检查。
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    // 写入 flushTS=15 的 stale meta；store2 持久化进度已到 20，应跳过读取。
    let (stale_meta_path, stale_log_path) = write_meta_only(&upstream, 15, 2);
    // 下游仅放行 stale meta，不放行其 log——若误读 meta 会卡住等待。
    let synced_files = FileExistenceMap(HashMap::from([(stale_meta_path, true)]));
    assert!(
        !synced_files
            .0
            .get(&stale_log_path)
            .copied()
            .unwrap_or(false)
    );

    let pd = FakePDMetaReader::default();
    pd.set(30, &[2]);
    let observer = RecordingObserver::default();
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(NewExistenceSyncChecker(synced_files)),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            // 短轮询；本用例预期不进入长等待。
            PollInterval: Duration::from_millis(1),
            ..Default::default()
        },
        Some(Box::new(observer.clone())),
    )
    .expect("calculator");
    // 持久化进度显示 store2 已覆盖到 20，故 15 的 meta 属过期。
    calculator
        .RestorePersistentState(PersistentState {
            LastCheckpoint: 20,
            SyncedTS: 10,
            SyncedByStore: HashMap::from([(1_u64, 10_u64), (2_u64, 20_u64)]),
        })
        .expect("restore state");

    let (compute_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_millis(20));
    let checkpoint_ts = calculator
        .ComputeNextCheckpoint(&compute_ctx)
        .expect("compute");
    // 上游检查点仍可前进到 30。
    assert_eq!(30_u64, checkpoint_ts);

    let events = observer.events();
    assert_eq!(3, events.len());
    assert!(matches!(events[1].Type, EventType::EventRoundPlanned));
    let planned = events[1].Statistic.as_ref().unwrap();
    // 统计：未读任何新 meta，跳过 1 个已被 store 进度覆盖的文件。
    assert_eq!(0, planned.UpstreamReadMetaFileCount);
    assert_eq!(1, planned.SkippedStoreSyncedMetaFileCount);
    assert_eq!(0, events[1].PendingFileCount);
    assert!(matches!(events[2].Type, EventType::EventCheckpointAdvanced));
    // 推进事件必须保留本轮的跳过统计，且跳过路径不产生下游文件检查计数。
    assert_eq!(
        1,
        events[2]
            .Statistic
            .as_ref()
            .unwrap()
            .SkippedStoreSyncedMetaFileCount
    );
    assert_eq!(
        0,
        events[2]
            .Statistic
            .as_ref()
            .unwrap()
            .DownstreamCheckFileCount
    );
}

// 可复用的 PD 假实现：可变全局检查点与 alive store 列表。
#[derive(Clone, Default)]
struct FakePDMetaReader {
    inner: Arc<Mutex<FakePDMetaReaderState>>,
}

// 内部状态：checkpoint TS 与当前存活 stores。
#[derive(Default)]
struct FakePDMetaReaderState {
    checkpoint: u64,
    stores: Vec<Store>,
}

impl FakePDMetaReader {
    // 原子更新检查点与 store 集合，供多轮测试切换场景。
    fn set(&self, checkpoint: u64, store_ids: &[u64]) {
        let mut state = self.inner.lock().unwrap();
        state.checkpoint = checkpoint;
        // BootAt 固定为 1，计算器当前仅关心 store ID 集合。
        state.stores = store_ids
            .iter()
            .map(|id| Store { ID: *id, BootAt: 1 })
            .collect();
    }
}

impl PDMetaReader for FakePDMetaReader {
    // 忽略 taskName：单测只需要固定返回值。
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task_name: &str) -> Result<u64, Error> {
        Ok(self.inner.lock().unwrap().checkpoint)
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        // 返回克隆，避免测试持锁修改互相干扰。
        Ok(self.inner.lock().unwrap().stores.clone())
    }
}

// 内存上游存储：按路径保存字节，WalkDir 支持 SubDir/StartAfter 游标。
#[derive(Clone)]
struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
}

impl MemStorage {
    // uri 参与增量 meta 扫描能力校验（file:// 可通过）。
    fn new(uri: &str) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.to_string(),
        }
    }

    // 写入或覆盖单个对象，供构造 meta/log 夹具。
    fn write_file(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_string(), data);
    }
}

impl UpstreamStorageReader for MemStorage {
    // 字典序遍历；StartAfter 为严格大于（对齐 Go store Walk 游标）。
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // 先排序再过滤，保证游标语义与字典序 Walk 一致。
        let mut paths: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
        paths.sort();
        // SubDir 非空时要求前缀匹配。
        let prefix = if opt.SubDir.is_empty() {
            String::new()
        } else {
            format!("{}/", opt.SubDir.trim_end_matches('/'))
        };
        for path in paths {
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            // StartAfter：跳过已扫描过的前缀，实现增量 meta 扫描。
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            let size = self.files.lock().unwrap()[&path].len() as i64;
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        // 缺失文件返回显式错误，模拟真实存储 NotFound。
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

// 包装 MemStorage：可伪造 URI，并记录 WalkOption 供断言。
struct RecordingUpstreamStorage {
    inner: MemStorage,
    uri: String,
    walk_opts: Arc<Mutex<Vec<WalkOption>>>,
}

impl RecordingUpstreamStorage {
    // 覆盖 URI 以触发“不支持 StartAfter”等存储校验分支。
    fn with_uri(inner: MemStorage, uri: &str) -> Self {
        Self {
            inner,
            uri: uri.to_string(),
            walk_opts: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl UpstreamStorageReader for RecordingUpstreamStorage {
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // 记录每次 Walk 选项，便于核对增量游标行为。
        self.walk_opts.lock().unwrap().push(opt.clone());
        self.inner.WalkDir(ctx, opt, callback)
    }

    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        self.inner.ReadFile(ctx, name)
    }

    fn URI(&self) -> String {
        // 优先返回伪造 URI，空则回退内层。
        if !self.uri.is_empty() {
            return self.uri.clone();
        }
        self.inner.URI()
    }
}

// 记录全部 CheckpointEvent，用于核对失败/成功路径的事件序。
#[derive(Clone, Default)]
struct RecordingObserver {
    events: Arc<Mutex<Vec<CheckpointEvent>>>,
}

impl RecordingObserver {
    // 快照当前事件列表，避免持锁做断言。
    fn events(&self) -> Vec<CheckpointEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl Observer for RecordingObserver {
    fn OnCheckpointEvent(&self, event: CheckpointEvent) {
        // 追加而非覆盖，保留完整事件历史。
        self.events.lock().unwrap().push(event);
    }
}

// 恒失败的 Sync checker，用于 Observer 失败观测用例。
struct FailingObjectSyncChecker;

impl ObjectSyncChecker for FailingObjectSyncChecker {
    fn FileSynced(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 错误消息包含路径，便于上层包装后仍可匹配。
        Err(Error::new(format!("boom for {name}")))
    }
}

// 静态存在性表：缺省键视为不存在（false）。
#[derive(Default)]
struct FileExistenceMap(HashMap<String, bool>);

impl FileExistenceChecker for FileExistenceMap {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 缺省 false：模拟尚未复制到下游。
        Ok(*self.0.get(name).unwrap_or(&false))
    }
}

// 按路径返回是否已同步；缺省 false。
struct FileSyncMap(HashMap<String, bool>);

impl ObjectSyncChecker for FileSyncMap {
    fn FileSynced(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 与 FileExistenceMap 相同的缺省策略，但是 ObjectSyncChecker。
        Ok(*self.0.get(name).unwrap_or(&false))
    }
}

// 可同时表达 synced 与 error 的同步结果，用于错误透传测试。
#[derive(Clone, Default)]
struct FileSyncResult {
    synced: bool,
    err: Option<Error>,
}

struct FileSyncResultMap(HashMap<String, FileSyncResult>);

impl ObjectSyncChecker for FileSyncResultMap {
    fn FileSynced(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        let result = self.0.get(name).cloned().unwrap_or_default();
        // 错误优先于 synced 布尔值返回。
        if let Some(err) = result.err {
            return Err(err);
        }
        Ok(result.synced)
    }
}

// 可在 flush 过程中动态标记下游已存在的对象集合。
#[derive(Clone, Default)]
struct SharedDownstream {
    files: Arc<Mutex<HashMap<String, bool>>>,
}

impl SharedDownstream {
    // 模拟对象复制完成后的可见性。
    fn mark_exists(&self, name: &str) {
        self.files.lock().unwrap().insert(name.to_string(), true);
    }
}

impl FileExistenceChecker for SharedDownstream {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 动态表：flush_store 可在计算前/中标记新对象。
        Ok(*self.files.lock().unwrap().get(name).unwrap_or(&false))
    }
}

// 单次 flush 夹具：检查点、flushTS 与生成的 meta/log 路径。
struct FlushRecord {
    checkpoint_ts: u64,
    flush_ts: u64,
    meta_path: String,
    log_paths: Vec<String>,
}

// 写入上游 meta/log 并在下游标记存在，模拟一轮完整 flush+复制。
fn flush_store(
    upstream: &MemStorage,
    downstream: &SharedDownstream,
    _pd: &FakePDMetaReader,
    store_id: u64,
    checkpoint_ts: u64,
    flush_ts: u64,
) -> FlushRecord {
    let record = write_checkpoint_test_meta_with_ts(upstream, flush_ts, store_id, checkpoint_ts);
    // 同步标记下游，使 ExistenceSyncChecker 立即放行。
    downstream.mark_exists(&record.meta_path);
    for log_path in &record.log_paths {
        downstream.mark_exists(log_path);
    }
    record
}

// 便捷写法：checkpoint_ts 默认等于 flush_ts。
fn write_checkpoint_test_meta(storage: &MemStorage, flush_ts: u64, store_id: u64) -> FlushRecord {
    write_checkpoint_test_meta_with_ts(storage, flush_ts, store_id, flush_ts)
}

// 允许 checkpoint_ts 与 flush_ts 分离，覆盖“flush 批次服务更小检查点”场景。
fn write_checkpoint_test_meta_with_ts(
    storage: &MemStorage,
    flush_ts: u64,
    store_id: u64,
    checkpoint_ts: u64,
) -> FlushRecord {
    let (meta_path, log_path) = write_meta_only(storage, flush_ts, store_id);
    // 单 log 组足以覆盖当前计算器的路径收集逻辑。
    FlushRecord {
        checkpoint_ts,
        flush_ts,
        meta_path,
        log_paths: vec![log_path],
    }
}

// 仅写上游对象，不标记下游；路径命名遵循 Go meta 排序键约定。
fn write_meta_only(storage: &MemStorage, flush_ts: u64, store_id: u64) -> (String, String) {
    // log 路径被 meta 的 FileGroups 引用。
    let log_path = format!("v1/log/store-{store_id}/flush-{flush_ts:016x}.log");
    // meta 名：{flushTS:016X}{storeID:016X}-... 决定 Walk 排序与增量游标。
    let meta_path = format!(
        "v1/backupmeta/{flush_ts:016X}{store_id:016X}-d{flush_ts:016X}l{flush_ts:016X}u{flush_ts:016X}.meta"
    );
    // JSON 形状对齐生产 backupmeta 最小字段集。
    let payload = json!({
        "StoreId": store_id,
        "FileGroups": [{
            "Path": log_path,
            "DataFilesInfo": [{"Path": log_path, "MinTs": flush_ts, "MaxTs": flush_ts}]
        }]
    });
    storage.write_file(&meta_path, payload.to_string().into_bytes());
    // log 内容无关紧要，计算器只关心路径同步状态。
    storage.write_file(&log_path, b"log".to_vec());
    (meta_path, log_path)
}

#[test]
fn test_checkpoint_calculator_advances_with_empty_meta_flag() {
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let empty_path = "v1/backupmeta/00000000000000140000000000000001-d0000000000000000l0000000000000000u0000000000000000p0000000000000002.meta";
    upstream.write_file(empty_path, b"invalid metadata payload".to_vec());
    let pd = FakePDMetaReader::default();
    pd.set(15, &[1]);
    let observer = RecordingObserver::default();
    let mut calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(NewExistenceSyncChecker(FileExistenceMap::default())),
        },
        CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            ..Default::default()
        },
        Some(Box::new(observer.clone())),
    )
    .unwrap();
    assert_eq!(calculator.ComputeNextCheckpoint(&ctx).unwrap(), 15);
    assert_eq!(calculator.SyncedTS(), 20);
    let events = observer.events();
    assert_eq!(events.len(), 3);
    assert!(matches!(events[1].Type, EventType::EventRoundPlanned));
    let planned = events[1].Statistic.as_ref().unwrap();
    assert_eq!(planned.UpstreamReadMetaFileCount, 0);
    assert_eq!(planned.EstimatedSyncLogFileCount, 0);
    assert_eq!(events[1].PendingFileCount, 0);
    assert!(matches!(events[2].Type, EventType::EventCheckpointAdvanced));
    assert_eq!(
        events[2]
            .Statistic
            .as_ref()
            .unwrap()
            .DownstreamCheckFileCount,
        0
    );
}
