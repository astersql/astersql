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

// 分区级 LOCK/UNLOCK STATS 集成测试（对应 Go `lock_partition_stats_test.go`）。
//
// 验证：锁定分区后 ANALYZE 被跳过且行数不变；重复加锁/解锁告警；
// 整表已锁时不可再锁/解锁单个分区；分区 DDL（reorganize/drop/truncate/exchange）
// 后锁信息 GC；整表锁定时新增分区自动继承锁；解锁时全局 count/modify_count 回写。

use std::sync::Arc;
use std::time::Duration;

use astersql_domain::Domain;
use astersql_meta_model::TableInfo;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};

use crate::lock_table_stats_test::{
    SELECT_TABLE_LOCK_SQL, assert_columns_initialized, assert_warning, assert_warning_contains,
    exec, query,
};
use crate::main_test::setup_common_tests;

/// 创建带两个 RANGE 分区的表 `t`，并做一次初始 ANALYZE。
///
/// 返回 mock store、Domain（会话/元数据域）、TestKit 与表元数据。
pub(super) fn setup_partitioned_table() -> (Arc<AnalyzeStatsStore>, Arc<Domain>, TestKit, TableInfo)
{
    setup_common_tests();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store.clone());
    exec(&mut testkit, "set @@tidb_analyze_version = 2");
    exec(&mut testkit, "use test");
    exec(&mut testkit, "drop table if exists t");
    // RANGE 分区：p0 < 10，p1 < 20。
    exec(
        &mut testkit,
        "create table t(a int, b varchar(10), index idx_b (b)) partition by range(a) (partition p0 values less than (10), partition p1 values less than (20))",
    );
    exec(&mut testkit, "analyze table test.t");
    let table = domain
        .table_by_name("test", "t")
        .expect("typed InfoSchema test.t metadata")
        .as_ref()
        .clone();
    (store, domain, testkit, table)
}

/// 取出分区表前两个分区定义的物理 ID（p0, p1）。
fn partition_ids(table: &TableInfo) -> (i64, i64) {
    let definitions = &table
        .Partition
        .as_ref()
        .expect("partition info")
        .Definitions;
    (definitions[0].ID, definitions[1].ID)
}

/// 读取指定物理 ID（表或分区）持久化统计中的 realtime_count。
fn stats_count(domain: &Domain, physical_id: i64) -> i64 {
    domain
        .stats_context()
        .persisted_physical_stats(physical_id)
        .expect("physical statistics")
        .realtime_count
}

/// 锁定/解锁单个分区：锁定期间 ANALYZE 跳过且行数不变，解锁后可更新。
#[test]
fn test_lock_and_unlock_partition_stats() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, _) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_eq!(stats_count(&domain, p0), 0);
    exec(&mut testkit, "lock stats t partition p0");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
    assert_eq!(query(&testkit, "show stats_locked").Rows().len(), 1);

    // 写入数据后 ANALYZE 应因分区锁定而跳过，p0 行数仍为 0。
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "analyze table test.t");
    assert_warning_contains(&testkit, "skip analyze locked table: test.t partition (p0)");
    assert_eq!(stats_count(&domain, p0), 0);

    exec(&mut testkit, "unlock stats t partition p0");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
    exec(&mut testkit, "analyze table test.t partition p0");
    assert_eq!(stats_count(&domain, table.ID), 2);
    assert!(query(&testkit, "show stats_locked").Rows().is_empty());
}

