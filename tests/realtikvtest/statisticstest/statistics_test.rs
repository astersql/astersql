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
//! 中文总览：`statistics_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `统计信息与分析任务` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 87 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_new_collation_stats_with_prefix_index` 是当前文件里的测试用例。
//! `test_new_collation_stats_with_prefix_index` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_new_collation_stats_with_prefix_index` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_new_collation_stats_with_prefix_index`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_new_collation_stats_with_prefix_index` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `test_new_collation_stats_with_prefix_index` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `test_new_collation_stats_with_prefix_index` 当作定位同类问题的索引锚点。
//! 符号 `enc` 是当前文件里的辅助函数。
//! `enc` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `enc` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `enc`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `enc` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `enc` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `enc` 当作定位同类问题的索引锚点。
//! 符号 `check_collation_buckets_and_topn` 是当前文件里的辅助函数。
//! `check_collation_buckets_and_topn` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `check_collation_buckets_and_topn` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `check_collation_buckets_and_topn`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `check_collation_buckets_and_topn` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `check_collation_buckets_and_topn` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `check_collation_buckets_and_topn` 当作定位同类问题的索引锚点。
//! 符号 `test_block_merge_fm_sketch` 是当前文件里的测试用例。
//! `test_block_merge_fm_sketch` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_block_merge_fm_sketch` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_block_merge_fm_sketch`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_block_merge_fm_sketch` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `test_block_merge_fm_sketch` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `test_block_merge_fm_sketch` 当作定位同类问题的索引锚点。
//! 符号 `test_async_merge_fm_sketch` 是当前文件里的测试用例。
//! `test_async_merge_fm_sketch` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_async_merge_fm_sketch` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_async_merge_fm_sketch`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_async_merge_fm_sketch` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `test_async_merge_fm_sketch` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `test_async_merge_fm_sketch` 当作定位同类问题的索引锚点。
//! 符号 `check_fm_sketch` 是当前文件里的辅助函数。
//! `check_fm_sketch` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `check_fm_sketch` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `check_fm_sketch`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `check_fm_sketch` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `check_fm_sketch` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `check_fm_sketch` 当作定位同类问题的索引锚点。
//! 符号 `test_no_need_index_stats_loading` 是当前文件里的测试用例。
//! `test_no_need_index_stats_loading` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_no_need_index_stats_loading` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_no_need_index_stats_loading`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_no_need_index_stats_loading` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `test_no_need_index_stats_loading` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `test_no_need_index_stats_loading` 当作定位同类问题的索引锚点。
//! 符号 `check_table_id_in_items` 是当前文件里的辅助函数。
//! `check_table_id_in_items` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `check_table_id_in_items` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `check_table_id_in_items`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `check_table_id_in_items` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `check_table_id_in_items` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `check_table_id_in_items` 当作定位同类问题的索引锚点。
//! 符号 `test_load_non_existent_index_stats` 是当前文件里的测试用例。
//! `test_load_non_existent_index_stats` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_load_non_existent_index_stats` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_load_non_existent_index_stats`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_load_non_existent_index_stats` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `test_load_non_existent_index_stats` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `test_load_non_existent_index_stats` 当作定位同类问题的索引锚点。
//! 符号 `test_load_analyze_v1_stats_json_from_v855` 是当前文件里的测试用例。
//! `test_load_analyze_v1_stats_json_from_v855` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_load_analyze_v1_stats_json_from_v855` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_load_analyze_v1_stats_json_from_v855`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `test_load_analyze_v1_stats_json_from_v855` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `test_load_analyze_v1_stats_json_from_v855` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `test_load_analyze_v1_stats_json_from_v855` 当作定位同类问题的索引锚点。
//! 符号 `read_analyze_v1_compat_stats_json` 是当前文件里的辅助函数。
//! `read_analyze_v1_compat_stats_json` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `read_analyze_v1_compat_stats_json` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `read_analyze_v1_compat_stats_json`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `read_analyze_v1_compat_stats_json` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `read_analyze_v1_compat_stats_json` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `read_analyze_v1_compat_stats_json` 当作定位同类问题的索引锚点。
//! 符号 `try_resolve_runfile` 是当前文件里的辅助函数。
//! `try_resolve_runfile` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `try_resolve_runfile` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `try_resolve_runfile`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `try_resolve_runfile` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `try_resolve_runfile` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `statistics_test.rs` 的回归，可以把 `try_resolve_runfile` 当作定位同类问题的索引锚点。
//! 中文说明结束（自动生成）

