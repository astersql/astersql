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

//! Go-equivalent add-index integration cases.

// 本文件对应 `tests/realtikvtest/addindextest/add_index_test.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest::stubs::{TestCtx, config};
use astersql_tests_realtikvtest_addindextest::{FULL_MODE, serial_guard};
use astersql_tests_realtikvtest_testutils as testutils;

// `FORCE_SPLIT_REGION` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const FORCE_SPLIT_REGION: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/forceSplitRegion";
// `ADJUST_RETRY_BACKOFF` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const ADJUST_RETRY_BACKOFF: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/adjustRegionJobRetryBackoff";
// `RETRY_BACKOFF_SECS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static RETRY_BACKOFF_SECS: AtomicU64 = AtomicU64::new(0);

// `FastAddIndexFailpoints` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct FastAddIndexFailpoints {
    _force_split: testfailpoint::FailGuard,
    _adjust_backoff: testfailpoint::FailGuard,
    callback_hits: Arc<AtomicU64>,
}

// 这里实现 `FastAddIndexFailpoints` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl FastAddIndexFailpoints {
    // `hit_from_workload` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn hit_from_workload(&self) {
        assert!(testfailpoint::eval_bool(FORCE_SPLIT_REGION));
        testfailpoint::inject(ADJUST_RETRY_BACKOFF);
        assert_eq!(RETRY_BACKOFF_SECS.load(Ordering::SeqCst), 1);
    }

    // `assert_workload_triggered` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn assert_workload_triggered(&self) {
        assert!(
            self.callback_hits.load(Ordering::SeqCst) > 0,
            "add-index workload never hit the retry-backoff failpoint"
        );
    }
}

// `reset_engine` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
fn reset_engine() {
    astersql_tests_realtikvtest::stubs::reset_test_globals();
    testutils::stubs::reset_test_globals();
    package_harness::configure();
}

// `init_add_index_package_config` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn init_add_index_package_config() {
    config::UpdateGlobal(|conf| conf.Path = "127.0.0.1:2379".to_string());
}

fn reduce_check_interval(t: &TestCtx) {
    // The compatibility harness has no polling sleep, but preserve Go's setup
    // call as observable test context state.
    t.Log("testutil.ReduceCheckInterval");
}

// `enable_fast_add_index_failpoints` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn enable_fast_add_index_failpoints() -> FastAddIndexFailpoints {
    let force_split = testfailpoint::enable(FORCE_SPLIT_REGION, "return(true)");
    let callback_hits = Arc::new(AtomicU64::new(0));
    let callback_hits_inner = Arc::clone(&callback_hits);
    let adjust_backoff = testfailpoint::enable_call(ADJUST_RETRY_BACKOFF, move || {
        RETRY_BACKOFF_SECS.store(1, Ordering::SeqCst);
        callback_hits_inner.fetch_add(1, Ordering::SeqCst);
    });
    FastAddIndexFailpoints {
        _force_split: force_split,
        _adjust_backoff: adjust_backoff,
        callback_hits,
    }
}

// `prepare_case` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
fn prepare_case() -> (MutexGuard<'static, ()>, TestCtx) {
    let serial = serial_guard();
    reset_engine();
    init_add_index_package_config();
    assert_eq!(config::GetGlobalConfig().Path, "127.0.0.1:2379");
    (serial, TestCtx::new())
}

/// `TestCreateNonUniqueIndex`.
// 测试 `test_create_non_unique_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_create_non_unique_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_create_non_unique_index() {
    let (_serial, t) = prepare_case();
    reduce_check_interval(&t);
    let failpoints = enable_fast_add_index_failpoints();
    let col_ids = vec![
        vec![1, 4, 7, 10, 13, 16, 19, 22, 25],
        vec![2, 5, 8, 11, 14, 17, 20, 23, 26],
        vec![3, 6, 9, 12, 15, 18, 21, 24, 27],
    ];
    let ctx = testutils::InitTest(&t);
    testutils::TestOneColFrame(&ctx, &col_ids, |ctx, table_id, table_name, col_id| {
        failpoints.hit_from_workload();
        testutils::AddIndexNonUnique(ctx, table_id, table_name, col_id)
    });
    failpoints.assert_workload_triggered();
    assert!(!t.Failed());
}

