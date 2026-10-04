// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 统计 handle/util 单元测试：用假会话/InfoSchema 复现 Go `util_test.go` 场景。
//
// Same-path Go->Rust mapping for `util_test.go`. The canonical `util` crate
// decouples every Go test scenario from a real mock TiKV store behind small
// traits (`SessionContext`, `SessionPool`, `InfoSchema`), so each Go test is
// reproduced here by driving the real production functions
// (`is_special_global_index`, `call_with_sctx`, `CachedTableInfoGetter`)
// with fakes that implement those traits, instead of `testkit`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::*;
use astersql_testkit_testfailpoint::enable;

// ---------------------------------------------------------------------------
// TestIsSpecialGlobalIndex
// ---------------------------------------------------------------------------

/// Go builds `create table t(a int, b int, c int, d varchar(20), unique index
/// b(b) global, index c(c), unique index ub_s((b+1)) global, unique index
/// ud_s(d(3)) global, index b_s((b+1)), index d_s(d(3))) partition by hash(a)
/// partitions 5` and asserts `IsSpecialGlobalIndex` is true only for the two
/// *global* indexes built over a virtual generated column (`ub_s`, the
/// `(b+1)` expression index) or a prefix (`ud_s`, `d(3)`). Non-global indexes
/// (`c`, `b_s`, `d_s`) and the plain global column index (`b`) must be false.
///
/// `is_special_global_index` only needs the column/index shape, so the table
/// is modeled directly instead of executing DDL through a mock store.
#[test]
/// 仅全局+虚拟生成列/前缀索引应为 special；普通全局列索引与非全局索引为 false。
fn test_is_special_global_index() {
    // Columns: a(0), b(1), c(2), d(3), plus the hidden virtual generated
    // column backing the `(b+1)` expression indexes (4).
    let table = TableInfo {
        columns: vec![
            ColumnInfo {
                virtual_generated: false,
            }, // a
            ColumnInfo {
                virtual_generated: false,
            }, // b
            ColumnInfo {
                virtual_generated: false,
            }, // c
            ColumnInfo {
                virtual_generated: false,
            }, // d
            ColumnInfo {
                virtual_generated: true,
            }, // (b+1) generated column
        ],
    };
    let column = |offset: usize| IndexColumn {
        offset,
        length: UNSPECIFIED_LENGTH,
    };
    let prefix = |offset: usize, length: i32| IndexColumn { offset, length };

    let cases: Vec<(&str, IndexInfo, bool)> = vec![
        (
            "b",
            IndexInfo {
                global: true,
                columns: vec![column(1)],
            },
            false,
        ),
        (
            "c",
            IndexInfo {
                global: false,
                columns: vec![column(2)],
            },
            false,
        ),
        (
            "ub_s",
            IndexInfo {
                global: true,
                columns: vec![column(4)],
            },
            true,
        ),
        (
            "ud_s",
            IndexInfo {
                global: true,
                columns: vec![prefix(3, 3)],
            },
            true,
        ),
        (
            "b_s",
            IndexInfo {
                global: false,
                columns: vec![column(4)],
            },
            false,
        ),
        (
            "d_s",
            IndexInfo {
                global: false,
                columns: vec![prefix(3, 3)],
            },
            false,
        ),
    ];

    let mut checked = 0;
    for (name, index, want_special) in &cases {
        checked += 1;
        assert_eq!(
            *want_special,
            is_special_global_index(index, &table),
            "index {name} special-ness mismatch"
        );
    }
    assert_eq!(cases.len(), checked);
}

// ---------------------------------------------------------------------------
// Shared fakes for the CallWithSCtx tests.
// ---------------------------------------------------------------------------

/// 构造 `update_sctx_vars_for_stats` 所需的默认全局变量表。
fn default_global_vars(time_zone: &str) -> HashMap<&'static str, String> {
    HashMap::from([
        (TIDB_ENABLE_ASYNC_MERGE_GLOBAL_STATS, "0".to_owned()),
        (TIDB_ANALYZE_PARTITION_CONCURRENCY, "0".to_owned()),
        (TIDB_ANALYZE_VERSION, "0".to_owned()),
        (TIDB_ENABLE_HISTORICAL_STATS, "0".to_owned()),
        (TIDB_PARTITION_PRUNE_MODE, "static".to_owned()),
        (TIDB_ENABLE_ANALYZE_SNAPSHOT, "0".to_owned()),
        (TIDB_ANALYZE_SKIP_COLUMN_TYPES, String::new()),
        (TIDB_SKIP_MISSING_PARTITION_STATS, "0".to_owned()),
        (INNODB_LOCK_WAIT_TIMEOUT, "50".to_owned()),
        (TIME_ZONE, time_zone.to_owned()),
    ])
}

