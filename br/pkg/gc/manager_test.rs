// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Tests ported from `br/pkg/gc/manager_test.go`.
//!
//! 覆盖 `NewManager` 路由及 global / keyspace 两条 Manager 实现的设置、删除与查询。
//! 用 `mock_test` 中的内存 PD 断言屏障是否落在正确作用域（全局 vs 指定 keyspace）。
//! `withKeyspaceConfig` 隔离线程局部 keyspace 名，避免用例间串扰（对齐 Go 临时改配置）。
//! 断言助手 `requireBarrier` / `requireNoBarrier` 校验 ID 与 `BackupTS-1`；
//! 不引入真实 PD / unistore，保证本机与 CI 可无依赖跑通。

use std::cell::RefCell;

use crate::manager::{NewManager, NullspaceID};
use crate::mock_test::{getState, newTestMockPD, requireBarrier, requireNoBarrier, testKeyspaceID};
use crate::safepoint::{BRServiceSafePoint, Context};

thread_local! {
    /// Mirrors Go `config.KeyspaceName` temporarily swapped by withKeyspaceConfig.
    /// NewManager routes by keyspaceID (same as Go); this keeps isolation hygiene.
    /// 线程局部存当前用例的 keyspace 名；Drop 守卫负责还原，防止污染后续测试。
    static KEYSPACE_NAME: RefCell<String> = const { RefCell::new(String::new()) };
}

/// withKeyspaceConfig temporarily sets keyspace config; restored on Drop.
///
/// 进入时替换 `KEYSPACE_NAME`，作用域结束自动恢复旧值；闭包内执行具体断言。
fn withKeyspaceConfig<F: FnOnce()>(keyspace_name: &str, f: F) {
    // Drop 时把保存的旧名写回线程局部，即使闭包 panic 也会还原。
    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            KEYSPACE_NAME.with(|n| {
                *n.borrow_mut() = std::mem::take(&mut self.0);
            });
        }
    }
    let prev = KEYSPACE_NAME.with(|n| n.replace(keyspace_name.to_string()));
    let _guard = Guard(prev);
    f();
}

/// 构造测试用 `BRServiceSafePoint`，缩短各子用例样板代码。
fn sp(id: &str, ttl: i64, backup_ts: u64) -> BRServiceSafePoint {
    BRServiceSafePoint {
        ID: id.to_string(),
        TTL: ttl,
        BackupTS: backup_ts,
    }
}

/// TestNewManager — Go TestNewManager (GlobalMode / KeyspaceMode).
///
/// 验证工厂按 NullspaceID / testKeyspaceID 分别落到全局与 keyspace 状态，互不泄漏。
#[test]
fn test_new_manager() {
    // GlobalMode: keyspaceName = "" (global mode)
    // 空名 + NullspaceID：屏障应只出现在全局状态，keyspace 侧为空。
    withKeyspaceConfig("", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), NullspaceID);
        let ctx = Context::Background();
        let safe_point = sp("br-test-global", 300, 1000);
        // 成功设置后立刻读全局快照；失败则用例应在 expect 处中止。
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");

        // Verify barrier exists in global state
        // 屏障 TS 期望为 BackupTS-1，与生产路径一致。
        requireBarrier(
            &getState(&ctx, &mock_pd, NullspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );
        // Verify barrier does NOT exist in keyspace state
        // 交叉作用域必须为空，防止全局路径误写 keyspace。
        requireNoBarrier(&getState(&ctx, &mock_pd, testKeyspaceID), &safe_point.ID);
    });

    // KeyspaceMode: keyspaceName = "test_keyspace"
    // 命名 keyspace + testKeyspaceID：屏障只在该 keyspace，全局侧为空。
    withKeyspaceConfig("test_keyspace", || {
        let mock_pd = newTestMockPD();
        // 工厂第二参决定实现类型；此处必须与 testKeyspaceID 一致。
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), testKeyspaceID);
        let ctx = Context::Background();
        let safe_point = sp("br-test-keyspace", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");

        // 正向：目标 keyspace 有屏障；负向：Nullspace 无同名屏障。
        requireBarrier(
            &getState(&ctx, &mock_pd, testKeyspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );
        requireNoBarrier(&getState(&ctx, &mock_pd, NullspaceID), &safe_point.ID);
    });
}

