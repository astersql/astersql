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

use std::sync::Arc;

fn raw_binding(sql_digest: &str, bind_sql: &str) -> Arc<crate::Binding> {
    Arc::new(crate::Binding {
        BindSQL: bind_sql.to_owned(),
        SQLDigest: sql_digest.to_owned(),
        Status: crate::StatusEnabled.to_owned(),
        ..crate::Binding::default()
    })
}

#[test]
fn cross_db_digest_map_tracks_add_remove_and_deduplicates() {
    let b1 = raw_binding("b1", "SELECT * FROM db1.t1");
    let b2 = raw_binding("b2", "SELECT * FROM db2.t1");
    let b3 = raw_binding("b3", "SELECT * FROM db2.t3");
    let digest1 = crate::noDBDigestFromBinding(&b1).unwrap();
    let digest3 = crate::noDBDigestFromBinding(&b3).unwrap();
    assert_eq!(digest1, crate::noDBDigestFromBinding(&b2).unwrap());

    let cache = crate::newBindingCache(i64::MAX);
    for binding in [&b1, &b2, &b3] {
        cache
            .SetBinding(binding.SQLDigest.clone(), Arc::clone(binding))
            .unwrap();
    }
    assert_eq!(cache.Size(), 3);

    let map = crate::newDigestBiMap();
    map.Add(digest1.clone(), "b1".to_owned());
    map.Add(digest1.clone(), "b2".to_owned());
    map.Add(digest3.clone(), "b3".to_owned());
    map.Add(digest1.clone(), "b2".to_owned());
    assert_eq!(map.NoDBDigest2SQLDigest(&digest1).len(), 2);
    assert_eq!(map.NoDBDigest2SQLDigest(&digest3), vec!["b3"]);
    assert_eq!(map.SQLDigest2NoDBDigest("b2"), Some(digest1.clone()));

    map.Del("b2");
    assert_eq!(map.NoDBDigest2SQLDigest(&digest1), vec!["b1"]);
    assert_eq!(map.SQLDigest2NoDBDigest("b2"), None);
    assert_eq!(map.All().len(), 2);

    cache.RemoveBinding("b2");
    assert!(cache.GetBinding("b2").is_none());
    assert_eq!(cache.Size(), 2);
}

#[test]
fn duplicate_sets_replace_without_growing_usage_or_size() {
    let cache = crate::newBindingCache(i64::MAX);
    let bindings = [
        raw_binding("db1", "SELECT * FROM db1.t1"),
        raw_binding("db2", "SELECT * FROM db2.t1"),
        raw_binding("db3", "SELECT * FROM db3.t1"),
    ];
    for binding in &bindings {
        cache
            .SetBinding(binding.SQLDigest.clone(), Arc::clone(binding))
            .unwrap();
    }
    let usage = cache.GetMemUsage();
    for binding in &bindings {
        cache
            .SetBinding(binding.SQLDigest.clone(), Arc::clone(binding))
            .unwrap();
    }
    assert_eq!(cache.Size(), 3);
    assert_eq!(cache.GetMemUsage(), usage);
}

#[test]
fn invalid_binding_sql_is_rejected_without_mutating_cache() {
    let cache = crate::newBindingCache(i64::MAX);
    let invalid = raw_binding("invalid", "select * from");
    assert!(cache.SetBinding("invalid".to_owned(), invalid).is_err());
    assert_eq!(cache.Size(), 0);
    assert_eq!(cache.GetMemUsage(), 0);
}

#[test]
fn collect_table_names_matches_go_cases() {
    let cases = [
        (
            "select /*+ HASH_JOIN(t1, t2) */ * from t1 t1 join t1 t2 on t1.a=t2.a where t1.b is not null;",
            vec!["t1", "t1"],
        ),
        ("select * from t", vec!["t"]),
        ("select * from t1, t2, t3;", vec!["t1", "t2", "t3"]),
        (
            "select * from t1 where t1.a > (select max(a) from t2);",
            vec!["t1", "t2"],
        ),
        (
            "select * from t1 where t1.a > (select max(a) from t2 where t2.a > (select max(a) from t3));",
            vec!["t1", "t2", "t3"],
        ),
        (
            "select a,b,c,d,* from t1 where t1.a > (select max(a) from t2 where t2.a > (select max(a) from t3));",
            vec!["t1", "t2", "t3"],
        ),
    ];
    for (sql, expected) in cases {
        let statement = crate::Statement {
            SQL: sql.to_owned(),
            ..crate::Statement::default()
        };
        let names: Vec<_> = crate::CollectTableNames(&statement)
            .into_iter()
            .map(|table| table.Name)
            .collect();
        assert_eq!(names, expected, "sql: {sql}");
    }
}

/// 测试辅助函数：按给定的 SQL 摘要与原始 SQL 构造一条绑定。
///
/// 绑定的 `BindSQL` 固定为带 `use_index` 提示的 SQL（提示优化器走指定索引），
/// 状态置为“启用”（enabled），并用 `Arc` 包装以便在缓存与断言之间共享。
fn binding(sql_digest: &str, original_sql: &str) -> std::sync::Arc<crate::Binding> {
    std::sync::Arc::new(crate::Binding {
        OriginalSQL: original_sql.to_owned(),
        BindSQL: "select /*+ use_index(t, idx) */ * from t".to_owned(),
        Status: crate::StatusEnabled.to_owned(),
        SQLDigest: sql_digest.to_owned(),
        ..crate::Binding::default()
    })
}

/// 验证绑定缓存的核心生命周期：写入、按容量淘汰、关闭清空。
///
/// 测试流程：
/// 1. 以恰好能容纳两条绑定的内存容量创建缓存，写入两条绑定并确认缓存大小为 2；
/// 2. 把内存上限（mem capacity）缩小到只够一条绑定，触发淘汰，缓存大小降为 1，
///    且保留的是较新写入的 "digest-2"（体现按访问新旧淘汰的策略）；
/// 3. 关闭缓存后所有条目被清空，缓存大小归零。
#[test]
fn canonical_binding_cache_sets_evicts_and_closes() {
    let first = binding("digest-1", "select * from t where a = 1");
    let second = binding("digest-2", "select * from t where a = 2");
    // 容量取两条绑定内存占用之和（向上取整），保证初始时两条都能放入缓存。
    let capacity = first.size().ceil() as i64 + second.size().ceil() as i64;
    let cache = crate::newBindingCache(capacity);
    cache
        .SetBinding(first.SQLDigest.clone(), std::sync::Arc::clone(&first))
        .unwrap();
    cache
        .SetBinding(second.SQLDigest.clone(), std::sync::Arc::clone(&second))
        .unwrap();
    assert_eq!(cache.Size(), 2);
    // 缩小内存上限到仅容纳一条绑定，应立即触发淘汰。
    cache.SetMemCapacity(first.size().ceil() as i64);
    assert_eq!(cache.Size(), 1);
    assert!(cache.GetBinding("digest-2").is_some());
    cache.Close();
    assert_eq!(cache.Size(), 0);
}
