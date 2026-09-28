// Copyright 2026 AsterSQL.

//! CRR service 的 Go/Rust 公共契约对照测试。
//! 覆盖：HTTP 探针与状态 JSON、默认 RetryInterval、nil mux panic、错误降级、关机落盘与指标。
//! 边界用静态 PD/空 Upstream/可失败 Sync/内存 ResumeState，避免真实网络。
//! 断言依据与 Go parity 一致：状态字段名、503/200 语义、连续失败计数与指标刷写。
//! 不改行为：仅通过场景编排验证既有 API 契约。
//! 场景划分对应 Go parity 的正常/边界/错误/清理四组，便于两侧对照失败点。
//! HTTP 断言只关心状态码与关键 JSON 子串，不绑定完整序列化顺序。
//! ResumeState 失败注入验证服务在 Persist 抖动下仍可继续并最终落盘。
//! ImmediateWatcher 不推进 PD，专注服务生命周期与取消语义。
//! FailingSyncChecker 覆盖下游错误传播到状态机的路径。
//! StaticPD/EmptyUpstream 把计算路径压成“几乎空跑”，突出服务层契约。
//! TestMux/TestWriter 是纯内存替身，不绑定具体 HTTP 框架。
//! 指标读取走 crate 内 skipped_* helper，确认 Status→metrics 桥接。
//! 若契约变更，应同步更新 Go parity 与本文件注释中的预期说明。
//! 本文件故意保持与实现分文件，符合仓库“测试与源码分离”约定。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    CalculatorDeps, CheckpointCalculatorConfig, CheckpointEvent, Context, Error, EventType,
    NewCalculator, PDMetaReader, PersistentState, Store, UpstreamStorageReader, WalkOption,
};

use crate::Config;
use crate::http::{
    HttpMux, HttpRequest, HttpResponseWriter, RegisterOrPanic, STATUS_OK,
    STATUS_SERVICE_UNAVAILABLE,
};
use crate::metrics::skipped_store_synced_meta_file_count_metric;
use crate::service::{Deps, New, ResumeStateStore, Service, UpstreamCheckpointWaiter};
use crate::status::{
    GetStatusFileName, STATE_DEGRADED, STATE_STOPPED, encode_status_snapshot, new_status_store,
};

#[test]
/// 总入口：串联四组契约场景，失败即整体失败。
fn go_rust_public_contract_matches() {
    contract_normal_status_and_http();
    contract_boundary_defaults_and_register_guard();
    contract_error_paths_and_no_double_count();
    contract_resource_cleanup_on_shutdown();
}