/// TestGlobalManager — Go TestGlobalManager subtests.
///
/// 覆盖全局 Manager 的 Set / Delete / Get 三条主路径（对应 Go 子测试名）。
#[test]
fn test_global_manager() {
    // SetServiceSafePoint
    // 设置后全局有屏障，指定 keyspace 无屏障。
    withKeyspaceConfig("", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), NullspaceID);
        let ctx = Context::Background();
        let safe_point = sp("br-test", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");
        requireBarrier(
            &getState(&ctx, &mock_pd, NullspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );
        requireNoBarrier(&getState(&ctx, &mock_pd, testKeyspaceID), &safe_point.ID);
    });

    // DeleteServiceSafePoint
    // 先 Set 再 Delete，最终全局状态应清除该 ID。
    withKeyspaceConfig("", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), NullspaceID);
        let ctx = Context::Background();
        let safe_point = sp("br-test", 300, 1000);

        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");
        requireBarrier(
            &getState(&ctx, &mock_pd, NullspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );

        // 删除后同一 ID 在全局状态必须消失。
        mgr.DeleteServiceSafePoint(&ctx, safe_point.clone())
            .expect("DeleteServiceSafePoint");
        requireNoBarrier(&getState(&ctx, &mock_pd, NullspaceID), &safe_point.ID);
    });

    // GetGCSafePoint
    // MockPD 初始安全点为 0，只验证查询通路可达。
    withKeyspaceConfig("", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd), NullspaceID);
        let ctx = Context::Background();
        let safe_point = mgr.GetGCSafePoint(&ctx).expect("GetGCSafePoint");
        // MockPD returns 0 as initial safe point
        // 未 advance 时全局 GC 安全点恒为 0。
        assert_eq!(safe_point, 0);
    });
}

/// TestKeyspaceManager — Go TestKeyspaceManager subtests.
///
/// 覆盖 keyspace Manager：Set 屏障、TTL=0 转删除、显式 Delete、Get 安全点。
#[test]
fn test_keyspace_manager() {
    // SetGCBarrier
    // 设置后仅 testKeyspaceID 有屏障，Nullspace 侧保持空。
    withKeyspaceConfig("test_keyspace", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), testKeyspaceID);
        let ctx = Context::Background();
        let safe_point = sp("br-test-barrier", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");
        requireBarrier(
            &getState(&ctx, &mock_pd, testKeyspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );
        requireNoBarrier(&getState(&ctx, &mock_pd, NullspaceID), &safe_point.ID);
    });

    // SetGCBarrier_ZeroTTL_CallsDelete
    // TTL=0 应走删除分支：先前存在的屏障被清掉。
    withKeyspaceConfig("test_keyspace", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), testKeyspaceID);
        let ctx = Context::Background();

        let mut safe_point = sp("br-test-delete", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");
        requireBarrier(
            &getState(&ctx, &mock_pd, testKeyspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );

        // Set with TTL=0, should delete
        // 与统一 Manager「TTL<=0 即删除」语义对齐。
        safe_point.TTL = 0;
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint TTL=0");
        requireNoBarrier(&getState(&ctx, &mock_pd, testKeyspaceID), &safe_point.ID);
    });

    // DeleteGCBarrier
    // 显式 DeleteServiceSafePoint 清除 keyspace 屏障。
    withKeyspaceConfig("test_keyspace", || {
        let mock_pd = newTestMockPD();
        let mgr = NewManager(std::sync::Arc::new(mock_pd.clone()), testKeyspaceID);
        let ctx = Context::Background();
        let safe_point = sp("br-test-to-delete", 300, 1000);
        mgr.SetServiceSafePoint(&ctx, safe_point.clone())
            .expect("SetServiceSafePoint");
        requireBarrier(
            &getState(&ctx, &mock_pd, testKeyspaceID),
            &safe_point.ID,
            safe_point.BackupTS - 1,
        );

        // 显式删除与 TTL=0 殊途同归，最终 keyspace 侧无该屏障。
        mgr.DeleteServiceSafePoint(&ctx, safe_point.clone())
            .expect("DeleteServiceSafePoint");
        requireNoBarrier(&getState(&ctx, &mock_pd, testKeyspaceID), &safe_point.ID);
    });

    // GetGCSafePoint
    // keyspace 路径初始安全点同样为 0。
    withKeyspaceConfig("test_keyspace", || {
        let mock_pd = newTestMockPD();
        // 走 GetGCState 而非全局 UpdateGCSafePoint(0)。
        let mgr = NewManager(std::sync::Arc::new(mock_pd), testKeyspaceID);
        let ctx = Context::Background();
        let safe_point = mgr.GetGCSafePoint(&ctx).expect("GetGCSafePoint");
        assert_eq!(safe_point, 0);
    });
}
