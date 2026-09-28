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

// 索引修改（add/drop index）相关的 DDL 测试模块。
//
// 本文件迁移自 Go(TiDB) 的 `index_modify_test.go`，原始测试覆盖以下场景：
// - add/drop index、primary key（主键）在普通表、分区表（partition table）、
//   clustered index（聚簇索引，行数据按主键组织）表上的执行；
// - global index（全局索引，跨分区统一编码的索引）的编码与回查；
// - add index 期间与并发 DML（delete/insert/update）交错时的正确性，
//   以及回滚（rollback）路径（重复键、NULL 值等导致 DDL 失败回滚）；
// - add index 后自动 analyze（收集统计信息）与 stats 版本对齐；
// - vector index（向量索引）与 columnar index（列存倒排索引，依赖 TiFlash 副本）
//   的创建、错误路径与取消/回滚。
//
// 下方巨大的块注释保留了 Go 测试的机械翻译版本作为语义参考（当前尚不可编译），
// 文件末尾的两个测试是针对 Rust 侧 `executor` 模块已实现能力的可运行用例。

// 以下块注释为 Go 原测试的迁移参考实现，保留完整语义以便后续逐步启用。
/*
// 这段逻辑覆盖 add/drop index、主键、全局索引、analyze、vector index、columnar index 和 rollback 测试语义。

// indexModifyLease 对应 Go 常量 indexModifyLease，用于控制 schema lease 和轮询 ticker。
const INDEX_MODIFY_LEASE: Duration = Duration::from_millis(600);

// TestAddPrimaryKey1 对应 Go 同名测试：普通表添加 primary key。
#[test]
fn test_add_primary_key1() {
    test_add_index(
        TestAddIndexType::PLAIN,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint, unique key(c1))",
        "primary",
    );
}

// TestAddPrimaryKey2 对应 Go 同名测试：range partition 表添加 primary key。
#[test]
fn test_add_primary_key2() {
    test_add_index(
        TestAddIndexType::PARTITION,
        r#"create table test_add_index (c1 bigint, c2 bigint, c3 bigint, key(c1))
           partition by range (c3) (
           partition p0 values less than (3440),
           partition p1 values less than (61440),
           partition p2 values less than (122880),
           partition p3 values less than (204800),
           partition p4 values less than maxvalue)"#,
        "primary",
    );
}

// TestAddPrimaryKey3 对应 Go 同名测试：hash partition 表添加 primary key。
#[test]
fn test_add_primary_key3() {
    test_add_index(
        TestAddIndexType::PARTITION,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint, key(c1)) partition by hash (c3) partitions 4;",
        "primary",
    );
}

// TestAddPrimaryKey4 对应 Go 同名测试：range columns partition 表添加 primary key。
#[test]
fn test_add_primary_key4() {
    test_add_index(
        TestAddIndexType::PARTITION,
        r#"create table test_add_index (c1 bigint, c2 bigint, c3 bigint, key(c1))
           partition by range columns (c3) (
           partition p0 values less than (3440),
           partition p1 values less than (61440),
           partition p2 values less than (122880),
           partition p3 values less than (204800),
           partition p4 values less than maxvalue)"#,
        "primary",
    );
}

// TestAddIndex1 对应 Go 同名测试：普通表添加二级索引。
#[test]
fn test_add_index1() {
    test_add_index(
        TestAddIndexType::PLAIN,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint, primary key(c1))",
        "",
    );
}

// TestAddIndex1WithShardRowID 对应 Go 同名测试：shard row id 表添加索引。
#[test]
fn test_add_index1_with_shard_row_id() {
    test_add_index(
        TestAddIndexType::PARTITION | TestAddIndexType::SHARD_ROW_ID,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint) SHARD_ROW_ID_BITS = 4 pre_split_regions = 4;",
        "",
    );
}

// TestAddIndex2 对应 Go 同名测试：range partition 主键表添加二级索引。
#[test]
fn test_add_index2() {
    test_add_index(
        TestAddIndexType::PARTITION,
        r#"create table test_add_index (c1 bigint, c2 bigint, c3 bigint, primary key(c1))
           partition by range (c1) (
           partition p0 values less than (3440),
           partition p1 values less than (61440),
           partition p2 values less than (122880),
           partition p3 values less than (204800),
           partition p4 values less than maxvalue)"#,
        "",
    );
}

// TestAddIndex2WithShardRowID 对应 Go 同名测试：range partition + shard row id。
#[test]
fn test_add_index2_with_shard_row_id() {
    test_add_index(
        TestAddIndexType::PARTITION | TestAddIndexType::SHARD_ROW_ID,
        r#"create table test_add_index (c1 bigint, c2 bigint, c3 bigint)
           SHARD_ROW_ID_BITS = 4 pre_split_regions = 4
           partition by range (c1) (
           partition p0 values less than (3440),
           partition p1 values less than (61440),
           partition p2 values less than (122880),
           partition p3 values less than (204800),
           partition p4 values less than maxvalue)"#,
        "",
    );
}

// TestAddIndex3 对应 Go 同名测试：hash partition 主键表添加二级索引。
#[test]
fn test_add_index3() {
    test_add_index(
        TestAddIndexType::PARTITION,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint, primary key(c1)) partition by hash (c1) partitions 4;",
        "",
    );
}

// TestAddIndex3WithShardRowID 对应 Go 同名测试：hash partition + shard row id。
#[test]
fn test_add_index3_with_shard_row_id() {
    test_add_index(
        TestAddIndexType::PARTITION | TestAddIndexType::SHARD_ROW_ID,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint) SHARD_ROW_ID_BITS = 4 pre_split_regions = 4 partition by hash (c1) partitions 4;",
        "",
    );
}

// TestAddIndex4 对应 Go 同名测试：range columns partition 主键表添加索引。
#[test]
fn test_add_index4() {
    test_add_index(
        TestAddIndexType::PARTITION,
        r#"create table test_add_index (c1 bigint, c2 bigint, c3 bigint, primary key(c1))
           partition by range columns (c1) (
           partition p0 values less than (3440),
           partition p1 values less than (61440),
           partition p2 values less than (122880),
           partition p3 values less than (204800),
           partition p4 values less than maxvalue)"#,
        "",
    );
}

// TestAddIndex4WithShardRowID 对应 Go 同名测试：range columns partition + shard row id。
#[test]
fn test_add_index4_with_shard_row_id() {
    test_add_index(
        TestAddIndexType::PARTITION | TestAddIndexType::SHARD_ROW_ID,
        r#"create table test_add_index (c1 bigint, c2 bigint, c3 bigint)
           SHARD_ROW_ID_BITS = 4 pre_split_regions = 4
           partition by range columns (c1) (
           partition p0 values less than (3440),
           partition p1 values less than (61440),
           partition p2 values less than (122880),
           partition p3 values less than (204800),
           partition p4 values less than maxvalue)"#,
        "",
    );
}

// TestAddIndex5 对应 Go 同名测试：clustered primary key 表添加二级索引。
#[test]
fn test_add_index5() {
    test_add_index(
        TestAddIndexType::CLUSTERED_INDEX,
        "create table test_add_index (c1 bigint, c2 bigint, c3 bigint, primary key(c2, c3))",
        "",
    );
}

// testAddIndexType 对应 Go 的 uint8 bit flag。
// Rust 用 bitflags 记录普通表、分区表、clustered index 和 shard row id 四类用例。
bitflags::bitflags! {
    struct TestAddIndexType: u8 {
        const PLAIN = 1;
        const PARTITION = 1 << 1;
        const CLUSTERED_INDEX = 1 << 2;
        const SHARD_ROW_ID = 1 << 3;
    }
}

// testAddIndex 对应 Go 同名辅助函数。
// 它在 add index 后台执行期间穿插 delete/insert，再检查查询结果、admin check table 和索引元数据。
fn test_add_index(tp: TestAddIndexType, create_table_sql: &str, idx_tp: &str) {
    let is_test_shard_row_id = tp.contains(TestAddIndexType::SHARD_ROW_ID);
    let mut opts = vec![];
    if !is_test_shard_row_id {
        // Go 对非 shard row id 用 WithDDLChecker 包装 store；shard row id 不适合 SchemaTracker 检查。
        opts.push(mockstore::WithDDLChecker());
    }
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, opts);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");

    if is_test_shard_row_id {
        atomic::StoreUint32(&ddl::EnableSplitTableRegion, 1);
        tk.MustExec("set global tidb_scatter_region = 'table'");
        // Go defer 恢复 EnableSplitTableRegion 和 tidb_scatter_region。
    }
    if tp.contains(TestAddIndexType::CLUSTERED_INDEX) {
        tk.Session().GetSessionVars().EnableClusteredIndex = vardef::ClusteredIndexDefModeOn;
    }
    tk.MustExec("drop table if exists test_add_index");
    tk.MustExec(create_table_sql);

    let done = channel::bounded::<errors::Error>(1);
    let start = -10;
    let mut num = defaultBatchSize;
    batchInsert(&tk, "test_add_index", start, num);

    // Go 先插入离散行并记录 otherKeys，避免后续随机 delete 产生重复键。
    let mut other_keys = Vec::new();
    let mut base = defaultBatchSize * 20;
    for i in 1..100 {
        if is_test_shard_row_id {
            base = (i % 4) << 61;
        }
        let mut n = base + i * defaultBatchSize + i;
        for j in 0..rand::Intn(20) {
            n += j;
            tk.MustExec(format!("insert into test_add_index values ({}, {}, {})", n, n, n));
            other_keys.push(n);
        }
    }
    let v = math::MaxInt64 - defaultBatchSize / 2;
    tk.MustExec(format!("insert into test_add_index values ({}, {}, {})", v, v, v));
    other_keys.push(v);

    let add_idx_sql = format!("alter table test_add_index add {} key c3_index(c3)", idx_tp);
    testddlutil::SessionExecInGoroutine(&store, "test", &add_idx_sql, done.clone());

    let mut deleted_keys = HashSet::new();
    let ticker = time::NewTicker(INDEX_MODIFY_LEASE / 2);
    loop {
        select! {
            err = done.recv() => {
                if err.is_none() {
                    break;
                }
                require::no_error(err);
            }
            _ = ticker.C() => {
                // Go 在性能较差时限制写入行数，避免 add index 一直无法完成。
                if num > defaultBatchSize * 10 {
                    break;
                }
                for i in num..num + 5 {
                    let n = rand::Intn(num);
                    deleted_keys.insert(n);
                    tk.MustExec(format!("delete from test_add_index where c1 = {}", n));
                    tk.MustExec(format!("insert into test_add_index values ({}, {}, {})", i, i, i));
                }
                num += 5;
            }
        }
    }

    if is_test_shard_row_id {
        require::greater_or_equal(tk.MustQuery("show table test_add_index regions").Rows().len(), 16);
        tk.MustExec("admin check table test_add_index");
        return;
    }

    // Go 根据未删除的连续 key 和 otherKeys 生成期望结果，再通过 c3_index 查询验证索引可读。
    let mut keys: Vec<i32> = (start..num).filter(|k| !deleted_keys.contains(k)).collect();
    keys.extend(other_keys);
    let expected_rows = keys.iter().map(|key| vec![format!("{}", key)]).collect::<Vec<_>>();
    tk.MustQuery(format!("select c1 from test_add_index where c3 >= {} order by c1", start)).Check(expected_rows);
    tk.MustExec("admin check table test_add_index");
    if tp.contains(TestAddIndexType::PARTITION) {
        return;
    }

    // Go 重新开启事务并遍历 records，随后检查目标 index 名和 ID；保留事务/迭代资源收尾点。
    sessiontxn::NewTxn(context::Background(), tk.Session()).expect("Go require.NoError: new txn");
    let tbl = external::GetTableByName(&tk, "test", "test_add_index");
    let mut handles = kv::NewHandleMap();
    tables::IterRecords(tbl, tk.Session(), tbl.Cols(), |h, _data, _cols| {
        handles.Set(h, ());
        Ok(true)
    })
    .expect("Go require.NoError: iter records");
    let idx_name = if idx_tp.is_empty() { "c3_index" } else { "primary" };
    let nidx = tbl.Indices().find(|idx| idx.Meta().Name.L == idx_name);
    require::not_nil(nidx);
    require::greater(nidx.Meta().ID, 0);
    let txn = tk.Session().Txn(true).expect("Go require.NoError: txn");
    txn.Rollback().expect("Go require.NoError: rollback");

    sessiontxn::NewTxn(context::Background(), tk.Session()).expect("Go require.NoError: new txn");
    tk.MustExec("admin check table test_add_index");
    tk.MustExec("drop table test_add_index");
}

// TestAddIndexForGeneratedColumn 对应 Go 同名测试。
// 它验证 generated column 上添加/删除索引，以及 date/id 表达式回填后查询与 admin check。
#[test]
fn test_add_index_for_generated_column() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    exec_script(&tk, &[
        "create table t(y year NOT NULL DEFAULT '2155')",
        "insert into t values (?) -- Go 循环插入 0..50",
        "insert into t values()",
        "ALTER TABLE t ADD COLUMN y1 year as (y + 2)",
        "delete from t where y = 2155",
        "alter table t add index idx_y(y1)",
        "alter table t drop index idx_y",
        "drop table if exists gcai_table",
        "create table gcai_table (id int primary key);",
        "insert into gcai_table values(1);",
        "ALTER TABLE gcai_table ADD COLUMN d date DEFAULT '9999-12-31';",
        "ALTER TABLE gcai_table ADD COLUMN d1 date as (DATE_SUB(d, INTERVAL 31 DAY));",
        "ALTER TABLE gcai_table ADD INDEX idx(d1);",
        "ALTER TABLE gcai_table ADD COLUMN id1 int as (id+5);",
        "ALTER TABLE gcai_table ADD INDEX idx1(id1);",
    ]);
    assert_no_index_named(&tk, "test", "t", "idx_c2");
    tk.MustQuery("select * from gcai_table").Check(testkit::Rows("1 9999-12-31 9999-11-30"));
    tk.MustQuery("select d1 from gcai_table use index(idx)").Check(testkit::Rows("9999-11-30"));
    tk.MustExec("admin check table gcai_table");
    tk.MustQuery("select * from gcai_table").Check(testkit::Rows("1 9999-12-31 9999-11-30 6"));
    tk.MustQuery("select id1 from gcai_table use index(idx1)").Check(testkit::Rows("6"));
    tk.MustExec("admin check table gcai_table");
}

// TestAnalyzeStuck 对应 Go 同名测试。
// 通过 beforeAnalyzeTable failpoint 睡眠超过 ddl.DefaultCumulativeTimeout，确认 add index/modify column 仍能完成并最终写 stats_meta。
#[test]
fn test_analyze_stuck() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    let tk = testkit::NewTestKit(&store);
    exec_script(&tk, &[
        "set @@tidb_stats_update_during_ddl = 1",
        "use test",
        "drop table if exists t_add_index_stuck",
        "create table t_add_index_stuck (c1 int, c2 int, c3 int)",
        "insert into t_add_index_stuck values (?, ?, ?) -- Go 循环插入 0..10",
    ]);
    let old_cumulative_timeout = ddl::DefaultCumulativeTimeout;
    ddl::DefaultCumulativeTimeout = Duration::from_secs(2);
    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/beforeAnalyzeTable", || {
        time::Sleep(ddl::DefaultCumulativeTimeout + Duration::from_secs(10));
    });
    execute_in_background_and_wait(&tk, "alter table t_add_index_stuck add index c3_index(c3)", Duration::from_secs(70));
    assert_index_exists(&tk, "test", "t_add_index_stuck", "c3_index");
    require::eventually(|| tk.MustQuery("show stats_meta where table_name = 't_add_index_stuck'").Rows().len() > 0, Duration::from_secs(60), Duration::from_millis(200));
    execute_in_background_and_wait(&tk, "alter table t_add_index_stuck modify column c2 bigint", Duration::from_secs(70));
    require::eventually(|| tk.MustQuery("show stats_meta where table_name = 't_add_index_stuck'").Rows().len() > 0, Duration::from_secs(60), Duration::from_millis(200));
    ddl::DefaultCumulativeTimeout = old_cumulative_timeout;
}

// TestAnalyzeOwnerResignNoReRun 对应 Go 同名测试。
// analyzeTableDone failpoint 模拟 DDL owner resign，断言 beforeAnalyzeTable 只调用一次。
#[test]
fn test_analyze_owner_resign_no_rerun() {
    let (store, _dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    let tk = testkit::NewTestKit(&store);
    exec_script(&tk, &[
        "use test",
        "set @@tidb_stats_update_during_ddl = 1",
        "drop table if exists t_analyze_owner_resign",
        "create table t_analyze_owner_resign (c1 int, c2 int, key(c2))",
        "insert into t_analyze_owner_resign values (?, ?) -- Go 循环插入 0..10",
    ]);
    let call_count = atomic::Int32::new(0);
    let resigned_flag = atomic::Int32::new(0);
    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/beforeAnalyzeTable", || {
        call_count.Add(1);
    });
    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/analyzeTableDone", |job: model::Job| {
        if resigned_flag.CompareAndSwap(0, 1) {
            let tk2 = testkit::NewTestKit(&store);
            tk2.MustExec("use test");
            // Go 连续更新 processing=0/1 来模拟 DDL job 表写冲突。
            tk2.MustExec(format!("update mysql.tidb_ddl_job set processing = 0 where job_id = {}", job.ID));
            tk2.MustExec(format!("update mysql.tidb_ddl_job set processing = 1 where job_id = {}", job.ID));
        }
    });
    tk.Session().Execute(context::Background(), "alter table t_analyze_owner_resign add index idx_c2(c2)")
        .expect("Go require.NoError: add index");
    require::equal(call_count.Load(), 1, "analyze should not be re-run after owner resigns");
}

// TestAddPrimaryKeyRollback1 对应 Go 同名测试：重复主键导致回滚。
#[test]
fn test_add_primary_key_rollback1() {
    let err_msg = format!("[kv:1062]Duplicate entry '{}' for key 't1.PRIMARY'", defaultBatchSize * 2 - 10);
    test_add_index_rollback("PRIMARY", "alter table t1 add primary key c3_index (c3);", &err_msg, false);
}

// TestAddPrimaryKeyRollback2 对应 Go 同名测试：主键列含 NULL 导致回滚。
#[test]
fn test_add_primary_key_rollback2() {
    test_add_index_rollback("PRIMARY", "alter table t1 add primary key c3_index (c3);", "[ddl:1138]Invalid use of NULL value", true);
}

// TestAddUniqueIndexRollback 对应 Go 同名测试：唯一索引遇到重复值回滚。
#[test]
fn test_add_unique_index_rollback() {
    let err_msg = format!("[kv:1062]Duplicate entry '{}' for key 't1.c3_index'", defaultBatchSize * 2 - 10);
    test_add_index_rollback("c3_index", "create unique index c3_index on t1 (c3)", &err_msg, false);
}

// testAddIndexRollback 对应 Go 同名辅助函数。
// 它在后台 add index 回滚期间继续写入，并保护会影响 duplicate error message 的关键行不被删除。
fn test_add_index_rollback(idx_name: &str, add_idx_sql: &str, err_msg: &str, has_null_vals_in_key: bool) {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("create table t1 (c1 int, c2 int, c3 int, unique key(c1))");
    let base = defaultBatchSize * 2;
    let mut count = base;
    batchInsert(&tk, "t1", 0, count);
    if has_null_vals_in_key {
        for i in count - 10..count {
            tk.MustExecWithArgs("insert into t1 values (?, ?, null)", vec![i + 10, i]);
        }
    } else {
        for i in count - 10..count {
            tk.MustExecWithArgs("insert into t1 values (?, ?, ?)", vec![i + 10, i, i]);
        }
    }

    let done = channel::bounded::<errors::Error>(1);
    backgroundExec(&store, "test", add_idx_sql, done.clone());
    let mut times = 0;
    let ticker = time::NewTicker(INDEX_MODIFY_LEASE / 2);
    loop {
        select! {
            err = done.recv() => {
                require::equal_error(err, err_msg);
                break;
            }
            _ = ticker.C() => {
                if times >= 10 {
                    break;
                }
                for i in count..count + 5 {
                    let n = rand::Intn(count);
                    if n == defaultBatchSize * 2 - 10 || n == defaultBatchSize * 2 {
                        continue;
                    }
                    tk.MustExecWithArgs("delete from t1 where c1 = ?", vec![n]);
                    tk.MustExecWithArgs("insert into t1 values (?, ?, ?)", vec![i + 10, i, i]);
                }
                count += 5;
                times += 1;
            }
        }
    }
    assert_no_index_named(&tk, "test", "t1", idx_name);
    for i in base - 10..base {
        tk.MustExecWithArgs("delete from t1 where c1 = ?", vec![i + 10]);
    }
    tk.MustExec(add_idx_sql);
    tk.MustExec("drop table t1");
}

// TestAddIndexWithSplitTable 对应 Go 同名测试：AUTO_RANDOM 主键表 split 后 add index。
#[test]
fn test_add_index_with_split_table() {
    let create_sql = "CREATE TABLE test_add_index(a bigint PRIMARY KEY AUTO_RANDOM(4), b varchar(255), c bigint)";
    let split_sql = format!("SPLIT TABLE test_add_index BETWEEN ({}) AND ({}) REGIONS 16;", math::MinInt64, math::MaxInt64);
    test_add_index_with_split_table(create_sql, &split_sql);
}

// TestAddIndexWithShardRowID 对应 Go 同名测试：shard row id 预切分后 add index。
#[test]
fn test_add_index_with_shard_row_id() {
    test_add_index_with_split_table(
        "create table test_add_index(a bigint, b bigint, c bigint) SHARD_ROW_ID_BITS = 4 pre_split_regions = 4;",
        "",
    );
}

// testAddIndexWithSplitTable 对应 Go 同名辅助函数。
// 它并发批量插入不同 key range，再在 add index 时持续 delete/insert/update，最后 admin check table。
fn test_add_index_with_split_table(create_sql: &str, split_table_sql: &str) {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    let has_auto_random_field = !split_table_sql.is_empty();
    if !has_auto_random_field {
        atomic::StoreUint32(&ddl::EnableSplitTableRegion, 1);
        tk.MustExec("set global tidb_scatter_region = 'table'");
    }
    tk.MustExec(create_sql);

    // Go 的 batchInsertRows 会拼接单条 multi-values insert；这里保留 needVal 控制 AUTO_RANDOM 空值插入的差异。
    let batch_insert_rows = |tk: &testkit::TestKit, need_val: bool, tbl: &str, start: i32, end: i32| -> Result<(), errors::Error> {
        let mut dml = format!("insert into {} values", tbl);
        for i in start..end {
            dml.push_str(if need_val { &format!("({}, {}, {})", i, i, i) } else { "()" });
            if i != end - 1 {
                dml.push(',');
            }
        }
        tk.Exec(dml).map(|_| ())
    };

    let done = channel::bounded::<errors::Error>(1);
    let start = -20;
    let initial_num = defaultBatchSize;
    // Go 开 10 个 goroutine 分散写入不同高位前缀，验证 region split 后 add index 的并发路径。
    run_parallel(10, |i| {
        let tk1 = testkit::NewTestKit(&store);
        tk1.MustExec("use test");
        let base = (i % 8) << 60;
        batch_insert_rows(&tk1, !has_auto_random_field, "test_add_index", base + start, base + initial_num)
    });

    if has_auto_random_field {
        tk.MustQuery(split_table_sql).Check(testkit::Rows("15 1"));
    }
    tk.MustQuery("select @@session.tidb_wait_split_region_finish").Check(testkit::Rows("1"));
    require::len(tk.MustQuery("show table test_add_index regions").Rows(), 16);
    testddlutil::SessionExecInGoroutine(&store, "test", "alter table test_add_index add index idx(a)", done.clone());
    poll_ddl_and_mutate(&tk, done, INDEX_MODIFY_LEASE / 5, 1000, |tk, i| {
        tk.MustExec(format!("delete from test_add_index where a = {}", i + 1));
        if has_auto_random_field {
            tk.MustExec("insert into test_add_index values ()");
        } else {
            tk.MustExec(format!("insert into test_add_index values ({}, {}, {})", i, i, i));
        }
        tk.MustExec(format!("update test_add_index set b = {}", i * 10));
    });
    tk.MustExec("admin check table test_add_index");
}

// TestAddAnonymousIndex 对应 Go 同名测试。
// 它验证匿名索引命名、重复匿名索引、大小写不敏感和列名 primary 的自动命名规则。
#[test]
fn test_add_anonymous_index() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    exec_script(&tk, &[
        "create table t_anonymous_index (c1 int, c2 int, C3 int)",
        "alter table t_anonymous_index add index (c1, c2)",
    ]);
    expect_error(&tk, "alter table t_anonymous_index drop index");
    exec_script(&tk, &[
        "alter table t_anonymous_index drop index c1",
        "alter table t_anonymous_index add index (c1)",
    ]);
    expect_error(&tk, "alter table t_anonymous_index add index c1 (c2)");
    exec_script(&tk, &[
        "alter table t_anonymous_index add index c1_3 (c1)",
        "alter table t_anonymous_index add index (c1, c2, C3)",
        "alter table t_anonymous_index add index (c1)",
        "alter table t_anonymous_index drop index c1",
        "alter table t_anonymous_index drop index c1_2",
        "alter table t_anonymous_index drop index c1_3",
        "alter table t_anonymous_index drop index c1_4",
        "alter table t_anonymous_index add index (C3)",
        "alter table t_anonymous_index drop index c3",
        "alter table t_anonymous_index add index c3 (C3)",
        "alter table t_anonymous_index drop index C3",
        "create table t_primary (`primary` int, b int, key (`primary`))",
        "alter table t_primary add index (`primary`);",
        "alter table t_primary add primary key(b);",
        "create table t_primary_2 (`primary` int, key primary_2 (`primary`), key (`primary`))",
        "create table t_primary_3 (`primary_2` int, key(`primary_2`), `primary` int, key(`primary`));",
    ]);
    assert_index_names(&tk, "test", "t_primary", &["primary_2", "primary_3", "primary"]);
    assert_index_names(&tk, "test", "t_primary_2", &["primary_2", "primary_3"]);
    assert_index_names(&tk, "test", "t_primary_3", &["primary_2", "primary_3"]);
}

// TestAddIndexWithPK 对应 Go 同名测试。
// 它分别在 IntOnly 和 On clustered index 模式下，验证 primary key 列可重复出现在新增索引中。
#[test]
fn test_add_index_with_pk() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    for (name, mode) in [
        ("ClusteredIndexDefModeIntOnly", vardef::ClusteredIndexDefModeIntOnly),
        ("ClusteredIndexDefModeOn", vardef::ClusteredIndexDefModeOn),
    ] {
        run_case(name, || {
            tk.Session().GetSessionVars().EnableClusteredIndex = mode;
            exec_script(&tk, &[
                "drop table if exists test_add_index_with_pk",
                "create table test_add_index_with_pk(a int not null, b int not null default '0', primary key(a))",
                "insert into test_add_index_with_pk values(1, 2)",
                "alter table test_add_index_with_pk add index idx (a)",
                "insert into test_add_index_with_pk values(2, 2)",
                "alter table test_add_index_with_pk add index idx1 (a, b)",
                "drop table if exists test_add_index_with_pk1",
                "create table test_add_index_with_pk1(a int not null, b int not null default '0', c int, d int, primary key(c))",
                "insert into test_add_index_with_pk1 values(1, 1, 1, 1)",
                "alter table test_add_index_with_pk1 add index idx (c)",
                "insert into test_add_index_with_pk1 values(2, 2, 2, 2)",
                "drop table if exists test_add_index_with_pk2",
                "create table test_add_index_with_pk2(a int not null, b int not null default '0', c int unsigned, d int, primary key(c))",
                "insert into test_add_index_with_pk2 values(1, 1, 1, 1)",
                "alter table test_add_index_with_pk2 add index idx (c)",
                "insert into test_add_index_with_pk2 values(2, 2, 2, 2)",
                "drop table if exists t",
                "create table t (a int, b int, c int, primary key(a, b));",
                "insert into t values (1, 2, 3);",
                "create index idx on t (a, b);",
            ]);
        });
    }
}

// TestAddGlobalIndex 对应 Go 同名测试。
// 它验证 partition 表上的 unique/global primary/non-unique global index 编码，并覆盖 duplicate 与 multi schema change。
#[test]
fn test_add_global_index() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");

    exec_script(&tk, &[
        "create table test_t1 (a int, b int) partition by range (b) (partition p0 values less than (10), partition p1 values less than (maxvalue));",
        "insert test_t1 values (1, 1)",
        "alter table test_t1 add unique index p_a (a) global",
        "insert test_t1 values (2, 11)",
    ]);
    check_global_index_row_case(&tk, "test_t1", "p_a", true);

    exec_script(&tk, &[
        "create table test_t2 (a int, b int) partition by range (b) (partition p0 values less than (10), partition p1 values less than (maxvalue));",
        "insert test_t2 values (1, 1)",
        "alter table test_t2 add primary key (a) nonclustered global",
        "insert test_t2 values (2, 11)",
    ]);
    check_global_index_row_case(&tk, "test_t2", "primary", true);

    exec_script(&tk, &[
        "drop table if exists test_t2",
        "create table test_t2 (a int, b int) partition by range (b) (partition p0 values less than (10), partition p1 values less than (maxvalue));",
        "insert test_t2 values (2, 1)",
        "alter table test_t2 add key p_a (a) global",
        "insert test_t2 values (1, 11)",
    ]);
    check_global_index_row_case(&tk, "test_t2", "p_a", false);

    exec_script(&tk, &[
        "drop table if exists t",
        "create table t(a int, b int) partition by hash(b) partitions 64",
        "alter table t add unique index idx(a) global",
        "drop table t",
        "create table t(a int, b int) partition by hash(b) partitions 64",
        "insert into t values (1, 2), (1, 3)",
    ]);
    tk.MustContainErrMsg("alter table t add unique index idx(a) global", "[kv:1062]Duplicate entry '1' for key 't.idx'");
    exec_script(&tk, &[
        "drop table t",
        "create table t(a int, b int) partition by hash(b) partitions 64",
        "alter table t add unique index idx(a) global, add index idx1(b)",
    ]);
}

// checkGlobalIndexRow 对应 Go 同名辅助函数。
// 它在事务中检查：本地分区 index key 不存在，global index key 可解码出 index values、handle、pid，并能回查行数据。
fn check_global_index_row(
    ctx: sessionctx::Context,
    tbl_info: &model::TableInfo,
    index_info: &model::IndexInfo,
    pid: i64,
    idx_vals: Vec<types::Datum>,
    row_vals: Vec<types::Datum>,
) {
    sessiontxn::NewTxn(context::Background(), ctx).expect("Go require.NoError: new txn");
    let txn = ctx.Txn(true).expect("Go require.NoError: txn");
    let sc = ctx.GetSessionVars().StmtCtx;
    let tbl_col_map = tbl_info.Columns.iter().map(|col| (col.ID, &col.FieldType)).collect::<HashMap<_, _>>();

    let local_prefix = tablecodec::EncodeTableIndexPrefix(pid, index_info.ID);
    let it = txn.Iter(local_prefix.clone(), None).expect("Go require.NoError: iter local prefix");
    require::false_(it.Valid() && it.Key().HasPrefix(local_prefix));
    it.Close();

    let encoded_value = codec::EncodeKey(sc.TimeZone(), None, idx_vals.clone()).expect("Go require.NoError: encode key");
    let mut key = tablecodec::EncodeIndexSeekKey(tbl_info.ID, index_info.ID, encoded_value);
    let value = if index_info.Unique {
        kv::GetValue(context::Background(), txn, key.clone()).expect("Go require.NoError: unique global value")
    } else {
        let iter = txn.Iter(key.clone(), key.PrefixNext()).expect("Go require.NoError: non-unique global iter");
        require::true_(iter.Valid());
        key = iter.Key();
        iter.Value()
    };
    let idx_col_infos = tables::BuildRowcodecColInfoForIndexColumns(index_info, tbl_info);
    let col_vals = tablecodec::DecodeIndexKV(key, value, index_info.Columns.len(), tablecodec::HandleDefault, idx_col_infos)
        .expect("Go require.NoError: decode index kv");
    require::len(&col_vals, idx_vals.len() + 2);
    for (i, val) in idx_vals.iter().enumerate() {
        let (_remain, d) = codec::DecodeOne(col_vals[i].clone()).expect("Go require.NoError: decode idx datum");
        require::equal(val, &d);
    }
    let (_remain, pid_datum) = codec::DecodeOne(col_vals[idx_vals.len() + 1].clone()).expect("Go require.NoError: decode pid");
    require::equal(pid, pid_datum.GetInt64());
    let (_remain, handle_datum) = codec::DecodeOne(col_vals[idx_vals.len()].clone()).expect("Go require.NoError: decode handle");
    let h = kv::IntHandle(handle_datum.GetInt64());
    let row_key = tablecodec::EncodeRowKey(pid, h.Encoded());
    let row_value = kv::GetValue(context::Background(), txn, row_key).expect("Go require.NoError: row value");
    let row_value_datums = tablecodec::DecodeRowToDatumMap(row_value, tbl_col_map, time::UTC)
        .expect("Go require.NoError: decode row");
    for (i, val) in row_vals.iter().enumerate() {
        require::equal(val, row_value_datums[tbl_info.Columns[i].ID]);
    }
}

// TestDropIndexes 对应 Go 同名测试。
// 覆盖多索引删除、drop if exists、重复删除顺序和 partition 表 drop index/drop column 冲突。
#[test]
fn test_drop_indexes() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    test_drop_indexes(
        &store,
        "create table test_drop_indexes (id int, c1 int, c2 int, primary key(id) nonclustered, key i1(c1), key i2(c2));",
        "alter table test_drop_indexes drop index i1, drop index i2;",
        vec!["i1", "i2"],
    );
    test_drop_indexes(
        &store,
        "create table test_drop_indexes (id int, c1 int, c2 int, primary key(id) nonclustered, unique key i1(c1), key i2(c2));",
        "alter table test_drop_indexes drop primary key, drop index i1;",
        vec!["primary", "i1"],
    );
    test_drop_indexes(
        &store,
        "create table test_drop_indexes (uuid varchar(32), c1 int, c2 int, primary key(uuid) nonclustered, unique key i1(c1), key i2(c2));",
        "alter table test_drop_indexes drop primary key, drop index i1, drop index i2;",
        vec!["primary", "i1", "i2"],
    );
    test_drop_indexes_if_exists(&store);
    test_drop_indexes_from_partitioned_table(&store);
}

// testDropIndexes 对应 Go 同名辅助函数。
// 后台 drop 多个 index 的同时持续 update/insert，验证 drop DDL 可与写入交错完成。
fn test_drop_indexes(store: &kv::Storage, create_sql: &str, drop_idx_sql: &str, idx_names: Vec<&str>) {
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists test_drop_indexes");
    tk.MustExec(create_sql);
    for i in 0..100 {
        tk.MustExecWithArgs("insert into test_drop_indexes values (?, ?, ?)", vec![i, i, i]);
    }
    let _idx_ids: Vec<i64> = idx_names.iter().map(|idx| external::GetIndexID(&tk, "test", "test_drop_indexes", idx)).collect();
    let done = channel::bounded::<errors::Error>(1);
    testddlutil::SessionExecInGoroutine(store, "test", drop_idx_sql, done.clone());
    poll_ddl_and_mutate(&tk, done, INDEX_MODIFY_LEASE / 2, 500, |tk, i| {
        let n = rand::Intn(i.max(1));
        tk.MustExecWithArgs("update test_drop_indexes set c2 = 1 where c1 = ?", vec![n]);
        tk.MustExecWithArgs("insert into test_drop_indexes values (?, ?, ?)", vec![i, i, i]);
    });
}

// testDropIndexesIfExists 对应 Go 同名辅助函数。
// 它检查 drop index if exists 的 warning，以及同一 DDL 中重复 drop 的 UnsupportedDDLOperation。
fn test_drop_indexes_if_exists(store: &kv::Storage) {
    let tk = testkit::NewTestKit(store);
    exec_script(&tk, &[
        "use test;",
        "drop table if exists test_drop_indexes_if_exists;",
        "create table test_drop_indexes_if_exists (id int, c1 int, c2 int, primary key(id), key i1(c1), key i2(c2));",
    ]);
    tk.MustGetErrMsg("alter table test_drop_indexes_if_exists drop index i1, drop index i3;", "[ddl:1091]index i3 doesn't exist");
    tk.MustExec("alter table test_drop_indexes_if_exists drop index i1, drop index if exists i3;");
    tk.MustQuery("show warnings;").Check(testkit::RowsWithSep("|", "Note|1091|index i3 doesn't exist"));
    for sql in [
        "alter table test_drop_indexes_if_exists drop index i2, drop index i2;",
        "alter table test_drop_indexes_if_exists drop index if exists i2, drop index i2;",
        "alter table test_drop_indexes_if_exists drop index i2, drop index if exists i2;",
    ] {
        tk.MustGetErrCode(sql, errno::ErrUnsupportedDDLOperation);
    }
}

// testDropIndexesFromPartitionedTable 对应 Go 同名辅助函数。
// 它验证 partition table 上 drop index if exists 和 drop column if exists 的重复目标冲突。
fn test_drop_indexes_from_partitioned_table(store: &kv::Storage) {
    let tk = testkit::NewTestKit(store);
    exec_script(&tk, &[
        "use test;",
        "drop table if exists test_drop_indexes_from_partitioned_table;",
        "create table test_drop_indexes_from_partitioned_table (id int, c1 int, c2 int, primary key(id), key i1(c1), key i2(c2)) partition by range(id) (partition p0 values less than (6), partition p1 values less than maxvalue);",
        "insert into test_drop_indexes_from_partitioned_table values (?, ?, ?) -- Go 循环插入 0..20",
        "alter table test_drop_indexes_from_partitioned_table drop index i1, drop index if exists i2;",
        "alter table test_drop_indexes_from_partitioned_table add index i1(c1)",
    ]);
    tk.MustGetErrCode("alter table test_drop_indexes_from_partitioned_table drop index i1, drop index if exists i1;", errno::ErrUnsupportedDDLOperation);
    tk.MustExec("alter table test_drop_indexes_from_partitioned_table drop column c1, drop column c2;");
    tk.MustExec("alter table test_drop_indexes_from_partitioned_table add column c1 int");
    tk.MustGetErrCode("alter table test_drop_indexes_from_partitioned_table drop column c1, drop column if exists c1;", errno::ErrUnsupportedDDLOperation);
}

// TestDropPrimaryKey 对应 Go 同名测试：drop nonclustered primary key。
#[test]
fn test_drop_primary_key() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    test_drop_index(
        &store,
        "create table test_drop_index (c1 int, c2 int, c3 int, unique key(c1), primary key(c3) nonclustered)",
        "alter table test_drop_index drop primary key;",
        "primary",
    );
}

// TestDropIndex 对应 Go 同名测试：drop 普通二级索引。
#[test]
fn test_drop_index() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    test_drop_index(
        &store,
        "create table test_drop_index (c1 int, c2 int, c3 int, unique key(c1), key c3_index(c3))",
        "alter table test_drop_index drop index c3_index;",
        "c3_index",
    );
}

// testDropIndex 对应 Go 同名辅助函数。
// 后台 drop 单个索引时持续写入，完成后 explain 中不应再包含被删除索引名。
fn test_drop_index(store: &kv::Storage, create_sql: &str, drop_idx_sql: &str, idx_name: &str) {
    let tk = testkit::NewTestKit(store);
    exec_script(&tk, &[
        "use test",
        "drop table if exists test_drop_index",
        create_sql,
        "delete from test_drop_index",
        "insert into test_drop_index values (?, ?, ?) -- Go 循环插入 0..100",
    ]);
    let done = channel::bounded::<errors::Error>(1);
    testddlutil::SessionExecInGoroutine(store, "test", drop_idx_sql, done.clone());
    poll_ddl_and_mutate(&tk, done, INDEX_MODIFY_LEASE / 2, 500, |tk, i| {
        let n = rand::Intn(i.max(1));
        tk.MustExecWithArgs("update test_drop_index set c2 = 1 where c1 = ?", vec![n]);
        tk.MustExecWithArgs("insert into test_drop_index values (?, ?, ?)", vec![i, i, i]);
    });
    let rows = tk.MustQuery("explain select c1 from test_drop_index where c3 >= 0");
    require::not_contains(format!("{:?}", rows), idx_name);
    tk.MustExec("drop table test_drop_index");
}

// TestAnonymousIndex 对应 Go 同名测试。
// 它验证超长列名生成匿名索引时，重复名称会截断并追加 _2。
#[test]
fn test_anonymous_index() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![mockstore::WithDDLChecker()]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    exec_script(&tk, &[
        "DROP TABLE IF EXISTS t",
        "create table t(bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb int, b int)",
        "alter table t add index bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb(b)",
        "alter table t add index (bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb)",
    ]);
    require::len(tk.MustQuery("show index from t where key_name='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'").Rows(), 1);
    require::len(tk.MustQuery("show index from t where key_name='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb_2'").Rows(), 1);
}

// TestAddIndexWithDupIndex 对应 Go 同名测试。
// 它区分 public duplicate index 与非 public state 下后台 job 正在添加同名索引的错误文案。
#[test]
fn test_add_index_with_dup_index() {
    let store = testkit::CreateMockStoreWithSchemaLease(INDEX_MODIFY_LEASE, vec![]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    let err1 = dbterror::ErrDupKeyName.GenWithStack("index already exist %s", "idx");
    let err2 = dbterror::ErrDupKeyName.GenWithStack(
        "index already exist %s; a background job is trying to add the same index, please check by `ADMIN SHOW DDL JOBS`",
        "idx",
    );
    tk.MustExec("create table test_add_index_with_dup (a int, key idx (a))");
    let err = tk.ExecToErr("alter table test_add_index_with_dup add index idx (a)");
    require::error_is(err, errors::Cause(err1));
    let tbl = external::GetTableByName(&tk, "test", "test_add_index_with_dup");
    let index_info = tbl.Meta().FindIndexByName("idx");
    index_info.State = model::StateNone;
    let err = tk.ExecToErr("alter table test_add_index_with_dup add index idx (a)");
    require::error_is(err, errors::Cause(err2));
}

// TestAddIndexUniqueFailOnDuplicate 对应 Go 同名测试。
// 它禁用 distributed reorg 后构造重复 key，断言 add unique index 提前失败且结果计数不会过大。
#[test]
fn test_add_index_unique_fail_on_duplicate() {
    if kerneltype::IsNextGen() {
        skip("add-index always runs on DXF with ingest mode in nextgen");
    }
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    exec_script(&tk, &[
        "use test",
        "create table t (a bigint primary key clustered, b int);",
        "set @@global.tidb_enable_dist_task = 0;",
        "insert into t values (?, ?) -- Go 循环插入 1..=12",
        "insert into t values (0, 1);",
    ]);
    if kerneltype::IsClassic() {
        tk.MustExec("set @@tidb_ddl_reorg_worker_cnt = 1;");
    }
    if kerneltype::IsNextGen() {
        testfailpoint::Enable("github.com/pingcap/tidb/pkg/ddl/MockTableSize", "return(1024)");
    }
    tk.MustQuery("split table t by (0), (1), (2), (3), (4), (5), (6), (7), (8), (9), (10), (11), (12);").Check(testkit::Rows("13 1"));
    ddl::ResultCounterForTest = atomic::Int32::new(0);
    tk.MustGetErrCode("alter table t add unique index idx (b);", errno::ErrDupEntry);
    require::less(ddl::ResultCounterForTest.Load(), 6);
    ddl::ResultCounterForTest = nil;
}

// getJobsBySQL 对应 Go 同名辅助函数。
// 它从 mysql.tidb_ddl_history 或 mysql.tidb_ddl_job 读取 job_meta 并 Decode 成 model.Job。
fn get_jobs_by_sql(se: sessionapi::Session, tbl: &str, condition: &str) -> Result<Vec<model::Job>, errors::Error> {
    let rs = se.Execute(context::Background(), format!("select job_meta from mysql.{} {}", tbl, condition))?;
    if rs.len() != 1 {
        return Err(errors::New("row cnt is wrong"));
    }
    let rows = sqlexec::DrainRecordSet(context::Background(), rs[0], 8)?;
    terror::Call(rs[0].Close);
    let mut jobs = Vec::with_capacity(16);
    for row in rows {
        let job_binary = row.GetBytes(0);
        let mut job = model::Job::default();
        job.Decode(job_binary)?;
        jobs.push(job);
    }
    Ok(jobs)
}

// TestAddIndexWithAnalyze 对应 Go 同名测试。
// 它验证 add index/modify column 后 stats handle 中列和索引的分析版本同步，partition table 则不自动分析新增索引。
#[test]
fn test_add_index_with_analyze() {
    let (store, dom) = testkit::CreateMockStoreAndDomain();
    let tk = testkit::NewTestKit(&store);
    exec_script(&tk, &[
        "use test",
        "set @@tidb_stats_update_during_ddl = 1",
        "create table t(a int NOT NULL DEFAULT 10, b int, index idx_b(b))",
        "insert into t values (?, ?) -- Go 循环插入 0..50",
        "ALTER TABLE t ADD index idx(a)",
        "select * from t use index(idx) where a >1",
        "select * from t use index(idx_b) where b >1",
    ]);
    assert_stats_analyzed(&dom, "test", "t", &["a", "b"], &["idx", "idx_b"]);
    tk.MustExec("ALTER TABLE t modify column a varchar(10)");
    assert_stats_reanalyzed_after_modify(&dom, "test", "t", "a", "b", "idx", "idx_b");
    exec_script(&tk, &[
        "CREATE TABLE pt(id INT NOT NULL, stu_id INT NOT NULL) PARTITION BY RANGE (stu_id) (PARTITION p0 VALUES LESS THAN (25),PARTITION p1 VALUES LESS THAN (51))",
        "insert into pt values (?,?) -- Go 循环插入 0..50",
        "analyze table pt all columns",
        "ALTER TABLE pt ADD index idx(id)",
        "select * from pt use index(idx) where id >1",
    ]);
    assert_partition_add_index_not_analyzed(&dom, "test", "pt", "id");
    exec_script(&tk, &[
        "ALTER TABLE pt modify column id varchar(10)",
        "drop table if exists t1;",
        "create table t1( id int, a int, b int, index idx(id, a));",
        "insert into t1 values (1, 1, 1), (2, 2, 2), (3, 3, 3), (4, 4, 4), (5, 5, 5);",
        "analyze table t1 all columns with 1 topn, 10 buckets;",
        " ALTER TABLE t1 ADD INDEX idx_a(a), ADD INDEX idx_b(b);",
    ]);
    tk.MustQuery("explain select * from t1 where a = 1;").CheckContain("TableFullScan");
}

// TestCreateTableWithVectorIndex 对应 Go 同名测试。
// 覆盖 create table 中声明 vector index 的 TiFlash 副本数、匿名命名、视图读取和各种不支持表类型。
#[test]
fn test_create_table_with_vector_index() {
    let (mut store, mut dom) = testkit::CreateMockStoreAndDomain();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    let check_create_table_with_vector_idx = |tk: &testkit::TestKit, dom: &domain::Domain, replica_cnt: u64| {
        tk.MustExec("create table t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))) USING HNSW, vector index((VEC_L2_DISTANCE(b))));");
        assert_vector_index_meta(dom, "test", "t", replica_cnt, &["vector_index", "vector_index_2"]);
        tk.MustExec("insert into t values (1, '[1,2.1,3.3]');");
        tk.MustQuery("select * from t;").Check(testkit::Rows("1 [1,2.1,3.3]"));
        tk.MustExec("create view v as select * from t;");
        tk.MustQuery("select * from v;").Check(testkit::Rows("1 [1,2.1,3.3]"));
        tk.MustExec("DROP TABLE t");
    };
    require::equal(infoschema::GetTiFlashStoreCount(tk.Session().GetStore()).unwrap(), 0_u64);
    tk.MustContainErrMsg("create table t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))) USING HNSW);",
        "Unsupported add columnar index: unsupported TiFlash store count is 0");
    (store, dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(tiflashReplicaLease, vec![mockstore::WithMockTiFlash(2), mockstore::WithDDLChecker()]);
    tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    check_create_table_with_vector_idx(&tk, &dom, 1);
    for (sql, msg) in [
        ("create temporary table t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))) USING HNSW)", "`set TiFlash replica` is unsupported on temporary tables."),
        ("create global temporary table t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))) USING HNSW) on commit delete rows;", "`set TiFlash replica` is unsupported on temporary tables."),
        ("create table pt(id bigint, b vector(3), vector index((VEC_COSINE_DISTANCE(b))) USING HNSW) partition by range(id) (partition p0 values less than (20), partition p1 values less than (100));", "Unsupported add columnar index: unsupported partition table"),
        ("create table t(a int, b vector(3), c char(210) CHARACTER SET gbk COLLATE gbk_bin, vector index((VEC_COSINE_DISTANCE(b))));", "Unsupported `set TiFlash replica` settings for table contains gbk charset"),
        ("create table mysql.t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))));", "Unsupported `set TiFlash replica` settings for system table and memory table"),
        ("create table information_schema.t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))));", "Unsupported `set TiFlash replica` settings for system table and memory table"),
        ("create table t(a int, b vector(3), vector index((VEC_COSINE_DISTANCE(b))) USING HNSW INVISIBLE)", "[ddl:8200]INVISIBLE can not be used in VECTOR INDEX"),
    ] {
        tk.MustContainErrMsg(sql, msg);
    }
}

// TestCreateTableWithColumnarIndex 对应 Go 同名测试。
// 覆盖 create table 中声明 inverted columnar index 的 columnar/tiflash 两种存储类型和错误路径。
#[test]
fn test_create_table_with_columnar_index() {
    let restore = config::RestoreFunc();
    let (mut store, mut dom) = testkit::CreateMockStoreAndDomain();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    let check_create_table_with_columnar_idx = |tk: &testkit::TestKit, dom: &domain::Domain, replica_cnt: u64| {
        tk.MustExec("create table t(a int, b int, c int, columnar index idx(b) using inverted);");
        assert_columnar_index_meta(dom, "test", "t", replica_cnt, "idx");
        tk.MustExec("insert into t values (1, 2, 3);");
        tk.MustQuery("select * from t;").Check(testkit::Rows("1 2 3"));
        tk.MustExec("create view v as select * from t;");
        tk.MustQuery("select * from v;").Check(testkit::Rows("1 2 3"));
        tk.MustExec("DROP TABLE t");
    };
    config::UpdateGlobal(|conf| conf.CSE.ColumnarStoreType = "columnar".to_string());
    check_create_table_with_columnar_idx(&tk, &dom, 1);
    config::UpdateGlobal(|conf| conf.CSE.ColumnarStoreType = "tiflash".to_string());
    require::equal(infoschema::GetTiFlashStoreCount(tk.Session().GetStore()).unwrap(), 0_u64);
    tk.MustContainErrMsg("create table t(a int, b int, c int, columnar index idx(b) using inverted);",
        "Unsupported add columnar index: unsupported TiFlash store count is 0");
    (store, dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(tiflashReplicaLease, vec![mockstore::WithMockTiFlash(2), mockstore::WithDDLChecker()]);
    tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    check_create_table_with_columnar_idx(&tk, &dom, 1);
    for (sql, msg) in [
        ("create temporary table t(a int, b int, c int, columnar index idx(b) using inverted);", "`set TiFlash replica` is unsupported on temporary tables."),
        ("create global temporary table t(a int, b int, c int, columnar index idx(b) using inverted) on commit delete rows;", "`set TiFlash replica` is unsupported on temporary tables."),
        ("create table pt(id bigint, b int, c int, columnar index idx(b) using inverted) partition by range(id) (partition p0 values less than (20), partition p1 values less than (100));", "Unsupported add columnar index: unsupported partition table"),
        ("create table t(a int, b int, c char(210) CHARACTER SET gbk COLLATE gbk_bin, columnar index idx(b) using inverted);", "Unsupported `set TiFlash replica` settings for table contains gbk charset"),
        ("create table mysql.t(a int, b int, c int, columnar index idx(b) using inverted);", "Unsupported `set TiFlash replica` settings for system table and memory table"),
        ("create table information_schema.t(a int, b int, c int, columnar index idx(b) using inverted);", "Unsupported `set TiFlash replica` settings for system table and memory table"),
        ("create table t(a int, b int, c int, columnar index idx(b) using inverted INVISIBLE)", "[ddl:8200]INVISIBLE can not be used in INVERTED INDEX"),
    ] {
        tk.MustContainErrMsg(sql, msg);
    }
    restore();
}

// TestAddVectorIndexSimple 对应 Go 同名测试。
// 它覆盖 alter table add vector index 的错误、正常创建、show create、rename/drop/recreate、多 schema change 和匿名命名。
#[test]
fn test_add_vector_index_simple() {
    let (store, dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(tiflashReplicaLease, vec![mockstore::WithMockTiFlash(2)]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t, pt;");
    let tiflash = infosync::NewMockTiFlash();
    infosync::SetMockTiFlash(tiflash);
    // Go defer 关闭 mock TiFlash status server；这里只保留资源收尾语义。

    exec_script(&tk, &[
        "create table pt(a int, b vector, c int) PARTITION BY RANGE ( a ) (PARTITION p0 VALUES LESS THAN (6), PARTITION p1 VALUES LESS THAN (11), PARTITION p2 VALUES LESS THAN (21));",
    ]);
    tk.MustContainErrMsg("alter table pt add vector index idx((vec_cosine_distance(b))) USING HNSW;", "Unsupported add columnar index: unsupported partition table");
    exec_script(&tk, &[
        "create table t (a int, b vector, c vector(3), d vector(4));",
    ]);
    tk.MustContainErrMsg("alter table t add vector index idx((VEC_COSINE_DISTANCE(b))) USING HNSW COMMENT 'b comment';", "columnar replica must exist");
    tk.MustExec("alter table t set tiflash replica 2 location labels 'a','b';");
    for (sql, code_or_msg) in vector_index_error_cases() {
        expect_error_code_or_message(&tk, sql, code_or_msg);
    }
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess", "return(1)");
    exec_vector_index_normal_cases(&tk, &dom);
}

// TestAddColumnarIndexSimple 对应 Go 同名测试。
// 它与 vector index 用例平行，覆盖 inverted columnar index 的错误、元数据、DDL history、show create 和匿名命名。
#[test]
fn test_add_columnar_index_simple() {
    let (store, dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(tiflashReplicaLease, vec![mockstore::WithMockTiFlash(2)]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t, pt;");
    let tiflash = infosync::NewMockTiFlash();
    infosync::SetMockTiFlash(tiflash);

    exec_script(&tk, &[
        "create table pt(a int, b vector, c int) PARTITION BY RANGE ( a ) (PARTITION p0 VALUES LESS THAN (6), PARTITION p1 VALUES LESS THAN (11), PARTITION p2 VALUES LESS THAN (21));",
    ]);
    tk.MustContainErrMsg("alter table pt add columnar index idx(c) USING INVERTED;", "Unsupported add columnar index: unsupported partition table");
    tk.MustExec("create table t (a int, b vector(4), c int, d char(4));");
    tk.MustContainErrMsg("alter table t add columnar index idx(a) USING INVERTED COMMENT 'b comment';", "columnar replica must exist");
    tk.MustExec("alter table t set tiflash replica 2 location labels 'a','b';");
    for (sql, code_or_msg) in columnar_index_error_cases() {
        expect_error_code_or_message(&tk, sql, code_or_msg);
    }
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess", "return(1)");
    exec_columnar_index_normal_cases(&tk, &dom);
}

// testAddColumnarIndexRollback 对应 Go 同名辅助函数。
// 该函数同时服务 vector index 和 columnar index rollback，验证 TiFlash 同步失败、admin cancel、TiFlash 检查错误和正常完成四个路径。
fn test_add_columnar_index_rollback(prepare_sql: Vec<&str>, add_idx_sql: &str) {
    let (store, _dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(tiflashReplicaLease, vec![mockstore::WithMockTiFlash(2)]);
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t;");
    let limit = vardef::GetDDLErrorCountLimit();
    vardef::SetDDLErrorCountLimit(5);
    for sql in prepare_sql {
        tk.MustExec(sql);
    }
    ddl::SetWaitTimeWhenErrorOccurred(Duration::from_millis(100));

    let check_rollback_info = |expect_state: model::JobState| {
        let jobs = get_jobs_by_sql(tk.Session(), "tidb_ddl_history", "order by job_id desc limit 1")
            .expect("Go require.NoError: get DDL history");
        let curr_job = &jobs[0];
        require::equal(curr_job.Type, model::ActionAddColumnarIndex);
        require::equal(curr_job.State, expect_state);
        let (element, start, end, physical_id, err) =
            ddl::NewReorgHandlerForTest(testkit::NewTestKit(&store).Session()).GetDDLReorgHandle(curr_job);
        require::true_(meta::ErrDDLReorgElementNotExist.Equal(err));
        require::nil(element);
        require::nil(start);
        require::nil(end);
        require::equal(physical_id, 0);
    };

    tk.MustGetErrMsg(add_idx_sql, "[ddl:-1]DDL job rollback, error msg: MockTiFlash is not accessible");
    check_rollback_info(model::JobStateRollbackDone);

    let tiflash = infosync::NewMockTiFlash();
    infosync::SetMockTiFlash(tiflash);
    let tk1 = testkit::NewTestKit(&store);
    tk1.MustExec("use test");
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess", "return(0)");
    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", |job: model::Job| {
        // Go 在第二次进入 StateWriteReorganization 时 admin cancel 当前 DDL job。
        tk1.MustQuery(format!("admin cancel ddl jobs {}", job.ID));
    });
    tk.MustGetErrMsg(add_idx_sql, "[ddl:8214]Cancelled DDL job");
    tk.MustQuery("select count(1) from t1;").Check(testkit::Rows("4"));
    check_rollback_info(model::JobStateRollbackDone);

    testfailpoint::Disable("github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced");
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess", "return(-1)");
    tk.MustContainErrMsg(add_idx_sql, "[ddl:9014]TiFlash backfill index failed: mock a check error");
    check_rollback_info(model::JobStateRollbackDone);

    testfailpoint::Enable("github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess", "return(4)");
    tk.MustExec(add_idx_sql);
    check_rollback_info(model::JobStateSynced);
    testfailpoint::Disable("github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess");
    vardef::SetDDLErrorCountLimit(limit);
}

// TestAddVectorIndexRollback 对应 Go 同名测试。
#[test]
fn test_add_vector_index_rollback() {
    test_add_columnar_index_rollback(vec![
        "create table t1 (c1 int, b vector, c vector(3), unique key(c1));",
        "alter table t1 set tiflash replica 2 location labels 'a','b';",
        "insert into t1 values (1, '[1,6.6]', '[1,8.88,9.99]'), (2, '[2,6.6]', '[2,8.88,9.99]'), (3, '[3,6.6]', '[3,8.88,9.99]'), (4, '[4,6.6]', '[4,8.88,9.99]')",
    ], "alter table t1 add vector index v_idx((VEC_COSINE_DISTANCE(c))) USING HNSW COMMENT 'b comment';");
}

// TestAddColumnarIndexRollback 对应 Go 同名测试。
#[test]
fn test_add_columnar_index_rollback_case() {
    test_add_columnar_index_rollback(vec![
        "create table t1 (c1 int, b int, c vector(3), unique key(c1));",
        "alter table t1 set tiflash replica 2 location labels 'a','b';",
        "insert into t1 values (1, 1, '[1,8.88,9.99]'), (2, 2, '[2,8.88,9.99]'), (3, 3, '[3,8.88,9.99]'), (4, 4, '[4,8.88,9.99]')",
    ], "alter table t1 add columnar index c_idx(b) USING INVERTED COMMENT 'b comment';");
}

// TestInsertDuplicateBeforeIndexMerge 对应 Go 同名测试。
// 它在 beforeBackfillMerge failpoint 中插入/更新重复行，覆盖 partition + global/local unique index merge 前的重复处理。
#[test]
fn test_insert_duplicate_before_index_merge() {
    if kerneltype::IsNextGen() {
        skip("add-index always runs on DXF with ingest mode in nextgen");
    }
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    let tk2 = testkit::NewTestKit(&store);
    exec_script(&tk2, &[
        "set @@global.tidb_ddl_enable_fast_reorg = 1",
        "set @@global.tidb_enable_dist_task=0",
        "use test",
    ]);
    tk.MustExec("use test");
    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge", || {
        tk2.MustExec("insert ignore into t values (1, 2), (1, 2) on duplicate key update col1 = 0, col2 = 0");
    });
    exec_script(&tk, &[
        "drop table if exists t",
        "create table t (col1 int, col2 int, unique index i1(col2) /*T![global_index] GLOBAL */
) PARTITION BY HASH (col1) PARTITIONS 2",
        "alter table t add unique index i2(col1, col2)",
        "admin check table t",
        "drop table if exists t",
        "create table t (col1 int, col2 int, unique index i1(col1, col2)) PARTITION BY HASH (col1) PARTITIONS 2",
