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

// `common` 模块单元测试：按表结构选取自增分配器，以及 rebase / 取最大 base。

use crate::{
    Allocator, AllocatorType, AutoIDRequirement, CommonError, Context, GetGlobalAutoIDAlloc,
    GetMaxAutoIDBase, RebaseTableAllocators, TableInfo,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// 测试用分配器：用原子计数模拟全局 next，rebase 时写为 base+1。
struct MockAllocator {
    typ: AllocatorType,
    next: Arc<AtomicI64>,
}

impl Allocator for MockAllocator {
    fn NextGlobalAutoID(&self) -> Result<i64, CommonError> {
        Ok(self.next.load(Ordering::SeqCst))
    }

    fn GetType(&self) -> AllocatorType {
        self.typ
    }

    fn Rebase(&self, _ctx: &Context, base: i64, _alloc_ids: bool) -> Result<(), CommonError> {
        self.next.store(base + 1, Ordering::SeqCst);
        Ok(())
    }
}

/// 测试用 AutoIDRequirement：按 (table_id, 类型) 复用同一原子 next。
#[derive(Clone, Default)]
struct MockRequirement {
    allocators: Arc<Mutex<HashMap<(i64, AllocatorType), Arc<AtomicI64>>>>,
}

impl AutoIDRequirement for MockRequirement {
    fn StoreAvailable(&self) -> bool {
        true
    }

    fn NewAllocator(
        &self,
        _db_id: i64,
        table_id: i64,
        _unsigned: bool,
        allocator_type: AllocatorType,
        _cache_step: u64,
        _table_version: u16,
    ) -> Arc<dyn Allocator> {
        let mut guard = self.allocators.lock().expect("allocator map poisoned");
        let next = guard
            .entry((table_id, allocator_type))
            .or_insert_with(|| Arc::new(AtomicI64::new(1)))
            .clone();
        Arc::new(MockAllocator {
            typ: allocator_type,
            next,
        })
    }
}

struct UnavailableRequirement;

impl AutoIDRequirement for UnavailableRequirement {
    fn StoreAvailable(&self) -> bool {
        false
    }

    fn NewAllocator(
        &self,
        _db_id: i64,
        _table_id: i64,
        _unsigned: bool,
        _allocator_type: AllocatorType,
        _cache_step: u64,
        _table_version: u16,
    ) -> Arc<dyn Allocator> {
        panic!("an unavailable store must not create allocators")
    }
}

/// 构造带指定自增标志的 `TableInfo` 辅助函数。
fn table_info(
    id: i64,
    name: &str,
    has_auto_row_id: bool,
    has_auto_increment: bool,
    separate_auto_increment: bool,
    has_auto_random: bool,
) -> TableInfo {
    TableInfo {
        ID: id,
        Name: name.to_owned(),
        Version: 0,
        HasAutoRowID: has_auto_row_id,
        HasAutoIncrement: has_auto_increment,
        HasAutoRandom: has_auto_random,
        SeparateAutoIncrement: separate_auto_increment,
        AutoIncrementUnsigned: false,
        AutoRandomUnsigned: false,
    }
}

/// 覆盖无自增、仅 RowID、独立 AutoIncrement、仅 AutoRandom 等组合。
#[test]
fn test_alloc_global_auto_id() {
    let requirement = MockRequirement::default();
    // (table_id, 表信息, 期望错误子串, 期望分配器类型列表)
    let cases = [
        (
            11,
            table_info(11, "t11", false, false, false, false),
            "has no auto ID",
            Vec::<AllocatorType>::new(),
        ),
        (
            12,
            table_info(12, "t12", false, false, true, false),
            "has no auto ID",
            Vec::new(),
        ),
        (
            21,
            table_info(21, "t21", true, false, false, false),
            "",
            vec![AllocatorType::RowID],
        ),
        (
            22,
            table_info(22, "t22", true, false, true, false),
            "",
            vec![AllocatorType::RowID],
        ),
        (
            31,
            table_info(31, "t31", false, true, false, false),
            "",
            vec![AllocatorType::RowID],
        ),
        (
            32,
            table_info(32, "t32", false, true, true, false),
            "",
            vec![AllocatorType::AutoIncrement, AllocatorType::RowID],
        ),
        (
            41,
            table_info(41, "t41", true, true, false, false),
            "",
            vec![AllocatorType::RowID],
        ),
        (
            42,
            table_info(42, "t42", true, true, true, false),
            "",
            vec![AllocatorType::AutoIncrement, AllocatorType::RowID],
        ),
        (
            51,
            table_info(51, "t51", false, false, false, true),
            "",
            vec![AllocatorType::AutoRandom],
        ),
    ];

    for (table_id, ti, expect_err, expect_types) in cases {
        let result = GetGlobalAutoIDAlloc(Some(&requirement), 1, &ti);
        if !expect_err.is_empty() {
            match result {
                Err(err) => {
                    assert!(
                        err.to_string().contains(expect_err),
                        "table_id={table_id} err={err}"
                    )
                }
                Ok(_) => panic!("table_id={table_id} expected auto id error"),
            }
            continue;
        }
        let allocator_types = result
            .unwrap()
            .into_iter()
            .map(|alloc| alloc.GetType())
            .collect::<Vec<_>>();
        assert_eq!(expect_types, allocator_types, "table_id={table_id}");
    }
}

#[test]
fn test_alloc_global_auto_id_validates_requirement_and_db_id() {
    let ti = table_info(21, "t21", true, false, false, false);

    let err = GetGlobalAutoIDAlloc(None, 1, &ti)
        .err()
        .expect("nil requirement");
    assert_eq!(err.Message, "internal error: kv store should not be nil");

    let err = GetGlobalAutoIDAlloc(Some(&UnavailableRequirement), 1, &ti)
        .err()
        .expect("unavailable store");
    assert_eq!(err.Message, "internal error: kv store should not be nil");

    let requirement = MockRequirement::default();
    let err = GetGlobalAutoIDAlloc(Some(&requirement), 0, &ti)
        .err()
        .expect("zero db id");
    assert_eq!(err.Message, "internal error: dbID should not be 0");
}

/// 验证按类型选择性 rebase，以及 GetMaxAutoIDBase 取各分配器最大 base。
#[test]
fn test_rebase_table_allocators() {
    let requirement = MockRequirement::default();
    let ti = table_info(42, "t42", true, true, true, false);
    let allocators = GetGlobalAutoIDAlloc(Some(&requirement), 1, &ti).unwrap();
    assert_eq!(allocators.len(), 2);
    for alloc in &allocators {
        assert_eq!(alloc.NextGlobalAutoID().unwrap(), 1_i64);
    }
    let max_auto_id_base = GetMaxAutoIDBase(Some(&requirement), 1, &ti).unwrap();
    assert_eq!(max_auto_id_base, 0);

    let ctx = Context::Background();
    let mut allocator_types = Vec::with_capacity(allocators.len());
    for alloc in &allocators {
        alloc.Rebase(&ctx, 123, false).unwrap();
        allocator_types.push(alloc.GetType());
    }
    assert_eq!(
        allocator_types,
        vec![AllocatorType::AutoIncrement, AllocatorType::RowID]
    );

    // 空 map：不改变任何分配器。
    RebaseTableAllocators(&ctx, &HashMap::new(), Some(&requirement), 1, &ti).unwrap();
    for alloc in &allocators {
        assert_eq!(alloc.NextGlobalAutoID().unwrap(), 124);
    }
    let max_auto_id_base = GetMaxAutoIDBase(Some(&requirement), 1, &ti).unwrap();
    assert_eq!(max_auto_id_base, 123);

    // 仅 rebase AutoIncrement。
    RebaseTableAllocators(
        &ctx,
        &HashMap::from([(AllocatorType::AutoIncrement, 223)]),
        Some(&requirement),
        1,
        &ti,
    )
    .unwrap();
    assert_eq!(allocators[0].NextGlobalAutoID().unwrap(), 224);
    assert_eq!(allocators[1].NextGlobalAutoID().unwrap(), 124);
    let max_auto_id_base = GetMaxAutoIDBase(Some(&requirement), 1, &ti).unwrap();
    assert_eq!(max_auto_id_base, 223);

    // 同时 rebase 两种类型。
    RebaseTableAllocators(
        &ctx,
        &HashMap::from([
            (AllocatorType::AutoIncrement, 323),
            (AllocatorType::RowID, 423),
        ]),
        Some(&requirement),
        1,
        &ti,
    )
    .unwrap();
    assert_eq!(allocators[0].NextGlobalAutoID().unwrap(), 324);
    assert_eq!(allocators[1].NextGlobalAutoID().unwrap(), 424);
    let max_auto_id_base = GetMaxAutoIDBase(Some(&requirement), 1, &ti).unwrap();
    assert_eq!(max_auto_id_base, 423);
}
