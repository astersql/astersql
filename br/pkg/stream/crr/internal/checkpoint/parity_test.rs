// Copyright 2026 AsterSQL.

//! Go/Rust 公共契约对照：锁定 checkpoint 包对外默认值、游标与主路径语义。
//! 覆盖默认常量、path_suffix 分桶、meta_scan_start_after 大写十六进制、
//! 依赖校验错误、持久化恢复守卫、正常推进、等待上游、Sync 失败事件链、
//! ExistenceSyncChecker 适配、缺 alive store 阻塞 SyncedTS，以及等待超时。
//! 使用内存夹具复现 Go 调用顺序与错误文案，不改测试行为。
//! 断言同时核对数值结果与 Observer 事件类型，防止静默漂移。
//! 夹具刻意轻量：不引入 IntegrationHarness，聚焦公开 API 契约面。
//! 错误文案子串是跨语言稳定性锚点，修改实现时需同步更新本测试。
//! WaitingUpstream 路径验证“无进展不报错”，避免调用方把空转当失败。
//! 超时路径验证等待循环尊重 Context deadline，而非无限自旋。
//! ExistenceSyncChecker 段锁定适配器而非生产复制语义。
//! SyncedByStore 阻塞段是 alive-store 额外 blocker 的最小复现。
//! 本文件单测聚合多场景于一个函数，便于对照 Go 公共契约清单。
//! 任一子场景失败应优先怀疑跨语言默认值或事件序列漂移。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_streamhelper::Store;
use serde_json::json;

use crate::progress::path_suffix;
use crate::storage::meta_scan_start_after;
use crate::{
    Calculator, CalculatorDeps, CheckpointCalculatorConfig, CheckpointEvent, Context,
    DefaultMetaReadConcurrency, DefaultPollInterval, Error, EventType, ExistenceSyncChecker,
    FileExistenceChecker, NewCalculator, NewExistenceSyncChecker, ObjectSyncChecker, PDMetaReader,
    PersistentState, UpstreamStorageReader, WalkOption,
};

// 内存上游存储：字典序 Walk，支持 SubDir/StartAfter。
#[derive(Clone, Default)]
struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
}

impl MemStorage {
    // uri 用于增量 meta 扫描能力校验。
    fn new(uri: &str) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.to_string(),
        }
    }

    // 覆盖写入单个对象字节。
    fn write_file(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_string(), data);
    }
}

impl UpstreamStorageReader for MemStorage {
    // StartAfter 为严格大于，对齐增量游标。
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // 排序后过滤，保证游标与字典序一致。
        let mut paths: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
        paths.sort();
        let prefix = if opt.SubDir.is_empty() {
            String::new()
        } else {
            format!("{}/", opt.SubDir.trim_end_matches('/'))
        };
        for path in paths {
            // SubDir 前缀过滤。
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            // 跳过已扫描前缀，保证第二轮不重复读。
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            let size = self.files.lock().unwrap()[&path].len() as i64;
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        // 缺失返回显式错误，便于上层包装。
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }

    fn URI(&self) -> String {
        // 返回构造时注入的存储 URI。
        self.uri.clone()
    }
}

// 简易 PD：可变全局检查点与 alive stores。
#[derive(Default)]
struct FakePD {
    checkpoint: Mutex<u64>,
    stores: Mutex<Vec<Store>>,
}

impl FakePD {
    // 测试中切换上游进度与存活集合。
    fn set(&self, checkpoint: u64, store_ids: &[u64]) {
        *self.checkpoint.lock().unwrap() = checkpoint;
        // BootAt 固定占位，计算器当前只看 ID。
        *self.stores.lock().unwrap() = store_ids
            .iter()
            .map(|id| Store { ID: *id, BootAt: 1 })
            .collect();
    }
}

impl PDMetaReader for FakePD {
    // 忽略 taskName，返回注入的检查点。
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task_name: &str) -> Result<u64, Error> {
        Ok(*self.checkpoint.lock().unwrap())
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        // 克隆列表，避免持锁跨断言。
        Ok(self.stores.lock().unwrap().clone())
    }
}

// 按路径返回同步布尔；缺省 false。
struct SyncMap(HashMap<String, bool>);

impl ObjectSyncChecker for SyncMap {
    fn FileSynced(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 未登记路径视为尚未同步。
        Ok(*self.0.get(name).unwrap_or(&false))
    }
}

// 恒失败 Sync，用于失败事件链对照。
struct FailingSyncChecker;

impl ObjectSyncChecker for FailingSyncChecker {
    fn FileSynced(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 错误带路径，包装后仍可匹配 "boom"。
        Err(Error::new(format!("boom for {name}")))
    }
}

// 记录全部进度事件供序列断言。
#[derive(Clone)]
struct RecordingObserver {
    events: Arc<Mutex<Vec<CheckpointEvent>>>,
}

