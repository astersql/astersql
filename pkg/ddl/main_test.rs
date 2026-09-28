// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DDL 包测试入口环境配置。
//
// 对应 Go 侧 `TestMain` 中对全局 DDL 测试参数的初始化：schema 重试、
// 自增 ID 步长、回填（backfill，为已有数据补写索引/列）间隔、表锁、
// 表达式索引与异步提交（async commit）安全窗口等。Rust 侧以不可变
// 值对象承载这些设置，避免测试间污染进程全局可变状态。

use std::time::Duration;

/// DDL 集成测试共用的环境参数快照。
///
/// 字段含义与 Go `TestMain` 写入的全局变量一一对应，便于断言迁移一致性。
#[derive(Clone, Debug, Eq, PartialEq)]
struct DdlTestEnvironment {
    /// Common test setup completed before package-specific configuration.
    common_test_setup: bool,
    /// TiKV failpoints are enabled before DDL tests run.
    tikv_failpoints_enabled: bool,
    /// Schema 变更后轮询最新元数据的间隔。
    schema_retry_interval: Duration,
    /// Schema 重试最大次数。
    schema_retry_count: usize,
    /// 自增列（AUTO_INCREMENT）一次预分配的 ID 步长。
    auto_id_step: u64,
    /// 回填任务判定“已完成”的轮询间隔。
    backfill_finish_interval: Duration,
    /// Go tests expose test-only behavior through `ddl.RunInGoTest`.
    run_in_go_test: bool,
    /// 批量删除区间（delete range，GC 回收已删键范围）单批大小。
    batch_delete_range_size: usize,
    /// 是否启用表锁（table lock）。
    table_lock_enabled: bool,
    /// 慢查询阈值（毫秒）。
    slow_threshold_ms: u64,
    /// 是否允许表达式索引（对表达式建索引）。
    expression_index_enabled: bool,
    /// 异步提交的安全时间窗口；测试中置零以关闭该路径。
    async_commit_safe_window: Duration,
    /// 异步提交允许的时钟漂移上限。
    async_commit_clock_drift: Duration,
    /// InfoSync（集群信息同步组件）是否视为已初始化。
    info_sync_initialized: bool,
    /// Go additionally checks ingest resources during leak cleanup.
    ingest_leakage_cleanup: bool,
}

/// 构造与 Go `TestMain` 一致的 DDL 测试环境参数。
fn setup_for_ddl_tests() -> DdlTestEnvironment {
    // Mirrors TestMain's mutation order after common test setup and failpoint
    // activation. Keeping this as a value makes every test-setting observable
    // and avoids leaking mutable process globals between Rust tests.
    // 按 Go TestMain 的赋值顺序填充；用值对象代替进程全局可变配置。
    DdlTestEnvironment {
        common_test_setup: true,
        tikv_failpoints_enabled: true,
        schema_retry_interval: Duration::from_millis(50),
        schema_retry_count: 50,
        auto_id_step: 5_000,
        backfill_finish_interval: Duration::from_millis(50),
        run_in_go_test: true,
        batch_delete_range_size: 2,
        table_lock_enabled: true,
        slow_threshold_ms: 10_000,
        expression_index_enabled: true,
        async_commit_safe_window: Duration::ZERO,
        async_commit_clock_drift: Duration::ZERO,
        info_sync_initialized: true,
        ingest_leakage_cleanup: true,
    }
}

/// 校验 `setup_for_ddl_tests` 产出的参数与 Go 侧 TestMain 期望一致。
#[test]
fn test_main_environment_matches_go_setup() {
    let environment = setup_for_ddl_tests();
    assert!(environment.common_test_setup);
    assert!(environment.tikv_failpoints_enabled);
    assert_eq!(environment.schema_retry_interval, Duration::from_millis(50));
    assert_eq!(environment.schema_retry_count, 50);
    assert_eq!(environment.auto_id_step, 5_000);
    assert_eq!(
        environment.backfill_finish_interval,
        Duration::from_millis(50)
    );
    assert!(environment.run_in_go_test);
    assert_eq!(environment.batch_delete_range_size, 2);
    assert!(environment.table_lock_enabled);
    assert_eq!(environment.slow_threshold_ms, 10_000);
    assert!(environment.expression_index_enabled);
    assert_eq!(environment.async_commit_safe_window, Duration::ZERO);
    assert_eq!(environment.async_commit_clock_drift, Duration::ZERO);
    assert!(environment.info_sync_initialized);
    assert!(environment.ingest_leakage_cleanup);
}