"alter table t add unique index i2(col2) /*T![global_index] GLOBAL */
",
        "admin check table t",
    ]);
}

// exec_vector_index_normal_cases 承载 Go TestAddVectorIndexSimple 的正常路径和后续 DDL 变更。
// 这里集中保留 show create table、DDL history row count、cleanup index 错误、visibility、modify、rename/drop/recreate 和匿名命名断言。
fn exec_vector_index_normal_cases(tk: &testkit::TestKit, dom: &domain::Domain) {
    exec_script(tk, &[
        "drop table if exists t;",
        "create table t (a int, b vector(3));",
        "alter table t set tiflash replica 2 location labels 'a','b';",
        "insert into t values (1, '[1,2.1,3.3]');",
    ]);
    tk.MustQuery("SELECT * FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE WHERE table_name = 't'").Check(testkit::Rows());
    assert_no_indexes(dom, "test", "t");
    tk.MustExec("alter table t add vector index idx((VEC_COSINE_DISTANCE(b))) USING HNSW COMMENT 'b comment';");
    assert_vector_index_meta(dom, "test", "t", 2, &["idx"]);
    assert_last_add_columnar_job(tk, 1);
    exec_script(tk, &[
        "admin check table t",
        "admin check index t idx",
    ]);
    tk.MustContainErrMsg("admin cleanup index t idx", "columnar index `idx` is not supported for cleanup index");
    tk.MustContainErrMsg("alter table t drop column b;", "can't drop column b with Columnar Index covered now");
    tk.MustContainErrMsg("alter table t add index idx2(a), add vector index idx3((vec_l2_distance(b))) USING HNSW COMMENT 'b comment'", "Unsupported multi schema change for add columnar index");
    tk.MustContainErrMsg("alter table t alter index idx invisible", "[ddl:8200]INVISIBLE can not be used in VECTOR INDEX");
    tk.MustQuery("select distinct index_name, is_visible from information_schema.statistics where table_schema = 'test' and table_name = 't' order by index_name").Check(testkit::Rows("idx YES"));
    exec_script(tk, &[
        "alter table t alter index idx visible",
        "alter table t modify column b vector(3) not null",
        "alter table t rename index idx to vecIdx",
        "alter table t drop index vecIdx;",
        "create vector index idx on t ((VEC_COSINE_DISTANCE(b))) USING HNSW COMMENT 'b comment';",
        "alter table t add index idx2(a)",
        "alter table t drop index idx, drop index idx2",
        "admin check table t",
        "alter table t add vector index ((vec_l2_distance(b))) USING HNSW;",
        "alter table t add key vector_index_2(a);",
        "alter table t add vector index ((VEC_COSINE_DISTANCE(b))) USING HNSW;",
    ]);
    tk.MustContainErrMsg("alter table t modify column b vector(2)", "[ddl:8200]Unsupported modify column: columnar indexes on the column");
    assert_vector_anonymous_names(dom, "test", "t", &["vector_index", "vector_index_2", "vector_index_3"]);
}

