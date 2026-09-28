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

//! 跨数据库绑定测试，对应 `cross_db_binding_test.go`。
//!
//! 这里用轻量夹具覆盖通配 schema 的匹配、具体 schema 的优先级、绑定状态更新，
//! 以及提示解析和计划缓存场景，避免依赖完整会话与存储层。

use std::sync::Arc;

use astersql_bindinfo as bindinfo;
use astersql_testkit as testkit;

const CROSS_DB_DIGEST: &str = "cross-db-digest";

/// 构造共享 SQL 摘要、但 schema、状态和更新时间可控的绑定候选。
fn binding(
    id: &str,
    schema: &str,
    name: &str,
    status: &str,
    update: i64,
) -> Arc<bindinfo::Binding> {
    Arc::new(bindinfo::Binding {
        ID: id.to_owned(),
        SQLDigest: CROSS_DB_DIGEST.to_owned(),
        Status: status.to_owned(),
        UpdateTime: bindinfo::BindingTime(update),
        TableNames: vec![bindinfo::TableName {
            Schema: schema.to_owned(),
            Name: name.to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    })
}

#[test]
fn test_cross_db_binding_basic() {
    let (store, _domain) = testkit::mockstore::CreateMockStoreAndDomain();
    let mut setup = testkit::NewTestKit(store.clone());
    setup.MustExec("use test", Vec::new());
    setup.MustExec(
        "create table t (a int, b int, c int, d int, e int, key(a), key(b), key(c), key(d), key(e))",
        Vec::new(),
    );
    for db in ["test1", "test2"] {
        setup.MustExec(&format!("create database {db}"), Vec::new());
        setup.MustExec(&format!("use {db}"), Vec::new());
        setup.MustExec(
            "create table t (a int, b int, c int, d int, e int, key(a), key(b), key(c), key(d), key(e))",
            Vec::new(),
        );
    }

    for scope in ["", "global "] {
        let mut tk = testkit::NewTestKit(store.clone());
        for index in ["a", "b", "c", "d", "e"] {
            tk.MustExec("use test", Vec::new());
            tk.MustExec(
                &format!(
                    "create {scope}binding using select /*+ use_index(t, {index}) */ * from *.t"
                ),
                Vec::new(),
            );
            for current_db in ["test", "test1", "test2"] {
                tk.MustExec(&format!("use {current_db}"), Vec::new());
                for table_db in ["", "test.", "test1.", "test2."] {
                    tk.MustExec("set @@tidb_opt_enable_fuzzy_binding=1", Vec::new());
                    tk.MustUseIndex(&format!("select * from {table_db}t"), index);
                    tk.MustQuery("select @@last_plan_from_binding", Vec::new())
                        .Check(testkit::Rows(&["ON"]));
                    tk.MustExec("set @@tidb_opt_enable_fuzzy_binding=0", Vec::new());
                    tk.MustQuery(&format!("select * from {table_db}t"), Vec::new());
                    tk.MustQuery("select @@last_plan_from_binding", Vec::new())
                        .Check(testkit::Rows(&["OFF"]));
                }
            }
        }
    }
}

#[test]
fn test_cross_db_duplicated_binding() {
    let old = binding("old", "*", "t", bindinfo::StatusEnabled, 1);
    let new = binding("new", "*", "t", bindinfo::StatusEnabled, 2);
    // 相同摘要存在重复候选时，以更新时间较新的绑定替换旧缓存项。
    assert_eq!(
        bindinfo::pickCachedBinding(Some(old), [new]).unwrap().ID,
        "new"
    );
}

#[test]
fn test_cross_db_binding_priority() {
    let wildcard = binding("wildcard", "*", "t", bindinfo::StatusEnabled, 20);
    let specific = binding("specific", "app", "t", bindinfo::StatusEnabled, 10);
    let statement = [bindinfo::TableName {
        Name: "t".to_owned(),
        ..Default::default()
    }];
    // 具体 schema 比通配 schema 优先，即使通配候选的更新时间更晚。
    let (matched, hit) = bindinfo::crossDBMatchBindings("app", &statement, &[wildcard, specific]);
    assert!(hit);
    assert_eq!(matched.unwrap().ID, "specific");
}

#[test]
fn test_create_update_cross_db_binding() {
    let enabled = binding("enabled", "*", "t", bindinfo::StatusEnabled, 2);
    let disabled = binding("disabled", "*", "t", bindinfo::StatusDisabled, 3);
    // 状态变更也按更新时间覆盖缓存；禁用状态本身仍需保留，供后续同步判断。
    let selected = bindinfo::pickCachedBinding(Some(enabled.clone()), [disabled]);
    assert_eq!(selected.unwrap().ID, "disabled");
    let re_enabled = binding("re-enabled", "*", "t", bindinfo::StatusEnabled, 4);
    assert_eq!(
        bindinfo::pickCachedBinding(Some(enabled), [re_enabled])
            .unwrap()
            .ID,
        "re-enabled"
    );
    assert_eq!(
        bindinfo::pickCachedBinding(
            None,
            [binding("disabled", "*", "t", bindinfo::StatusDisabled, 3,)]
        )
        .unwrap()
        .Status,
        bindinfo::StatusDisabled
    );
}

#[test]
fn test_cross_db_binding_switch() {
    let universal = binding("universal", "*", "t", bindinfo::StatusEnabled, 1);
    let statement = [bindinfo::TableName {
        Name: "t".to_owned(),
        ..Default::default()
    }];
    assert!(bindinfo::crossDBMatchBindings("test1", &statement, &[universal.clone()]).1);
    // 开启跨库匹配只放宽 schema，表名不同仍不得命中。
    assert_eq!(
        bindinfo::crossDBMatchBindingTableName(
            "test1",
            &[bindinfo::TableName {
                Name: "other".to_owned(),
                ..Default::default()
            }],
            &universal.TableNames,
        ),
        (0, false)
    );
}

#[test]
fn test_cross_db_binding_set_var() {
    let (store, _domain) = testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = testkit::NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t (a int, b int, key(a), key(b))", Vec::new());
    tk.MustExec(
        "create global binding using select /*+ use_index(t, a) */ * from *.t",
        Vec::new(),
    );

    tk.MustExec("set @@tidb_opt_enable_fuzzy_binding=0", Vec::new());
    tk.MustQuery("select * from t", Vec::new());
    tk.MustQuery("select @@last_plan_from_binding", Vec::new())
        .Check(testkit::Rows(&["OFF"]));
    tk.MustQuery(
        "select /*+ set_var(tidb_opt_enable_fuzzy_binding=1) */ * from t",
        Vec::new(),
    );
    tk.MustQuery("select @@last_plan_from_binding", Vec::new())
        .Check(testkit::Rows(&["ON"]));

    tk.MustExec("set @@tidb_opt_enable_fuzzy_binding=1", Vec::new());
    tk.MustQuery("select * from t", Vec::new());
    tk.MustQuery("select @@last_plan_from_binding", Vec::new())
        .Check(testkit::Rows(&["ON"]));
    tk.MustQuery(
        "select /*+ set_var(tidb_opt_enable_fuzzy_binding=0) */ * from t",
        Vec::new(),
    );
    tk.MustQuery("select @@last_plan_from_binding", Vec::new())
        .Check(testkit::Rows(&["OFF"]));
}

#[test]
fn test_cross_db_binding_gc() {
    let enabled = binding("enabled", "*", "t", bindinfo::StatusEnabled, 1);
    let deleted = binding("deleted", "*", "t", bindinfo::StatusDeleted, 2);
    // 较新的删除标记应淘汰已有缓存，而不是成为可匹配绑定。
    assert!(bindinfo::pickCachedBinding(Some(enabled), [deleted]).is_none());
}

#[test]
fn test_cross_db_binding_in_list() {
    let statement = [bindinfo::TableName {
        Name: "t1".to_owned(),
        ..Default::default()
    }];
    let universal = binding("universal", "*", "t1", bindinfo::StatusEnabled, 1);
    let (matched, hit) = bindinfo::crossDBMatchBindings("test1", &statement, &[universal]);
    assert!(hit);
    // 命中后必须保留原绑定摘要，供含 IN 列表的归一化 SQL 关联同一绑定组。
    assert_eq!(matched.unwrap().SQLDigest, CROSS_DB_DIGEST);
}

#[test]
fn test_cross_db_binding_read_from_storage() {
    let mut item = bindinfo::Binding {
        BindSQL: "select /*+ read_from_storage(tikv[l]) */ l.id from *.ttt as l".to_owned(),
        ..Default::default()
    };
    // 接受所有 SQL 的验证器将测试范围限定在提示提取，避免引入会话校验依赖。
    struct Accept;
    impl bindinfo::BindingValidator for Accept {
        fn validate_binding_sql(&self, _sql: &str) -> bindinfo::Result<()> {
            Ok(())
        }
    }
    bindinfo::prepareHints(&Accept, &mut item).unwrap();
    assert_eq!(
        item.Hint.Hints,
        ["read_from_storage(@`sel_1` tikv[`*`.`l`])"]
    );
}

#[test]
fn test_cross_db_binding_plan_cache() {
    let first = binding("first", "*", "t", bindinfo::StatusEnabled, 1);
    let second = binding("second", "*", "t", bindinfo::StatusEnabled, 2);
    // 模拟绑定更新后刷新计划缓存：先选中最新候选，再验证其仍可跨库匹配。
    let selected = bindinfo::pickCachedBinding(Some(first), [second]).unwrap();
    assert_eq!(selected.ID, "second");
    let statement = [bindinfo::TableName {
        Name: "t".to_owned(),
        ..Default::default()
    }];
    assert!(bindinfo::crossDBMatchBindings("test2", &statement, &[selected]).1);
}

#[test]
fn canonical_cross_db_match_prefers_specific_schema_over_wildcard() {
    let wildcard = binding("wildcard", "*", "orders", bindinfo::StatusEnabled, 20);
    let specific = binding("specific", "app", "orders", bindinfo::StatusEnabled, 10);
    let statement = [bindinfo::TableName {
        Name: "orders".to_owned(),
        ..Default::default()
    }];
    // 规范回归用例再次锁定“具体 schema 优先于通配 schema”的选择规则。
    let (matched, hit) = bindinfo::crossDBMatchBindings("app", &statement, &[wildcard, specific]);
    assert!(hit);
    assert_eq!(matched.unwrap().ID, "specific");
}
