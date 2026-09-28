// Copyright 2026 AsterSQL.

//! 与 Go `br/pkg/utils/consts` 公开契约的 parity：CF 名字面量与长度稳定。

use crate::{DefaultCF, WriteCF};

#[test]
fn go_rust_public_contract_matches() {
    // Normal: exported CF names match Go literals
    // 必须与 Go 字面量字节级一致，否则 TiKV 选错列族。
    assert_eq!(DefaultCF, "default");
    assert_eq!(WriteCF, "write");

    // Boundary: constants are distinct and non-empty
    assert_ne!(DefaultCF, WriteCF);
    assert!(!DefaultCF.is_empty());
    assert!(!WriteCF.is_empty());

    // Error / misuse: byte lengths are stable for callers that treat them as CF ids
    // 长度契约防回归：调用方有时按 CF id 长度做缓冲。
    assert_eq!(DefaultCF.len(), 7);
    assert_eq!(WriteCF.len(), 5);

    // Resource/side-effect stand-in: values remain usable as owned Strings without mutation
    let owned = vec![DefaultCF.to_string(), WriteCF.to_string()];
    assert_eq!(owned[0], "default");
    assert_eq!(owned[1], "write");
    drop(owned);
}
