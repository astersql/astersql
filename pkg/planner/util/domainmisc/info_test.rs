// Copyright 2026 AsterSQL.

// 验证最新索引信息查询与 Go 实现保持一致。
//
// 测试覆盖 Domain 缺失、Schema 版本未变的快速路径，以及版本变化后查表、
// 表消失、表 ID 传递等分支，并明确区分“无需刷新”和“刷新后无索引”。

use std::cell::Cell;

use crate::info::{IndexInfo, LatestSchema, get_latest_index_info};

/// 可配置的 Schema 桩，同时记录是否实际触发了按表查询。
struct MockSchema {
    version: i64,
    indexes: Option<Vec<IndexInfo>>,
    /// 用内部可变性观测版本未变时应跳过的查询，而不改变 trait 的只读接口。
    table_lookup_calls: Cell<usize>,
    queried_table_id: Cell<Option<i64>>,
}

impl LatestSchema for MockSchema {
    fn schema_version(&self) -> i64 {
        self.version
    }

    fn table_indexes(&self, table_id: i64) -> Option<Vec<IndexInfo>> {
        self.table_lookup_calls
            .set(self.table_lookup_calls.get() + 1);
        self.queried_table_id.set(Some(table_id));
        self.indexes.clone()
    }
}

fn index(id: i64, name: &str) -> IndexInfo {
    IndexInfo {
        id,
        name: name.into(),
        columns: vec!["c".into()],
        public: true,
    }
}

#[test]
fn missing_domain_returns_the_go_error() {
    let error = get_latest_index_info(None, 42, 7).unwrap_err();

    assert_eq!(error, "domain not found for ctx");
}

#[test]
fn unchanged_schema_skips_the_table_lookup() {
    let schema = MockSchema {
        version: 7,
        indexes: Some(vec![index(1, "idx")]),
        table_lookup_calls: Cell::new(0),
        queried_table_id: Cell::new(None),
    };

    let result = get_latest_index_info(Some(&schema), 42, 7).unwrap();

    assert_eq!(result, (None, false));
    assert_eq!(schema.table_lookup_calls.get(), 0);
    assert_eq!(schema.queried_table_id.get(), None);
}

#[test]
fn changed_schema_returns_indexes_keyed_by_id_with_last_value_winning() {
    let schema = MockSchema {
        version: 8,
        indexes: Some(vec![index(1, "old"), index(2, "other"), index(1, "new")]),
        table_lookup_calls: Cell::new(0),
        queried_table_id: Cell::new(None),
    };

    // 与 Go map 赋值语义一致：重复索引 ID 应由列表中最后出现的值覆盖。
    let (indexes, changed) = get_latest_index_info(Some(&schema), 42, 7).unwrap();
    let indexes = indexes.expect("changed schema must return a map");

    assert!(changed);
    assert_eq!(schema.table_lookup_calls.get(), 1);
    assert_eq!(schema.queried_table_id.get(), Some(42));
    assert_eq!(indexes.len(), 2);
    assert_eq!(indexes.get(&1).map(|info| info.name.as_str()), Some("new"));
    assert_eq!(
        indexes.get(&2).map(|info| info.name.as_str()),
        Some("other")
    );
}

#[test]
fn changed_schema_with_missing_table_returns_an_empty_map() {
    let schema = MockSchema {
        version: 8,
        indexes: None,
        table_lookup_calls: Cell::new(0),
        queried_table_id: Cell::new(None),
    };

    // 表已消失仍代表完成过刷新，因此返回空映射和 changed=true，而不是 None。
    let (indexes, changed) = get_latest_index_info(Some(&schema), 42, 7).unwrap();

    assert!(changed);
    assert_eq!(indexes.expect("changed schema must return a map").len(), 0);
    assert_eq!(schema.table_lookup_calls.get(), 1);
    assert_eq!(schema.queried_table_id.get(), Some(42));
}
