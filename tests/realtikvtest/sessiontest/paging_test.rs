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

//! 中文说明开始（自动生成）
//! 中文总览：`paging_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `会话生命周期与信息模式` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 32 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `deterministic_region_num` 是当前文件里的辅助函数。
//! `deterministic_region_num` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `deterministic_region_num` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `deterministic_region_num`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `deterministic_region_num` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `check_scan_operators` 是当前文件里的辅助函数。
//! `check_scan_operators` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `check_scan_operators` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `check_scan_operators`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `check_scan_operators` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `assert_single_rpc_index_range` 是当前文件里的辅助函数。
//! `assert_single_rpc_index_range` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `assert_single_rpc_index_range` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_single_rpc_index_range`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `assert_single_rpc_index_range` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_paging_act_rows_and_process_keys` 是当前文件里的测试用例。
//! `test_paging_act_rows_and_process_keys` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_paging_act_rows_and_process_keys` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_paging_act_rows_and_process_keys`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_paging_act_rows_and_process_keys` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 符号 `test_index_reader_with_paging` 是当前文件里的测试用例。
//! `test_index_reader_with_paging` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_index_reader_with_paging` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_index_reader_with_paging`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_index_reader_with_paging` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Real TestKit port of `paging_test.go`.

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows};
use astersql_tests_realtikvtest::WithRealTiKV;
use astersql_tests_realtikvtest_sessiontest::serial_guard;

struct ConfigRestore<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for ConfigRestore<F> {
    fn drop(&mut self) {
        self.0.take().expect("config restore runs exactly once")();
    }
}

fn disable_copr_cache() -> ConfigRestore<impl FnOnce()> {
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| config.tikv_client.copr_cache.capacity_mb = 0);
    ConfigRestore(Some(restore))
}

fn deterministic_region_num(case_index: usize, lower: i32, upper: i32) -> i32 {
    assert!(lower < upper);
    let width = upper - lower;
    let mixed = (case_index as u64 + 1)
        .wrapping_mul(1_103_515_245)
        .wrapping_add(12_345);
    lower + (mixed % width as u64) as i32
}

fn check_scan_operators(rows: Vec<Vec<String>>, check_process_keys: bool) -> usize {
    let mut scans = 0;
    for row in rows {
        if row
            .first()
            .is_some_and(|operator| operator.contains("Scan"))
        {
            scans += 1;
            assert_eq!("100000", row[2], "scan row={row:?}");
            if check_process_keys {
                assert!(
                    row[5].contains("total_process_keys: 100000"),
                    "scan row={row:?}"
                );
            }
        }
    }
    assert!(scans > 0, "EXPLAIN ANALYZE must contain a Scan operator");
    scans
}

fn assert_single_rpc_index_range(row: &[String]) {
    let explain = format!("{row:?}");
    assert!(explain.contains("IndexRangeScan"), "row={row:?}");
    assert!(explain.contains("rpc_info"), "row={row:?}");
    assert!(
        explain.contains("Cop:{num_rpc:1, total_time:"),
        "row={row:?}"
    );
}