// exec_columnar_index_normal_cases 承载 Go TestAddColumnarIndexSimple 的正常路径和后续 DDL 变更。
fn exec_columnar_index_normal_cases(tk: &testkit::TestKit, dom: &domain::Domain) {
    exec_script(tk, &[
        "drop table if exists t;",
        "create table t (a int, b int);",
        "alter table t set tiflash replica 2 location labels 'a','b';",
        "insert into t values (1, 2);",
    ]);
    tk.MustQuery("SELECT * FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE WHERE table_name = 't'").Check(testkit::Rows());
    assert_no_indexes(dom, "test", "t");
    tk.MustExec("alter table t add columnar index idx(a) USING INVERTED COMMENT 'a comment';");
    assert_columnar_index_meta(dom, "test", "t", 2, "idx");
    assert_last_add_columnar_job(tk, 1);
    exec_script(tk, &[
        "admin check table t",
        "admin check index t idx",
    ]);
    tk.MustContainErrMsg("admin cleanup index t idx", "columnar index `idx` is not supported for cleanup index");
    tk.MustContainErrMsg("alter table t drop column a;", "can't drop column a with Columnar Index covered now");
    tk.MustContainErrMsg("alter table t add index idx2(a), add columnar index idx3(b) USING INVERTED COMMENT 'b comment'", "Unsupported multi schema change for add columnar index");
    tk.MustContainErrMsg("alter table t alter index idx invisible", "[ddl:8200]INVISIBLE can not be used in INVERTED INDEX");
    tk.MustQuery("select distinct index_name, is_visible from information_schema.statistics where table_schema = 'test' and table_name = 't' order by index_name").Check(testkit::Rows("idx YES"));
    exec_script(tk, &[
        "alter table t alter index idx visible",
        "alter table t modify column a int not null",
        "alter table t rename index idx to colIdx",
        "alter table t drop index colIdx;",
        "create columnar index idx on t (b) USING INVERTED COMMENT 'b comment';",
        "alter table t add index idx2(a)",
        "alter table t drop index idx, drop index idx2",
        "admin check table t",
        "alter table t add columnar index (b) USING INVERTED;",
        "alter table t add key a(a);",
        "alter table t add columnar index (a) USING INVERTED;",
    ]);
    tk.MustContainErrMsg("alter table t modify column a smallint", "[ddl:8200]Unsupported modify column: columnar indexes on the column");
    assert_columnar_anonymous_names(dom, "test", "t", &["b", "a", "a_2"]);
}

