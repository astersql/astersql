// Copyright 2026 AsterSQL.

//! 对照 `br/pkg/common/consts.go`：校验 `MaxStoreConcurrency` 的公开契约。
//!
//! 该常量限制 BR 对 TiKV 的连接池规模；Go 侧注释说明未来 store 增多时
//! 仍以 128 为经验上限。本测试只锁公开值与可用性，不覆盖连接池实现。

use crate::MaxStoreConcurrency;

#[test]
fn go_rust_public_contract_matches() {
    // Normal: exported constant matches Go.
    // 与 Go `const MaxStoreConcurrency = 128` 数值对齐。
    assert_eq!(MaxStoreConcurrency, 128);

    // Boundary: positive concurrency pool limit used by BR connection pools.
    // 连接池上限须为正，且不超过当前约定的安全区间。
    assert!(MaxStoreConcurrency > 0);
    assert!(MaxStoreConcurrency <= 1024);

    // Error/resource: constant is Copy and usable without allocation/cleanup.
    // Copy 语义保证可在多处使用而无需额外资源管理。
    let a = MaxStoreConcurrency;
    let b = a;
    assert_eq!(a, b);
}
