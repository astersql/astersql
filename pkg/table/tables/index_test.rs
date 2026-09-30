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

// 索引元数据与编解码行为的单元测试。
//
// 覆盖索引列去重、DDL 状态下的可写性、恢复数据判定、键值编码、部分索引条件，
// 以及在线回填期间原始键与临时键的选择规则。

use crate::index::{
    BackfillState, ColumnInfo, Index, IndexColumn, IndexError, IndexInfo, SchemaState,
    TEMP_INDEX_PREFIX, TableInfo, dedup_index_columns, gen_temp_index_key_by_state,
    is_index_writable, need_restored_data,
};
use crate::mutation_checker::Datum;

// 构造只保留名称和表列偏移量的索引列，便于各测试突出自身关注的字段。
fn index_column(name: &str, offset: usize) -> IndexColumn {
    IndexColumn {
        name: name.to_owned(),
        offset,
        length: None,
    }
}

// 构造采用指定模式状态的最小索引元数据，其余测试按需覆盖字段。
fn index_info(state: SchemaState) -> IndexInfo {
    IndexInfo {
        id: 1,
        name: "idx".to_owned(),
        columns: vec![],
        unique: false,
        primary: false,
        state,
        backfill_state: BackfillState::Inapplicable,
        condition: None,
    }
}

// 提供两列测试表；第二列需要恢复数据，用于覆盖新排序规则的兼容编码路径。
fn table_info() -> TableInfo {
    TableInfo {
        id: 10,
        columns: vec![
            ColumnInfo {
                id: 1,
                name: "a".to_owned(),
                needs_restored_data: false,
                field_type: 8, // MySQL BIGINT
                collation: String::new(),
            },
            ColumnInfo {
                id: 2,
                name: "b".to_owned(),
                needs_restored_data: true,
                field_type: 253, // MySQL VARSTRING
                collation: "utf8mb4_bin".to_owned(),
            },
        ],
    }
}

// 构造绑定到第一列的索引，统一测试所用的表 ID、索引 ID 和新排序规则配置。
fn build_index(unique: bool, state: SchemaState) -> Index {
    let mut info = index_info(state);
    info.unique = unique;
    info.columns = vec![index_column("a", 0)];
    Index::new(true, 99, table_info(), info).unwrap()
}

// 去重依据是表列偏移量而非索引列名称，并保留每个偏移量首次出现的项。
#[test]
fn dedup_index_columns_uses_column_offset() {
    let columns = vec![
        index_column("alias_a", 2),
        index_column("alias_b", 2),
        index_column("alias_a", 3),
    ];

    assert_eq!(
        dedup_index_columns(&columns),
        vec![columns[0].clone(), columns[2].clone()]
    );
}

// 只有删除阶段禁止写索引；尚未进入 DDL 和写入、公开阶段均允许写入。
#[test]
fn index_is_writable_unless_in_a_delete_state() {
    assert!(is_index_writable(&index_info(SchemaState::None)));
    assert!(!is_index_writable(&index_info(SchemaState::DeleteOnly)));
    assert!(!is_index_writable(&index_info(
        SchemaState::DeleteReorganization
    )));
    assert!(is_index_writable(&index_info(SchemaState::WriteOnly)));
    assert!(is_index_writable(&index_info(
        SchemaState::WriteReorganization
    )));
    assert!(is_index_writable(&index_info(SchemaState::Public)));
}

// 恢复数据仅在启用新排序规则，且索引涉及所需列或前缀列时生成。
#[test]
fn restored_data_requires_new_collation_and_relevant_column() {
    let columns = table_info().columns;
    assert!(!need_restored_data(
        false,
        &[index_column("b", 1)],
        &columns
    ));
    assert!(need_restored_data(true, &[index_column("b", 1)], &columns));
    let mut prefix = index_column("a", 0);
    prefix.length = Some(4);
    assert!(need_restored_data(true, &[prefix], &columns));
    assert!(!need_restored_data(true, &[index_column("a", 0)], &columns));
}

// 索引列偏移量必须能映射到表列，越界时应在构造阶段返回明确错误。
#[test]
fn index_constructor_rejects_out_of_range_column_offset() {
    let mut info = index_info(SchemaState::Public);
    info.columns = vec![index_column("missing", 9)];
    assert_eq!(
        Index::new(false, 1, table_info(), info),
        Err(IndexError::ColumnOffset(9))
    );
}

#[test]
fn empty_partial_index_condition_is_unconditional() {
    let mut info = index_info(SchemaState::Public);
    info.condition = Some(String::new());
    let index = Index::new(false, 1, table_info(), info).unwrap();
    assert!(
        index
            .meet_partial_condition(&[], |_, _| unreachable!())
            .unwrap()
    );
}

#[test]
#[cfg(not(feature = "expression-runtime"))]
fn partial_index_requires_expression_runtime() {
    let mut info = index_info(SchemaState::Public);
    info.condition = Some("a > 0".to_owned());
    assert!(matches!(
        Index::new(false, 1, table_info(), info),
        Err(IndexError::Evaluation(_))
    ));
}

