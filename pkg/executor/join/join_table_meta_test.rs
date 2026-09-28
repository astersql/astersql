// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// JoinTableMeta 对 KeyMode、列顺序、null map 与 EncodedRow 访问的单元测试。
//
// 覆盖 OneInt64 / FixedSerialized / VariableSerialized 选择、非法下标拒绝，
// 以及 key 切片、null bit、row_data 偏移与原子 used 标志的真实读写。


use crate::join_table_meta::{
    EncodedRow, FieldKind, FieldType, KeyMode, key_property, new_table_meta,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// 构造测试用 FieldType。
fn field(kind: FieldKind, fixed_length: Option<usize>, nullable: bool) -> FieldType {
    FieldType {
        kind,
        fixed_length,
        nullable,
    }
}

/// 单兼容 int → OneInt64；符号不一致或非 8 字节定长 → Fixed；变长 text → Variable。
#[test]
fn table_meta_selects_one_int_fixed_and_variable_key_modes() {
    let int = field(FieldKind::SignedInt, Some(8), false);
    let uint = field(FieldKind::UnsignedInt, Some(8), false);
    let fixed = field(FieldKind::SignedInt, Some(4), false);
    let text = field(
        FieldKind::Text {
            collation: "utf8mb4_bin".into(),
        },
        None,
        true,
    );
    let build = vec![int.clone(), fixed.clone(), text.clone()];
    assert_eq!(
        new_table_meta(
            &[0],
            &build,
            &[int.clone()],
            &[int.clone()],
            &[],
            &[],
            false
        )
        .unwrap()
        .key_mode,
        KeyMode::OneInt64
    );
    assert_eq!(
        new_table_meta(&[0], &build, &[int.clone()], &[uint], &[], &[], false)
            .unwrap()
            .key_mode,
        KeyMode::FixedSerialized
    );
    let fixed_meta = new_table_meta(
        &[0, 1],
        &build,
        &[int.clone(), fixed.clone()],
        &[int.clone(), fixed],
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(fixed_meta.key_mode, KeyMode::FixedSerialized);
    assert_eq!(fixed_meta.fixed_key_length, 12);
    let variable = new_table_meta(
        &[2],
        &build,
        std::slice::from_ref(&text),
        std::slice::from_ref(&text),
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(variable.key_mode, KeyMode::VariableSerialized);
    assert!(key_property(&text).requires_serialization);
}

/// 列顺序为 key → other condition → 输出；越界 key 下标返回错误。
#[test]
fn table_meta_orders_saved_columns_and_rejects_invalid_shapes() {
    let types = vec![
        field(FieldKind::SignedInt, Some(8), false),
        field(FieldKind::Bytes, None, true),
        field(FieldKind::Float, Some(8), false),
    ];
    let meta = new_table_meta(
        &[2],
        &types,
        &[types[2].clone()],
        &[types[2].clone()],
        &[1, 2],
        &[0, 1],
        true,
    )
    .unwrap();
    assert_eq!(meta.row_columns_order, [1, 2, 0]);
    assert_eq!(meta.saved_column_count, 3);
    assert_eq!(meta.null_map_length, 4);
    assert!(
        new_table_meta(
            &[3],
            &types,
            &[types[0].clone()],
            &[types[0].clone()],
            &[],
            &[],
            false
        )
        .is_err()
    );
}

/// EncodedRow 的 key/null/offset 访问与 set_used_flag 原子写入可观测。
#[test]
fn encoded_row_key_null_map_offset_and_atomic_used_flag_are_real() {
    let types = vec![field(FieldKind::SignedInt, Some(8), true)];
    let meta = new_table_meta(&[0], &types, &types, &types, &[], &[], true).unwrap();
    let row = EncodedRow {
        bytes: vec![0, 1, 2, 3, 4, 5, 6, 7, 9],
        null_map: vec![1],
        key_offset: 0,
        key_length: 8,
        row_data_offset: 8,
        used: AtomicBool::new(false),
    };
    assert_eq!(meta.serialized_key_length(&row), 8);
    assert_eq!(meta.key_bytes(&row), &[0, 1, 2, 3, 4, 5, 6, 7]);
    assert!(meta.is_column_null(&row, 0));
    assert_eq!(meta.advance_to_row_data(&row), 8);
    assert!(!meta.is_current_row_used_atomic(&row));
    meta.set_used_flag(&row);
    assert!(row.used.load(Ordering::Acquire));
}

#[test]
/// Used-flag metadata reserves an atomic four-byte null map; ordinary nullable rows use bits.
fn table_meta_null_map_alignment_matches_used_flag_mode() {
    let nullable_int = field(FieldKind::SignedInt, Some(8), true);
    let normal = new_table_meta(
        &[0],
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        &[],
        &[],
        false,
    )
    .unwrap();
    let with_used = new_table_meta(
        &[0],
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        &[],
        &[],
        true,
    )
    .unwrap();
    assert_eq!(normal.null_map_length, 1);
    assert_eq!(with_used.null_map_length, 4);
    assert!(with_used.need_used_flag);
}

#[test]
/// Null-map reads before the used-flag write are thread-safe only outside the atomic word.
fn table_meta_null_map_thread_safety_boundary_is_explicit() {
    let int = field(FieldKind::SignedInt, Some(8), false);
    let meta =
        new_table_meta(&[0], &[int.clone()], &[int.clone()], &[int], &[], &[], true).unwrap();
    let row = EncodedRow {
        bytes: vec![0; 8],
        null_map: vec![0],
        key_offset: 0,
        key_length: 8,
        row_data_offset: 8,
        used: AtomicBool::new(false),
    };
    assert!(!meta.is_current_row_used_atomic(&row));
    meta.set_used_flag(&row);
    assert!(meta.is_current_row_used_atomic(&row));
}

#[test]
/// Mixed signed and unsigned integer keys retain a serialization mode instead of raw inlining.
fn table_meta_mixed_integer_keys_require_sign_aware_serialization() {
    let signed = field(FieldKind::SignedInt, Some(8), false);
    let unsigned = field(FieldKind::UnsignedInt, Some(8), false);
    let meta = new_table_meta(
        &[0],
        &[signed.clone()],
        &[signed],
        &[unsigned],
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(meta.key_mode, KeyMode::FixedSerialized);
    assert_eq!(meta.fixed_key_length, 9);
    assert_eq!(meta.saved_column_count, 0);
}

#[test]
fn table_meta_does_not_save_unrequested_non_inlined_columns() {
    let text = field(
        FieldKind::Text {
            collation: "utf8mb4_general_ci".into(),
        },
        None,
        true,
    );
    let int = field(FieldKind::SignedInt, Some(8), false);
    let meta = new_table_meta(
        &[0],
        &[text.clone(), int],
        std::slice::from_ref(&text),
        std::slice::from_ref(&text),
        &[],
        &[],
        false,
    )
    .unwrap();

    assert_eq!(meta.row_columns_order, []);
    assert_eq!(meta.saved_column_count, 0);
    assert_eq!(meta.null_map_length, 0);
}

#[test]
fn table_meta_float_key_uses_go_fixed_serialized_mode() {
    let float = field(FieldKind::Float, Some(8), false);
    let meta = new_table_meta(
        &[0],
        std::slice::from_ref(&float),
        std::slice::from_ref(&float),
        std::slice::from_ref(&float),
        &[],
        &[],
        false,
    )
    .unwrap();

    assert_eq!(meta.key_mode, KeyMode::FixedSerialized);
}