/// 同时锁定多个分区：两分区统计均冻结，解锁后各自与全局行数正确更新。
#[test]
fn test_lock_and_unlock_partitions_stats() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    // 记录锁定前的分区统计快照，用于比对 ANALYZE 是否被跳过。
    let partition0_stats = domain
        .stats_context()
        .persisted_physical_stats(p0)
        .expect("initial p0 statistics");
    let partition1_stats = domain
        .stats_context()
        .persisted_physical_stats(p1)
        .expect("initial p1 statistics");
    exec(&mut testkit, "lock stats t partition p0, p1");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "insert into t(a, b) values(11,'a')");
    exec(&mut testkit, "insert into t(a, b) values(12,'b')");
    exec(&mut testkit, "analyze table test.t partition p0, p1");
    assert_eq!(stats_count(&domain, p0), 0);
    assert_eq!(stats_count(&domain, p1), 0);
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(p0)
            .expect("locked p0 statistics"),
        partition0_stats
    );
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(p1)
            .expect("locked p1 statistics"),
        partition1_stats
    );
    assert_eq!(query(&testkit, "show stats_locked").Rows().len(), 2);

    exec(&mut testkit, "unlock stats t partition p0, p1");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
    exec(&mut testkit, "analyze table test.t partition p0, p1");
    assert_eq!(stats_count(&domain, p0), 2);
    assert_eq!(stats_count(&domain, p1), 2);
    assert_eq!(stats_count(&domain, table.ID), 4);
    assert!(query(&testkit, "show stats_locked").Rows().is_empty());
}

/// 重复锁定已锁分区 / 解锁未锁分区应产生 skip 警告。
#[test]
fn test_lock_and_unlock_partition_stats_repeatedly() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t partition p0");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
    exec(&mut testkit, "lock stats t partition p0");
    assert_warning(
        &testkit,
        "skip locking locked partition of table test.t: p0",
    );
    exec(&mut testkit, "unlock stats t partition p0");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
    exec(&mut testkit, "unlock stats t partition p0");
    assert_warning(
        &testkit,
        "skip unlocking unlocked partition of table test.t: p0",
    );
}

/// 整表已锁定时，再锁单个分区应被跳过。
#[test]
fn test_skip_lock_partition() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    // 整表锁定会写入全局 + 各分区共 3 条锁记录。
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["3"]]);
    exec(&mut testkit, "lock stats t partition p0");
    assert_warning(&testkit, "skip locking partitions of locked table: test.t");
}

/// 整表锁定时解锁单个分区应失败（告警且锁记录数不变）。
#[test]
fn test_unlock_one_partition_of_locked_table_would_fail() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["3"]]);
    exec(&mut testkit, "unlock stats t partition p0");
    assert_warning(
        &testkit,
        "skip unlocking partitions of locked table: test.t",
    );
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["3"]]);
}

/// 仅分区锁定时对整表 UNLOCK 会告警，分区锁仍保留。
#[test]
fn test_unlock_the_unlocked_table_would_generate_warning() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t partition p0");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
    exec(&mut testkit, "unlock stats t");
    assert_warning(&testkit, "skip unlocking unlocked table: test.t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
}

/// 一次锁定大量已锁分区时，警告消息列出全部被跳过的分区名。
#[test]
fn test_skip_lock_a_lot_of_partitions() {
    setup_common_tests();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    exec(&mut testkit, "set @@tidb_analyze_version = 2");
    exec(&mut testkit, "use test");
    exec(&mut testkit, "drop table if exists t");
    exec(
        &mut testkit,
        "create table t(a int, b varchar(10), index idx_b (b)) partition by range(a) (partition p0 values less than (10), partition p1 values less than (20), partition a values less than (30), partition b values less than (40), partition g values less than (90), partition h values less than (100))",
    );
    exec(&mut testkit, "lock stats t partition p0, p1, a, b, g, h");
    // 再次锁定同一批分区 → 全部 skip，警告按名列出。
    exec(&mut testkit, "lock stats t partition p0, p1, a, b, g, h");
    assert_warning(
        &testkit,
        "skip locking locked partitions of table test.t: a, b, g, h, p0, p1",
    );
}

/// REORGANIZE PARTITION 后 GC 应清理对应分区锁记录。
#[test]
fn test_reorganize_partition_should_clean_up_lock_info() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t partition p0, p1");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
    exec(
        &mut testkit,
        "alter table t reorganize partition p0, p1 into (partition p0 values less than (20))",
    );
    // GC（垃圾回收）过期统计与锁元数据。
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC reorganized partition statistics");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
}

/// DROP PARTITION 后 GC 只清理被删分区的锁，其它分区锁保留。
#[test]
fn test_drop_partition_should_clean_up_lock_info() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t partition p0, p1");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
    exec(&mut testkit, "alter table t drop partition p0");
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC dropped partition statistics");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
}

