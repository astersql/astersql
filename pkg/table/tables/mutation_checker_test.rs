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

// Mutation 一致性检查的单元测试。
//
// 重点验证行与索引变更之间的 handle、索引列值和操作类型约束，以及与 Go 实现
// 一致的跳过条件；测试数据刻意保持最小化，以便直接定位失败的契约。

use crate::mutation_checker::{
    ConsistencyError, Datum, IndexLayout, Mutation, MutationFlags, check_data_consistency,
    check_handle_consistency, check_index_keys, check_row_insertion_consistency,
    compare_index_and_value,
};
use std::cmp::Ordering;
use std::collections::HashMap;

/// 构造一个索引列顺序与行列顺序相反的布局，用于验证按偏移取值而非按位置取值。
fn layouts() -> HashMap<i64, IndexLayout> {
    [(
        7,
        IndexLayout {
            id: 7,
            column_offsets: vec![1, 0],
        },
    )]
    .into_iter()
    .collect()
}

/// 构造后续测试共用的索引变更，调用方只覆盖待验证的 value 与 handle。
fn index_mutation(value: Vec<u8>, handle: Option<i64>) -> Mutation {
    Mutation {
        key: b"index".to_vec(),
        value,
        index_id: 7,
        handle,
        indexed_values: vec![Datum::Bytes(b"seven".to_vec()), Datum::Int(1)],
        ..Default::default()
    }
}

#[test]
fn scalar_comparison_is_numeric_across_signed_and_unsigned_values() {
    // 有符号与无符号整数应按数值比较，不能依赖枚举变体的派生顺序。
    assert_eq!(
        compare_index_and_value(&Datum::Int(7), &Datum::Uint(7), false),
        Ordering::Equal
    );
    assert_eq!(
        compare_index_and_value(&Datum::Int(-1), &Datum::Uint(0), false),
        Ordering::Less
    );
    assert_eq!(
        compare_index_and_value(&Datum::Uint(8), &Datum::Int(7), false),
        Ordering::Greater
    );
    assert_eq!(
        compare_index_and_value(&Datum::Null, &Datum::Null, false),
        Ordering::Equal
    );
    assert_eq!(
        compare_index_and_value(
            &Datum::Bytes(b"a".to_vec()),
            &Datum::Bytes(b"b".to_vec()),
            false,
        ),
        Ordering::Less
    );
}

#[test]
fn row_insertion_checks_only_values_present_in_encoded_mutation() {
    // 行 value 可能只编码部分列；检查器只核对实际解码出的列及其原始偏移。
    let expected = vec![Datum::Int(1), Datum::Bytes(b"two".to_vec())];
    let mut mutation = Mutation {
        key: b"row".to_vec(),
        value: b"encoded".to_vec(),
        row_values: vec![(1, Datum::Bytes(b"two".to_vec()))],
        ..Default::default()
    };
    assert_eq!(
        check_row_insertion_consistency(Some(&expected), &mutation),
        Ok(())
    );
    mutation.row_values[0].1 = Datum::Bytes(b"wrong".to_vec());
    assert_eq!(
        check_row_insertion_consistency(Some(&expected), &mutation),
        Err(ConsistencyError::InconsistentRowValue)
    );
    assert_eq!(check_row_insertion_consistency(None, &mutation), Ok(()));
}

#[test]
fn multi_value_comparison_matches_any_zero_delimited_member() {
    // 多值索引的简化编码以 NUL 分隔成员，任一成员相等即视为匹配。
    let members = Datum::Bytes(b"one\0seven\0nine".to_vec());
    assert_eq!(
        compare_index_and_value(&Datum::Bytes(b"seven".to_vec()), &members, true),
        Ordering::Equal
    );
    assert_ne!(
        compare_index_and_value(&Datum::Bytes(b"eight".to_vec()), &members, true),
        Ordering::Equal
    );
}

#[test]
fn handle_consistency_accepts_matching_put_and_masks_temporary_index_id() {
    let row = Mutation {
        key: b"row".to_vec(),
        value: b"value".to_vec(),
        handle: Some(11),
        ..Default::default()
    };
    let mut index = index_mutation(b"value".to_vec(), Some(11));
    // 临时索引标记占用高位，查找布局前必须先将其屏蔽。
    index.index_id |= 0x7fff_0000_0000_0000_i64;

    assert_eq!(check_handle_consistency(&row, &[index], &layouts()), Ok(()));
}

#[test]
fn handle_consistency_reports_mismatch_and_missing_index() {
    let row = Mutation {
        key: b"row".to_vec(),
        value: b"value".to_vec(),
        handle: Some(11),
        ..Default::default()
    };
    assert_eq!(
        check_handle_consistency(
            &row,
            &[index_mutation(b"value".to_vec(), Some(12))],
            &layouts(),
        ),
        Err(ConsistencyError::InconsistentHandle {
            row_handle: 11,
            index_handle: 12,
            index_id: 7,
        })
    );
    let mut missing = index_mutation(b"value".to_vec(), Some(11));
    missing.index_id = 99;
    assert_eq!(
        check_handle_consistency(&row, &[missing], &layouts()),
        Err(ConsistencyError::MissingIndex(99))
    );
}