/// 正常路径：未启动 livez=503；推进后 readyz=200；失败后 readyz=503；status JSON 含关键字段。
fn contract_normal_status_and_http() {
    // 与 Go 常量一致的 resume 文件相对路径。
    assert_eq!(GetStatusFileName(), "crr-checkpoint/resume-state.json");

    let (status, _) = new_status_store("task");
    // 手工拼装 Service：绕过 New，直接注入已绑定的 StatusObserver。
    let svc = Arc::new(Service {
        calc: Mutex::new(
            NewCalculator(
                CalculatorDeps {
                    PD: Box::new(StaticPD { checkpoint: 1 }),
                    Upstream: Box::new(EmptyUpstream),
                    Sync: Box::new(StaticSync { synced: true }),
                },
                CheckpointCalculatorConfig {
                    TaskName: "task".into(),
                    // 缩短轮询以便测试快速结束。
                    PollInterval: Duration::from_millis(5),
                    ..Default::default()
                },
                None,
            )
            .expect("calculator"),
        ),
        status: status.clone(),
        observer: crate::status::StatusObserver {
            store: status.clone(),
        },
        pd: Box::new(ImmediateWatcher),
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

    // start 前 Live=false，livez 必须 503。
    let mut live = TestWriter::default();
    mux.serve("/livez", &mut live);
    assert_eq!(STATUS_SERVICE_UNAVAILABLE, live.status);

    // 启动后开始接受事件；随后模拟成功推进。
    status.start();
    status.apply_event(CheckpointEvent {
        // 成功推进事件：写入上游/同步水位与空统计。
        Type: EventType::EventCheckpointAdvanced,
        Time: Some(SystemTime::now()),
        UpstreamCheckpoint: 42,
        SyncedTS: 42,
        Statistic: Some(Default::default()),
        ..Default::default()
    });
    // 持久化镜像与事件一致，供 Status 快照读取。
    status.set_persistent_state(PersistentState {
        LastCheckpoint: 42,
        SyncedTS: 42,
        SyncedByStore: HashMap::from([(1, 42)]),
    });

    // 推进后 Ready=true，readyz 应为 200。
    let mut ready = TestWriter::default();
    mux.serve("/readyz", &mut ready);
    assert_eq!(STATUS_OK, ready.status);

    status.apply_event(CheckpointEvent {
        // 计算失败应使 Ready=false。
        Type: EventType::EventCalculationFailed,
        Time: Some(SystemTime::now()),
        Err: Some(Error::new("boom")),
        ..Default::default()
    });
    // 降级后 readyz 回到 503。
    let mut degraded = TestWriter::default();
    mux.serve("/readyz", &mut degraded);
    assert_eq!(STATUS_SERVICE_UNAVAILABLE, degraded.status);

    // /status 在降级时仍应 200 返回 JSON。
    let mut status_resp = TestWriter::default();
    mux.serve("/status", &mut status_resp);
    assert_eq!(STATUS_OK, status_resp.status);
    let body = String::from_utf8(status_resp.body).expect("utf8");
    // snake_case 字段名与 Go JSON 标签一致。
    assert!(body.contains("\"safe_checkpoint\":42"));
    assert!(body.contains("\"task_name\":\"task\""));
}

/// 边界：RetryInterval=0 被钳到 1s；nil mux panic；Run 可被取消干净退出。
fn contract_boundary_defaults_and_register_guard() {
    let service = Arc::new(
        New(
            Deps {
                PD: Box::new(StaticPD { checkpoint: 0 }),
                Watcher: Box::new(ImmediateWatcher),
                Upstream: Box::new(EmptyUpstream),
                Sync: Box::new(StaticSync { synced: true }),
                State: None,
            },
            Config {
                CalculatorConfig: CheckpointCalculatorConfig {
                    TaskName: "task".into(),
                    ..Default::default()
                },
                // 零值触发默认填充为 1s。
                RetryInterval: Duration::ZERO,
            },
        )
        .expect("service"),
    );
    // New 应对零 RetryInterval 填默认 1 秒。
    assert_eq!(Duration::from_secs(1), service.cfg.RetryInterval);

    // 直接调用真实注册入口，校验与 Go `svc.Register(nil)` 相同的 panic 文案。
    let nil_mux_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RegisterOrPanic::<TestMux>(&service, None);
    }));
    let panic_message = nil_mux_panic
        .expect_err("nil mux must panic")
        .downcast::<&'static str>()
        .expect("panic payload");
    assert_eq!("service: nil mux", *panic_message);

    let ctx = Context::Background();
    // 短跑 Run：ImmediateWatcher 自旋直到取消。
    let (cancel_ctx, cancel) = Context::WithCancel(&ctx);
    let service = New(
        Deps {
            PD: Box::new(StaticPD { checkpoint: 10 }),
            Watcher: Box::new(ImmediateWatcher),
            Upstream: Box::new(EmptyUpstream),
            Sync: Box::new(StaticSync { synced: true }),
            State: None,
        },
        Config {
            CalculatorConfig: CheckpointCalculatorConfig {
                TaskName: "task".into(),
                PollInterval: Duration::from_millis(5),
                ..Default::default()
            },
            RetryInterval: Duration::from_millis(5),
        },
    )
    .expect("service");

    let handle = std::thread::spawn(move || service.Run(&cancel_ctx));
    std::thread::sleep(Duration::from_millis(20));
    cancel();
    // 取消后 Run 应 Ok 返回，无泄漏 panic。
    assert!(handle.join().expect("join").is_ok());
}