// vector_index_error_cases 对应 Go TestAddVectorIndexSimple 中的错误用例清单。
fn vector_index_error_cases() -> Vec<(&'static str, ErrorExpectation)> {
    vec![
        ("alter table t add key idx(a) USING HNSW;", ErrorExpectation::Message("[ddl:8200]'USING HNSW' can be only used for VECTOR INDEX")),
        ("alter table t add vector index ((vec_cosine_distance(n))) USING HNSW;", ErrorExpectation::Message("[schema:1054]Unknown column 'n' in 't'")),
        ("alter table t add vector index ((vec_cosine_distance(a))) USING HNSW;", ErrorExpectation::Code(errno::ErrUnsupportedDDLOperation)),
        ("alter table t add vector index ((vec_cosine_distance(a,'[1,2.1,3.3]'))) USING HNSW;", ErrorExpectation::Message("Unsupported add vector index: only support vector type, but this is type: int(11)")),
        ("alter table t add vector index ((vec_l1_distance(b))) USING HNSW;", ErrorExpectation::Code(errno::ErrUnsupportedDDLOperation)),
        ("alter table t add vector index ((vec_negative_inner_product(b))) USING HNSW;", ErrorExpectation::Code(errno::ErrUnsupportedDDLOperation)),
        ("alter table t add vector index ((lower(b))) USING HNSW;", ErrorExpectation::Code(errno::ErrUnsupportedDDLOperation)),
        ("alter table t add vector index idx((vec_cosine_distance(c))) USING HNSW;", ErrorExpectation::Code(errno::ErrDupKeyName)),
    ]
}

