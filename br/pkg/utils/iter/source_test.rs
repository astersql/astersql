// Copyright 2026 AsterSQL.

//! `source.rs` 的 Go/Rust 对等回归测试。

use crate::{CollectAll, Context, OfRange};

#[test]
fn of_range_supports_all_go_fixed_width_integer_types() {
    let ctx = Context::background();

    let mut i8s = OfRange(1i8, 4i8);
    assert_eq!(CollectAll(&ctx, &mut *i8s).Item.unwrap(), vec![1i8, 2, 3]);

    let mut i16s = OfRange(1i16, 4i16);
    assert_eq!(CollectAll(&ctx, &mut *i16s).Item.unwrap(), vec![1i16, 2, 3]);

    let mut u8s = OfRange(1u8, 4u8);
    assert_eq!(CollectAll(&ctx, &mut *u8s).Item.unwrap(), vec![1u8, 2, 3]);

    let mut u16s = OfRange(1u16, 4u16);
    assert_eq!(CollectAll(&ctx, &mut *u16s).Item.unwrap(), vec![1u16, 2, 3]);
}
