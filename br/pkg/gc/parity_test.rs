// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Parity tests proving the Rust `br/pkg/gc` public contract matches the Go
//! sources (`manager.go`, `manager_global.go`, `manager_keyspace.go`,
//! `safepoint.go`).
//!
//! 契约对等测试：用自包含 MockPd 验证 Rust 公开 API 与 Go manager/safepoint 语义一致。
//! 覆盖常量、ID 生成、NewManager 路由、错误传播、CheckGCSafePoint 与 Keeper 生命周期。
//! 与 mock_test 夹具独立，可单独注入 fail_* 开关以覆盖负向路径。
//! 断言重点是调用参数（BackupTS-1、TTL=0 删除）与作用域隔离，而非 PD 实现细节。
//! 单测 `go_rust_public_contract_matches` 按段落组织，任一段失败即整体失败。
//! Mock 故意不模拟 unistore 的 txn/gc 交叉约束，以免干扰契约焦点。
//! Keeper 取消后用短 sleep 观测，避免依赖真实 TTL 分钟级等待。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::manager::{
    GCBarrierInfo, GCState, GCStatesClient, Manager, NewManager, NullspaceID, PdClient,
};
use crate::safepoint::{
    BRServiceSafePoint, CheckGCSafePoint, Context, DefaultBRGCSafePointTTL,
    DefaultCheckpointGCSafePointTTL, DefaultStreamPauseSafePointTTL,
    DefaultStreamStartSafePointTTL, MakeSafePointID, SharedError, StartServiceSafePointKeeper,
};

/// 对等测试固定非全局 keyspace，与 mock_test::testKeyspaceID 数值一致。
const TEST_KEYSPACE_ID: u32 = 100;

/// Per-keyspace GC state mirroring the semantics of unistore MockPD's
/// gcStatesManager: monotone safe points, service safe points (global path)
/// and GC barriers (keyspace path).
///
/// 单 keyspace 内存态：本文件 Mock 比 mock_test 更精简，专注契约断言。
#[derive(Default)]
struct KeyspaceState {
    /// 当前 GC 安全点；UpdateGCSafePoint 仅在更大时推进。
    gc_safe_point: u64,
    /// 全局服务安全点表；keyspace 路径不写入此 map。
    service_safe_points: HashMap<String, u64>,
    /// keyspace SetGCBarrier 写入；全局路径在本 Mock 中不填 barriers。
    barriers: HashMap<String, GCBarrierInfo>,
}

/// Mock PD 可变内核：分桶状态、调用记录与各 API 的失败注入开关。
#[derive(Default)]
struct MockPdInner {
    states: HashMap<u32, KeyspaceState>,
    /// 记录每次 UpdateServiceGCSafePoint 的 (id, ttl, safe_point)。
    update_service_calls: Vec<(String, i64, u64)>,
    /// 非空时 UpdateServiceGCSafePoint 立即失败。
    fail_update_service: Option<String>,
    /// 非空时 UpdateGCSafePoint 立即失败。
    fail_update_gc: Option<String>,
    /// 非空时 SetGCBarrier 立即失败。
    fail_set_barrier: Option<String>,
    /// 非空时 DeleteGCBarrier 立即失败。
    fail_delete_barrier: Option<String>,
    /// 非空时 GetGCState 立即失败。
    fail_get_state: Option<String>,
}

/// 线程安全 Mock PD；实现 PdClient 供 NewManager 挂载。
#[derive(Default)]
struct MockPd {
    inner: Mutex<MockPdInner>,
}

/// 构造通用 SharedError，供 fail_* 与参数校验共用。
fn mock_err(message: &str) -> SharedError {
    Box::new(std::io::Error::other(message.to_string()))
}

