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

// pdhelper 迁移相关 Aster 单元测试。
//
// 用 MockSession 模拟 PD Region 统计与受限 COUNT SQL，验证：
// TTL 缓存命中/LRU 驱逐/过期、大小表路径选择、分区名转义、
// 失败即失败（fail-closed）、失败结果缓存，以及清理 worker 启停幂等。
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::{
    InternalSourceContext, PDHelper, PdHelperError, RegionStats, SessionContext,
    approximate_table_count_key,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 测试用会话上下文，记录是否打开了 internal stats foreground 标记。
struct TestContext {
    /// 对应 `kv.WithInternalSourceType` 的 foreground stats 标记。
    internal_stats_foreground: bool,
}

impl InternalSourceContext for TestContext {
    fn with_internal_stats_foreground(mut self) -> Self {
        self.internal_stats_foreground = true;
        self
    }
}

/// 可配置 PD/SQL 返回值并计数调用次数的假会话。
struct MockSession {
    /// `get_pd_region_stats` 调用次数。
    pd_calls: AtomicUsize,
    /// `exec_restricted_count` 调用次数。
    sql_calls: AtomicUsize,
    /// PD 查询预设结果。
    pd_result: Mutex<Result<Option<RegionStats>, PdHelperError>>,
    /// 受限 COUNT SQL 预设结果。
    sql_result: Mutex<Result<Option<i64>, PdHelperError>>,
    /// 最近一次执行的 SQL 文本。
    last_sql: Mutex<Option<String>>,
    /// 最近一次 SQL 调用携带的上下文。
    last_sql_context: Mutex<Option<TestContext>>,
}

impl MockSession {
    /// 构造带固定 PD/SQL 结果的 MockSession。
    fn new(
        pd_result: Result<Option<RegionStats>, PdHelperError>,
        sql_result: Result<Option<i64>, PdHelperError>,
    ) -> Self {
        Self {
            pd_calls: AtomicUsize::new(0),
            sql_calls: AtomicUsize::new(0),
            pd_result: Mutex::new(pd_result),
            sql_result: Mutex::new(sql_result),
            last_sql: Mutex::new(None),
            last_sql_context: Mutex::new(None),
        }
    }

    /// 返回已发生的 PD 调用次数。
    fn pd_calls(&self) -> usize {
        self.pd_calls.load(Ordering::SeqCst)
    }

    /// 返回已发生的受限 SQL 调用次数。
    fn sql_calls(&self) -> usize {
        self.sql_calls.load(Ordering::SeqCst)
    }
}

impl SessionContext<TestContext> for MockSession {
    fn get_pd_region_stats(
        &self,
        _ctx: &TestContext,
        _physical_id: i64,
        include_stats: bool,
    ) -> Result<Option<RegionStats>, PdHelperError> {
        // Go 路径始终请求带统计的 Region 信息。
        assert!(include_stats, "Go always requests region statistics");
        self.pd_calls.fetch_add(1, Ordering::SeqCst);
        self.pd_result.lock().unwrap().clone()
    }

    fn exec_restricted_count(
        &self,
        ctx: TestContext,
        sql: &str,
    ) -> Result<Option<i64>, PdHelperError> {
        self.sql_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_sql.lock().unwrap() = Some(sql.to_owned());
        *self.last_sql_context.lock().unwrap() = Some(ctx);
        self.sql_result.lock().unwrap().clone()
    }
}

/// 构造“大表”会话：Region 数 > 2，应走 PD storage_keys 路径而不跑 SQL。
fn large_table(storage_keys: i64) -> MockSession {
    MockSession::new(
        Ok(Some(RegionStats {
            count: 3,
            storage_keys,
        })),
        Ok(Some(999)),
    )
}

/// 便捷封装：以固定分区名调用 `GetApproximateTableCountFromStorage`。
fn get(helper: &PDHelper, session: &MockSession, id: i64, db: &str, table: &str) -> (f64, bool) {
    helper.GetApproximateTableCountFromStorage(
        TestContext::default(),
        session,
        id,
        db,
        table,
        "partition",
    )
}

#[test]
/// 核对 TTL 缓存：命中不重复打 PD、LRU 容量驱逐、过期后重新拉取。
fn ttl_cache_matches_go_hit_eviction_and_expiration_sequence() {
    let helper = PDHelper::with_cache_config(Duration::from_millis(50), 2);
    let session = large_table(1);

    // 首次未命中走 PD；第二次应命中缓存。
    assert_eq!(get(&helper, &session, 1, "db", "table"), (1.0, true));
    assert_eq!(session.pd_calls(), 1);
    assert_eq!(get(&helper, &session, 1, "db", "table"), (1.0, true));
    assert_eq!(session.pd_calls(), 1);

    // 容量为 2：再插入第 3 个键会挤掉最久未用项，随后访问被挤出的键会再次 miss。
    get(&helper, &session, 2, "db1", "table1");
    assert_eq!(session.pd_calls(), 2);
    get(&helper, &session, 3, "db2", "table2");
    get(&helper, &session, 1, "db", "table");
    assert_eq!(session.pd_calls(), 4);
    get(&helper, &session, 3, "db2", "table2");
    assert_eq!(session.pd_calls(), 4);

    // TTL=50ms，休眠后三条都应过期并重新打 PD。
    thread::sleep(Duration::from_millis(100));
    get(&helper, &session, 1, "db", "table");
    get(&helper, &session, 2, "db1", "table1");
    get(&helper, &session, 3, "db2", "table2");
    assert_eq!(session.pd_calls(), 7);
}

#[test]
/// 核对缓存键格式，以及大表（Region 数>2）直接使用 PD storage_keys。
fn key_and_large_table_pd_path_match_go() {
    assert_eq!(
        approximate_table_count_key(-7, "db_name", "t", "p_1"),
        "-7_db_name_t_p_1"
    );

    let helper = PDHelper::with_cache_config(Duration::from_secs(1), 8);
    let session = large_table(4_294_967_300);
    assert_eq!(
        get(&helper, &session, 42, "db", "table"),
        (4_294_967_300.0, true)
    );
    assert_eq!(session.pd_calls(), 1);
    assert_eq!(session.sql_calls(), 0);
}

#[test]
/// PD 的 StorageKeys 在 Go 中是 int64；转换为 float64 时必须保留负号。
fn pd_storage_keys_preserve_go_signed_i64_semantics() {
    let helper = PDHelper::with_cache_config(Duration::from_secs(1), 8);
    let session = MockSession::new(
        Ok(Some(RegionStats {
            count: 3,
            storage_keys: -7,
        })),
        Ok(Some(999)),
    );

    assert_eq!(get(&helper, &session, 43, "db", "table"), (-7.0, true));
    assert_eq!(session.sql_calls(), 0);
}

#[test]
/// 小表走受限 COUNT，并核对标识符反引号转义与 foreground 上下文。
fn small_table_uses_restricted_count_with_escaped_partition_sql() {
    let helper = PDHelper::with_cache_config(Duration::from_secs(1), 8);
    let session = MockSession::new(
        Ok(Some(RegionStats {
            count: 2,
            storage_keys: 1_000_000,
        })),
        Ok(Some(17)),
    );

    let result = helper.GetApproximateTableCountFromStorage(
        TestContext::default(),
        &session,
        9,
        "db`name",
        "table name",
        "p`1",
    );

    assert_eq!(result, (17.0, true));
    assert_eq!(session.sql_calls(), 1);
    assert_eq!(
        session.last_sql.lock().unwrap().as_deref(),
        Some("select count(*) from `db``name`.`table name` partition(`p``1`)")
    );
    assert_eq!(
        session.last_sql_context.lock().unwrap().as_ref(),
        Some(&TestContext {
            internal_stats_foreground: true
        })
    );
}

#[test]
/// 无分区名时 COUNT SQL 不带 `partition(...)` 子句。
fn small_table_without_partition_omits_partition_clause() {
    let helper = PDHelper::with_cache_config(Duration::from_secs(1), 8);
    let session = MockSession::new(
        Ok(Some(RegionStats {
            count: 1,
            storage_keys: 99,
        })),
        Ok(Some(5)),
    );

    assert_eq!(
        helper.GetApproximateTableCountFromStorage(
            TestContext::default(),
            &session,
            10,
            "db",
            "tbl",
            "",
        ),
        (5.0, true)
    );
    assert_eq!(
        session.last_sql.lock().unwrap().as_deref(),
        Some("select count(*) from `db`.`tbl`")
    );
}

#[test]
/// 存储不支持、PD 失败、SQL 失败或空结果时均返回 (0, false)，且前两类不跑 SQL。
fn unsupported_storage_pd_error_sql_error_and_empty_row_fail_closed() {
    let cases = [
        MockSession::new(Ok(None), Ok(Some(1))),
        MockSession::new(Err(PdHelperError::new("PD unavailable")), Ok(Some(1))),
        MockSession::new(
            Ok(Some(RegionStats {
                count: 1,
                storage_keys: 1,
            })),
            Err(PdHelperError::new("restricted SQL failed")),
        ),
        MockSession::new(
            Ok(Some(RegionStats {
                count: 1,
                storage_keys: 1,
            })),
            Ok(None),
        ),
    ];

    for (index, session) in cases.iter().enumerate() {
        let helper = PDHelper::with_cache_config(Duration::from_secs(1), 8);
        assert_eq!(
            get(&helper, session, index as i64, "db", "tbl"),
            (0.0, false)
        );
    }
    assert_eq!(
        cases[0].sql_calls(),
        0,
        "unsupported storage does not run SQL"
    );
    assert_eq!(cases[1].sql_calls(), 0, "PD failure does not run SQL");
}

#[test]
/// 失败结果也会写入缓存：第二次命中返回 has_pd=true，且不再打 PD。
fn failed_lookup_is_cached_exactly_like_go() {
    let helper = PDHelper::with_cache_config(Duration::from_secs(1), 8);
    let session = MockSession::new(Err(PdHelperError::new("PD unavailable")), Ok(Some(1)));

    assert_eq!(get(&helper, &session, 1, "db", "tbl"), (0.0, false));
    assert_eq!(get(&helper, &session, 1, "db", "tbl"), (0.0, true));
    assert_eq!(session.pd_calls(), 1);
}

#[test]
/// Start/Stop 清理 worker 可重复调用，且 Start 的 once 与 Go 一样是包级的。
fn cleanup_worker_start_and_stop_are_idempotent() {
    let helper = PDHelper::with_cache_config(Duration::from_millis(10), 2);
    let second = PDHelper::with_cache_config(Duration::from_millis(10), 2);

    helper.Start();
    assert!(helper.cleanup_worker_started());
    helper.Start();
    second.Start();
    assert!(
        !second.cleanup_worker_started(),
        "Go globalPDHelperOnce suppresses Start on every later helper"
    );

    helper.Stop();
    helper.Stop();
    second.Stop();
}