/// 错误路径：失败→Degraded；RoundPlanned 不抹掉 AliveStoreCount；失败 Sync 下 Run 仍可取消。
fn contract_error_paths_and_no_double_count() {
    let (status, observer) = new_status_store("task");
    status.start();
    // 经 Observer 入口注入失败事件，与计算器回调路径一致。
    observer.OnCheckpointEvent(CheckpointEvent {
        Type: EventType::EventCalculationFailed,
        Time: Some(SystemTime::now()),
        Err: Some(Error::new("boom")),
        ..Default::default()
    });
    let snapshot = status.snapshot_copy();
    // 首次失败进入 DEGRADED 且 ConsecutiveFailures=1。
    assert_eq!(STATE_DEGRADED, snapshot.State);
    assert!(!snapshot.Ready);
    assert_eq!(1, snapshot.ConsecutiveFailures);
    assert_eq!("boom", snapshot.LastError);

    status.apply_event(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        Time: Some(SystemTime::now()),
        // Planned 写入 3；后续 Failed 不得清零该字段。
        AliveStoreCount: 3,
        ..Default::default()
    });
    status.apply_event(CheckpointEvent {
        Type: EventType::EventCalculationFailed,
        Time: Some(SystemTime::now()),
        Err: Some(Error::new("boom")),
        ..Default::default()
    });
    // 证明 Failed 事件未覆盖 Planned 的 AliveStoreCount。
    assert_eq!(3, status.snapshot_copy().AliveStoreCount);

    status.apply_event(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        Time: Some(SystemTime::now()),
        // 显式 Planned 到 0 才应反映为 0。
        AliveStoreCount: 0,
        ..Default::default()
    });
    // 仅 Planned(0) 可将计数归零。
    assert_eq!(0, status.snapshot_copy().AliveStoreCount);

    let ctx = Context::Background();
    let (cancel_ctx, cancel) = Context::WithCancel(&ctx);
    let service = New(
        Deps {
            PD: Box::new(StaticPD { checkpoint: 5 }),
            Watcher: Box::new(ImmediateWatcher),
            Upstream: Box::new(EmptyUpstream),
            // 下游检查恒失败，迫使计算错误重试。
            Sync: Box::new(FailingSyncChecker),
            State: None,
        },
        Config {
            CalculatorConfig: CheckpointCalculatorConfig {
                TaskName: "task".into(),
                PollInterval: Duration::from_millis(5),
                ..Default::default()
            },
            RetryInterval: Duration::from_millis(5),
        },
    )
    .expect("service");
    let handle = std::thread::spawn(move || service.Run(&cancel_ctx));
    std::thread::sleep(Duration::from_millis(30));
    cancel();
    assert!(handle.join().expect("join").is_ok());
}