impl MockPd {
    /// 克隆指定 keyspace 快照，避免长时间持锁做断言。
    fn state_of(&self, keyspace_id: u32) -> KeyspaceStateSnapshot {
        let inner = self.inner.lock().unwrap();
        let state = inner.states.get(&keyspace_id);
        KeyspaceStateSnapshot {
            gc_safe_point: state.map(|s| s.gc_safe_point).unwrap_or(0),
            service_safe_points: state
                .map(|s| s.service_safe_points.clone())
                .unwrap_or_default(),
            barriers: state.map(|s| s.barriers.clone()).unwrap_or_default(),
        }
    }

    /// 测试侧直接写入 GC 安全点，绕过 Update 单调逻辑以布置前置条件。
    fn set_gc_safe_point(&self, keyspace_id: u32, ts: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.states.entry(keyspace_id).or_default().gc_safe_point = ts;
    }
}

/// `state_of` 返回的拥有型快照，字段与 KeyspaceState 对应。
struct KeyspaceStateSnapshot {
    gc_safe_point: u64,
    service_safe_points: HashMap<String, u64>,
    barriers: HashMap<String, GCBarrierInfo>,
}

/// 绑定单一 keyspace 的 GCStatesClient；可读 fail_get/set/delete 注入。
struct MockGcStatesClient {
    pd: Arc<MockPd>,
    keyspace_id: u32,
}

impl GCStatesClient for MockGcStatesClient {
    fn GetGCState(&self, _ctx: &Context) -> Result<GCState, SharedError> {
        let inner = self.pd.inner.lock().unwrap();
        // 注入优先于正常读。
        if let Some(message) = &inner.fail_get_state {
            return Err(mock_err(message));
        }
        let state = inner.states.get(&self.keyspace_id);
        // TxnSafePoint 简化为与 GC 相同，契约测试不依赖二者差异。
        Ok(GCState {
            GCSafePoint: state.map(|s| s.gc_safe_point).unwrap_or(0),
            TxnSafePoint: state.map(|s| s.gc_safe_point).unwrap_or(0),
            GCBarriers: state
                .map(|s| s.barriers.values().cloned().collect())
                .unwrap_or_default(),
        })
    }

    fn SetGCBarrier(
        &self,
        _ctx: &Context,
        barrier_id: &str,
        barrier_ts: u64,
        ttl_seconds: i64,
    ) -> Result<GCBarrierInfo, SharedError> {
        let mut inner = self.pd.inner.lock().unwrap();
        if let Some(message) = &inner.fail_set_barrier {
            return Err(mock_err(message));
        }
        // 空 ID / 零 TS / 非正 TTL 拒绝，对齐生产侧基本校验。
        if barrier_id.is_empty() || barrier_ts == 0 || ttl_seconds <= 0 {
            return Err(mock_err("invalid barrier arguments"));
        }
        let info = GCBarrierInfo {
            BarrierID: barrier_id.to_string(),
            BarrierTS: barrier_ts,
            TTL: ttl_seconds,
        };
        inner
            .states
            .entry(self.keyspace_id)
            .or_default()
            .barriers
            .insert(barrier_id.to_string(), info.clone());
        Ok(info)
    }

    fn DeleteGCBarrier(
        &self,
        _ctx: &Context,
        barrier_id: &str,
    ) -> Result<Option<GCBarrierInfo>, SharedError> {
        let mut inner = self.pd.inner.lock().unwrap();
        if let Some(message) = &inner.fail_delete_barrier {
            return Err(mock_err(message));
        }
        // 缺失 ID 返回 Ok(None)，删除应成功而非报错。
        Ok(inner
            .states
            .entry(self.keyspace_id)
            .or_default()
            .barriers
            .remove(barrier_id))
    }
}

