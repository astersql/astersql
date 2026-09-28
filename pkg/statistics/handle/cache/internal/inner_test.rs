// Copyright 2026 AsterSQL.

// `StatsCacheInner` 统计缓存内层接口的契约测试。
//
// 用基于 `HashMap` 的简易实现验证 trait 对象安全（可放入 `Box<dyn …>`），
// 以及 Put/Get 后表对象通过 `Arc` 共享、Copy 能独立复制条目数。

use super::*;
use statistics::Table;
use std::collections::HashMap;
use std::sync::Arc;

/// 测试用无淘汰缓存：表 ID → 统计表（`Table`）的映射。
#[derive(Default)]
struct TestCache(HashMap<i64, Arc<Table>>);

/// 实现 `StatsCacheInner`：读写下沉到内部 HashMap，代价恒为 0，容量相关操作为空。
impl StatsCacheInner for TestCache {
    fn Get(&self, tid: i64) -> Option<Arc<Table>> {
        self.0.get(&tid).cloned()
    }
    fn Put(&mut self, tid: i64, table: Arc<Table>) -> bool {
        self.0.insert(tid, table);
        true
    }
    fn Del(&mut self, tid: i64) {
        self.0.remove(&tid);
    }
    fn Cost(&self) -> i64 {
        0
    }
    fn Values(&self) -> Vec<Arc<Table>> {
        self.0.values().cloned().collect()
    }
    fn Len(&self) -> usize {
        self.0.len()
    }
    fn Copy(&self) -> Box<dyn StatsCacheInner> {
        Box::new(Self(self.0.clone()))
    }
    fn SetCapacity(&mut self, _capacity: i64) {}
    fn Close(&mut self) {}
    fn TriggerEvict(&mut self) {}
    fn WaitForAsyncUpdates(&mut self) {}
}

/// 校验 trait 可对象化装箱，且 Get 返回与 Put 相同的 Arc 表指针。
#[test]
fn cache_contract_is_object_safe_and_keeps_shared_tables() {
    let table = Arc::new(Table::New(7, 0, 0));
    // 以 dyn 形式持有，证明接口可对象安全使用
    let mut cache: Box<dyn StatsCacheInner> = Box::new(TestCache::default());
    assert!(cache.Put(7, table.clone()));
    assert!(Arc::ptr_eq(&cache.Get(7).unwrap(), &table));
    assert_eq!(cache.Len(), 1);
    assert_eq!(cache.Copy().Len(), 1);
}