/// 内存全局变量表，实现 `GlobalVariableAccessor`。
struct FakeGlobalVars {
    values: Mutex<HashMap<&'static str, String>>,
}

impl GlobalVariableAccessor for FakeGlobalVars {
    fn get_global_sys_var(&self, name: &str) -> Result<String, StatsError> {
        self.values
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| **key == name)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| StatsError::GlobalVariable {
                name: name.to_owned(),
                message: "unknown global variable in fake session".to_owned(),
            })
    }
}

/// Minimal `SessionContext`: only `session_variables`, `set_system_variable`,
/// and `location` are exercised by `update_sctx_vars_for_stats`, which is the
/// only thing `call_with_sctx` invokes before running the caller's callback in
/// these tests. Any other trait method is unreachable from this suite.
struct FakeSessionContext {
    variables: Arc<SessionVariables>,
    location: Mutex<String>,
}

impl FakeSessionContext {
    fn new(global_vars: Arc<FakeGlobalVars>) -> Self {
        Self {
            variables: Arc::new(SessionVariables::new(global_vars)),
            location: Mutex::new(String::new()),
        }
    }
}

impl SessionContext for FakeSessionContext {
    fn session_variables(&self) -> Arc<SessionVariables> {
        Arc::clone(&self.variables)
    }

    fn transaction(&self, _active: bool) -> Result<Arc<dyn Transaction>, StatsError> {
        unreachable!("this suite never starts a transaction through CallWithSCtx")
    }

    fn sql_executor(&self) -> Arc<dyn SqlExecutor> {
        unreachable!("this suite never executes SQL through CallWithSCtx")
    }

    fn restricted_sql_executor(&self) -> Arc<dyn RestrictedSqlExecutor> {
        unreachable!("this suite never executes restricted SQL through CallWithSCtx")
    }

    fn set_system_variable(&self, name: &str, value: &str) -> Result<(), StatsError> {
        if name == TIME_ZONE {
            *self.location.lock().unwrap() = value.to_owned();
        }
        Ok(())
    }

    fn location(&self) -> String {
        self.location.lock().unwrap().clone()
    }
}

/// `SessionPool` fake that always runs the callback against the same
/// in-memory session and records whether the pool considered the session
/// released afterward, mirroring Go's `defer sessionPool.Put(se)` running
/// even when the callback itself returns an error.
struct FakeSessionPool {
    context: FakeSessionContext,
    released: Arc<AtomicBool>,
}

impl SessionPool for FakeSessionPool {
    fn with_session(
        &self,
        callback: &mut dyn FnMut(&dyn SessionContext) -> Result<(), StatsError>,
    ) -> Result<(), StatsError> {
        let result = callback(&self.context);
        self.released.store(true, Ordering::Release);
        result
    }
}

// ---------------------------------------------------------------------------
// TestCallSCtxFailed
// ---------------------------------------------------------------------------

/// Go's `TestCallSCtxFailed` asserts that when the callback returns an error,
/// `CallWithSCtx` propagates it and the internal session used for the call is
/// still released (`infosync.ContainsInternalSession` is false afterward).
/// The Rust fake tracks release the same way Go's session pool `Put` does:
/// unconditionally, once the callback returns.
#[test]
/// 回调失败时错误上抛，且会话仍被池释放。
fn test_call_sctx_failed() {
    let global_vars = Arc::new(FakeGlobalVars {
        values: Mutex::new(default_global_vars("UTC")),
    });
    let released = Arc::new(AtomicBool::new(false));
    let pool = FakeSessionPool {
        context: FakeSessionContext::new(global_vars),
        released: Arc::clone(&released),
    };

    let err = call_with_sctx(
        &pool,
        |_sctx| Err(StatsError::Sql("simulated error".to_owned())),
        &[],
    );

    let err = err.expect_err("callback failure must propagate");
    assert!(err.to_string().contains("simulated error"));
    assert!(
        released.load(Ordering::Acquire),
        "session must be released even when the callback fails"
    );
}