impl PdClient for Arc<MockPd> {
    fn UpdateGCSafePoint(&self, _ctx: &Context, safe_point: u64) -> Result<u64, SharedError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(message) = &inner.fail_update_gc {
            return Err(mock_err(message));
        }
        // 全局路径写 Nullspace；safe_point=0 时仅查询不推进。
        let state = inner.states.entry(NullspaceID).or_default();
        if safe_point > state.gc_safe_point {
            state.gc_safe_point = safe_point;
        }
        Ok(state.gc_safe_point)
    }

    fn UpdateServiceGCSafePoint(
        &self,
        _ctx: &Context,
        service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64, SharedError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(message) = &inner.fail_update_service {
            return Err(mock_err(message));
        }
        // 先记调用再改状态，便于断言 Set/Delete 参数序列。
        inner
            .update_service_calls
            .push((service_id.to_string(), ttl, safe_point));
        let state = inner.states.entry(NullspaceID).or_default();
        if ttl <= 0 {
            state.service_safe_points.remove(service_id);
        } else {
            state
                .service_safe_points
                .insert(service_id.to_string(), safe_point);
        }
        let min_service = state.service_safe_points.values().min().copied();
        Ok(min_service.unwrap_or(state.gc_safe_point))
    }

    fn GetGCStatesClient(&self, keyspace_id: u32) -> Arc<dyn GCStatesClient> {
        // 每次新建绑定客户端，共享底层 MockPd。
        Arc::new(MockGcStatesClient {
            pd: self.clone(),
            keyspace_id,
        })
    }
}

/// Manager wrapper mirroring Go mock_test.go's mockManager: real manager
/// backed by the mock PD, plus call counting and error injection.
///
/// Keeper 场景包装：字符串错误注入足够，无需类型化 DeadlineExceeded。
struct MockManagerWrapper {
    /// 真实 NewManager 实例。
    inner: Arc<dyn Manager>,
    /// Set 调用计数（含失败）。
    set_safe_point_calls: AtomicU64,
    /// Set 错误注入消息。
    set_safe_point_err: Mutex<Option<String>>,
    /// GetGCSafePoint 错误注入消息。
    gc_safe_point_err: Mutex<Option<String>>,
}

impl Manager for MockManagerWrapper {
    fn GetGCSafePoint(&self, ctx: &Context) -> Result<u64, SharedError> {
        // 注入错误时不访问内层。
        if let Some(message) = self.gc_safe_point_err.lock().unwrap().clone() {
            return Err(mock_err(&message));
        }
        self.inner.GetGCSafePoint(ctx)
    }

    fn SetServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // 失败前也计数，验证 Keeper 同步首次调用发生过。
        self.set_safe_point_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(message) = self.set_safe_point_err.lock().unwrap().clone() {
            return Err(mock_err(&message));
        }
        self.inner.SetServiceSafePoint(ctx, sp)
    }

    fn DeleteServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // 删除不做注入，直接转发。
        self.inner.DeleteServiceSafePoint(ctx, sp)
    }
}

/// 空 MockPd 工厂。
fn new_pd() -> Arc<MockPd> {
    Arc::new(MockPd::default())
}

/// 缩短样板：构造 BRServiceSafePoint。
fn sp(id: &str, ttl: i64, backup_ts: u64) -> BRServiceSafePoint {
    BRServiceSafePoint {
        ID: id.to_string(),
        TTL: ttl,
        BackupTS: backup_ts,
    }
}

