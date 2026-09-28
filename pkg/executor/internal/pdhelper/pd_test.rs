// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 对应 Go `pd_test.go`：验证 PDHelper TTL 缓存的命中、LRU 驱逐与过期行为。
//
// 测试前通过 `main_test::test_main` 校验包级配置，再用 MockClient 统计 PD miss 次数。
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use crate::{InternalSourceContext, PDHelper, PdHelperError, RegionStats, SessionContext};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 测试上下文，跟踪 internal stats foreground 标记。
struct TestContext {
    /// 是否已标记为内部统计前台来源。
    internal_stats_foreground: bool,
}

impl InternalSourceContext for TestContext {
    fn with_internal_stats_foreground(mut self) -> Self {
        self.internal_stats_foreground = true;
        self
    }
}

#[derive(Default)]
/// 假 PD/SQL 客户端：每次 PD 查询递增 miss 计数。
struct MockClient {
    /// PD 未命中（实际查询）次数。
    miss_cnt: AtomicI32,
}

impl MockClient {
    /// 返回当前 miss 计数。
    fn get_miss_cnt(&self) -> i32 {
        self.miss_cnt.load(Ordering::SeqCst)
    }

    /// 模拟 PD 返回：Region 数=3（大表路径），storage_keys=1。
    fn get_fake_approximate_table_count_from_storage(
        &self,
    ) -> Result<Option<RegionStats>, PdHelperError> {
        self.miss_cnt.fetch_add(1, Ordering::SeqCst);
        Ok(Some(RegionStats {
            count: 3,
            storage_keys: 1,
        }))
    }
}

impl SessionContext<TestContext> for MockClient {
    fn get_pd_region_stats(
        &self,
        _ctx: &TestContext,
        _physical_id: i64,
        include_stats: bool,
    ) -> Result<Option<RegionStats>, PdHelperError> {
        assert!(include_stats);
        self.get_fake_approximate_table_count_from_storage()
    }

    fn exec_restricted_count(
        &self,
        ctx: TestContext,
        _sql: &str,
    ) -> Result<Option<i64>, PdHelperError> {
        assert!(ctx.internal_stats_foreground);
        Ok(Some(1))
    }
}

#[test]
/// 端到端核对 TTL 缓存：配置断言、命中、容量驱逐、过期后重新 miss。
fn test_ttl_cache() {
    // 先跑包级 TestMain 设置并断言关键配置。
    let setup = crate::main_test::test_main();
    assert_eq!(setup.config.autoid_step, 5_000);
    assert_eq!(setup.config.slow_threshold_ms, 30_000);
    assert_eq!(setup.config.async_commit_safe_window_ms, 0);
    assert_eq!(setup.config.async_commit_allowed_clock_drift_ms, 0);
    assert!(setup.config.allows_expression_index);
    // TTL=100ms，容量=2，便于观察驱逐与过期。
    let helper = PDHelper::with_cache_config(Duration::from_millis(100), 2);
    let client = MockClient::default();
    let ctx = TestContext::default();

    assert_eq!(
        helper.GetApproximateTableCountFromStorage(
            ctx.clone(),
            &client,
            1,
            "db",
            "table",
            "partition",
        ),
        // 首次查询 miss；同键再次查询应命中。
        (1.0, true)
    );
    assert_eq!(client.get_miss_cnt(), 1);

    helper.GetApproximateTableCountFromStorage(ctx.clone(), &client, 1, "db", "table", "partition");
    assert_eq!(client.get_miss_cnt(), 1);

    helper.GetApproximateTableCountFromStorage(
        ctx.clone(),
        &client,
        2,
        "db1",
        "table1",
        "partition",
    );
    assert_eq!(client.get_miss_cnt(), 2);

    helper.GetApproximateTableCountFromStorage(
        ctx.clone(),
        &client,
        3,
        "db2",
        "table2",
        "partition",
    );
    helper.GetApproximateTableCountFromStorage(ctx.clone(), &client, 1, "db", "table", "partition");
    assert_eq!(client.get_miss_cnt(), 4);

    helper.GetApproximateTableCountFromStorage(
        ctx.clone(),
        &client,
        3,
        "db2",
        "table2",
        "partition",
    );
    assert_eq!(client.get_miss_cnt(), 4);

    // 过期后三个键均应重新 miss。
    std::thread::sleep(Duration::from_millis(200));
    for (physical_id, db, table) in [
        (1, "db", "table"),
        (2, "db1", "table1"),
        (3, "db2", "table2"),
    ] {
        helper.GetApproximateTableCountFromStorage(
            ctx.clone(),
            &client,
            physical_id,
            db,
            table,
            "partition",
        );
    }
    assert_eq!(client.get_miss_cnt(), 7);
}
