// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Tests ported from `br/pkg/gc/safepoint_test.go`.
//!
//! 覆盖 MakeSafePointID、MarshalLogObject 与 Keeper/CheckGCSafePoint 参数化套件。
//! `run_suite` 对 Nullspace 与 testKeyspace 各跑一遍，验证作用域隔离与续约行为。
//! 使用 mockManager 注入 DeadlineExceeded，对齐 Go ErrorIs 负向路径。
//! PeriodicRefresh 在 Rust 侧始终执行（Go short 模式会跳过）。
//! 每个子块结束后 tear_down，防止后台线程干扰后续用例。
//! 屏障隔离断言依赖 mockPD 分桶状态与 BackupTS-1 约定。
//! Marshal 用例精确断言 Go `time.Duration.String()` 文案，包括负 TTL。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::manager::{KeyspaceID, NullspaceID};
use crate::mock_test::{
    DeadlineExceeded, error_is_deadline_exceeded, getState, newMockManagerWrapper, requireBarrier,
    requireNoBarrier, testKeyspaceID,
};
use crate::safepoint::{
    BRServiceSafePoint, CheckGCSafePoint, Context, MakeSafePointID, StartServiceSafePointKeeper,
};

/// 构造测试用服务安全点。
fn sp(id: &str, ttl: i64, backup_ts: u64) -> BRServiceSafePoint {
    BRServiceSafePoint {
        ID: id.to_string(),
        TTL: ttl,
        BackupTS: backup_ts,
    }
}

/// TestMakeSafePointID — Go TestMakeSafePointID (Format / Uniqueness / Concurrent).
///
/// 校验 br-UUID 格式、100 次唯一性与并发唯一性。
#[test]
fn test_make_safe_point_id() {
    // Format
    // 单次生成须匹配 br- 前缀与 UUID 分段长度。
    {
        let id = MakeSafePointID();
        let re = regex_lite_uuid_br(&id);
        assert!(re, "ID {id:?} should match br-{{uuid}} pattern");
    }

    // Uniqueness
    // 顺序生成 100 个 ID，集合中不得重复。
    {
        let mut ids = HashMap::new();
        let count = 100;
        for _ in 0..count {
            let id = MakeSafePointID();
            assert!(!ids.contains_key(&id), "duplicate ID generated: {id}");
            ids.insert(id, true);
        }
    }

    // Concurrent_Uniqueness
    // 100 线程并发生成，共享 map 检测碰撞。
    {
        let ids = Arc::new(Mutex::new(HashMap::<String, bool>::new()));
        let count = 100;
        let mut handles = Vec::with_capacity(count);
        for _ in 0..count {
            let ids = ids.clone();
            handles.push(std::thread::spawn(move || {
                let id = MakeSafePointID();
                let mut guard = ids.lock().unwrap();
                assert!(!guard.contains_key(&id), "duplicate ID generated: {id}");
                guard.insert(id, true);
            }));
        }
        for handle in handles {
            handle.join().expect("thread");
        }
    }
}

/// 轻量校验 `br-` + 小写十六进制 UUID 形态，避免引入 regex 依赖。
fn regex_lite_uuid_br(id: &str) -> bool {
    // ^br-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$
    // 必须带 br- 前缀。
    let Some(rest) = id.strip_prefix("br-") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('-').collect();
    // UUID 五段长度 8-4-4-4-12。
    if parts.len() != 5 {
        return false;
    }
    let lens = [8, 4, 4, 4, 12];
    for (part, &len) in parts.iter().zip(lens.iter()) {
        if part.len() != len || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        // 仅接受小写十六进制。
        if part.chars().any(|c| c.is_ascii_uppercase()) {
            return false;
        }
    }
    true
}

/// TestBRServiceSafePoint_MarshalLogObject — Go zap Object fields via Rust string API.
///
/// 精确断言 Go `time.Duration.String()` 风格 TTL 文案。
#[test]
fn test_br_service_safe_point_marshal_log_object() {
    // NormalValues: TTL=300 → Go "5m0s"; Rust Debug Duration is also accepted via
    // 正常值：ID/TTL/BackupTS 字段均需出现。
    // go_duration_string cross-check of the seconds value embedded in MarshalLogObject.
    {
        let safe_point = sp("br-test-id", 300, 1000);
        let rendered = safe_point.MarshalLogObject();
        // ID 字段精确匹配。
        assert!(rendered.contains("ID=br-test-id"), "{rendered}");
        assert!(
            rendered.contains("TTL=5m0s"),
            "TTL unexpected in {rendered}"
        );
        // BackupTS 字段。
        assert!(rendered.contains("BackupTS=1000"), "{rendered}");
    }

    // ZeroValues: should not panic
    // 全零输入不得 panic，TTL 可为 0ns/0s。
    {
        let safe_point = sp("", 0, 0);
        let rendered = safe_point.MarshalLogObject();
        assert!(rendered.contains("ID="), "{rendered}");
        assert!(
            rendered.contains("TTL=0s"),
            "zero TTL unexpected in {rendered}"
        );
        assert!(rendered.contains("BackupTS=0"), "{rendered}");
    }

    // LargeTTL: 86400 → Go "24h0m0s"
    // 一天 TTL：接受 86400s 或 24h0m0s。
    {
        let safe_point = sp("br-test", 86400, 1000);
        let rendered = safe_point.MarshalLogObject();
        assert!(
            rendered.contains("TTL=24h0m0s"),
            "large TTL unexpected in {rendered}"
        );
    }

    // NegativeTTL: Go preserves the sign instead of clamping to zero.
    {
        let safe_point = sp("br-test", -1, 1000);
        let rendered = safe_point.MarshalLogObject();
        assert!(
            rendered.contains("TTL=-1s"),
            "negative TTL unexpected in {rendered}"
        );
    }
}