/// 单测聚合 Go/Rust 公开契约；失败任一段即整体失败。
#[test]
fn go_rust_public_contract_matches() {
    let ctx = Context::Background();

    // Constants match safepoint.go.
    // TTL 常量必须与 safepoint.go 字面量逐项相等。
    assert_eq!(DefaultBRGCSafePointTTL, 300);
    assert_eq!(DefaultCheckpointGCSafePointTTL, 72 * 60);
    assert_eq!(DefaultStreamStartSafePointTTL, 1800);
    assert_eq!(DefaultStreamPauseSafePointTTL, 24 * 3600);

    // MakeSafePointID: "br-" + UUID, unique across calls.
    // 前缀 br-，总长 br- + 36 位 UUID；两次调用不得碰撞。
    let id1 = MakeSafePointID();
    let id2 = MakeSafePointID();
    assert!(id1.starts_with("br-"));
    assert_eq!(id1.len(), "br-".len() + 36);
    assert_ne!(id1, id2);

    // --- NewManager mode selection (manager.go) ---
    // GlobalMode: NullspaceID routes to the deprecated service safepoint API,
    // never touches keyspace barriers.
    // 全局模式：写入服务安全点，keyspace 屏障表保持空。
    {
        let pd = new_pd();
        let mgr = NewManager(Arc::new(pd.clone()), NullspaceID);
        let safe_point = sp("br-test-global", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone()).unwrap();
        let state = pd.state_of(NullspaceID);
        // BackupTS-1 semantics as in UpdateServiceGCSafePoint.
        // 1000-1=999，与 UpdateServiceGCSafePoint 入参一致。
        assert_eq!(
            state.service_safe_points.get("br-test-global").copied(),
            Some(999)
        );
        assert!(pd.state_of(TEST_KEYSPACE_ID).barriers.is_empty());

        // DeleteServiceSafePoint sends TTL=0, safePoint=0.
        // 删除调用序列：先 (id,300,999) 再 (id,0,0)。
        mgr.DeleteServiceSafePoint(&ctx, safe_point).unwrap();
        let state = pd.state_of(NullspaceID);
        assert!(state.service_safe_points.is_empty());
        let calls = pd.inner.lock().unwrap().update_service_calls.clone();
        assert_eq!(
            calls,
            vec![
                ("br-test-global".to_string(), 300, 999),
                ("br-test-global".to_string(), 0, 0),
            ]
        );
    }

    // KeyspaceMode: non-null keyspace routes to GC barriers scoped to that
    // keyspace only.
    // keyspace 模式：屏障仅落在 TEST_KEYSPACE_ID，全局服务点为空。
    {
        let pd = new_pd();
        let mgr = NewManager(Arc::new(pd.clone()), TEST_KEYSPACE_ID);
        let safe_point = sp("br-test-keyspace", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone()).unwrap();
        let state = pd.state_of(TEST_KEYSPACE_ID);
        let barrier = state.barriers.get("br-test-keyspace").unwrap();
        // BarrierTS/TTL 与入参 BackupTS-1 及 TTL 一致。
        assert_eq!(barrier.BarrierTS, 999);
        assert_eq!(barrier.TTL, 300);
        // 全局侧 barriers 与 service_safe_points 均应空。
        assert!(pd.state_of(NullspaceID).barriers.is_empty());
        assert!(pd.state_of(NullspaceID).service_safe_points.is_empty());

        // TTL <= 0 in SetServiceSafePoint deletes the barrier (keyspace-only
        // behavior from manager_keyspace.go).
        // TTL=0 走删除分支，屏障表应清空。
        mgr.SetServiceSafePoint(&ctx, sp("br-test-keyspace", 0, 1000))
            .unwrap();
        assert!(pd.state_of(TEST_KEYSPACE_ID).barriers.is_empty());

        // Deleting a non-existent barrier succeeds (DeleteGCBarrier returns nil info).
        // 删除不存在的屏障仍 Ok，对齐 Go nil info。
        mgr.DeleteServiceSafePoint(&ctx, sp("no-such-barrier", 300, 1000))
            .unwrap();
    }

    // --- GetGCSafePoint (both managers) ---
    // 全局与 keyspace 各自读到预置的不同安全点，证明路由隔离。
    {
        let pd = new_pd();
        // 预置不同安全点，验证两路径互不串读。
        pd.set_gc_safe_point(NullspaceID, 42);
        pd.set_gc_safe_point(TEST_KEYSPACE_ID, 77);
        let global = NewManager(Arc::new(pd.clone()), NullspaceID);
        assert_eq!(global.GetGCSafePoint(&ctx).unwrap(), 42);
        let keyspace = NewManager(Arc::new(pd.clone()), TEST_KEYSPACE_ID);
        // keyspace 读 77，而非全局 42。
        assert_eq!(keyspace.GetGCSafePoint(&ctx).unwrap(), 77);
    }

    // --- Error propagation (errors.Trace keeps the cause) ---
    // 错误信息应保留 cause 文本，便于上层识别 PD 故障。
    {
        let pd = new_pd();
        pd.inner.lock().unwrap().fail_update_service = Some("pd unavailable".to_string());
        pd.inner.lock().unwrap().fail_update_gc = Some("pd unavailable".to_string());
        let mgr = NewManager(Arc::new(pd.clone()), NullspaceID);
        // 全局 Set/Get 均应带上 "pd unavailable"。
        let err = mgr
            .SetServiceSafePoint(&ctx, sp("id", 300, 1000))
            .unwrap_err();
        assert!(err.to_string().contains("pd unavailable"));
        let err = mgr.GetGCSafePoint(&ctx).unwrap_err();
        assert!(err.to_string().contains("pd unavailable"));

        // keyspace 路径分别注入 set/delete/get 三类失败。
        let pd = new_pd();
        pd.inner.lock().unwrap().fail_set_barrier = Some("barrier rejected".to_string());
        pd.inner.lock().unwrap().fail_delete_barrier = Some("delete rejected".to_string());
        pd.inner.lock().unwrap().fail_get_state = Some("state unavailable".to_string());
        let mgr = NewManager(Arc::new(pd.clone()), TEST_KEYSPACE_ID);
        // Set 失败文案。
        let err = mgr
            .SetServiceSafePoint(&ctx, sp("id", 300, 1000))
            .unwrap_err();
        assert!(err.to_string().contains("barrier rejected"));
        // Delete 失败文案。
        let err = mgr
            .DeleteServiceSafePoint(&ctx, sp("id", 300, 1000))
            .unwrap_err();
        assert!(err.to_string().contains("delete rejected"));
        // Get 失败文案。
        let err = mgr.GetGCSafePoint(&ctx).unwrap_err();
        assert!(err.to_string().contains("state unavailable"));
    }

    // --- CheckGCSafePoint (safepoint.go) ---
    // 严格大于安全点才通过；相等或更小报 ErrBackupGCSafepointExceeded 文案。
    {
        let pd = new_pd();
        pd.set_gc_safe_point(NullspaceID, 100);
        let mgr = NewManager(Arc::new(pd.clone()), NullspaceID);

        // ts > safepoint passes.
        // 101 > 100：允许备份。
        CheckGCSafePoint(&ctx, mgr.as_ref(), 101).unwrap();
        // ts == safepoint fails with ErrBackupGCSafepointExceeded semantics.
        // 等于安全点也视为已越过，拒绝。
        let err = CheckGCSafePoint(&ctx, mgr.as_ref(), 100).unwrap_err();
        assert!(err.to_string().contains("GC safepoint 100 exceed TS 100"));
        // ts < safepoint fails too.
        let err = CheckGCSafePoint(&ctx, mgr.as_ref(), 99).unwrap_err();
        assert!(err.to_string().contains("GC safepoint 100 exceed TS 99"));

        // GetGCSafePoint failure is swallowed (only warned) per Go semantics.
        // 读安全点失败只告警，不使 Check 失败（与 Go 一致）。
        pd.inner.lock().unwrap().fail_update_gc = Some("pd down".to_string());
        CheckGCSafePoint(&ctx, mgr.as_ref(), 1).unwrap();
    }

    // --- StartServiceSafePointKeeper (safepoint.go) ---
    // Keeper：参数校验、预检、同步首次 Set、取消后停止续约。
    {
        // Invalid arguments: empty ID or non-positive TTL.
        // 空 ID / TTL=0 / TTL<0 均拒绝启动。
        let pd = new_pd();
        let mgr = NewManager(Arc::new(pd.clone()), NullspaceID);
        let err = StartServiceSafePointKeeper(&ctx, sp("", 300, 1000), mgr.clone()).unwrap_err();
        assert!(err.to_string().contains("invalid service safe point"));
        let err = StartServiceSafePointKeeper(&ctx, sp("id", 0, 1000), mgr.clone()).unwrap_err();
        assert!(err.to_string().contains("invalid service safe point"));
        let err = StartServiceSafePointKeeper(&ctx, sp("id", -1, 1000), mgr).unwrap_err();
        assert!(err.to_string().contains("invalid service safe point"));

        // BackupTS behind GC safepoint fails the pre-check.
        // BackupTS 落后于 GC 时预检失败，不启动后台线程。
        let pd = new_pd();
        pd.set_gc_safe_point(NullspaceID, 500);
        let mgr = NewManager(Arc::new(pd.clone()), NullspaceID);
        let err = StartServiceSafePointKeeper(&ctx, sp("id", 300, 400), mgr).unwrap_err();
        assert!(err.to_string().contains("exceed TS"));

        // Immediate SetServiceSafePoint failure surfaces synchronously.
        // 首次同步 Set 失败应立即返回，且调用计数为 1。
        let pd = new_pd();
        let wrapper = Arc::new(MockManagerWrapper {
            inner: NewManager(Arc::new(pd.clone()), NullspaceID),
            set_safe_point_calls: AtomicU64::new(0),
            set_safe_point_err: Mutex::new(Some("set failed".to_string())),
            gc_safe_point_err: Mutex::new(None),
        });
        let err =
            StartServiceSafePointKeeper(&ctx, sp("id", 300, 1000), wrapper.clone()).unwrap_err();
        assert!(err.to_string().contains("set failed"));
        // The initial synchronous call is counted even though it failed.
        assert_eq!(wrapper.set_safe_point_calls.load(Ordering::SeqCst), 1);

        // Happy path: initial set happens synchronously; keeper thread exits on
        // context cancel (resource cleanup parity with Go ticker Stop/ctx.Done).
        // 成功路径：同步写入 999；cancel 后短时间内调用次数不再增加。
        let pd = new_pd();
        let (keeper_ctx, cancel) = Context::WithCancel();
        let wrapper = Arc::new(MockManagerWrapper {
            inner: NewManager(Arc::new(pd.clone()), NullspaceID),
            set_safe_point_calls: AtomicU64::new(0),
            set_safe_point_err: Mutex::new(None),
            gc_safe_point_err: Mutex::new(None),
        });
        StartServiceSafePointKeeper(&keeper_ctx, sp("keeper-id", 300, 1000), wrapper.clone())
            .unwrap();
        assert_eq!(wrapper.set_safe_point_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            pd.state_of(NullspaceID)
                .service_safe_points
                .get("keeper-id")
                .copied(),
            Some(999)
        );
        cancel();
        std::thread::sleep(Duration::from_millis(50));
        // After cancel no further updates happen.
        // 再等一轮，确认后台 ticker 已退出。
        let calls_after_cancel = wrapper.set_safe_point_calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            wrapper.set_safe_point_calls.load(Ordering::SeqCst),
            calls_after_cancel
        );
    }

    // --- MarshalLogObject renders all four fields in Go order ---
    // 日志序列化需含 ID/TTL/BackupTime/BackupTS 四段，与 Go zap 字段对齐。
    {
        // 1<<18 物理毫秒折叠后 BackupTS 字面量为 262144。
        let rendered = sp("log-id", 300, 1 << 18).MarshalLogObject();
        assert!(rendered.contains("ID=log-id"));
        // TTL 以 Debug Duration 渲染，只要求键名存在。
        assert!(rendered.contains("TTL="));
        assert!(rendered.contains("BackupTime="));
        assert!(rendered.contains("BackupTS=262144"));
    }
}
