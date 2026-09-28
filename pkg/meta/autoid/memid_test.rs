// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Port of `pkg/meta/autoid/memid_test.go` (`TestInMemoryAlloc`).
//
// 内存 AutoID 分配器单元测试。AutoID（自增 ID）用于 `_tidb_rowid` / `AUTO_INCREMENT`；
// 覆盖有符号与无符号耗尽、rebase（抬高水位）以及从表元数据种子初始值等路径。

use crate::*;

/// Corresponds to Go `TestInMemoryAlloc`.
/// 验证内存分配器的顺序分配、批量步长、rebase 与溢出失败。
#[test]
fn test_in_memory_alloc() {
    // 有符号 AUTO_INCREMENT 表：默认从 1 起分配。
    let table = TableInfo {
        has_auto_increment_column: true,
        auto_increment_unsigned: false,
        ..TableInfo::default()
    };
    let alloc = new_allocator_from_temp_table_info(&table).expect("allocator");
    let ctx = Context::background();

    // 连续单步分配：next_global 与 alloc 返回的 end 应对齐。
    assert_eq!(alloc.next_global_auto_id().unwrap(), 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 1);
    assert_eq!(alloc.next_global_auto_id().unwrap(), 2);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 2);

    // n / increment / offset 组合：分别影响批量大小与对齐偏移。
    assert_eq!(alloc.alloc(&ctx, 10, 1, 1).unwrap().1, 12);
    assert_eq!(alloc.alloc(&ctx, 1, 10, 1).unwrap().1, 21);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 30).unwrap().1, 30);

    // rebase 抬高水位后分配；向更小值 rebase 不得回退已用 ID。
    alloc.rebase(&ctx, 40, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 41);
    assert_eq!(alloc.next_global_auto_id().unwrap(), 42);
    alloc.rebase(&ctx, 10, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 42);

    // 有符号接近 i64::MAX 时下一次分配应失败。
    alloc.rebase(&ctx, i64::MAX - 2, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, i64::MAX - 1);
    assert!(matches!(
        alloc.alloc(&ctx, 1, 1, 1),
        Err(AutoIdError::AutoIncrementReadFailed(_))
    ));

    // 无符号表：水位用 u64 语义解释，耗尽到 u64::MAX 附近。
    let unsigned_table = TableInfo {
        has_auto_increment_column: true,
        auto_increment_unsigned: true,
        ..TableInfo::default()
    };
    let alloc = new_allocator_from_temp_table_info(&unsigned_table).expect("unsigned allocator");
    let n = (u64::MAX - 2) as i64;
    alloc.rebase(&ctx, n, true).unwrap();
    assert_eq!(alloc.next_global_auto_id().unwrap() as u64, u64::MAX - 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1 as u64, u64::MAX - 1);
    assert!(matches!(
        alloc.alloc(&ctx, 1, 1, 1),
        Err(AutoIdError::AutoIncrementReadFailed(_))
    ));

    // 表元数据中已有 auto_increment_id 时，分配器应从该种子起步。
    let seeded = TableInfo {
        has_auto_increment_column: true,
        auto_increment_unsigned: true,
        auto_increment_id: 100,
        ..TableInfo::default()
    };
    let alloc = new_allocator_from_temp_table_info(&seeded).expect("seeded allocator");
    assert_eq!(alloc.next_global_auto_id().unwrap(), 100);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 100);
}

/// Go's in-memory allocator deliberately ignores the context passed to `Alloc`
/// and `Rebase`; temporary-table ID allocation must therefore still proceed
/// after cancellation.
#[test]
fn in_memory_allocator_ignores_canceled_context_like_go() {
    let alloc = InMemoryAllocator::new(false, AllocatorType::RowId);
    let ctx = Context::background();
    ctx.cancel();

    alloc.rebase(&ctx, 40, true).unwrap();
    assert_eq!(alloc.base(), 40);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap(), (40, 41));
}
