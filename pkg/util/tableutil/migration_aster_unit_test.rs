// Copyright 2026 AsterSQL.
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

// tableutil 迁移补充单元测试。
//
// 覆盖 TempTable 会话态字段读写，以及 `SetTempTableFromMeta` /
// `TempTableFromMeta` 与 Go 可赋值函数变量一致的未初始化 panic 与工厂替换。

use std::any::Any;
use std::sync::Arc;

use astersql_util_tableutil::{
    SetTempTableFromMeta, TempTable, TempTableFactory, TempTableFromMeta, autoid, model,
};

/// 测试用 autoID 分配器：固定返回可断言的 base/end/next。
struct TestAllocator;

impl autoid::Allocator for TestAllocator {
    fn alloc(
        &self,
        _ctx: &autoid::Context,
        _n: u64,
        _increment: i64,
        _offset: i64,
    ) -> autoid::Result<(i64, i64)> {
        Ok((0, 0))
    }

    fn alloc_seq_cache(&self) -> autoid::Result<(i64, i64, i64)> {
        Ok((0, 0, 0))
    }

    fn rebase(
        &self,
        _ctx: &autoid::Context,
        _new_base: i64,
        _alloc_ids: bool,
    ) -> autoid::Result<()> {
        Ok(())
    }

    fn force_rebase(&self, _new_base: i64) -> autoid::Result<()> {
        Ok(())
    }

    fn rebase_seq(&self, _new_base: i64) -> autoid::Result<(i64, bool)> {
        Ok((0, false))
    }

    fn transfer(&self, _database_id: i64, _table_id: i64) -> autoid::Result<()> {
        Ok(())
    }

    fn base(&self) -> i64 {
        41
    }

    fn end(&self) -> i64 {
        42
    }

    fn next_global_auto_id(&self) -> autoid::Result<i64> {
        Ok(42)
    }

    fn get_type(&self) -> autoid::AllocatorType {
        autoid::AllocatorType::RowId
    }
}

/// 简易 TempTable 实现，用于断言会话态字段与共享对象语义。
struct TestTempTable {
    allocator: Arc<dyn autoid::Allocator>,
    modified: bool,
    stats: Arc<dyn Any + Send + Sync>,
    size: i64,
    meta: Arc<model::TableInfo>,
}

impl TestTempTable {
    /// 用给定元数据构造测试临时表。
    fn new(meta: Arc<model::TableInfo>) -> Self {
        Self {
            allocator: Arc::new(TestAllocator),
            modified: false,
            stats: Arc::new(String::from("session stats")),
            size: 0,
            meta,
        }
    }
}

impl TempTable for TestTempTable {
    fn GetAutoIDAllocator(&self) -> Arc<dyn autoid::Allocator> {
        Arc::clone(&self.allocator)
    }

    fn SetModified(&mut self, modified: bool) {
        self.modified = modified;
    }

    fn GetModified(&self) -> bool {
        self.modified
    }

    fn GetStats(&self) -> Arc<dyn Any + Send + Sync> {
        Arc::clone(&self.stats)
    }

    fn GetSize(&self) -> i64 {
        self.size
    }

    fn SetSize(&mut self, size: i64) {
        self.size = size;
    }

    fn GetMeta(&self) -> &model::TableInfo {
        &self.meta
    }
}

/// 验证 TempTable 读写 modified/size/meta/autoID/stats。
#[test]
fn temp_table_preserves_go_session_state_and_shared_objects() {
    let meta = Arc::new(model::TableInfo {
        ID: 587,
        ..Default::default()
    });
    let mut table = TestTempTable::new(Arc::clone(&meta));

    assert!(!table.GetModified());
    table.SetModified(true);
    table.SetSize(68);
    assert!(table.GetModified());
    assert_eq!(table.GetSize(), 68);
    assert_eq!(table.GetMeta().ID, 587);
    assert_eq!(
        table.GetAutoIDAllocator().next_global_auto_id().unwrap(),
        42
    );
    assert_eq!(
        table
            .GetStats()
            .downcast_ref::<String>()
            .map(String::as_str),
        Some("session stats")
    );
}

/// 验证工厂未初始化 panic，以及注册后按 TableInfo 构造并恢复旧工厂。
#[test]
fn temp_table_factory_matches_go_assignable_function_variable() {
    let previous = SetTempTableFromMeta(None);
    let uninitialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TempTableFromMeta(Arc::new(model::TableInfo::default()))
    }));
    assert!(uninitialized.is_err());

    let factory: TempTableFactory = Arc::new(|meta| {
        let mut table = TestTempTable::new(meta);
        table.SetSize(17);
        Box::new(table)
    });
    SetTempTableFromMeta(Some(factory));

    let meta = Arc::new(model::TableInfo {
        ID: 2026,
        ..Default::default()
    });
    let table = TempTableFromMeta(Arc::clone(&meta));
    assert_eq!(table.GetMeta().ID, 2026);
    assert_eq!(table.GetSize(), 17);

    SetTempTableFromMeta(previous);
}
