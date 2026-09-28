// Copyright 2026 AsterSQL.

//! mockid 与 Go 公开契约的对等测试：顺序 Alloc、Rebase 空操作、并发唯一性。
//! 不引入真实存储；失败即表示 Rust 原子语义偏离 `mockid.go`。

use crate::{IDAllocator, NewIDAllocator};

/// Go atomic.AddUint64 wraps modulo 2^64 and returns zero after MaxUint64.
#[test]
fn alloc_wraps_after_u64_max_like_go_atomic_add() {
    let alloc = crate::mockid::new_id_allocator_with_base(u64::MAX);
    assert_eq!(alloc.Alloc().unwrap(), 0);
    assert_eq!(alloc.Alloc().unwrap(), 1);
}

/// 覆盖正常序列、边界起点、Rebase 空操作与并发 Alloc 唯一性。
#[test]
fn go_rust_public_contract_matches() {
    // normal: Alloc returns 1,2,3...
    // 正常路径：连续 Alloc 得到严格递增 1,2,3。
    let alloc = NewIDAllocator();
    assert_eq!(alloc.Alloc().unwrap(), 1);
    assert_eq!(alloc.Alloc().unwrap(), 2);
    assert_eq!(alloc.Alloc().unwrap(), 3);

    // boundary: fresh allocator starts at 0 base → first id is 1
    // 边界：新实例互不影响，各自从 1 起号。
    let a2 = NewIDAllocator();
    assert_eq!(a2.Alloc().unwrap(), 1);

    // Rebase is no-op and never errors
    // Rebase 不改变计数；随后 Alloc 应继续为 4。
    assert!(alloc.Rebase().is_ok());
    assert_eq!(alloc.Alloc().unwrap(), 4);

    // concurrent Alloc is race-free and unique
    // 并发：8 线程各 100 次，共 800 个 id，排序去重后仍为 800。
    let shared = std::sync::Arc::new(NewIDAllocator());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let a = std::sync::Arc::clone(&shared);
        handles.push(std::thread::spawn(move || {
            let mut ids = Vec::new();
            for _ in 0..100 {
                ids.push(a.Alloc().unwrap());
            }
            ids
        }));
    }
    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().unwrap());
    }
    all.sort_unstable();
    assert_eq!(all.len(), 800);
    all.dedup();
    assert_eq!(all.len(), 800, "Alloc ids must be unique under concurrency");

    // resource: type is usable behind Arc (Drop is trivial)
    // 资源：可安全置于 Arc，析构无额外清理义务。
    let _: std::sync::Arc<IDAllocator> = shared;
}