// columnar_index_error_cases 对应 Go TestAddColumnarIndexSimple 中的错误用例清单。
fn columnar_index_error_cases() -> Vec<(&'static str, ErrorExpectation)> {
    vec![
        ("alter table t add key idx(d) USING INVERTED;", ErrorExpectation::Message("[ddl:8200]'USING INVERTED' can be only used for COLUMNAR INDEX")),
        ("alter table t add columnar index (n) USING INVERTED;", ErrorExpectation::Message("[schema:1054]Unknown column 'n' in 't'")),
        ("alter table t add columnar index (b) USING INVERTED;", ErrorExpectation::Message("only support integer type, but this is type")),
        ("alter table t add columnar index (d) USING INVERTED;", ErrorExpectation::Message("only support integer type, but this is type")),
        ("alter table t add columnar index idx(c) USING INVERTED;", ErrorExpectation::Code(errno::ErrDupKeyName)),
    ]
}

// ErrorExpectation 是辅助枚举，用于保留 Go 中 MustGetErrCode 与 MustContainErrMsg 的差异。
enum ErrorExpectation {
    Code(errno::Errno),
    Message(&'static str),
}

fn exec_script(tk: &testkit::TestKit, sqls: &[&str]) {
    for sql in sqls {
        // 带有 “Go 循环” 的字符串表示原 Go 代码中该处是 for-range 批量执行。
        tk.MustExec(sql);
    }
}

fn expect_error(tk: &testkit::TestKit, sql: &str) {
    require::error(tk.ExecToErr(sql));
}

fn expect_error_code_or_message(tk: &testkit::TestKit, sql: &str, expect: ErrorExpectation) {
    match expect {
        ErrorExpectation::Code(code) => tk.MustGetErrCode(sql, code),
        ErrorExpectation::Message(msg) => tk.MustContainErrMsg(sql, msg),
    }
}

fn poll_ddl_and_mutate<F>(tk: &testkit::TestKit, done: channel::Receiver<errors::Error>, interval: Duration, limit: i32, mut mutate: F)
where
    F: FnMut(&testkit::TestKit, i32),
{
    let ticker = time::NewTicker(interval);
    let mut num = 0;
    loop {
        select! {
            err = done.recv() => {
                if err.is_none() {
                    break;
                }
                require::no_error(err);
            }
            _ = ticker.C() => {
                if num >= limit {
                    break;
                }
                for i in num..num + 20 {
                    mutate(tk, i);
                }
                num += 20;
            }
        }
    }
}

fn execute_in_background_and_wait(tk: &testkit::TestKit, sql: &str, timeout: Duration) {
    let done = channel::bounded::<errors::Error>(1);
    go(|| {
        let (_, err) = tk.Session().Execute(context::Background(), sql);
        done.send(err);
    });
    select! {
        err = done.recv() => require::no_error(err),
        _ = time::After(timeout) => fail("DDL did not finish in expected time"),
    }
}

fn check_global_index_row_case(tk: &testkit::TestKit, table_name: &str, index_name: &str, unique: bool) {
    let tbl = external::GetTableByName(tk, "test", table_name);
    let tbl_info = tbl.Meta();
    let index_info = tbl_info.FindIndexByName(index_name);
    require::not_nil(index_info);
    require::true_(index_info.Global);
    require::equal(index_info.Unique, unique);
    // Go 分别检查 p0/p1 两行；这里把两个 pid、idxVals、rowVals 的构造语义集中保留。
    check_global_index_row(tk.Session(), tbl_info, index_info, tbl_info.Partition.Definitions[0].ID, vec![types::NewDatum(1)], vec![types::NewDatum(1), types::NewDatum(1)]);
    check_global_index_row(tk.Session(), tbl_info, index_info, tbl_info.Partition.Definitions[1].ID, vec![types::NewDatum(2)], vec![types::NewDatum(2), types::NewDatum(11)]);
}

fn assert_no_index_named(tk: &testkit::TestKit, db: &str, tbl: &str, idx_name: &str) {
    let table = external::GetTableByName(tk, db, tbl);
    for idx in table.Indices() {
        require::false_(strings::EqualFold(idx.Meta().Name.L, idx_name));
    }
}

fn assert_index_exists(tk: &testkit::TestKit, db: &str, tbl: &str, idx_name: &str) {
    let table = external::GetTableByName(tk, db, tbl);
    require::true_(table.Indices().any(|idx| strings::EqualFold(idx.Meta().Name.L, idx_name)));
}

fn assert_index_names(tk: &testkit::TestKit, db: &str, tbl: &str, expected: &[&str]) {
    let table = external::GetTableByName(tk, db, tbl);
    for (idx, name) in expected.iter().enumerate() {
        require::equal(table.Indices()[idx].Meta().Name.L, *name);
    }
}

fn assert_no_indexes(dom: &domain::Domain, db: &str, tbl: &str) {
    let table = dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl)).unwrap();
    require::equal(table.Meta().Indices.len(), 0);
}