impl RecordingObserver {
    // 空事件缓冲起点。
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // 快照事件，避免断言时持锁。
    fn events(&self) -> Vec<CheckpointEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl crate::Observer for RecordingObserver {
    fn OnCheckpointEvent(&self, event: CheckpointEvent) {
        // 追加保留完整历史。
        self.events.lock().unwrap().push(event);
    }
}

// 写入对齐 Go 命名的 meta（及返回 log 路径）；parity 不强制写 log 字节。
fn write_checkpoint_test_meta(
    storage: &MemStorage,
    flush_ts: u64,
    store_id: u64,
) -> (String, String) {
    // log 路径被 FileGroups 引用。
    let log_path = format!("v1/log/store-{store_id}/flush-{flush_ts:016x}.log");
    // meta 名前导键为 flushTS+storeID，决定 Walk 排序。
    let meta_path = format!(
        "v1/backupmeta/{flush_ts:016X}{store_id:016X}-d{flush_ts:016X}l{flush_ts:016X}u{flush_ts:016X}.meta"
    );
    // 最小 backupmeta JSON 字段集。
    let payload = json!({
        "StoreId": store_id,
        "FileGroups": [{
            "Path": log_path,
            "DataFilesInfo": [{"Path": log_path, "MinTs": flush_ts, "MaxTs": flush_ts}]
        }]
    });
    storage.write_file(&meta_path, payload.to_string().into_bytes());
    (meta_path, log_path)
}

#[test]
fn go_rust_public_contract_matches() {
    // 默认常量：与 Go DefaultPollInterval / DefaultMetaReadConcurrency 一致。
    assert_eq!(DefaultPollInterval, Duration::from_secs(2));
    assert_eq!(DefaultMetaReadConcurrency, 16);

    // 路径后缀分桶：.log / 无后缀 / 过长后缀归入 <other>。
    assert_eq!(path_suffix("a/b/c.log"), ".log");
    assert_eq!(path_suffix("a/b/c"), "<none>");
    assert_eq!(path_suffix("a/b/c.verylong"), "<other>");

    // StartAfter 游标使用大写十六进制；0 表示无游标。
    let stalled = 0x06745A03F7B80004u64;
    assert_eq!(
        meta_scan_start_after(stalled),
        "v1/backupmeta/06745A03F7B80004FFFFFFFFFFFFFFFF~"
    );
    assert!(meta_scan_start_after(0).is_empty());

    // 依赖校验：不支持 StartAfter 的 URI 应在构造期失败。
    let ctx = Context::Background();
    let upstream = MemStorage::new("azure://bucket/prefix/");
    let pd = FakePD::default();
    let err = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(SyncMap(HashMap::new())),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("StartAfter-capable upstream storage")
    );

    // 空任务名拒绝构造。
    let upstream = MemStorage::new("file:///tmp/upstream");
    let err = NewCalculator(
        CalculatorDeps {
            PD: Box::new(FakePD::default()),
            Upstream: Box::new(upstream),
            Sync: Box::new(SyncMap(HashMap::new())),
        },
        CheckpointCalculatorConfig {
            TaskName: String::new(),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("task name must not be empty"));

    // 持久化恢复守卫：计算已开始后禁止 Restore。
    let upstream = MemStorage::new("file:///tmp/upstream");
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(FakePD::default()),
            Upstream: Box::new(upstream),
            Sync: Box::new(SyncMap(HashMap::new())),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    // 人为标记已开始计算。
    calc.state.last_checkpoint = 1;
    let err = calc
        .RestorePersistentState(PersistentState::default())
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot restore persistent state after checkpoint calculation started")
    );