#[test]
/// Go 的 CallWithSCtx 会通过 util.Recover 吞掉普通 panic，并返回 nil。
fn test_call_with_sctx_recovers_panic() {
    let global_vars = Arc::new(FakeGlobalVars {
        values: Mutex::new(default_global_vars("UTC")),
    });
    let pool = FakeSessionPool {
        context: FakeSessionContext::new(global_vars),
        released: Arc::new(AtomicBool::new(false)),
    };

    let result = call_with_sctx(
        &pool,
        |_sctx| -> Result<(), StatsError> { panic!("simulated panic") },
        &[],
    );

    assert_eq!(Ok(()), result);
}

// ---------------------------------------------------------------------------
// TestCallWithSCtxSyncsStmtCtxTimeZone
// ---------------------------------------------------------------------------

/// Go's `TestCallWithSCtxSyncsStmtCtxTimeZone` proves that each `CallWithSCtx`
/// invocation re-syncs the statement time zone from the *current* global
/// `time_zone`, even when the callback issues no SQL of its own (some stats
/// paths, e.g. `MergePartitionStats2GlobalStats`, read `StmtCtx.TimeZone()`
/// directly). The canonical Rust design folds "session location" and
/// "statement time zone" into `SessionVariables::statement_time_zone` /
/// `SessionContext::location`, both refreshed by `update_sctx_vars_for_stats`
/// on every `call_with_sctx` call; this test drives that exact code path.
#[test]
/// 每次 CallWithSCtx 都从最新全局 time_zone 同步语句时区。
fn test_call_with_sctx_syncs_stmt_ctx_time_zone() {
    let global_vars = Arc::new(FakeGlobalVars {
        values: Mutex::new(default_global_vars("UTC")),
    });
    let pool = FakeSessionPool {
        context: FakeSessionContext::new(Arc::clone(&global_vars)),
        released: Arc::new(AtomicBool::new(false)),
    };

    let mut old_stmt_tz = String::new();
    call_with_sctx(
        &pool,
        |sctx| {
            old_stmt_tz = sctx.session_variables().statement_time_zone();
            Ok(())
        },
        &[],
    )
    .expect("first CallWithSCtx must succeed");
    assert!(!old_stmt_tz.is_empty());

    global_vars
        .values
        .lock()
        .unwrap()
        .insert(TIME_ZONE, "Asia/Shanghai".to_owned());

    let mut vars_tz = String::new();
    let mut stmt_tz = String::new();
    call_with_sctx(
        &pool,
        |sctx| {
            // Deliberately runs no SQL: some stats paths read StmtCtx
            // directly before any statement executes.
            vars_tz = sctx.location();
            stmt_tz = sctx.session_variables().statement_time_zone();
            Ok(())
        },
        &[],
    )
    .expect("second CallWithSCtx must succeed");

    assert!(!vars_tz.is_empty());
    assert!(!stmt_tz.is_empty());
    assert_ne!(old_stmt_tz, vars_tz);
    assert_eq!(vars_tz, stmt_tz);
}

#[test]
/// ExecRows 命中 Go 同名 failpoint 时应在访问执行器前返回超时错误。
fn test_exec_rows_honors_timeout_failpoint() {
    let global_vars = Arc::new(FakeGlobalVars {
        values: Mutex::new(default_global_vars("UTC")),
    });
    let pool = FakeSessionPool {
        context: FakeSessionContext::new(global_vars),
        released: Arc::new(AtomicBool::new(false)),
    };
    let _guard = enable(EXEC_ROWS_TIMEOUT_FAILPOINT, "return(true)");

    let error = exec_rows(&pool.context, "select 1", &[]).expect_err("failpoint must fail");
    assert!(error.to_string().contains("inject timeout error"));
}

#[test]
/// 更新统计会话变量时只保留 Go 白名单中的 analyze-skip 类型。
fn test_update_sctx_vars_filters_analyze_skip_column_types() {
    let values = default_global_vars("UTC");
    let global_vars = Arc::new(FakeGlobalVars {
        values: Mutex::new(HashMap::from([
            (TIDB_ENABLE_ASYNC_MERGE_GLOBAL_STATS, "0".to_owned()),
            (TIDB_ANALYZE_PARTITION_CONCURRENCY, "0".to_owned()),
            (TIDB_ANALYZE_VERSION, "0".to_owned()),
            (TIDB_ENABLE_HISTORICAL_STATS, "0".to_owned()),
            (TIDB_PARTITION_PRUNE_MODE, "static".to_owned()),
            (TIDB_ENABLE_ANALYZE_SNAPSHOT, "0".to_owned()),
            (
                TIDB_ANALYZE_SKIP_COLUMN_TYPES,
                "JSON,text,int,mediumblob,invalid".to_owned(),
            ),
            (TIDB_SKIP_MISSING_PARTITION_STATS, "0".to_owned()),
            (INNODB_LOCK_WAIT_TIMEOUT, "50".to_owned()),
            (TIME_ZONE, values[TIME_ZONE].clone()),
        ])),
    });
    let context = FakeSessionContext::new(global_vars);

    update_sctx_vars_for_stats(&context).expect("global variables must update");

    assert_eq!(
        vec!["json", "text", "mediumblob"],
        context.session_variables().analyze_skip_column_types()
    );
}

