// Copyright 2026 AsterSQL.

// MapCache 与 Go mapCache 语义对齐的契约测试。
//
// 验证 Put 替换同一键、Len/Get 指针一致性、Copy 独立长度，以及 Del 后成本归零。

use super::*;
use cache_internal::StatsCacheInner;
use statistics::Table;
use std::sync::Arc;

/// Put 两次同一键后应保留替换表，Copy 长度独立，Del 后 Get 为空且 Cost 为 0。
#[test]
fn put_replace_copy_and_delete_match_the_go_map_cache() {
    let first = Arc::new(Table::New(1, 1, 0));
    let replacement = Arc::new(Table::New(1, 2, 0));
    let mut cache = NewMapCache();
    assert!(cache.Put(1, first));
    assert!(cache.Put(1, replacement.clone()));
    assert_eq!(cache.Len(), 1);
    assert!(Arc::ptr_eq(&cache.Get(1).unwrap(), &replacement));
    assert_eq!(cache.Copy().Len(), 1);
    cache.Del(1);
    assert_eq!(cache.Len(), 0);
    assert!(cache.Get(1).is_none());
    assert_eq!(cache.Cost(), 0);
}
