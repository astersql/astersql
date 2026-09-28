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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `prepare_db` 负责 准备 db。
// 中文总览：函数 `check_table_and_indexes` 负责 检查 table and indexes。
// 中文总览：函数 `test_add_index_on_system_table` 负责 添加 索引 on system 表。
// 中文总览：类型 `CollationAddIndexCase` 负责 CollationAddIndexCase。
// 中文总览：函数 `test_add_index_on_user_keyspace_with_different_new_collation` 负责 添加 索引 on user keyspace 携带 different 创建 collation。
// 中文总览：类型 `RestoreCollation` 负责 RestoreCollation。

//! Go-equivalent cross-keyspace add-index integration tests.
//!
//! The SQL actors below are the canonical `pkg/testkit` sessions backed by
//! `ConcreteSession`, `Domain`, and the in-memory KV implementation.  The only
//! deterministic boundary is the canonical multi-store test cluster, which
//! shares the same cross-keyspace coordinator used by successful Domain DDL.

use astersql_ddl::index::TaskKeyBuilder;
use astersql_testkit::mockstore::CreateCrossKeyspaceTestCluster;
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_tests_realtikvtest_addindextest1::serial_guard;
use astersql_util_collate::{NewCollationEnabled, SetNewCollationEnabledForTest};

// 该辅助函数负责 准备 db。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn prepare_db(tk: &mut TestKit, database: &str) {
    tk.MustExec(
        &format!("create database if not exists {database}"),
        Vec::new(),
    );
    tk.MustExec(&format!("use {database}"), Vec::new());
}

// 该辅助函数负责 检查 table and indexes。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn check_table_and_indexes(
    tk: &mut TestKit,
    table_name: &str,
    indexes: &[&str],
    expected_count: &str,
) {
    tk.MustExec(&format!("admin check table {table_name}"), Vec::new());
    tk.MustQuery(&format!("select count(*) from {table_name}"), Vec::new())
        .Check(Rows(&[expected_count]));
    for index_name in indexes {
        tk.MustQuery(
            &format!("select count(*) from {table_name} force index({index_name})"),
            Vec::new(),
        )
        .Check(Rows(&[expected_count]));
    }
}

/// Go `TestAddIndexOnSystemTable`.
// 该用例覆盖 添加 索引 on system 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_add_index_on_system_table() {
    let _serial = serial_guard();
    let cluster = CreateCrossKeyspaceTestCluster(&[("SYSTEM", true), ("keyspace1", false)]);
    let system_store = cluster.store("SYSTEM");
    let user_store = cluster.store("keyspace1");
    let mut tk = NewTestKit(user_store.clone());

    prepare_db(&mut tk, "crossks");
    tk.MustExec("create table t (a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 2)", Vec::new());
    tk.MustExec("alter table t add index idx_a (a)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    tk.MustQuery("select count(*) from t force index(idx_a)", Vec::new())
        .Check(Rows(&["1"]));

    let (_, table) = user_store
        .domain()
        .stats_table("crossks", "t")
        .expect("user-keyspace table");
    assert!(table.Indices.iter().any(|index| index.Name.L == "idx_a"));
    assert!(
        system_store.domain().stats_table("crossks", "t").is_none(),
        "user DDL metadata must not leak into SYSTEM keyspace"
    );

    let job_id = user_store
        .domain()
        .last_ddl_job_id_for_test()
        .expect("ADD INDEX DDL job ID");
    assert!(job_id > 0);
    let task_key = user_store
        .domain()
        .last_ddl_task_key_for_test()
        .expect("ADD INDEX distributed task key");
    assert_eq!(TaskKeyBuilder::new().build(job_id), task_key);
    assert_eq!(
        1,
        system_store
            .domain()
            .cross_task_count_for_key_for_test(&task_key)
    );
    assert_eq!(
        0,
        user_store
            .domain()
            .cross_task_count_for_key_for_test(&task_key)
    );
}

// 该类型围绕 CollationAddIndexCase 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
// 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
// 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
// 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。

struct CollationAddIndexCase {
    name: &'static str,
    table: &'static str,
    setup_sql: &'static [&'static str],
    add_index_sql: &'static [&'static str],
    indexes: &'static [&'static str],
    dml_sql: &'static [&'static str],
}