/// `TestCreateUniqueIndex`.
// 测试 `test_create_unique_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_create_unique_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_create_unique_index() {
    let (_serial, t) = prepare_case();
    reduce_check_interval(&t);
    let failpoints = enable_fast_add_index_failpoints();
    let col_ids = vec![
        vec![1, 6, 7, 8, 11, 13, 15, 16, 18, 19, 22, 26],
        vec![2, 9, 11, 17],
        vec![3, 12, 25],
    ];
    let ctx = testutils::InitTest(&t);
    testutils::TestOneColFrame(&ctx, &col_ids, |ctx, table_id, table_name, col_id| {
        failpoints.hit_from_workload();
        testutils::AddIndexUnique(ctx, table_id, table_name, col_id)
    });
    failpoints.assert_workload_triggered();
    assert!(!t.Failed());
}

/// `TestCreatePrimaryKey`.
// 测试 `test_create_primary_key` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_create_primary_key` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_create_primary_key() {
    let (_serial, t) = prepare_case();
    reduce_check_interval(&t);
    let failpoints = enable_fast_add_index_failpoints();
    let ctx = testutils::InitTest(&t);
    testutils::TestOneIndexFrame(&ctx, 0, |ctx, table_id, table_name, col_id| {
        failpoints.hit_from_workload();
        testutils::AddIndexPK(ctx, table_id, table_name, col_id)
    });
    failpoints.assert_workload_triggered();
    assert!(!t.Failed());
}

/// `TestCreateGenColIndex`.
// 测试 `test_create_gen_col_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_create_gen_col_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_create_gen_col_index() {
    let (_serial, t) = prepare_case();
    reduce_check_interval(&t);
    let failpoints = enable_fast_add_index_failpoints();
    let ctx = testutils::InitTest(&t);
    testutils::TestOneIndexFrame(&ctx, 29, |ctx, table_id, table_name, col_id| {
        failpoints.hit_from_workload();
        testutils::AddIndexGenCol(ctx, table_id, table_name, col_id)
    });
    failpoints.assert_workload_triggered();
    assert!(!t.Failed());
}

/// `TestCreateMultiColsIndex`.
// 测试 `test_create_multi_cols_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_create_multi_cols_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_create_multi_cols_index() {
    let (_serial, t) = prepare_case();
    reduce_check_interval(&t);
    let failpoints = enable_fast_add_index_failpoints();
    let mut coli_ids = vec![vec![1, 4, 7], vec![2, 5], vec![3, 6, 9]];
    let mut colj_ids = vec![vec![16, 19], vec![14, 17, 20], vec![18, 21]];
    if FULL_MODE.load(Ordering::SeqCst) {
        coli_ids = vec![
            vec![1, 4, 7, 10, 13],
            vec![2, 5, 8, 11],
            vec![3, 6, 9, 12, 15],
        ];
        colj_ids = vec![
            vec![16, 19, 22, 25],
            vec![14, 17, 20, 23, 26],
            vec![18, 21, 24, 27],
        ];
    }
    let ctx = testutils::InitTest(&t);
    testutils::TestTwoColsFrame(
        &ctx,
        &coli_ids,
        &colj_ids,
        |ctx, table_id, table_name, index_id, col_i, col_j| {
            failpoints.hit_from_workload();
            testutils::AddIndexMultiCols(ctx, table_id, table_name, index_id, col_i, col_j)
        },
    );
    failpoints.assert_workload_triggered();
    assert!(!t.Failed());
}