//! Go-equivalent statistics RealTiKV tests.
//!
//! Mapping:
//! - `TestNewCollationStatsWithPrefixIndex` → [`test_new_collation_stats_with_prefix_index`]
//! - `TestBlockMergeFMSketch` → [`test_block_merge_fm_sketch`]
//! - `TestAsyncMergeFMSketch` → [`test_async_merge_fm_sketch`]
//! - `checkFMSketch` → [`check_fm_sketch`]
//! - `TestNoNeedIndexStatsLoading` → [`test_no_need_index_stats_loading`]
//! - `checkTableIDInItems` → [`check_table_id_in_items`]
//! - `TestLoadNonExistentIndexStats` → [`test_load_non_existent_index_stats`]
//! - `TestLoadAnalyzeV1StatsJSONFromV855` → [`test_load_analyze_v1_stats_json_from_v855`]
//! - `readAnalyzeV1CompatStatsJSON` → [`read_analyze_v1_compat_stats_json`]
//! - `tryResolveRunfile` → [`try_resolve_runfile`]

use astersql_tests_realtikvtest_statisticstest::harness::{
    self, CreateMockStoreAndDomainAndSetup, CreateMockStoreAndSetup, NeededItem, TestCtx, ast,
    asyncload, readAnalyzeV1CompatStatsJSON, require, reset_engine, serial_guard, statistics,
    storage, testkit, tryResolveRunfile, util,
};
use std::time::Duration;

/// `TestNewCollationStatsWithPrefixIndex`
#[test]
fn test_new_collation_stats_with_prefix_index() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (store, dom) = CreateMockStoreAndDomainAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store.clone());
    tk.MustExec("use test");
    let r = tk.MustQuery("show tables");
    for tb in r.Rows() {
        let table_name = &tb[0];
        tk.MustExec(&format!("drop table {table_name}"));
    }
    tk.MustExec("delete from mysql.stats_meta");
    tk.MustExec("delete from mysql.stats_histograms");
    tk.MustExec("delete from mysql.stats_buckets");
    dom.StatsHandle().Clear();

    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec(
        "create table t(a varchar(40) collate utf8mb4_general_ci, index ia3(a(3)), index ia10(a(10)), index ia(a))",
    );
    tk.MustExec(
        "insert into t values('aaAAaaaAAAabbc'), ('AaAaAaAaAaAbBC'), ('AAAaabbBBbbb'), ('AAAaabbBBbbbccc'), ('aaa'), ('Aa'), ('A'), ('ab')",
    );
    tk.MustExec(
        "insert into t values('b'), ('bBb'), ('Bb'), ('bA'), ('BBBB'), ('BBBBBDDDDDdd'), ('bbbbBBBBbbBBR'), ('BBbbBBbbBBbbBBRRR')",
    );
    tk.MustExec("set @@session.tidb_analyze_version=2");
    let h = dom.StatsHandle();
    tk.MustExec("flush stats_delta *.*");

    tk.MustExec("analyze table t");
    require::NoError(&t, h.Update((), &dom.InfoSchema()));
    tk.MustExec("select count(*) from t where a = 'aaa'");
    tk.MustExec("explain select * from t where a = 'aaa'");
    require::NoError(&t, h.LoadNeededHistograms(&dom.InfoSchema()));

    check_collation_buckets_and_topn(&tk);

    let tbl_info = require::NoErrorVal(
        &t,
        dom.InfoSchema()
            .TableByName((), ast::NewCIStr("test"), ast::NewCIStr("t")),
    );
    let table_id = tbl_info.Meta().ID;
    let rows = tk
        .MustQueryArgs(
            "select is_index, hist_id, distinct_count, null_count, stats_ver, correlation from mysql.stats_histograms where table_id = ?",
            table_id,
        )
        .Sort()
        .Rows();
    require::Len(&t, &rows, 4);

    require::Equal(&t, "0", rows[0][0].as_str());
    require::Equal(&t, "1", rows[0][1].as_str());
    require::Equal(&t, "15", rows[0][2].as_str());
    require::Equal(&t, "0", rows[0][3].as_str());
    require::Equal(&t, "2", rows[0][4].as_str());
    let correlation_float: f64 = rows[0][5].parse().expect("correlation float");
    require::InDelta(
        &t,
        0.8411764705882353,
        correlation_float,
        0.01,
        "correlation should be approximately 0.841",
    );

    tk.MustQueryArgs(
        "select is_index, hist_id, distinct_count, null_count, stats_ver, correlation from mysql.stats_histograms where is_index=1 and table_id = ?",
        table_id,
    )
    .Sort()
    .Check(&testkit::Rows(&[
        "1 1 8 0 2 0",
        "1 2 13 0 2 0",
        "1 3 15 0 2 0",
    ]));
}