const COLLATION_CASES: &[CollationAddIndexCase] = &[
    CollationAddIndexCase {
        name: "clustered varchar primary key and secondary varchar index",
        table: "t_varchar_pk",
        setup_sql: &[
            "drop table if exists t_varchar_pk",
            "create table t_varchar_pk (id varchar(32) collate utf8mb4_general_ci, fk varchar(32) collate utf8mb4_general_ci, primary key (id) clustered)",
            "insert into t_varchar_pk values ('aaa', 'abc'), ('bbb', 'bbc'), ('ccc', 'cbc')",
        ],
        add_index_sql: &["alter table t_varchar_pk add index idx_fk(fk)"],
        indexes: &["idx_fk"],
        dml_sql: &[
            "insert into t_varchar_pk values ('ddd', 'dbc')",
            "update t_varchar_pk set fk = 'updated' where id = 'ddd'",
            "delete from t_varchar_pk where id = 'ddd'",
        ],
    },
    CollationAddIndexCase {
        name: "composite clustered primary key with varchar part and secondary int index",
        table: "t_composite_varchar_pk",
        setup_sql: &[
            "drop table if exists t_composite_varchar_pk",
            "create table t_composite_varchar_pk (id1 varchar(32) collate utf8mb4_general_ci, id2 int, fk int, primary key (id1, id2) clustered)",
            "insert into t_composite_varchar_pk values ('ax', 1, 10), ('by', 2, 20), ('cz', 3, 30)",
        ],
        add_index_sql: &["alter table t_composite_varchar_pk add index idx_fk(fk)"],
        indexes: &["idx_fk"],
        dml_sql: &[
            "insert into t_composite_varchar_pk values ('dw', 4, 40)",
            "update t_composite_varchar_pk set fk = 41 where id1 = 'dw' and id2 = 4",
            "delete from t_composite_varchar_pk where id1 = 'dw' and id2 = 4",
        ],
    },
    CollationAddIndexCase {
        name: "generated columns with string transformations",
        table: "t_add_generated_column_index",
        setup_sql: &[
            "drop table if exists t_add_generated_column_index",
            "create table t_add_generated_column_index (id varchar(32) collate utf8mb4_general_ci, raw varchar(32) collate utf8mb4_general_ci, g_lower varchar(32) generated always as (lower(raw)) virtual, g_upper varchar(32) generated always as (upper(raw)) virtual, g_concat varchar(80) generated always as (concat(id, ':', raw)) virtual, g_substr varchar(32) generated always as (substr(raw, 1, 2)) virtual, primary key (id) clustered)",
            "insert into t_add_generated_column_index(id, raw) values ('aaa', 'abc'), ('bbb', 'bbc'), ('ccc', 'cbc')",
        ],
        add_index_sql: &[
            "alter table t_add_generated_column_index add index idx_g_lower(g_lower)",
            "alter table t_add_generated_column_index add index idx_g_upper(g_upper)",
            "alter table t_add_generated_column_index add index idx_g_concat(g_concat)",
            "alter table t_add_generated_column_index add index idx_g_substr(g_substr)",
        ],
        indexes: &["idx_g_lower", "idx_g_upper", "idx_g_concat", "idx_g_substr"],
        dml_sql: &[
            "insert into t_add_generated_column_index(id, raw) values ('ddd', 'dbc')",
            "update t_add_generated_column_index set raw = 'updated' where id = 'ddd'",
            "delete from t_add_generated_column_index where id = 'ddd'",
        ],
    },
    CollationAddIndexCase {
        name: "expression indexes with string transformations",
        table: "t_add_expression_index",
        setup_sql: &[
            "drop table if exists t_add_expression_index",
            "create table t_add_expression_index (id varchar(32) collate utf8mb4_general_ci, raw varchar(32) collate utf8mb4_general_ci, primary key (id) clustered)",
            "insert into t_add_expression_index values ('aaa', 'abc'), ('bbb', 'bbc'), ('ccc', 'cbc')",
        ],
        add_index_sql: &[
            "alter table t_add_expression_index add index idx_lower ((lower(raw)))",
            "alter table t_add_expression_index add index idx_upper ((upper(raw)))",
            "alter table t_add_expression_index add index idx_concat ((concat(id, ':', raw)))",
            "alter table t_add_expression_index add index idx_substr ((substr(raw, 1, 2)))",
        ],
        indexes: &["idx_lower", "idx_upper", "idx_concat", "idx_substr"],
        dml_sql: &[
            "insert into t_add_expression_index values ('ddd', 'dbc')",
            "update t_add_expression_index set raw = 'updated' where id = 'ddd'",
            "delete from t_add_expression_index where id = 'ddd'",
        ],
    },
];