// ============================================================================
// SafepointKeeperSuite - parameterized tests for keyspace scope
// 按 keyspace_id 参数化的 Keeper 套件夹具与用例驱动。
// ============================================================================

/// 单作用域套件：持有 mock Manager、可取消 Context 与 tear_down。
struct SafepointKeeperSuite {
    /// 当前套件作用域（Nullspace 或 testKeyspace）。
    keyspace_id: KeyspaceID,
    /// 可注入错误的 Manager 包装。
    mgr: Arc<crate::mock_test::mockManager>,
    /// 传给 Keeper 的可取消上下文。
    ctx: Context,
    /// tear_down 时调用以停止后台线程。
    cancel: Option<Box<dyn Fn() + Send + Sync + 'static>>,
}

impl SafepointKeeperSuite {
    /// 创建包装 Manager 与可取消 ctx。
    fn setup(keyspace_id: KeyspaceID) -> Self {
        let mgr = newMockManagerWrapper(keyspace_id);
        let (ctx, cancel) = Context::WithCancel();
        Self {
            keyspace_id,
            mgr,
            ctx,
            cancel: Some(Box::new(cancel)),
        }
    }

    /// 取消 Keeper 后台线程，避免用例泄漏。
    fn tear_down(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }

    /// 对侧作用域：全局↔testKeyspace，用于隔离断言。
    fn other_keyspace_id(&self) -> KeyspaceID {
        if self.keyspace_id == NullspaceID {
            testKeyspaceID
        } else {
            NullspaceID
        }
    }

    /// 本作用域有屏障，对侧无同名屏障。
    fn require_barrier_isolation(&self, safe_point: &BRServiceSafePoint) {
        requireBarrier(
            &getState(&self.ctx, &self.mgr.mockPD, self.keyspace_id),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );
        requireNoBarrier(
            &getState(&self.ctx, &self.mgr.mockPD, self.other_keyspace_id()),
            &safe_point.ID,
        );
    }

    /// 推进本作用域 GC 安全点以布置预检失败场景。
    fn set_gc_safe_point(&self, ts: u64) {
        self.mgr
            .setGCSafePoint(&self.ctx, self.keyspace_id, ts)
            .expect("setGCSafePoint");
    }
}