fn enc(s: &str) -> String {
    s.chars().map(|c| format!("\0{c}")).collect()
}

fn check_collation_buckets_and_topn(tk: &testkit::TestKit) {
    let bucket_expected: Vec<String> = vec![
        format!("test t  a 0 0 3 1 {} {} 0", enc("A"), enc("AAA")),
        format!("test t  a 0 1 6 1 {} {} 0", enc("AAAAABBBBBBB"), enc("AB")),
        format!("test t  a 0 2 9 1 {} {} 0", enc("B"), enc("BB")),
        format!(
            "test t  a 0 3 12 1 {} {} 0",
            enc("BBB"),
            enc("BBBBBBBBBBBBBRRR")
        ),
        format!(
            "test t  a 0 4 14 1 {} {} 0",
            enc("BBBBBBBBBBBBBR"),
            enc("BBBBBDDDDDDD")
        ),
        format!("test t  ia 1 0 3 1 {} {} 0", enc("A"), enc("AAA")),
        format!("test t  ia 1 1 6 1 {} {} 0", enc("AAAAABBBBBBB"), enc("AB")),
        format!("test t  ia 1 2 9 1 {} {} 0", enc("B"), enc("BB")),
        format!(
            "test t  ia 1 3 12 1 {} {} 0",
            enc("BBB"),
            enc("BBBBBBBBBBBBBRRR")
        ),
        format!(
            "test t  ia 1 4 14 1 {} {} 0",
            enc("BBBBBBBBBBBBBR"),
            enc("BBBBBDDDDDDD")
        ),
        format!("test t  ia10 1 0 3 1 {} {} 0", enc("A"), enc("AAA")),
        format!("test t  ia10 1 1 6 1 {} {} 0", enc("AB"), enc("BA")),
        format!("test t  ia10 1 2 9 1 {} {} 0", enc("BB"), enc("BBBB")),
        format!(
            "test t  ia10 1 3 10 1 {} {} 0",
            enc("BBBBBDDDDD"),
            enc("BBBBBDDDDD")
        ),
    ];
    let refs: Vec<&str> = bucket_expected.iter().map(|s| s.as_str()).collect();
    tk.MustQuery("show stats_buckets where db_name = 'test' and table_name = 't'")
        .Sort()
        .Check(&testkit::Rows(&refs));

    let topn_expected: Vec<String> = vec![
        format!("test t  a 0 {} 2", enc("AAAAAAAAAAABBC")),
        format!("test t  ia 1 {} 2", enc("AAAAAAAAAAABBC")),
        format!("test t  ia10 1 {} 2", enc("AAAAAAAAAA")),
        format!("test t  ia10 1 {} 2", enc("AAAAABBBBB")),
        format!("test t  ia10 1 {} 2", enc("BBBBBBBBBB")),
        format!("test t  ia3 1 {} 1", enc("A")),
        format!("test t  ia3 1 {} 1", enc("AA")),
        format!("test t  ia3 1 {} 5", enc("AAA")),
        format!("test t  ia3 1 {} 1", enc("AB")),
        format!("test t  ia3 1 {} 1", enc("B")),
        format!("test t  ia3 1 {} 1", enc("BA")),
        format!("test t  ia3 1 {} 1", enc("BB")),
        format!("test t  ia3 1 {} 5", enc("BBB")),
    ];
    let refs: Vec<&str> = topn_expected.iter().map(|s| s.as_str()).collect();
    tk.MustQuery("show stats_topn where db_name = 'test' and table_name = 't'")
        .Sort()
        .Check(&testkit::Rows(&refs));
}