/// 关机清理：SaveState 允许一次失败后仍最终落盘；指标与 stop 状态正确。
fn contract_resource_cleanup_on_shutdown() {
    // save_failures=1：首次 Persist 失败，后续成功。
    let store = MemoryResumeStateStore::with_state_and_save_failures(
        PersistentState {
            LastCheckpoint: 1,
            SyncedTS: 1,
            SyncedByStore: HashMap::from([(1, 1)]),
        },
        1,
    );
    let ctx = Context::Background();
    let (cancel_ctx, cancel) = Context::WithCancel(&ctx);
    let service = New(
        Deps {
            PD: Box::new(StaticPD { checkpoint: 2 }),
            Watcher: Box::new(ImmediateWatcher),
            Upstream: Box::new(EmptyUpstream),
            Sync: Box::new(StaticSync { synced: true }),
            State: Some(Box::new(store.clone())),
        },
        Config {
            CalculatorConfig: CheckpointCalculatorConfig {
                TaskName: "task".into(),
                PollInterval: Duration::from_millis(5),
                ..Default::default()
            },
            RetryInterval: Duration::from_millis(20),
        },
    )
    .expect("service");
    let handle = std::thread::spawn(move || service.Run(&cancel_ctx));
    std::thread::sleep(Duration::from_millis(30));
    cancel();
    assert!(handle.join().expect("join").is_ok());
    // 首次失败后，重试或关机清理必须真正成功落盘，而不只是发生过尝试。
    assert!(store.save_count() >= 2);
    assert!(store.successful_save_count() >= 1);

    let (status, _) = new_status_store("task");
    status.start();
    status.apply_event(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        Time: Some(SystemTime::now()),
        Statistic: Some(
            astersql_br_pkg_stream_crr_internal_checkpoint::FileStatistic {
                // RoundPlanned 携带 Statistic 时应刷 skipped_* 指标。
                SkippedStoreSyncedMetaFileCount: 4,
                ..Default::default()
            },
        ),
        ..Default::default()
    });
    // 指标名与 Go prometheus 标签对齐。
    assert_eq!(4.0, skipped_store_synced_meta_file_count_metric("task"));

    let snapshot = status.snapshot_copy();
    let encoded = encode_status_snapshot(&snapshot).expect("json");
    // JSON 需包含 statistic 对象字段。
    assert!(encoded.contains("\"statistic\""));

    // stop 后 Live/Ready 均为 false。
    status.stop();
    let stopped = status.snapshot_copy();
    // stop 后状态机进入 STOPPED。
    assert_eq!(STATE_STOPPED, stopped.State);
    // 停止后探针语义：不可存活、不可就绪。
    assert!(!stopped.Live);
    assert!(!stopped.Ready);
}

#[derive(Default)]
/// 内存 mux：按 path 保存 handler，供 serve 直接调用。
struct TestMux {
    routes: HashMap<String, crate::http::HttpHandler>,
}

impl HttpMux for TestMux {
    fn HandleFunc(&mut self, path: &str, handler: crate::http::HttpHandler) {
        // 后注册覆盖同 path，与常见 mux 行为一致。
        self.routes.insert(path.to_string(), handler);
    }
}

impl TestMux {
    /// 构造假 GET 请求并写入 TestWriter。
    fn serve(&self, path: &str, writer: &mut TestWriter) {
        let handler = self
            .routes
            .get(path)
            // 未注册路由视为测试编排错误。
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
/// 捕获状态码/头/体，便于断言 HTTP 契约。
struct TestWriter {
    headers: HashMap<String, String>,
    status: u16,
    body: Vec<u8>,
}

impl HttpResponseWriter for TestWriter {
    fn Header(&mut self) -> &mut HashMap<String, String> {
        // 允许 handler 在写状态码前设置 Content-Type。
        &mut self.headers
    }

    fn WriteHeader(&mut self, status: u16) {
        // 覆盖写入：最后一次 WriteHeader 生效。
        self.status = status;
    }

    fn WriteBody(&mut self, body: &[u8]) {
        // 整体替换 body，不追加。
        self.body = body.to_vec();
    }
}

/// 固定全局检查点的 PD；Stores 恒空，避免 plan_round 依赖。
struct StaticPD {
    checkpoint: u64,
}

impl PDMetaReader for StaticPD {
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task: &str) -> Result<u64, Error> {
        // 忽略 task 名：契约测试不区分多任务。
        Ok(self.checkpoint)
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        // 空 store 列表使计算器跳过存活集合推进。
        Ok(Vec::new())
    }
}

/// 无对象上游：Walk 空、Read 失败；URI 使用受支持的 s3 scheme。
struct EmptyUpstream;

impl UpstreamStorageReader for EmptyUpstream {
    fn WalkDir(
        &self,
        _ctx: &Context,
        _opt: &WalkOption,
        _callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // 无文件可枚举。
        Ok(())
    }