    // 正常轮：检查点前进且 SyncedTS 跟随 store flush。
    let upstream = MemStorage::new("file:///tmp/upstream");
    // 单 store@20 的完整可同步夹具。
    let (meta_path, log_path) = write_checkpoint_test_meta(&upstream, 20, 1);
    let pd = FakePD::default();
    pd.set(20, &[1]);
    let mut sync = HashMap::new();
    // meta/log 均标记已同步。
    sync.insert(meta_path.clone(), true);
    sync.insert(log_path.clone(), true);
    // 挂载 Observer 以便后续扩展事件断言（本段主要锁数值）。
    let observer = RecordingObserver::new();
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(SyncMap(sync)),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            PollInterval: Duration::from_millis(1),
            ..Default::default()
        },
        Some(Box::new(observer.clone())),
    )
    .unwrap();
    let checkpoint = calc.ComputeNextCheckpoint(&ctx).unwrap();
    // 数值三件套应对齐到 20。
    assert_eq!(checkpoint, 20);
    assert_eq!(calc.SyncedTS(), 20);
    assert_eq!(calc.LastCheckpoint(), 20);
    let _ = observer.events();

    // 上游未推进：返回旧检查点并发出 WaitingUpstream。
    // PD 检查点=5，且内部 last 也将设为 5。
    let upstream = MemStorage::new("file:///tmp/upstream");
    let pd = FakePD::default();
    pd.set(5, &[1]);
    let observer = RecordingObserver::new();
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(SyncMap(HashMap::new())),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        Some(Box::new(observer.clone())),
    )
    .unwrap();
    // 将内部 last 设为与 PD 相同，模拟无进展。
    calc.state.last_checkpoint = 5;
    let checkpoint = calc.ComputeNextCheckpoint(&ctx).unwrap();
    assert_eq!(checkpoint, 5);
    let events = observer.events();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].Type, EventType::EventWaitingUpstream));

    // Sync 失败：错误返回且事件链含 CalculationFailed。
    let upstream = MemStorage::new("file:///tmp/upstream");
    // 写入 meta 后注入失败 Sync。
    write_checkpoint_test_meta(&upstream, 10, 1);
    let pd = FakePD::default();
    pd.set(10, &[1]);
    let observer = RecordingObserver::new();
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            // 强制走失败事件链。
            Sync: Box::new(FailingSyncChecker),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        Some(Box::new(observer.clone())),
    )
    .unwrap();
    let err = calc.ComputeNextCheckpoint(&ctx).unwrap_err();
    assert!(err.to_string().contains("check sync status"));
    let events = observer.events();
    // UpstreamAdvanced → RoundPlanned → CalculationFailed。
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0].Type, EventType::EventUpstreamAdvanced));
    assert!(matches!(events[1].Type, EventType::EventRoundPlanned));
    assert!(matches!(events[2].Type, EventType::EventCalculationFailed));
    assert!(events[2].Err.as_ref().unwrap().to_string().contains("boom"));

    // ExistenceSyncChecker：存在即同步。
    // 局部存在性表，仅用于适配器冒烟。
    struct Exists(HashMap<String, bool>);
    impl FileExistenceChecker for Exists {
        fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
            Ok(*self.0.get(name).unwrap_or(&false))
        }
    }
    let checker = NewExistenceSyncChecker(Exists(HashMap::from([(meta_path.clone(), true)])));
    assert!(checker.FileSynced(&ctx, &meta_path).unwrap());

    // 新增未观察的 alive store 阻塞 SyncedTS，但检查点仍可前进。
    let upstream = MemStorage::new("file:///tmp/upstream");
    // 首轮仅 store1@10，建立基准 SyncedTS。
    let (m1, l1) = write_checkpoint_test_meta(&upstream, 10, 1);
    let pd = FakePD::default();
    pd.set(10, &[1]);
    let mut sync = HashMap::new();
    sync.insert(m1.clone(), true);
    sync.insert(l1.clone(), true);
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream.clone()),
            Sync: Box::new(SyncMap(sync)),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(calc.ComputeNextCheckpoint(&ctx).unwrap(), 10);
    assert_eq!(calc.SyncedTS(), 10);

    // 第二轮写入 store1@20，同时 PD 宣告 store2 alive 但无 flush。
    let (m2, l2) = write_checkpoint_test_meta(&upstream, 20, 1);
    let pd = FakePD::default();
    pd.set(20, &[1, 2]);
    // 热替换 PD/Sync 依赖，模拟运行中拓扑变化。
    calc.deps.PD = Box::new(pd);
    let mut sync = HashMap::new();
    sync.insert(m1, true);
    sync.insert(l1, true);
    sync.insert(m2, true);
    sync.insert(l2, true);
    calc.deps.Sync = Box::new(SyncMap(sync));
    assert_eq!(calc.ComputeNextCheckpoint(&ctx).unwrap(), 20);
    // SyncedTS 仍停在 10，被缺进度的 store2 卡住。
    assert_eq!(calc.SyncedTS(), 10);

    // 资源/取消：等待同步时超时。
    let upstream = MemStorage::new("file:///tmp/upstream");
    // store1@10 未同步 log；store2@20 在 alive 列表中。
    let (meta_path, log_path) = write_checkpoint_test_meta(&upstream, 10, 1);
    let (_meta2, _log2) = write_checkpoint_test_meta(&upstream, 20, 2);
    let pd = FakePD::default();
    // 仅宣告 store2，逼近 removed-store 等待路径。
    pd.set(20, &[2]);
    let mut sync = HashMap::new();
    // store1 的 log 故意未同步，触发等待循环。
    sync.insert(meta_path, true);
    sync.insert(log_path, false);
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(pd),
            Upstream: Box::new(upstream),
            Sync: Box::new(SyncMap(sync)),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            // 缩短轮询，加快超时触发。
            PollInterval: Duration::from_millis(5),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    // 短超时：验证阻塞等待而非立即返回旧值。
    let (timeout_ctx, _cancel) = Context::WithTimeout(&ctx, Duration::from_millis(20));
    let err = calc.ComputeNextCheckpoint(&timeout_ctx).unwrap_err();
    assert!(err.to_string().contains("context deadline exceeded"));
}