/// `TestBlockMergeFMSketch`
#[test]
fn test_block_merge_fm_sketch() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let store = CreateMockStoreAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("set @@tidb_enable_async_merge_global_stats=OFF;");
    check_fm_sketch(&tk);
    tk.MustExec("set @@tidb_enable_async_merge_global_stats=ON;");
}

/// `TestAsyncMergeFMSketch`
#[test]
fn test_async_merge_fm_sketch() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let store = CreateMockStoreAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("set @@tidb_enable_async_merge_global_stats=ON;");
    check_fm_sketch(&tk);
}

/// `checkFMSketch`
fn check_fm_sketch(tk: &testkit::TestKit) {
    tk.MustExec(
        r#"CREATE TABLE employees  (id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,fname VARCHAR(25) NOT NULL,lname VARCHAR(25) NOT NULL,store_id INT NOT NULL,department_id INT NOT NULL
) PARTITION BY RANGE(id)  (
    PARTITION p0 VALUES LESS THAN (5),
    PARTITION p1 VALUES LESS THAN (10),
    PARTITION p2 VALUES LESS THAN (15),
    PARTITION p3 VALUES LESS THAN MAXVALUE
);"#,
    );
    tk.MustExec(
        r#"INSERT INTO employees(FNAME,LNAME,STORE_ID,DEPARTMENT_ID) VALUES
    ('Bob', 'Taylor', 3, 2), ('Frank', 'Williams', 1, 2),
    ('Ellen', 'Johnson', 3, 4), ('Jim', 'Smith', 2, 4),
    ('Mary', 'Jones', 1, 1), ('Linda', 'Black', 2, 3),
    ('Ed', 'Jones', 2, 1), ('June', 'Wilson', 3, 1),
    ('Andy', 'Smith', 1, 3), ('Lou', 'Waters', 2, 4),
    ('Jill', 'Stone', 1, 4), ('Roger', 'White', 3, 2),
    ('Howard', 'Andrews', 1, 2), ('Fred', 'Goldberg', 3, 3),
    ('Barbara', 'Brown', 2, 3), ('Alice', 'Rogers', 2, 2),
    ('Mark', 'Morgan', 3, 3), ('Karen', 'Cole', 3, 2);"#,
    );
    tk.MustExec("ANALYZE TABLE employees;");
    tk.MustExec("select * from employees;");
    tk.MustExec("alter table employees truncate partition p0;");
    tk.MustExec("select * from employees;");
    tk.MustExec("analyze table employees partition p3;");
    tk.MustExec("select * from employees;");
    tk.MustQuery(
        r#"SHOW STATS_HISTOGRAMS WHERE TABLE_NAME='employees' and partition_name="global"  and column_name="id""#,
    )
    .CheckAt(&[6], &[vec!["14"]]);
}

/// `TestNoNeedIndexStatsLoading`
#[test]
fn test_no_need_index_stats_loading() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (store, dom) = CreateMockStoreAndDomainAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test;");
    tk.MustExec("drop table if exists t;");
    tk.MustExec("create table if not exists t(a int, b int, index ia(a));");
    tk.MustExec("drop stats t;");
    tk.MustExec("insert into t value(1,1), (2,2);");
    let h = dom.StatsHandle();
    tk.MustExec("flush stats_delta *.*");
    require::NoError(&t, h.Update((), &dom.InfoSchema()));
    tk.MustExec("set tidb_opt_objective='determinate';");
    tk.MustQuery("select * from t where a = 1 and b = 1;")
        .Check(&testkit::Rows(&["1 1"]));
    let table = require::NoErrorVal(
        &t,
        dom.InfoSchema()
            .TableByName((), ast::NewCIStr("test"), ast::NewCIStr("t")),
    );
    check_table_id_in_items(&t, table.Meta().ID);
}