/// TRUNCATE PARTITION 后 GC 清理该分区锁记录。
#[test]
fn test_truncate_partition_should_clean_up_lock_info() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t partition p0, p1");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
    exec(&mut testkit, "alter table t truncate partition p0");
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC truncated partition statistics");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
}

/// EXCHANGE PARTITION 交换物理 ID，但锁记录条数应保持不变。
#[test]
fn test_exchange_partition_should_change_nothing() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t partition p0, p1");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
    exec(
        &mut testkit,
        "create table t1(a int, b varchar(10), index idx_b (b))",
    );
    let exchange_table_id = domain
        .table_by_name("test", "t1")
        .expect("typed InfoSchema test.t1")
        .ID;
    exec(
        &mut testkit,
        "alter table t exchange partition p0 with table t1",
    );
    // 交换后：原 t1 的 ID 成为 p0，原 p0 成为普通表 t1。
    let exchanged_partition_id = domain
        .table_by_name("test", "t")
        .expect("typed InfoSchema exchanged test.t")
        .GetPartitionInfo()
        .expect("partition info")
        .Definitions[0]
        .ID;
    let exchanged_table_id = domain
        .table_by_name("test", "t1")
        .expect("typed InfoSchema exchanged test.t1")
        .ID;
    assert_eq!(exchanged_partition_id, exchange_table_id);
    assert_eq!(exchanged_table_id, p0);
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC exchanged partition statistics");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
}

/// 整表锁定后 ADD PARTITION：新分区应自动被锁，ANALYZE 被跳过。
#[test]
fn test_new_partition_should_be_locked_if_whole_table_locked() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    let (p0, p1) = partition_ids(&table);
    assert_columns_initialized(&domain, p0);
    assert_columns_initialized(&domain, p1);
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["3"]]);
    exec(
        &mut testkit,
        "alter table t add partition (partition p2 values less than (30))",
    );
    exec(&mut testkit, "insert into t(a, b) values(21,'a')");
    exec(&mut testkit, "insert into t(a, b) values(22,'b')");
    // flush stats_delta：把内存中的增量修改刷到 mysql.stats_* 元表。
    exec(&mut testkit, "flush stats_delta *.*");
    let rows = query(
        &testkit,
        "select count, modify_count, table_id from mysql.stats_table_locked order by table_id",
    )
    .Rows();
    // 4 条锁记录：全局 + p0 + p1 + 新建 p2；p2 上有 2 行增量。
    assert_eq!(rows.len(), 4);
    assert_eq!(&rows[0][..2], ["0", "0"]);
    assert_eq!(&rows[1][..2], ["0", "0"]);
    assert_eq!(&rows[2][..2], ["0", "0"]);
    assert_eq!(&rows[3][..2], ["2", "2"]);

    exec(&mut testkit, "analyze table t partition p2");
    assert_warning_contains(&testkit, "skip analyze locked table: test.t partition (p2)");
    exec(&mut testkit, "unlock stats t");
    query(
        &testkit,
        &format!(
            "select count, modify_count from mysql.stats_meta where table_id = {}",
            table.ID
        ),
    )
    .Check(vec![vec!["2", "2"]]);
}

/// 解锁分区后，锁定期间累积的 count/modify_count 应正确回写到全局 stats_meta。
#[test]
fn test_unlock_some_partitions_updates_global_count_correctly() {
    let (_store, domain, mut testkit, table) = setup_partitioned_table();
    exec(&mut testkit, "lock stats t partition p0, p1");
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "analyze table test.t partition p0, p1");
    assert_eq!(stats_count(&domain, table.ID), 0);
    exec(&mut testkit, "flush stats_delta *.*");
    let rows = query(
        &testkit,
        "select count, modify_count, table_id from mysql.stats_table_locked order by table_id",
    )
    .Rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(&rows[0][..2], ["2", "2"]);
    assert_eq!(&rows[1][..2], ["0", "0"]);

    exec(&mut testkit, "unlock stats t partition p0, p1");
    query(
        &testkit,
        &format!(
            "select count, modify_count, table_id from mysql.stats_meta where table_id = {}",
            table.ID
        ),
    )
    .Check(vec![vec![
        "2".to_owned(),
        "2".to_owned(),
        table.ID.to_string(),
    ]]);
}