// 唯一索引仅在键值均非空时可省略句柄；NULL 或非唯一索引必须携带句柄以消歧。
#[test]
fn unique_non_null_key_is_distinct_but_null_and_non_unique_include_handle() {
    let unique = build_index(true, SchemaState::Public);
    let (distinct_key, distinct) = unique.gen_index_key(&[Datum::Int(7)], 55).unwrap();
    assert!(distinct);
    assert!(distinct_key.starts_with(b"t99_i1"));
    assert!(!distinct_key.ends_with(&55_i64.to_be_bytes()));

    let (null_key, distinct) = unique.gen_index_key(&[Datum::Null], 55).unwrap();
    assert!(!distinct);
    assert!(null_key.ends_with(&55_i64.to_be_bytes()));

    let non_unique = build_index(false, SchemaState::Public);
    let (key, distinct) = non_unique
        .gen_index_key(&[Datum::Bytes(b"x".to_vec())], -2)
        .unwrap();
    assert!(!distinct);
    assert!(key.ends_with(&(-2_i64).to_be_bytes()));
}

// 调用方提供的索引值数量必须与索引列数量完全一致。
#[test]
fn index_key_validates_value_count() {
    let index = build_index(false, SchemaState::Public);
    assert_eq!(
        index.gen_index_key(&[], 1),
        Err(IndexError::ValueCount {
            expected: 1,
            actual: 0,
        })
    );
}

// 验证索引值布局依次编码未改动标志、句柄以及带长度的恢复数据。
#[test]
fn index_value_encodes_untouched_handle_and_restored_data() {
    let mut info = index_info(SchemaState::Public);
    info.unique = true;
    info.columns = vec![index_column("b", 1)];
    let index = Index::new(true, 99, table_info(), info).unwrap();
    assert!(index.restored_data);

    let value = index.gen_index_value(true, true, 7, &[Datum::Bytes(b"abc".to_vec())]);
    assert_eq!(value[0], 1);
    assert_eq!(&value[1..9], &7_i64.to_be_bytes());
    assert_eq!(value[9], 3);
    assert_eq!(&value[10..14], &3_u32.to_be_bytes());
    assert_eq!(&value[14..], b"abc");
}

// 无条件索引直接匹配；部分索引将 SQL NULL 视为不匹配，并原样传播求值错误。
#[test]
fn partial_index_condition_treats_null_as_false_and_propagates_error() {
    let plain = build_index(false, SchemaState::Public);
    assert_eq!(
        plain.meet_partial_condition(&[], |_condition, _row| panic!("not called")),
        Ok(true)
    );

    let mut conditional = plain.clone();
    conditional.index_info.condition = Some("a > 0".to_owned());
    assert_eq!(
        conditional.meet_partial_condition(&[Datum::Int(1)], |condition, row| {
            assert_eq!(condition, "a > 0");
            assert_eq!(row, &[Datum::Int(1)]);
            Ok(Some(true))
        }),
        Ok(true)
    );
    assert_eq!(
        conditional.meet_partial_condition(&[], |_condition, _row| Ok(None)),
        Ok(false)
    );
    assert_eq!(
        conditional.meet_partial_condition(&[], |_condition, _row| {
            Err(IndexError::Evaluation("bad expression".to_owned()))
        }),
        Err(IndexError::Evaluation("bad expression".to_owned()))
    );
}

#[test]
fn go_merge_49_partial_index_uses_its_own_collation_mode() {
    let mut index = build_index(false, SchemaState::Public);
    index.index_info.condition = Some("a = 'x'".to_owned());
    for use_new in [false, true] {
        index.use_new_collation = use_new;
        assert_eq!(
            index.meet_partial_condition_with_collation(&[], |sql, _, mode| {
                assert_eq!(sql, "a = 'x'");
                assert_eq!(mode, use_new);
                Ok(Some(true))
            }),
            Ok(true)
        );
    }
}

// 在线回填状态决定是否双写原始键与临时键，以及临时索引值采用的版本。
#[test]
fn temporary_index_key_tracks_backfill_state() {
    let key = b"index-key";
    for (state, backfill_state, original_is_empty, version) in [
        (SchemaState::DeleteOnly, BackfillState::Running, true, 1),
        (SchemaState::WriteOnly, BackfillState::Running, true, 2),
        (
            SchemaState::WriteReorganization,
            BackfillState::ReadyToMerge,
            false,
            3,
        ),
        (
            SchemaState::WriteReorganization,
            BackfillState::Merging,
            false,
            3,
        ),
    ] {
        let mut info = index_info(state);
        info.backfill_state = backfill_state;
        let (original, temporary, actual_version) = gen_temp_index_key_by_state(&info, key);
        assert_eq!(original.is_empty(), original_is_empty);
        if !original_is_empty {
            assert_eq!(original, key);
        }
        let temporary = temporary.unwrap();
        assert_eq!(temporary[0], TEMP_INDEX_PREFIX);
        assert_eq!(&temporary[1..], key);
        assert_eq!(actual_version, version);
    }
    for state in [
        SchemaState::None,
        SchemaState::DeleteOnly,
        SchemaState::Public,
    ] {
        assert_eq!(
            gen_temp_index_key_by_state(&index_info(state), key),
            (key.to_vec(), None, 0)
        );
    }
}