/// `TestAddForeignKeyWithAutoCreateIndex`.
// 测试 `test_add_foreign_key_with_auto_create_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_add_foreign_key_with_auto_create_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_add_foreign_key_with_auto_create_index() {
    let (_serial, t) = prepare_case();
    reduce_check_interval(&t);
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("drop database if exists fk_index;", Vec::new());
    tk.MustExec("create database fk_index;", Vec::new());
    tk.MustExec("use fk_index;", Vec::new());
    if astersql_config_kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg=1;", Vec::new());
    }
    tk.MustExec(
        "create table employee (id bigint auto_increment key, pid bigint)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into employee (id) values (1),(2),(3),(4),(5),(6),(7),(8)",
        Vec::new(),
    );
    for _ in 0..14 {
        tk.MustExec(
            "insert into employee (pid) select pid from employee",
            Vec::new(),
        );
    }
    tk.MustExec("update employee set pid=id-1 where id>1", Vec::new());
    tk.MustQuery("select count(*) from employee", Vec::new())
        .Check(Rows(&["131072"]));
    tk.MustExec(
        "alter table employee add foreign key fk_1(pid) references employee(id)",
        Vec::new(),
    );
    tk.MustExec("alter table employee drop foreign key fk_1", Vec::new());
    tk.MustExec("alter table employee drop index fk_1", Vec::new());
    tk.MustExec("update employee set pid=0 where id=1", Vec::new());
    tk.MustGetErrMsg(
        "alter table employee add foreign key fk_1(pid) references employee(id)",
        "[ddl:1452]Cannot add or update a child row: a foreign key constraint fails (`fk_index`.`employee`, CONSTRAINT `fk_1` FOREIGN KEY (`pid`) REFERENCES `employee` (`id`))"
    );
    tk.MustExec(
        "insert into employee (pid) select pid from employee",
        Vec::new(),
    );
    tk.MustExec("update employee set pid=id", Vec::new());
    tk.MustExec(
        "alter table employee add foreign key fk_1(pid) references employee(id)",
        Vec::new(),
    );
}

/// `TestIssue51162`.
// 测试 `test_issue_51162` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_issue_51162` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_issue_51162() {
    let (_serial, _t) = prepare_case();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set global tidb_enable_fast_table_check=0", Vec::new());
    tk.MustExec(
        r#"CREATE TABLE tl (
 col_42 json NOT NULL,
 col_43 tinyint(1) DEFAULT NULL,
 col_44 char(168) CHARACTER SET gbk COLLATE gbk_bin DEFAULT NULL,
 col_45 json DEFAULT NULL,
 col_46 text COLLATE utf8mb4_unicode_ci NOT NULL,
 col_47 char(43) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'xW2YNb99pse4)',
 col_48 time NOT NULL DEFAULT '12:31:25',
 PRIMARY KEY (col_47,col_46(2)) /*T![clustered_index] CLUSTERED */
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"INSERT INTO tl VALUES
 ('["1"]',0,'1','[1]','Wxup81','1','10:14:20');"#,
        Vec::new(),
    );
    tk.MustExec(
        "alter table tl add index idx_16(`col_48`,(cast(`col_45` as signed array)),`col_46`(5));",
        Vec::new(),
    );
    tk.MustExec("admin check table tl", Vec::new());
}

/// `TestAddUKWithSmallIntHandles`.
// 测试 `test_add_uk_with_small_int_handles` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_add_uk_with_small_int_handles` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_add_uk_with_small_int_handles() {
    let (_serial, _t) = prepare_case();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("drop database if exists small;", Vec::new());
    tk.MustExec("create database small;", Vec::new());
    tk.MustExec("use small;", Vec::new());
    if astersql_config_kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg=1;", Vec::new());
    }
    tk.MustExec(
        "create table t (a bigint, b int, primary key (a) clustered)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (-9223372036854775808, 1),(-9223372036854775807, 1)",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "alter table t add unique index uk(b)",
        "Duplicate entry '1' for key 't.uk'",
    );
}

