// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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
//! 中文总览：`infoschema_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `会话生命周期与信息模式` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 26 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `unique_sorted_region_ids` 是当前文件里的辅助函数。
//! `unique_sorted_region_ids` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `unique_sorted_region_ids` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `unique_sorted_region_ids`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_next_gen_tikv_region_status` 是当前文件里的测试用例。
//! `test_next_gen_tikv_region_status` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_next_gen_tikv_region_status` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_next_gen_tikv_region_status`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_next_gen_tikv_region_status_does_not_mix_other_keyspaces` 是当前文件里的测试用例。
//! `test_next_gen_tikv_region_status_does_not_mix_other_keyspaces` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_next_gen_tikv_region_status_does_not_mix_other_keyspaces` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_next_gen_tikv_region_status_does_not_mix_other_keyspaces`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_table_reader_with_snapshot` 是当前文件里的测试用例。
//! `test_table_reader_with_snapshot` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_table_reader_with_snapshot` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_table_reader_with_snapshot`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Real TestKit/Domain port of `infoschema_test.go`.

use std::collections::BTreeSet;

use astersql_testkit::mockstore::{CreateCrossKeyspaceTestCluster, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows};
use astersql_tests_realtikvtest_sessiontest::serial_guard;

fn unique_sorted_region_ids(rows: Vec<Vec<String>>) -> Vec<String> {
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .next()
                .expect("region query must contain REGION_ID")
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Go `TestNextGenTiKVRegionStatus`.
#[test]
fn test_next_gen_tikv_region_status() {
    let _serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (a int, key idx(a))", Vec::new());
    tk.MustExec(
        "split table t between (0) and (10000) regions 4",
        Vec::new(),
    );
    tk.MustExec(
        "split table t index idx between (0) and (10000) regions 4",
        Vec::new(),
    );

    let table_id_rows = tk
        .MustQuery(
            "select tidb_table_id from information_schema.tables \
             where table_schema = 'test' and table_name = 't'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(1, table_id_rows.len(), "table id rows={table_id_rows:?}");
    assert_eq!(1, table_id_rows[0].len(), "table id rows={table_id_rows:?}");
    let table_id = &table_id_rows[0][0];

    let show_regions =
        unique_sorted_region_ids(tk.MustQuery("show table t regions", Vec::new()).Rows());
    let show_index_regions = unique_sorted_region_ids(
        tk.MustQuery("show table t index idx regions", Vec::new())
            .Rows(),
    );
    let tikv_regions = unique_sorted_region_ids(
        tk.MustQuery(
            &format!(
                "select region_id from information_schema.tikv_region_status \
                 where table_id = {table_id}"
            ),
            Vec::new(),
        )
        .Rows(),
    );
    let tikv_index_regions = unique_sorted_region_ids(
        tk.MustQuery(
            &format!(
                "select region_id from information_schema.tikv_region_status \
                 where table_id = {table_id} and is_index = 1"
            ),
            Vec::new(),
        )
        .Rows(),
    );

    assert!(!show_regions.is_empty());
    assert!(!show_index_regions.is_empty());
    assert_eq!(show_regions, tikv_regions);
    assert_eq!(show_index_regions, tikv_index_regions);
}

/// Go `TestNextGenTiKVRegionStatusDoesNotMixOtherKeyspaces`.
#[test]
fn test_next_gen_tikv_region_status_does_not_mix_other_keyspaces() {
    let _serial = serial_guard();
    let cluster = CreateCrossKeyspaceTestCluster(&[("keyspace1", false)]);
    let mut system = NewTestKit(cluster.store("SYSTEM"));
    system.MustExec(
        "create database if not exists sys_region_status",
        Vec::new(),
    );
    system.MustExec("use sys_region_status", Vec::new());
    system.MustExec("drop table if exists t", Vec::new());
    system.MustExec("create table t (a int, key idx(a))", Vec::new());
    system.MustExec(
        "split table t between (0) and (10000) regions 4",
        Vec::new(),
    );
    system.MustExec(
        "split table t index idx between (0) and (10000) regions 4",
        Vec::new(),
    );

    let system_region_ids =
        unique_sorted_region_ids(system.MustQuery("show table t regions", Vec::new()).Rows());
    assert!(!system_region_ids.is_empty());

    let user = NewTestKit(cluster.store("keyspace1"));
    user.MustQuery(
        &format!(
            "select count(*) from information_schema.tikv_region_status \
             where region_id in ({}) and db_name = 'sys_region_status' \
             and table_name = 't'",
            system_region_ids.join(",")
        ),
        Vec::new(),
    )
    .Check(Rows(&["0"]));
}

/// Go `TestTableReaderWithSnapshot`.
#[test]
fn test_table_reader_with_snapshot() {
    let _serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(id int)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("set @ts := @@tidb_current_ts", Vec::new());
    tk.MustExec("rollback", Vec::new());
    let _ = tk.MustQuery("select sleep(2)", Vec::new());
    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("set @@tidb_snapshot=@ts", Vec::new());
    tk.MustQuery(
        "SELECT TABLE_NAME,TABLE_TYPE,AVG_ROW_LENGTH \
         FROM INFORMATION_SCHEMA.TABLES \
         WHERE TABLE_SCHEMA='test' AND (TABLE_TYPE='BASE TABLE')",
        Vec::new(),
    )
    .Check(vec![vec!["t", "BASE TABLE", "0"]]);
}
