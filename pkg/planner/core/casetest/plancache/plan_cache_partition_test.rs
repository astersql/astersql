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

// 分区裁剪模式与脏分区 ID 对计划缓存键隔离的用例。
//
// 分区裁剪（partition prune）按谓词只访问相关分区；`dynamic`/`static` 两种裁剪模式
// 下计划形态可能不同，因此必须进入缓存键。脏表/脏分区 ID 集合表示统计或元数据已
// 变更的表，同样参与键计算；集合内顺序不应影响键的相等性（通常会规范化排序）。

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

fn new_testkit() -> TestKit {
    TestKit::new(CreateMockStoreAndDomain().0)
}

fn assert_cache(tk: &mut TestKit, expected: &str) {
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&[expected]));
}

/// 对齐 Go `TestPlanCachePartitionSuite` 的分区 PointGet 与 BatchPointGet 复用。
#[test]
fn prepared_partition_point_get_and_batch_point_get_rebuild_safely() {
    let mut tk = new_testkit();
    tk.MustExec(
        "create table t (a int primary key, b varchar(255), c varchar(255), key (b)) partition by range (a) (partition p_neg values less than (0), partition p0 values less than (1000000), partition p1m values less than (2000000))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (-1,NULL,NULL),(0,0,0),(1,1,1),(1000000,1000000,1000000),(1999999,1999999,1999999)",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec(
        "prepare point_stmt from 'select a,c,b from t where a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @a=2000000", Vec::new());
    tk.MustQuery("execute point_stmt using @a", Vec::new())
        .Check(Rows(&[]));
    assert_cache(&mut tk, "0");
    tk.MustExec("set @a=1999999", Vec::new());
    tk.MustQuery("execute point_stmt using @a", Vec::new())
        .Check(Rows(&["1999999 1999999 1999999"]));
    assert_cache(&mut tk, "1");

    tk.MustExec(
        "prepare batch_stmt from 'select a,c,b from t where a in (?,?,?)'",
        Vec::new(),
    );
    tk.MustExec("set @a=1999999,@b=0,@c=-1", Vec::new());
    tk.MustQuery("execute batch_stmt using @a,@b,@c", Vec::new())
        .Sort()
        .Check(Rows(&[
            "-1 <nil> <nil>",
            "0 0 0",
            "1999999 1999999 1999999",
        ]));
    assert_cache(&mut tk, "0");
    tk.MustQuery("execute batch_stmt using @a,@b,@c", Vec::new());
    assert_cache(&mut tk, "1");
}

/// 对齐 Go `runPreparedPlanCachePartitionIndex`：参数切换到其他分区仍复用安全计划。
#[test]
fn prepared_partition_index_cache_crosses_partitions() {
    let mut tk = new_testkit();
    tk.MustExec("create table tp (b varchar(255), a int primary key nonclustered, key (b)) partition by key(a) partitions 3", Vec::new());
    tk.MustExec(
        "insert into tp values ('Ab',1),('abc',2),('BC',3),('AC',4),('BA',5),('cda',6)",
        Vec::new(),
    );
    tk.MustExec("analyze table tp", Vec::new());
    tk.MustExec(
        "prepare stmt from 'select * from tp where a in (?,?,?)'",
        Vec::new(),
    );
    tk.MustExec("set @a=1,@b=3,@c=4", Vec::new());
    tk.MustQuery("execute stmt using @a,@b,@c", Vec::new())
        .Sort()
        .Check(Rows(&["AC 4", "Ab 1", "BC 3"]));
    assert_cache(&mut tk, "0");
    tk.MustQuery("execute stmt using @a,@b,@c", Vec::new());
    assert_cache(&mut tk, "1");
    tk.MustExec("set @a=2,@b=5,@c=4", Vec::new());
    tk.MustQuery("execute stmt using @a,@b,@c", Vec::new())
        .Sort()
        .Check(Rows(&["AC 4", "BA 5", "abc 2"]));
    assert_cache(&mut tk, "1");
}

/// 对齐 Go `runNonPreparedPlanCachePartitionIndex` 的非预处理语句分区集变化。
#[test]
fn non_prepared_partition_index_rebuilds_only_for_a_new_partition_set() {
    let mut tk = new_testkit();
    tk.MustExec("set @@tidb_enable_non_prepared_plan_cache=1", Vec::new());
    tk.MustExec("create table tn (b varchar(255), a int primary key nonclustered, key (b)) partition by key(a) partitions 3", Vec::new());
    tk.MustExec(
        "insert into tn values ('Ab',1),('abc',2),('BC',3),('AC',4),('BA',5),('cda',6)",
        Vec::new(),
    );
    tk.MustQuery("select * from tn where a in (2,1,4,1,1,5,5)", Vec::new())
        .Sort()
        .Check(Rows(&["AC 4", "Ab 1", "BA 5", "abc 2"]));
    assert_cache(&mut tk, "0");
    tk.MustQuery("select * from tn where a in (2,1,4,1,1,5,5)", Vec::new());
    assert_cache(&mut tk, "1");
    tk.MustQuery("select * from tn where a in (1,3,4)", Vec::new())
        .Sort()
        .Check(Rows(&["AC 4", "Ab 1", "BC 3"]));
    assert_cache(&mut tk, "0");
    tk.MustQuery("select * from tn where a in (1,3,4)", Vec::new());
    assert_cache(&mut tk, "1");
}

/// 对齐 Go `TestPlanCacheFixControlRebuild` 的 Fix33031 安全退化。
#[test]
fn fix_control_rejects_partitioned_cached_point_get() {
    let mut tk = new_testkit();
    tk.MustExec("create table tf (a int primary key, b varchar(255), key (b)) partition by hash(a) partitions 5", Vec::new());
    tk.MustExec(
        "insert into tf values(0,0),(1,1),(2,2),(3,3),(4,4)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare stmt from 'select * from tf where a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @a=2", Vec::new());
    tk.MustQuery("execute stmt using @a", Vec::new());
    tk.MustExec("set @a=3", Vec::new());
    tk.MustQuery("execute stmt using @a", Vec::new());
    assert_cache(&mut tk, "1");
    tk.MustExec("set @@tidb_opt_fix_control='33031:ON'", Vec::new());
    tk.MustExec("set @a=1", Vec::new());
    tk.MustQuery("execute stmt using @a", Vec::new())
        .Check(Rows(&["1 1"]));
    tk.MustQuery("show warnings", Vec::new())
        .CheckContain("Fix33031 fix-control set and partitioned table");
    assert_cache(&mut tk, "0");
}

/// 对齐 Go `TestPreparedStmtPartitionUnion` 的 static prune 不可缓存契约。
#[test]
fn static_partition_union_stays_uncacheable() {
    let mut tk = new_testkit();
    tk.MustExec(
        "create table tu (a int, b int, unique key (a)) partition by hash(a) partitions 3",
        Vec::new(),
    );
    for value in 0..100 {
        tk.MustExec(
            "insert into tu values (?, ?)",
            vec![value.into(), value.into()],
        );
    }
    tk.MustExec("set tidb_partition_prune_mode='static'", Vec::new());
    tk.MustQuery(
        "select b from tu where a=1 or a=10 or a=10 or a=999999",
        Vec::new(),
    )
    .Sort()
    .Check(Rows(&["1", "10"]));
    tk.MustExec(
        "prepare stmt from 'select b from tu where a=1 or a=10 or a=10 or a=999999'",
        Vec::new(),
    );
    tk.MustQuery("execute stmt", Vec::new())
        .Sort()
        .Check(Rows(&["1", "10"]));
    assert_cache(&mut tk, "0");
    tk.MustQuery("execute stmt", Vec::new());
    tk.MustQuery("show warnings", Vec::new())
        .CheckContain("partitioned tables is un-cacheable");
    assert_cache(&mut tk, "0");
}

/// 验证：`partition_prune_mode` 不同则缓存键不同；脏 ID 集合顺序不影响键相等。
#[test]
fn partition_prune_mode_and_dirty_partitions_isolate_cache_entries() {
    use astersql_planner_core::{NewPlanCacheKey, PlanCacheKeyContext, PlanCacheStmt, ast};

    let mut statement =
        PlanCacheStmt::<()>::new(ast::misc::Prepared::default(), "select * from orders");
    statement.SchemaVersion = 11;
    // dynamic 模式 + 含重复的脏表 ID 列表。
    let dynamic = PlanCacheKeyContext {
        partition_prune_mode: "dynamic".to_owned(),
        dirty_table_ids: vec![102, 101, 102],
        ..Default::default()
    };
    // static 模式 + 同集合（去重后应与 dynamic 的脏集合语义相同，但模式不同）。
    let static_mode = PlanCacheKeyContext {
        partition_prune_mode: "static".to_owned(),
        dirty_table_ids: vec![101, 102],
        ..Default::default()
    };
    let dynamic_key = NewPlanCacheKey(&dynamic, &statement).unwrap().key.unwrap();
    let static_key = NewPlanCacheKey(&static_mode, &statement)
        .unwrap()
        .key
        .unwrap();
    assert_ne!(dynamic_key.AsBytes(), static_key.AsBytes());

    // 脏 ID 顺序打乱后，与原 dynamic 键字节序列仍应相等。
    let reordered = PlanCacheKeyContext {
        partition_prune_mode: "dynamic".to_owned(),
        dirty_table_ids: vec![101, 102],
        ..Default::default()
    };
    assert_eq!(
        dynamic_key.AsBytes(),
        NewPlanCacheKey(&reordered, &statement)
            .unwrap()
            .key
            .unwrap()
            .AsBytes()
    );
}