/// Go `TestAddIndexOnUserKeyspaceWithDifferentNewCollation`.
// 该用例覆盖 添加 索引 on user keyspace 携带 different 创建 collation。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_add_index_on_user_keyspace_with_different_new_collation() {
    let _serial = serial_guard();
    let original_collation = NewCollationEnabled();
    // 该类型围绕 RestoreCollation 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。

    struct RestoreCollation(bool);
    impl Drop for RestoreCollation {
        // 该辅助函数负责 收尾删除。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
        // 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
        // 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
        // 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

        fn drop(&mut self) {
            SetNewCollationEnabledForTest(self.0);
        }
    }
    let _restore = RestoreCollation(original_collation);

    let cluster = CreateCrossKeyspaceTestCluster(&[("SYSTEM", true), ("keyspacecollate", false)]);
    let user_store = cluster.store("keyspacecollate");
    let mut tk = NewTestKit(user_store.clone());
    SetNewCollationEnabledForTest(false);
    assert!(!NewCollationEnabled());
    prepare_db(&mut tk, "crossks_collate");

    let mut previous_job_id = 0;
    let mut backfill_init_count = 0;
    for case in COLLATION_CASES {
        for sql in case.setup_sql {
            tk.MustExec(sql, Vec::new());
        }
        for sql in case.add_index_sql {
            SetNewCollationEnabledForTest(false);
            tk.MustExec(sql, Vec::new());
            let job_id = user_store
                .domain()
                .last_ddl_job_id_for_test()
                .expect("ADD INDEX DDL job ID");
            assert!(
                job_id > previous_job_id,
                "{} did not submit a new DDL job for {:?}",
                case.name,
                sql
            );
            previous_job_id = job_id;
            let resolution = user_store
                .domain()
                .last_backfill_collation_for_test()
                .expect("backfill collation resolution");
            assert!(resolution.DefaultUseNewCollation);
            assert!(!resolution.UseNewCollation);
            assert!(!resolution.ReorgUseNewCollation);
            assert!(!NewCollationEnabled());
            backfill_init_count += 1;
        }

        let (_, table) = user_store
            .domain()
            .stats_table("crossks_collate", case.table)
            .expect("collation case table");
        for index in case.indexes {
            assert!(
                table
                    .Indices
                    .iter()
                    .any(|candidate| candidate.Name.L == *index),
                "{} missing index {index}",
                case.name
            );
        }
        check_table_and_indexes(&mut tk, case.table, case.indexes, "3");
        for sql in case.dml_sql {
            tk.MustExec(sql, Vec::new());
        }
        check_table_and_indexes(&mut tk, case.table, case.indexes, "3");
    }
    assert_eq!(10, backfill_init_count);
}

/// Go `TestCrossKSInfoSchemaSync`.
// 该用例覆盖 跨 keyspace info schema sync。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_cross_ks_info_schema_sync() {
    let _serial = serial_guard();
    let cluster = CreateCrossKeyspaceTestCluster(&[
        ("SYSTEM", true),
        ("keyspace1", false),
        ("keyspace2", false),
    ]);
    let system_store = cluster.store("SYSTEM");
    let ks1_store = cluster.store("keyspace1");
    let ks2_store = cluster.store("keyspace2");

    assert!(system_store.domain().cross_keyspaces_for_test().is_empty());
    assert_eq!(
        vec!["SYSTEM".to_owned()],
        ks1_store.domain().cross_keyspaces_for_test()
    );
    assert_eq!(
        vec!["SYSTEM".to_owned()],
        ks2_store.domain().cross_keyspaces_for_test()
    );

    let mut ks1_tk = NewTestKit(ks1_store.clone());
    prepare_db(&mut ks1_tk, "crossks");
    ks1_tk.MustExec("create table t (a int)", Vec::new());
    ks1_tk.MustExec("insert into t values (1)", Vec::new());
    ks1_tk.MustExec("alter table t add index idx_a (a)", Vec::new());
    assert_eq!(
        vec!["keyspace1".to_owned()],
        system_store.domain().cross_keyspaces_for_test()
    );

    // User tables only wait for their owning user-keyspace server.
    ks1_tk.MustExec("create table t1 (a int)", Vec::new());
    let summary = ks1_store
        .domain()
        .last_cross_sync_summary_for_test()
        .expect("ks1 user-table sync summary");
    assert_eq!((1, 0), (summary.ServerCount, summary.AssumedServerCount));

    let mut system_tk = NewTestKit(system_store.clone());
    prepare_db(&mut system_tk, "crossks");
    system_tk.MustExec("create table t (a int)", Vec::new());
    let summary = system_store
        .domain()
        .last_cross_sync_summary_for_test()
        .expect("SYSTEM user-table sync summary");
    assert_eq!((1, 0), (summary.ServerCount, summary.AssumedServerCount));

    // Once ks1 has issued user DDL, a system-table DDL from ks1 synchronizes
    // the SYSTEM server plus its assumed ks1 server.
    ks1_tk.MustExec("alter table mysql.user add index(file_priv)", Vec::new());
    let summary = ks1_store
        .domain()
        .last_cross_sync_summary_for_test()
        .expect("ks1 system-table sync summary");
    assert_eq!((2, 1), (summary.ServerCount, summary.AssumedServerCount));

    // ks2 has not issued a user-table DDL, so its system-table change only
    // waits for the local server.
    assert!(
        !system_store
            .domain()
            .cross_keyspaces_for_test()
            .contains(&"keyspace2".to_owned())
    );
    let mut ks2_tk = NewTestKit(ks2_store.clone());
    ks2_tk.MustExec("alter table mysql.user add index(file_priv)", Vec::new());
    let summary = ks2_store
        .domain()
        .last_cross_sync_summary_for_test()
        .expect("ks2 system-table sync summary");
    assert_eq!((1, 0), (summary.ServerCount, summary.AssumedServerCount));

    // SYSTEM system-table DDL reaches both initialized user runtimes.
    system_tk.MustExec("alter table mysql.user add index(file_priv)", Vec::new());
    let summary = system_store
        .domain()
        .last_cross_sync_summary_for_test()
        .expect("SYSTEM system-table sync summary");
    assert_eq!((3, 2), (summary.ServerCount, summary.AssumedServerCount));
}