fn assert_vector_index_meta(dom: &domain::Domain, db: &str, tbl: &str, replica_cnt: u64, names: &[&str]) {
    let table = dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl)).unwrap();
    require::equal(table.Meta().TiFlashReplica.Count, replica_cnt);
    for (idx, name) in names.iter().enumerate() {
        require::equal(table.Meta().Indices[idx].Tp, ast::IndexTypeVector);
        require::equal(table.Meta().Indices[idx].Name.O, *name);
    }
}

fn assert_columnar_index_meta(dom: &domain::Domain, db: &str, tbl: &str, replica_cnt: u64, name: &str) {
    let table = dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl)).unwrap();
    require::equal(table.Meta().TiFlashReplica.Count, replica_cnt);
    require::equal(table.Meta().Indices[0].Tp, ast::IndexTypeInverted);
    require::equal(table.Meta().Indices[0].Name.O, name);
}

fn assert_last_add_columnar_job(tk: &testkit::TestKit, row_count: i64) {
    let jobs = get_jobs_by_sql(tk.Session(), "tidb_ddl_history", "order by job_id desc limit 1").unwrap();
    require::equal(jobs.len(), 1);
    require::equal(jobs[0].Type, model::ActionAddColumnarIndex);
    require::equal(jobs[0].RowCount, row_count);
}