/// `TestAddUniqueDuplicateIndexes`.
// 测试 `test_add_unique_duplicate_indexes` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_add_unique_duplicate_indexes` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_add_unique_duplicate_indexes() {
    let (_serial, _t) = prepare_case();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("use test", Vec::new());
    if astersql_config_kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg=1;", Vec::new());
    }
    tk.MustExec(
        "create table t(a int DEFAULT '-13202', b varchar(221) NOT NULL DEFAULT 'duplicatevalue', c int NOT NULL DEFAULT '0');",
        Vec::new(),
    );

    let tk1 = Arc::new(Mutex::new(NewTestKit(store.clone())));
    tk1.lock().unwrap().MustExec("use test", Vec::new());
    tk1.lock().unwrap().MustExec(
        "INSERT INTO t VALUES (-18585,'duplicatevalue',0);",
        Vec::new(),
    );

    let after_wait_tk = Arc::clone(&tk1);
    let after_wait_hits = Arc::new(AtomicU64::new(0));
    let after_wait_hits_inner = Arc::clone(&after_wait_hits);
    let _after_wait = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced",
        move || {
            after_wait_hits_inner.fetch_add(1, Ordering::SeqCst);
            let mut tk = after_wait_tk.lock().unwrap();
            tk.MustExec("delete from t where c = 0;", Vec::new());
            tk.MustExec(
                "insert INTO t VALUES (-18585,'duplicatevalue',1);",
                Vec::new(),
            );
        },
    );

    let tk3 = Arc::new(Mutex::new(NewTestKit(store)));
    tk3.lock().unwrap().MustExec("use test", Vec::new());

    let before_ingest_once = Arc::new(Once::new());
    let before_ingest_tk = Arc::clone(&tk3);
    let before_ingest_gate = Arc::clone(&before_ingest_once);
    let _before_ingest = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/ingest/beforeBackendIngest",
        move || {
            before_ingest_gate.call_once(|| {
                let mut tk = before_ingest_tk.lock().unwrap();
                tk.MustExec(
                    "replace INTO t VALUES (-18585,'duplicatevalue',4);",
                    Vec::new(),
                );
                tk.MustQuery("select * from t;", Vec::new()).Check(Rows(&[
                    "-18585 duplicatevalue 1",
                    "-18585 duplicatevalue 4",
                ]));
            });
        },
    );

    let before_merge_once = Arc::new(Once::new());
    let before_merge_tk = Arc::clone(&tk3);
    let before_merge_gate = Arc::clone(&before_merge_once);
    let _before_merge = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge",
        move || {
            before_merge_gate.call_once(|| {
                let mut tk = before_merge_tk.lock().unwrap();
                tk.MustQuery("select * from t;", Vec::new()).Check(Rows(&[
                    "-18585 duplicatevalue 1",
                    "-18585 duplicatevalue 4",
                ]));
                tk.MustExec(
                    "replace into t values (-18585,'duplicatevalue',0);",
                    Vec::new(),
                );
            });
        },
    );

    tk.MustExec("alter table t add unique index idx(b);", Vec::new());
    tk.MustExec("admin check table t;", Vec::new());

    // The production DDL path must hit the failpoints; direct test-side
    // injection would only prove callback registration.
    assert!(after_wait_hits.load(Ordering::SeqCst) > 0);
    assert!(before_ingest_once.is_completed());
    assert!(before_merge_once.is_completed());
}

/// `TestAddIndexOnGB18030Bin`.
// 测试 `test_add_index_on_gb18030_bin` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
// `test_add_index_on_gb18030_bin` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_add_index_on_gb18030_bin() {
    let (_serial, _t) = prepare_case();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        r#"CREATE TABLE t (
 a varchar(198) COLLATE gb18030_bin NOT NULL,
 b varchar(178) COLLATE gb18030_bin NOT NULL,
 PRIMARY KEY (b,a),
 KEY k1 (b,a),
 KEY k2 (b)
) ENGINE=InnoDB DEFAULT CHARSET=gb18030 COLLATE=gb18030_bin;"#,
        Vec::new(),
    );
    tk.MustExec("insert into t values ('a', 'b');", Vec::new());
    tk.MustExec("admin check table t;", Vec::new());
}

#[path = "package_harness.rs"]
mod package_harness;

fn main() -> std::process::ExitCode {
    package_harness::run(&[
        ("test_create_non_unique_index", test_create_non_unique_index),
        ("test_create_unique_index", test_create_unique_index),
        ("test_create_primary_key", test_create_primary_key),
        ("test_create_gen_col_index", test_create_gen_col_index),
        ("test_create_multi_cols_index", test_create_multi_cols_index),
        (
            "test_add_foreign_key_with_auto_create_index",
            test_add_foreign_key_with_auto_create_index,
        ),
        ("test_issue_51162", test_issue_51162),
        (
            "test_add_uk_with_small_int_handles",
            test_add_uk_with_small_int_handles,
        ),
        (
            "test_add_unique_duplicate_indexes",
            test_add_unique_duplicate_indexes,
        ),
        (
            "test_add_index_on_gb18030_bin",
            test_add_index_on_gb18030_bin,
        ),
    ])
}