/// 在给定 keyspace 上跑完整 Keeper/Check 用例集。
fn run_suite(keyspace_id: KeyspaceID) {
    // --- Keeper Validation Tests ---
    // 参数与预检：合法启动、空 ID、TTL 非法、BackupTS 越界。
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        let safe_point = sp("br-test", 10, 1000);
        StartServiceSafePointKeeper(&s.ctx, safe_point.clone(), s.mgr.clone())
            .expect("ValidParams");
        // 合法参数：同步 Set 一次且屏障隔离正确。
        assert_eq!(s.mgr.getSetSafePointCalls(), 1);
        s.require_barrier_isolation(&safe_point);
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        let err = StartServiceSafePointKeeper(&s.ctx, sp("", 10, 1000), s.mgr.clone())
            .expect_err("EmptyID");
        // 空 ID 拒绝。
        assert!(err.to_string().contains("invalid"), "err={err}");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        let err = StartServiceSafePointKeeper(&s.ctx, sp("br-test", 0, 1000), s.mgr.clone())
            .expect_err("ZeroTTL");
        // TTL=0 拒绝。
        assert!(err.to_string().contains("invalid"), "err={err}");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        let err = StartServiceSafePointKeeper(&s.ctx, sp("br-test", -1, 1000), s.mgr.clone())
            .expect_err("NegativeTTL");
        // 负 TTL 拒绝。
        assert!(err.to_string().contains("invalid"), "err={err}");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.set_gc_safe_point(1000);
        let err = StartServiceSafePointKeeper(&s.ctx, sp("br-test", 10, 500), s.mgr.clone())
            .expect_err("BackupTSBehindSafePoint");
        // BackupTS 落后于 GC：预检 exceed。
        assert!(err.to_string().contains("exceed"), "err={err}");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.set_gc_safe_point(1000);
        let err = StartServiceSafePointKeeper(&s.ctx, sp("br-test", 10, 1000), s.mgr.clone())
            .expect_err("BackupTSEqualsSafePoint");
        // BackupTS 等于 GC：同样 exceed。
        assert!(err.to_string().contains("exceed"), "err={err}");
        s.tear_down();
    }

    // --- Keeper Behavior Tests ---
    // 行为：首次 Set、错误传播、周期续约、取消退出、读错误忽略。
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        let safe_point = sp("br-test", 10, 1000);
        StartServiceSafePointKeeper(&s.ctx, safe_point.clone(), s.mgr.clone())
            .expect("InitialSetCalled");
        // 启动后调用计数为 1。
        assert_eq!(s.mgr.getSetSafePointCalls(), 1);
        s.require_barrier_isolation(&safe_point);
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.mgr.inject_set_safe_point_err(Box::new(DeadlineExceeded));
        let err = StartServiceSafePointKeeper(&s.ctx, sp("br-test", 10, 1000), s.mgr.clone())
            .expect_err("SetServiceSafePointErrorPropagated");
        // 注入超时须以类型化 DeadlineExceeded 冒泡。
        assert!(
            error_is_deadline_exceeded(&err),
            "expected DeadlineExceeded, got {err}"
        );
        s.tear_down();
    }
    // PeriodicRefresh — Go skips in short mode; always run here (no ignored tests).
    // TTL=3 → 间隔 1s；2s 内应至少再续约一次。
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        // TTL=3s means update interval = 1s (TTL/3)
        // 轮询等待第二次 Set。
        let safe_point = sp("br-test", 3, 1000);
        StartServiceSafePointKeeper(&s.ctx, safe_point.clone(), s.mgr.clone())
            .expect("PeriodicRefresh");
        // 首次同步 Set 后进入等待循环。
        assert_eq!(s.mgr.getSetSafePointCalls(), 1);
        s.require_barrier_isolation(&safe_point);

        // 最多等 2 秒观察周期续约。
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if s.mgr.getSetSafePointCalls() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            s.mgr.getSetSafePointCalls() >= 2,
            "expected periodic refresh, calls={}",
            s.mgr.getSetSafePointCalls()
        );
        s.require_barrier_isolation(&safe_point);
        s.tear_down();
    }
    {
        // ContextCancelExits：取消后不再续约。
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        let safe_point = sp("br-test", 300, 1000);
        // 大 TTL 避免短时内二次 Set 干扰取消断言。
        StartServiceSafePointKeeper(&s.ctx, safe_point, s.mgr.clone()).expect("ContextCancelExits");
        let initial_calls = s.mgr.getSetSafePointCalls();
        assert_eq!(initial_calls, 1);
        s.tear_down();
        std::thread::sleep(Duration::from_millis(100));
        let final_calls = s.mgr.getSetSafePointCalls();
        // cancel 后短时间内调用次数冻结。
        assert_eq!(initial_calls, final_calls);
    }
    {
        // GetGCSafePointErrorIgnored 子场景。
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.mgr.inject_gc_safe_point_err(Box::new(DeadlineExceeded));
        // Should NOT return error because GetGCSafePoint error is ignored
        // 读 GC 失败被 Check 吞掉，Keeper 仍可启动。
        // in CheckGCSafePoint (returns nil on error)
        StartServiceSafePointKeeper(&s.ctx, sp("br-test", 10, 1000), s.mgr.clone())
            .expect("GetGCSafePointErrorIgnored");
        s.tear_down();
    }

    // --- CheckGCSafePoint Tests ---
    // 大于通过；等于/小于失败；读错误忽略。
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.set_gc_safe_point(100);
        // 200 > 100：通过。
        CheckGCSafePoint(&s.ctx, s.mgr.as_ref(), 200).expect("TSGreaterThanSafePoint");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.set_gc_safe_point(100);
        // 等于安全点：失败。
        let err = CheckGCSafePoint(&s.ctx, s.mgr.as_ref(), 100).expect_err("TSEqualsSafePoint");
        assert!(err.to_string().contains("exceed"), "err={err}");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.set_gc_safe_point(100);
        // 小于安全点：失败。
        let err = CheckGCSafePoint(&s.ctx, s.mgr.as_ref(), 50).expect_err("TSLessThanSafePoint");
        assert!(err.to_string().contains("exceed"), "err={err}");
        s.tear_down();
    }
    {
        let mut s = SafepointKeeperSuite::setup(keyspace_id);
        s.mgr.inject_gc_safe_point_err(Box::new(DeadlineExceeded));
        // Get 注入错误时 Check 仍 Ok。
        CheckGCSafePoint(&s.ctx, s.mgr.as_ref(), 100).expect("ErrorIgnored");
        s.tear_down();
    }
}

/// TestGlobalSafepointKeeper — Go suite.Run with NullspaceID.
///
/// 全局作用域跑完整套件。
#[test]
fn test_global_safepoint_keeper() {
    run_suite(NullspaceID);
}

/// TestKeyspaceSafepointKeeper — Go suite.Run with testKeyspaceID.
///
/// keyspace 作用域跑完整套件。
#[test]
fn test_keyspace_safepoint_keeper() {
    run_suite(testKeyspaceID);
}