fn assert_vector_anonymous_names(dom: &domain::Domain, db: &str, tbl: &str, names: &[&str]) {
    let table = dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl)).unwrap();
    require::equal(table.Meta().Indices.len(), names.len());
    for (idx, name) in names.iter().enumerate() {
        require::equal(table.Meta().Indices[idx].Name.O, *name);
    }
}

fn assert_columnar_anonymous_names(dom: &domain::Domain, db: &str, tbl: &str, names: &[&str]) {
    let table = dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl)).unwrap();
    require::equal(table.Meta().Indices.len(), names.len());
    for (idx, name) in names.iter().enumerate() {
        require::equal(table.Meta().Indices[idx].Name.O, *name);
    }
}

fn assert_stats_analyzed(dom: &domain::Domain, db: &str, tbl: &str, cols: &[&str], idxs: &[&str]) {
    let table = dom
        .InfoSchema()
        .TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl))
        .expect("Go require.NoError: reload table info");
    let stats_table = dom
        .StatsHandle()
        .StatsCache
        .Get(table.Meta().ID)
        .expect("Go require.True: stats table exists");

    let mut versions = Vec::new();
    for col_name in cols {
        let col_info = table.Meta().FindPublicColumnByName(*col_name);
        require::not_nil(col_info);
        require::true_(stats_table.ColAndIdxExistenceMap.Has(col_info.ID, false));
        let col_hist = stats_table.HistColl.GetCol(col_info.ID);
        require::not_nil(col_hist);
        versions.push(col_hist.Histogram.LastUpdateVersion);
    }
    for idx_name in idxs {
        let idx_info = table.Meta().FindIndexByName(*idx_name);
        require::not_nil(idx_info);
        require::true_(stats_table.ColAndIdxExistenceMap.Has(idx_info.ID, true));
        let idx_hist = stats_table.HistColl.GetIdx(idx_info.ID);
        require::not_nil(idx_hist);
        versions.push(idx_hist.Histogram.LastUpdateVersion);
    }
    // Go 重点检查同一轮 analyze 产生的列与索引 LastUpdateVersion 对齐。
    for pair in versions.windows(2) {
        require::equal(pair[0], pair[1]);
    }
}

