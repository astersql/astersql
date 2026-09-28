// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// `allocator` 模块单元测试：验证初始 base、并发 Rebase 取最大值、以及不会回退。

use std::sync::Arc;
use std::thread;

use crate::*;

/// 覆盖 Allocators 按类型取 base、多线程 Rebase 后 base 为最大值、以及更小值 Rebase 无效。
#[test]
fn TestAllocator() {
    let allocators = NewPanickingAllocatorsWithBase(false, 1, 2, 3);
    assert_eq!(allocators.Get(AllocatorType::AutoRandomType).Base(), 1);
    assert_eq!(allocators.Get(AllocatorType::AutoIncrementType).Base(), 2);
    let allocator = allocators.Get(AllocatorType::RowIDAllocType);
    // 16 个线程分别 Rebase 到 0..15，最终 base 应为 15。
    let threads = (0..16)
        .map(|value| {
            let allocator = Arc::clone(&allocator);
            thread::spawn(move || allocator.Rebase(value, false))
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(allocator.Base(), 15);
    // 用更小的值 rebase 不应降低 base。
    allocator.Rebase(4, false);
    assert_eq!(allocator.Base(), 15);
    assert_eq!(allocator.GetType(), AllocatorType::RowIDAllocType);
}