    // 故意缺失，防止误走 load_meta 成功路径。
    fn ReadFile(&self, _ctx: &Context, _name: &str) -> Result<Vec<u8>, Error> {
        Err(Error::new("missing"))
    }

    // 返回受支持的 s3 URI，通过 StartAfter 能力校验。
    fn URI(&self) -> String {
        "s3://bucket/prefix".into()
    }
}

/// 可配置恒定同步结果的 Sync 桩。
struct StaticSync {
    synced: bool,
}

impl astersql_br_pkg_stream_crr_internal_checkpoint::ObjectSyncChecker for StaticSync {
    fn FileSynced(&self, _ctx: &Context, _name: &str) -> Result<bool, Error> {
        // 忽略路径，返回构造时配置的恒定结果。
        Ok(self.synced)
    }
}

/// Wait 自旋直到 ctx 取消，用于驱动 Run 循环而不真正推进 PD。
struct ImmediateWatcher;

impl UpstreamCheckpointWaiter for ImmediateWatcher {
    fn WaitGlobalCheckpointAdvance(
        &self,
        ctx: &Context,
        _task: &str,
        _current: u64,
    ) -> Result<(), Error> {
        while ctx.Err().is_none() {
            std::thread::sleep(Duration::from_millis(1));
        }
        // 取消后返回 ctx 错误，供 Run 识别退出。
        Err(ctx.Err().expect("cancelled"))
    }
}

/// 下游检查恒失败，迫使计算器进入失败/重试路径。
struct FailingSyncChecker;

impl astersql_br_pkg_stream_crr_internal_checkpoint::ObjectSyncChecker for FailingSyncChecker {
    fn FileSynced(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        Err(Error::new(format!("boom for {name}")))
    }
}

#[derive(Clone, Default)]
/// 可注入初始状态与“前 N 次 Save 失败”的内存 ResumeStateStore。
struct MemoryResumeStateStore {
    inner: Arc<Mutex<MemoryResumeStateStoreInner>>,
}

#[derive(Default)]
struct MemoryResumeStateStoreInner {
    state: Option<PersistentState>,
    /// 剩余故意失败次数；递减到 0 后 Save 成功。
    save_failures_left: i32,
    /// SaveState 调用总次数（含失败）。
    saves: i32,
    /// SaveState 成功次数，用来证明关机清理最终完成落盘。
    successful_saves: i32,
}

impl MemoryResumeStateStore {
    /// 构造：预置 state，并设置前 failures 次 Save 返回错误。
    fn with_state_and_save_failures(state: PersistentState, failures: i32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MemoryResumeStateStoreInner {
                state: Some(state),
                save_failures_left: failures,
                ..Default::default()
            })),
        }
    }

    /// 观测 Save 尝试次数。
    fn save_count(&self) -> i32 {
        self.inner.lock().expect("lock").saves
    }

    /// 观测真正写入状态的次数，失败尝试不计入。
    fn successful_save_count(&self) -> i32 {
        self.inner.lock().expect("lock").successful_saves
    }
}

impl ResumeStateStore for MemoryResumeStateStore {
    /// 返回当前内存中的 resume 快照。
    fn LoadState(&self, _ctx: &Context) -> Result<Option<PersistentState>, Error> {
        Ok(self.inner.lock().expect("lock").state.clone())
    }

    /// 计数后按剩余失败次数决定是否写入。
    fn SaveState(&self, _ctx: &Context, state: PersistentState) -> Result<(), Error> {
        let mut inner = self.inner.lock().expect("lock");
        // 先计数再可能失败，确保 save_count 反映尝试次数。
        inner.saves += 1;
        if inner.save_failures_left > 0 {
            inner.save_failures_left -= 1;
            return Err(Error::new("persist boom"));
        }
        inner.state = Some(state);
        inner.successful_saves += 1;
        Ok(())
    }
}