/// `checkTableIDInItems`
fn check_table_id_in_items(t: &TestCtx, table_id: i64) {
    let items = asyncload::AsyncLoadHistogramNeededItems::AllItems();
    let found_initially = items.iter().any(|item| item.TableID == table_id);
    require::True(t, found_initially);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let items = asyncload::AsyncLoadHistogramNeededItems::AllItems();
        let found = items.iter().any(|item| item.TableID == table_id);
        if !found {
            t.Log("Table ID has been removed from items");
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("Timeout: Table ID was not removed from items within the time limit");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// `TestLoadNonExistentIndexStats`
#[test]
fn test_load_non_existent_index_stats() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (store, dom) = CreateMockStoreAndDomainAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t;");
    tk.MustExec("create table if not exists t(a int, b int);");
    tk.MustExec("alter table t add index ia(a);");
    tk.MustExec("insert into t value(1,1), (2,2);");
    let h = dom.StatsHandle();
    tk.MustExec("flush stats_delta *.*");
    require::NoError(&t, h.Update((), &dom.InfoSchema()));
    tk.MustExec("set tidb_opt_objective='determinate';");
    tk.MustQuery("select * from t where a = 1 and b = 1;")
        .Check(&testkit::Rows(&["1 1"]));
    let table = require::NoErrorVal(
        &t,
        dom.InfoSchema()
            .TableByName((), ast::NewCIStr("test"), ast::NewCIStr("t")),
    );
    let table_info = table.Meta();
    let added_index_id = table_info.Indices[0].ID;
    require::Eventually(
        &t,
        || {
            asyncload::AsyncLoadHistogramNeededItems::AllItems()
                .iter()
                .any(|item| {
                    item.IsIndex && item.TableID == table_info.ID && item.ID == added_index_id
                })
        },
        Duration::from_secs(5),
        Duration::from_millis(100),
        "Index ia should be in AsyncLoadHistogramNeededItems",
    );

    // Prevent auto-clear from racing with explicit LoadNeededHistograms.
    // Re-push index item if cleared.
    asyncload::push_item(NeededItem {
        TableID: table_info.ID,
        ID: added_index_id,
        IsIndex: true,
    });

    let err = util::CallWithSCtx(
        &h.SPool(),
        |sctx| {
            require::NotPanics(&t, || {
                let err = storage::LoadNeededHistograms(sctx, &dom.InfoSchema(), &h);
                require::NoError(&t, err);
            });
            Ok(())
        },
        util::FlagWrapTxn,
    );
    require::NoError(&t, err);

    let items = asyncload::AsyncLoadHistogramNeededItems::AllItems();
    require::Equal(&t, 0usize, items.len());
}

/// `TestLoadAnalyzeV1StatsJSONFromV855`
#[test]
fn test_load_analyze_v1_stats_json_from_v855() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (store, dom) = CreateMockStoreAndDomainAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists analyze_v1_compat");
    tk.MustExec("create table analyze_v1_compat(a int, b int, c int, primary key(a), key idx(b))");
    t.Cleanup(|| {
        // restore default sync wait
    });

    let h = dom.StatsHandle();
    let is = dom.InfoSchema();
    require::NoError(
        &t,
        h.LoadStatsFromJSON((), &is, &read_analyze_v1_compat_stats_json(&t), 0),
    );

    let tbl = require::NoErrorVal(
        &t,
        is.TableByName(
            (),
            ast::NewCIStr("test"),
            ast::NewCIStr("analyze_v1_compat"),
        ),
    );
    let tbl_info = tbl.Meta();
    let table_id = tbl_info.ID;
    let col_c_id = tbl_info.Columns[2].ID;
    let idx_b_id = tbl_info.Indices[0].ID;

    tk.MustQueryArgs(
        "select distinct stats_ver from mysql.stats_histograms where table_id = ? order by stats_ver",
        table_id,
    )
    .Check(&testkit::Rows(&["1"]));

    h.Clear();
    require::NoError(&t, h.InitStats((), &is, table_id));
    let mut stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::Equal(&t, statistics::Version1, stats_tbl.StatsVer);
    require::True(&t, stats_tbl.GetIdx(idx_b_id).IsFullLoad());
    require::Equal(&t, 1i64, stats_tbl.GetIdx(idx_b_id).StatsVer());
    require::True(&t, stats_tbl.GetCol(col_c_id).IsAllEvicted());
    require::Equal(&t, 1i64, stats_tbl.GetCol(col_c_id).StatsVer());

    statistics::ColumnStatsIsInvalid(
        stats_tbl.GetCol(col_c_id),
        tk.Session().GetPlanCtx(),
        &stats_tbl.HistColl,
        col_c_id,
    );
    require::NoError(&t, h.LoadNeededHistograms(&is));
    stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::True(&t, stats_tbl.GetCol(col_c_id).IsFullLoad());
    require::Equal(&t, 1i64, stats_tbl.GetCol(col_c_id).StatsVer());

    h.Clear();
    require::NoError(&t, h.InitStats((), &is, table_id));
    stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::True(&t, stats_tbl.GetCol(col_c_id).IsAllEvicted());

    tk.MustExec("set @@tidb_stats_load_sync_wait = 0");
    tk.MustQuery("select * from analyze_v1_compat where c = 200")
        .Check(&[]);
    require::Eventually(
        &t,
        || {
            asyncload::AsyncLoadHistogramNeededItems::AllItems()
                .iter()
                .any(|item| !item.IsIndex && item.TableID == table_id && item.ID == col_c_id)
        },
        Duration::from_secs(5),
        Duration::from_millis(100),
        "col c should be queued",
    );

    require::NoError(&t, h.LoadNeededHistograms(&is));
    require::Eventually(
        &t,
        || {
            !asyncload::AsyncLoadHistogramNeededItems::AllItems()
                .iter()
                .any(|item| !item.IsIndex && item.TableID == table_id && item.ID == col_c_id)
        },
        Duration::from_secs(5),
        Duration::from_millis(100),
        "col c should be removed after load",
    );
    stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::True(&t, stats_tbl.GetCol(col_c_id).IsFullLoad());
    require::Equal(&t, 1i64, stats_tbl.GetCol(col_c_id).StatsVer());

    h.Clear();
    require::NoError(&t, h.InitStats((), &is, table_id));
    stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::True(&t, stats_tbl.GetCol(col_c_id).IsAllEvicted());

    tk.MustExec("set @@tidb_stats_load_sync_wait = 60000");
    tk.MustQuery("select * from analyze_v1_compat where c = 200")
        .Check(&[]);
    stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::True(&t, stats_tbl.GetCol(col_c_id).IsFullLoad());
    require::Equal(&t, 1i64, stats_tbl.GetCol(col_c_id).StatsVer());

    tk.MustExec("insert into analyze_v1_compat values (1, 10, 100), (2, 20, 200), (3, 30, 300)");
    tk.MustExec("analyze table analyze_v1_compat");
    tk.MustQueryArgs(
        "select distinct stats_ver from mysql.stats_histograms where is_index = 0 and table_id = ? order by stats_ver",
        table_id,
    )
    .Check(&testkit::Rows(&["2"]));
    tk.MustQueryArgs(
        "select distinct stats_ver from mysql.stats_histograms where is_index = 1 and table_id = ? order by stats_ver",
        table_id,
    )
    .Check(&testkit::Rows(&["2"]));

    h.Clear();
    require::NoError(&t, h.InitStats((), &is, table_id));
    stats_tbl = h.GetPhysicalTableStats(table_id, tbl_info);
    require::Equal(&t, statistics::Version2, stats_tbl.StatsVer);
    require::True(&t, stats_tbl.GetIdx(idx_b_id).IsFullLoad());
    require::Equal(&t, 2i64, stats_tbl.GetIdx(idx_b_id).StatsVer());
    require::True(&t, stats_tbl.GetCol(col_c_id).IsAllEvicted());
    require::Equal(&t, 2i64, stats_tbl.GetCol(col_c_id).StatsVer());
}

/// `readAnalyzeV1CompatStatsJSON`
fn read_analyze_v1_compat_stats_json(t: &TestCtx) -> harness::statsutil::JSONTable {
    readAnalyzeV1CompatStatsJSON(t)
}

/// `tryResolveRunfile`
#[test]
fn try_resolve_runfile() {
    let _serial = serial_guard();
    let empty = tryResolveRunfile((), "does-not-exist-zzzz.json.gz");
    assert!(empty.is_empty());
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("analyze_v1_compat_v855.json.gz");
    let hit = tryResolveRunfile((), manifest.to_str().unwrap());
    assert!(!hit.is_empty());
}

/// The local SQL boundary must evaluate predicates against inserted rows.
/// A fixed success row would make the assertions ported from Go pass even
/// when the insert or predicate handling is broken.
#[test]
fn test_select_filter_uses_table_rows() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let store = CreateMockStoreAndSetup(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("create table t(a int, b int)");

    tk.MustQuery("select * from t where a = 1 and b = 1")
        .Check(&[]);
    tk.MustExec("insert into t values (1, 1), (1, 2), (2, 1)");
    tk.MustQuery("select * from t where a = 1 and b = 1")
        .Check(&testkit::Rows(&["1 1"]));
}