#[test]
fn deletes_and_untouched_index_values_skip_handle_check() {
    let row = Mutation {
        key: b"row".to_vec(),
        value: b"value".to_vec(),
        handle: Some(11),
        ..Default::default()
    };
    let deletion = index_mutation(vec![], Some(99));
    let mut untouched = index_mutation(b"value".to_vec(), Some(99));
    untouched.index_id |= 0x7fff_0000_0000_0000_i64;
    untouched.flags = MutationFlags {
        untouched: true,
        ..Default::default()
    };

    // 删除不会写入索引 value，untouched 临时值也不会提交，两者均不要求 handle 匹配。
    assert_eq!(
        check_handle_consistency(&row, &[deletion, untouched], &layouts()),
        Ok(())
    );
}

#[test]
fn untouched_is_skipped_only_for_existing_temporary_indexes() {
    let row = Mutation {
        key: b"row".to_vec(),
        value: b"value".to_vec(),
        handle: Some(11),
        ..Default::default()
    };
    let mut normal = index_mutation(b"value".to_vec(), Some(99));
    normal.flags.untouched = true;
    assert_eq!(
        check_handle_consistency(&row, &[normal], &layouts()),
        Err(ConsistencyError::InconsistentHandle {
            row_handle: 11,
            index_handle: 99,
            index_id: 7,
        })
    );

    let mut missing_temporary = index_mutation(b"value".to_vec(), Some(99));
    missing_temporary.index_id = 99 | 0x7fff_0000_0000_0000_i64;
    missing_temporary.flags.untouched = true;
    assert_eq!(
        check_handle_consistency(&row, &[missing_temporary], &layouts()),
        Err(ConsistencyError::MissingIndex(99))
    );
}

#[test]
fn index_keys_use_insert_row_for_put_and_remove_row_for_delete() {
    // 同一批 mutation 中，PUT 对照新行，空 value 表示的 DELETE 对照旧行。
    let insert = vec![Datum::Int(1), Datum::Bytes(b"seven".to_vec())];
    let remove = vec![Datum::Int(2), Datum::Bytes(b"old".to_vec())];
    let put = index_mutation(b"value".to_vec(), Some(1));
    let mut delete = index_mutation(vec![], Some(1));
    delete.indexed_values = vec![Datum::Bytes(b"old".to_vec()), Datum::Int(2)];

    assert_eq!(
        check_index_keys(Some(&insert), Some(&remove), &[put, delete], &layouts(),),
        Ok(())
    );
}

#[test]
fn index_key_mismatch_identifies_index_and_row_offset() {
    let insert = vec![Datum::Int(1), Datum::Bytes(b"wrong".to_vec())];
    assert_eq!(
        check_index_keys(
            Some(&insert),
            None,
            &[index_mutation(b"value".to_vec(), Some(1))],
            &layouts(),
        ),
        Err(ConsistencyError::InconsistentIndexedValue {
            index_id: 7,
            column_offset: 1,
        })
    );
}

#[test]
fn missing_source_row_is_not_an_error() {
    assert_eq!(
        check_index_keys(
            None,
            None,
            &[index_mutation(b"value".to_vec(), Some(1))],
            &layouts(),
        ),
        Ok(())
    );
}

#[test]
fn index_keys_skip_untouched_temporary_values_after_layout_lookup() {
    let insert = vec![Datum::Int(1), Datum::Bytes(b"wrong".to_vec())];
    let mut temporary = index_mutation(b"value".to_vec(), Some(1));
    temporary.index_id |= 0x7fff_0000_0000_0000_i64;
    temporary.flags.untouched = true;
    assert_eq!(
        check_index_keys(Some(&insert), None, &[temporary], &layouts()),
        Ok(())
    );

    let mut missing = index_mutation(b"value".to_vec(), Some(1));
    missing.index_id = 99 | 0x7fff_0000_0000_0000_i64;
    missing.flags.untouched = true;
    assert_eq!(
        check_index_keys(Some(&insert), None, &[missing], &layouts()),
        Err(ConsistencyError::MissingIndex(99))
    );
}

#[test]
fn top_level_checker_has_go_fast_exit_conditions() {
    let bad = index_mutation(b"value".to_vec(), Some(2));
    let row = Mutation {
        key: b"row".to_vec(),
        value: b"value".to_vec(),
        handle: Some(1),
        ..Default::default()
    };
    // 分区表、流水线 DML 或无有效 staging handle 时无法在此可靠校验，应与 Go 一样快退。
    for (partitioned, pipelined, staging) in [(true, false, 1), (false, true, 1), (false, false, 0)]
    {
        assert_eq!(
            check_data_consistency(
                partitioned,
                pipelined,
                staging,
                None,
                None,
                Some(&row),
                std::slice::from_ref(&bad),
                &layouts(),
            ),
            Ok(())
        );
    }
    assert!(matches!(
        check_data_consistency(false, false, 1, None, None, Some(&row), &[bad], &layouts(),),
        Err(ConsistencyError::InconsistentHandle { .. })
    ));
}
