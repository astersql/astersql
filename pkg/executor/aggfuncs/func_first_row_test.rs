// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// FIRST_ROW 聚合的单元测试。
//
// 验证「无行」与「首行为 SQL NULL」的区分，以及 merge 时已锁定的 NULL 首行
// 不会被对侧非空值覆盖。

use crate::func_first_row::{
    FirstRow, FirstRow4Decimal, FirstRow4Duration, FirstRow4Enum, FirstRow4Float32,
    FirstRow4Float64, FirstRow4Int, FirstRow4Json, FirstRow4Set, FirstRow4String, FirstRow4Time,
    FirstRow4VectorFloat32,
};

/// 校验无行/NULL 首行语义，以及已锁定 NULL 后 merge 不被覆盖、reset 后再 merge 可采纳对侧。
#[test]
fn first_row_distinguishes_no_row_from_a_null_first_row() {
    // 首个输入为 NULL：标记 got_first_row，且 is_null 为真。
    let mut empty = FirstRow::<i64>::default();
    assert!(!empty.got_first_row());
    empty.update([None, Some(2)]);
    assert!(empty.got_first_row());
    assert!(empty.is_null());
    assert_eq!(empty.value(), None);

    // 已锁定 NULL 时 merge 对侧非空值应被忽略；reset 后才能采纳。
    let mut source = FirstRow::default();
    source.update([Some(7)]);
    empty.merge(&source);
    assert!(empty.is_null());
    empty.reset();
    empty.merge(&source);
    assert_eq!(empty.value(), Some(&7));
}

/// 对齐 Go 的首行锁定和 partial merge 顺序：空批次不锁定，首个非空值不会被后续值覆盖。
#[test]
fn first_row_keeps_the_first_value_and_merges_only_into_empty_state() {
    let mut destination = FirstRow::<String>::default();
    destination.update([]);
    assert!(!destination.got_first_row());

    destination.update([Some("first".to_owned()), Some("second".to_owned())]);
    destination.update([Some("third".to_owned())]);
    assert_eq!(destination.value().map(String::as_str), Some("first"));

    let mut source = FirstRow::default();
    source.update([Some("source".to_owned())]);
    destination.merge(&source);
    assert_eq!(destination.value().map(String::as_str), Some("first"));

    let result = destination.into_result();
    assert_eq!(result, Some(Some("first".to_owned())));
}

/// Go 为每种执行类型提供独立实现；Rust 用同一泛型状态机承载这些类型别名。
/// 此测试固定全部公开别名均保持可分配、可重置的契约。
#[test]
fn all_go_first_row_specializations_have_rust_aliases() {
    fn assert_first_row_contract<T: Default>() {
        let mut row = FirstRow::<T>::default();
        assert!(!row.got_first_row());
        row.reset();
        assert!(!row.got_first_row());
    }

    assert_first_row_contract::<i64>();
    assert_first_row_contract::<f32>();
    assert_first_row_contract::<f64>();
    assert_first_row_contract::<crate::func_sum::Decimal>();
    assert_first_row_contract::<String>();
    assert_first_row_contract::<crate::func_max_min::TimeValue>();
    assert_first_row_contract::<crate::func_max_min::DurationValue>();
    assert_first_row_contract::<crate::func_max_min::BinaryJson>();
    assert_first_row_contract::<crate::func_max_min::VectorFloat32>();
    assert_first_row_contract::<crate::func_max_min::NamedValue>();

    let _: FirstRow4Int = FirstRow::default();
    let _: FirstRow4Float32 = FirstRow::default();
    let _: FirstRow4Float64 = FirstRow::default();
    let _: FirstRow4Decimal = FirstRow::default();
    let _: FirstRow4String = FirstRow::default();
    let _: FirstRow4Time = FirstRow::default();
    let _: FirstRow4Duration = FirstRow::default();
    let _: FirstRow4Json = FirstRow::default();
    let _: FirstRow4VectorFloat32 = FirstRow::default();
    let _: FirstRow4Enum = FirstRow::default();
    let _: FirstRow4Set = FirstRow::default();
}