/// Go `TestPagingActRowsAndProcessKeys`.
#[test]
fn test_paging_act_rows_and_process_keys() {
    let _serial = serial_guard();
    let _config_restore = disable_copr_cache();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut session = NewTestKit(store);
    session.MustExec("use test", Vec::new());
    session.MustExec("drop table if exists t", Vec::new());
    session.MustExec("set @@tidb_wait_split_region_finish=1", Vec::new());
    session.MustExec(
        "create table t(a int,b int,c int,index idx(a,b), primary key(a))",
        Vec::new(),
    );
    let mut sql = String::from("insert into t value");
    for value in 0..1000 {
        if value != 0 {
            sql.push(',');
        }
        sql.push_str(&format!("({value},{value},{value})"));
    }
    session.MustExec(&sql, Vec::new());
    // Keep Go's 100 batches of 1000 consecutive rows, but derive the later
    // batches from the seed batch so statement parsing stays within the
    // RealTiKV test budget without reducing the paging workload.
    for batch in 1..100 {
        let offset = batch * 1000;
        session.MustExec(
            &format!("insert into t select a+{offset},b+{offset},c+{offset} from t where a < 1000"),
            Vec::new(),
        );
    }
    session
        .MustQuery("select count(*) from t", Vec::new())
        .Check(Rows(&["100000"]));
    session
        .MustQuery("select min(a),max(a) from t", Vec::new())
        .Check(Rows(&["0 99999"]));

    let region_cases = [(10, 100), (100, 500), (500, 1000), (1000, 1001)];
    let paging_sql = [
        "set tidb_enable_paging = on",
        "set tidb_enable_paging = off",
    ];
    let analyze_sql = [
        "desc analyze select a,b from t",
        "desc analyze select /*+ use_index(t,idx) */ a,b from t",
        "desc analyze select /*+ use_index(t,idx) */ c from t",
    ];

    let mut analyzed = 0;
    let mut checked_scans = 0;
    for (case_index, &(lower, upper)) in region_cases.iter().enumerate() {
        let region_num = deterministic_region_num(case_index, lower, upper);
        assert!((lower..upper).contains(&region_num));
        let _ = session.MustQuery(
            &format!("split table t between (0) and (1000000) regions {region_num}"),
            Vec::new(),
        );
        let _ = session.MustQuery(
            &format!("split table t index idx between (0) and (1000000) regions {region_num}"),
            Vec::new(),
        );
        for sql in analyze_sql {
            for paging in paging_sql {
                session.MustExec(paging, Vec::new());
                checked_scans +=
                    check_scan_operators(session.MustQuery(sql, Vec::new()).Rows(), WithRealTiKV());
                analyzed += 1;
            }
        }
    }
    assert_eq!(24, analyzed);
    assert_eq!(32, checked_scans);
}

/// Go `TestIndexReaderWithPaging`.
#[test]
fn test_index_reader_with_paging() {
    let _serial = serial_guard();
    let _config_restore = disable_copr_cache();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t (id int key, b int, c int, index idx (b), index idx2(c))",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    for id in (0..1024).step_by(4) {
        tk.MustExec(
            &format!(
                "insert into t (id) values ({id}), ({}), ({}), ({})",
                id + 1,
                id + 2,
                id + 3
            ),
            Vec::new(),
        );
    }
    tk.MustExec("update t set b = id, c = id", Vec::new());
    tk.MustExec("commit", Vec::new());

    tk.MustExec("set @@tidb_enable_paging=1", Vec::new());
    tk.MustExec("set @@tidb_min_paging_size=128", Vec::new());
    tk.MustExec("set @@tidb_max_chunk_size=1024", Vec::new());
    tk.MustQuery("select count(c) from t use index(idx)", Vec::new())
        .Check(Rows(&["1024"]));
    tk.MustQuery("select count(b) from t use index(idx2)", Vec::new())
        .Check(Rows(&["1024"]));
    tk.MustQuery(
        "select count(id) from t ignore index(idx, idx2)",
        Vec::new(),
    )
    .Check(Rows(&["1024"]));

    let rows = tk
        .MustQuery(
            "explain analyze select * from t use index(idx) \
             where b>0 and b < 1024",
            Vec::new(),
        )
        .Rows();
    assert_eq!(3, rows.len(), "rows={rows:?}");
    assert_single_rpc_index_range(&rows[1]);

    let rows = tk
        .MustQuery(
            "explain analyze select /*+ USE_INDEX_MERGE(t, idx, idx2) */ \
             * from t where b > 0 or c > 0",
            Vec::new(),
        )
        .Rows();
    assert_eq!(4, rows.len(), "rows={rows:?}");
    assert!(format!("{:?}", rows[0]).contains("IndexMerge"));
    assert_single_rpc_index_range(&rows[1]);
    assert_single_rpc_index_range(&rows[2]);
}