fn assert_stats_reanalyzed_after_modify(dom: &domain::Domain, db: &str, tbl: &str, col_a: &str, col_b: &str, idx_a: &str, idx_b: &str) {
    let table = dom
        .InfoSchema()
        .TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl))
        .expect("Go require.NoError: reload table after modify column");
    let stats_table = dom
        .StatsHandle()
        .StatsCache
        .Get(table.Meta().ID)
        .expect("Go require.True: stats table exists after modify");

    let a_info = table.Meta().FindPublicColumnByName(col_a);
    let b_info = table.Meta().FindPublicColumnByName(col_b);
    let idx_info = table.Meta().FindIndexByName(idx_a);
    let idx_b_info = table.Meta().FindIndexByName(idx_b);
    for (id, is_index) in [
        (a_info.ID, false),
        (b_info.ID, false),
        (idx_info.ID, true),
        (idx_b_info.ID, true),
    ] {
        require::true_(stats_table.ColAndIdxExistenceMap.Has(id, is_index));
    }

    let col_a_ver = stats_table.HistColl.GetCol(a_info.ID).Histogram.LastUpdateVersion;
    let col_b_ver = stats_table.HistColl.GetCol(b_info.ID).Histogram.LastUpdateVersion;
    let idx_a_ver = stats_table.HistColl.GetIdx(idx_info.ID).Histogram.LastUpdateVersion;
    let idx_b_ver = stats_table.HistColl.GetIdx(idx_b_info.ID).Histogram.LastUpdateVersion;
    // Go 原测试同时断言 a 列和 idx(a) 版本相同，b 列和 idx_b 版本相同，并且修改后两组版本一致。
    require::equal(col_a_ver, idx_a_ver);
    require::equal(col_b_ver, idx_b_ver);
    require::equal(col_a_ver, col_b_ver);
    require::equal(idx_a_ver, idx_b_ver);
}

fn assert_partition_add_index_not_analyzed(dom: &domain::Domain, db: &str, tbl: &str, col: &str) {
    let table = dom
        .InfoSchema()
        .TableByName(context::Background(), ast::NewCIStr(db), ast::NewCIStr(tbl))
        .expect("Go require.NoError: reload partition table");
    let col_info = table.Meta().FindPublicColumnByName(col);
    require::not_nil(col_info);
    let idx_info = table.Meta().FindIndexByName("idx");
    require::not_nil(idx_info);

    let stats_table = dom
        .StatsHandle()
        .StatsCache
        .Get(table.Meta().ID)
        .expect("Go require.True: partition stats table exists");
    require::true_(stats_table.ColAndIdxExistenceMap.Has(col_info.ID, false));
    require::not_nil(stats_table.HistColl.GetCol(col_info.ID));
    // Go 这里明确要求 partition 表 add index with analyze 被禁止，所以新增 idx 没有分析记录。
    require::false_(stats_table.ColAndIdxExistenceMap.Has(col_info.ID, true));
}
*/

// 引入 Rust 侧 DDL 执行器的核心类型：
// - Executor：DDL 执行器，负责 create/drop schema、table、index 等操作；
// - MemoryJobBackend：内存版 DDL job（DDL 任务元数据）存储后端，用于测试；
// - SessionContext：会话上下文，携带执行 DDL 所需的会话状态；
// - Ident：库名 + 表名的限定标识符。
use crate::executor::{
    ColumnInfo, DdlAction, Executor, ExecutorError, Ident, IndexInfo, MemoryJobBackend, OnExist,
    SessionContext, TableInfo,
};
use std::time::Duration;

/// 验证索引的校验与重命名行为和 Go 版本的错误语义一致：
/// - 在不存在的列上建索引返回 `ColumnNotFound`；
/// - 索引名大小写不敏感，重复创建返回 `IndexExists`；
/// - rename 后旧名失效，drop 旧名返回 `IndexNotFound`，drop 新名成功。
#[test]
fn index_validation_and_rename_match_go_errors() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 先搭一个最小 schema/table 环境，让后续索引校验专注验证错误语义而非建表前置条件。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("t", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    let ident = Ident::new("test", "t");
    // 在不存在的列 "missing" 上建索引，应报列不存在错误。
    assert!(matches!(
        ddl.create_index(
            &mut session,
            &ident,
            IndexInfo::new("bad", vec!["missing".into()]),
            false
        ),
        Err(ExecutorError::ColumnNotFound(_))
    ));
    ddl.create_index(
        &mut session,
        &ident,
        IndexInfo::new("idx", vec!["a".into()]),
        false,
    )
    .unwrap();
    // 索引名比较不区分大小写："IDX" 与已存在的 "idx" 视为重复。
    assert!(matches!(
        ddl.create_index(
            &mut session,
            &ident,
            IndexInfo::new("IDX", vec!["a".into()]),
            false
        ),
        Err(ExecutorError::IndexExists(_))
    ));
    ddl.rename_index(&mut session, &ident, "idx", "renamed")
        .unwrap();
    // 重命名后旧索引名不再存在，按旧名删除应失败。
    assert!(matches!(
        ddl.drop_index(&mut session, &ident, "idx", false),
        Err(ExecutorError::IndexNotFound(_))
    ));
    // 用新名字删除成功，证明 rename 已同步更新内部索引目录。
    ddl.drop_index(&mut session, &ident, "renamed", false)
        .unwrap();
}

/// Go 的 rename-index 校验只把“另一个索引”占用目标名视为重复；因此仅改变
/// 当前索引名的大小写必须成功，并且仍要提交 RenameIndex job。
#[test]
fn case_only_index_rename_matches_go() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("t", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    let ident = Ident::new("test", "t");
    ddl.create_index(
        &mut session,
        &ident,
        IndexInfo::new("inDex", vec!["a".into()]),
        false,
    )
    .unwrap();

    ddl.rename_index(&mut session, &ident, "inDex", "IndEX")
        .unwrap();

    let table = &ddl.schemas["test"].tables["t"];
    assert_eq!("IndEX", table.indexes[0].name);
    assert_eq!(
        DdlAction::RenameIndex,
        ddl.backend().history().last().unwrap().action
    );
}

/// 验证不允许创建 INVISIBLE（不可见，优化器忽略）的主键索引：
/// 主键是行定位的核心索引，MySQL/TiDB 均禁止将其设为不可见。
#[test]
fn invisible_primary_key_is_rejected() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 与上面的用例一样，先创建单列表，隔离“主键不可见”这一条校验规则。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("t", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    // 构造一个同时标记 primary 与 invisible 的索引定义，触发校验错误。
    let mut index = IndexInfo::new("PRIMARY", vec!["a".into()]);
    index.primary = true;
    index.invisible = true;
    // 该错误应在语义检查阶段直接返回，不需要真的执行建索引流程。
    assert!(matches!(
        ddl.create_index(&mut session, &Ident::new("test", "t"), index, false),
        Err(ExecutorError::InvisiblePrimaryKey)
    ));
}