// ---------------------------------------------------------------------------
// TestTableItemByIDForInitStatsAvoidsV1PartitionScan
// ---------------------------------------------------------------------------

/// Fake `InfoSchema` V1 (i.e. `is_v2() == false`) with one normal table and
/// one partitioned table. `table_item_by_partition_id` panics exactly like
/// Go's `partitionItemLookupForbiddenInfoSchema.TableItemByPartitionID`,
/// proving `TableItemByIDForInitStats` never falls back to the expensive V1
/// per-partition scan and instead resolves partitions through the getter's
/// cached partition-to-table map.
struct PartitionItemLookupForbiddenInfoSchema {
    normal: Arc<TableMeta>,
    partitioned: Arc<TableMeta>,
}

impl InfoSchema for PartitionItemLookupForbiddenInfoSchema {
    fn schema_meta_version(&self) -> i64 {
        1
    }

    fn is_v2(&self) -> bool {
        false
    }

    fn table_by_id(&self, physical_id: i64) -> Option<Arc<TableMeta>> {
        [&self.normal, &self.partitioned]
            .into_iter()
            .find(|table| table.id == physical_id)
            .cloned()
    }

    fn find_table_by_partition_id(&self, partition_id: i64) -> Option<Arc<TableMeta>> {
        self.partitioned
            .partitions
            .iter()
            .any(|partition| partition.id == partition_id)
            .then(|| Arc::clone(&self.partitioned))
    }

    fn table_item_by_id(&self, id: i64) -> Option<TableItem> {
        [&self.normal, &self.partitioned]
            .into_iter()
            .find(|table| table.id == id)
            .map(|table| TableItem {
                id: table.id,
                schema_name: table.schema_name.clone(),
                table_name: table.table_name.clone(),
            })
    }

    fn table_item_by_partition_id(&self, partition_id: i64) -> Option<TableItem> {
        panic!("TableItemByPartitionID should not be called for partition ID {partition_id}");
    }

    fn partitioned_tables(&self) -> Vec<Arc<TableMeta>> {
        vec![Arc::clone(&self.partitioned)]
    }
}

#[test]
/// 验证 init stats 经缓存 pid→表映射解析分区，不触发 V1 昂贵的按分区扫描。
fn test_table_item_by_id_for_init_stats_avoids_v1_partition_scan() {
    let normal = Arc::new(TableMeta {
        id: 1,
        schema_name: "test".to_owned(),
        table_name: "normal".to_owned(),
        partitions: Vec::new(),
    });
    let partitioned = Arc::new(TableMeta {
        id: 2,
        schema_name: "test".to_owned(),
        table_name: "partitioned".to_owned(),
        partitions: vec![PartitionDefinition {
            id: 101,
            name: "p0".to_owned(),
        }],
    });
    let partition_id = partitioned.partitions[0].id;
    let is = PartitionItemLookupForbiddenInfoSchema {
        normal: Arc::clone(&normal),
        partitioned: Arc::clone(&partitioned),
    };
    let getter = new_table_info_getter();

    let item = getter
        .table_item_by_id_for_init_stats(&is, normal.id)
        .expect("normal table must resolve");
    assert_eq!("normal", item.table_name);

    let item = getter
        .table_item_by_id_for_init_stats(&is, partition_id)
        .expect("partition must resolve through the cached pid2tid map");
    assert_eq!("partitioned", item.table_name);

    assert!(
        getter
            .table_item_by_id_for_init_stats(&is, 1_i64 << 60)
            .is_none()
    );
}

#[test]
fn stats_refresh_ignores_deprecated_merge_concurrency() {
    let mut values = default_global_vars("UTC");
    values.remove("tidb_merge_partition_stats_concurrency");
    let global_vars = Arc::new(FakeGlobalVars {
        values: Mutex::new(values),
    });
    let context = FakeSessionContext::new(global_vars);
    update_sctx_vars_for_stats(&context).expect("refresh must not read the obsolete concurrency");
}
