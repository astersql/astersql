// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 表实现的行为回归测试。
//
// 覆盖列与索引元数据筛选、行变更及唯一索引原子性、序列缓存边界、
// 扫描描述构造，以及哈希、范围、列表等分区定位和重组流程。

use crate::index::{BackfillState, ColumnInfo, IndexColumn, IndexInfo, SchemaState, TableInfo};
use crate::mutation_checker::Datum;
use crate::partition::{
    ForKeyPruning, ForListColumnPruning, ForListPruning, ForRangeColumnsPruning, ForRangePruning,
    ListPartitionGroup, ListPartitionLocation, Partition, PartitionDefinition, PartitionError,
    PartitionExpr, PartitionedTable, partition_record_key,
};
use crate::tables::{
    Column, Constraint, PbColumnInfo, SequenceAllocator, SequenceCommon, SequenceInfo, TableCommon,
    TableError, TemporaryTable, build_partition_table_scan, build_table_scan, can_skip,
    convert_datum_to_tail_space_count, find_index_by_column_name, find_primary_index,
    overflow_shard_bits, primary_prefix_column_ids, seek_sequence_value,
    set_pb_columns_default_value, try_get_common_pk_column_ids, try_truncate_restored_data,
};
use crate::testutil::swap_reorg_part_fields;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

fn column(offset: usize, state: SchemaState, hidden: bool) -> Column {
    Column {
        info: ColumnInfo {
            id: offset as i64 + 1,
            name: format!("c{offset}"),
            needs_restored_data: false,
        },
        offset,
        state,
        hidden,
        generated: false,
        generated_stored: false,
        primary_key: false,
        common_handle: false,
        default_value: None,
        origin_default_value: None,
    }
}

#[test]
fn visible_and_hidden_columns_only_include_public_columns() {
    let columns = vec![
        column(0, SchemaState::Public, false),
        column(1, SchemaState::WriteOnly, false),
        column(2, SchemaState::Public, true),
        column(3, SchemaState::DeleteOnly, true),
    ];
    let meta = TableInfo {
        id: 9,
        columns: columns.iter().map(|column| column.info.clone()).collect(),
    };
    let table = TableCommon::new(meta, 9, columns, vec![], vec![], false).unwrap();

    assert_eq!(
        table
            .visible_columns()
            .iter()
            .map(|column| column.offset)
            .collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(
        table
            .hidden_columns()
            .iter()
            .map(|column| column.offset)
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn origin_default_overrides_virtual_column_null_placeholder() {
    let mut virtual_column = column(0, SchemaState::Public, false);
    virtual_column.generated = true;
    virtual_column.origin_default_value = Some(Datum::Int(7));
    let mut pb_columns = vec![PbColumnInfo {
        id: 1,
        default_value: None,
    }];

    set_pb_columns_default_value(&mut pb_columns, &[virtual_column]).unwrap();

    let mut encoded = vec![1];
    encoded.extend_from_slice(&7_i64.to_be_bytes());
    assert_eq!(pb_columns[0].default_value, Some(encoded));
}

fn index_info(name: &str, offset: usize, unique: bool) -> IndexInfo {
    IndexInfo {
        id: offset as i64 + 10,
        name: name.to_owned(),
        columns: vec![IndexColumn {
            name: format!("c{offset}"),
            offset,
            length: None,
        }],
        unique,
        primary: false,
        state: SchemaState::Public,
        backfill_state: BackfillState::Inapplicable,
        condition: None,
    }
}

fn two_column_table() -> TableCommon {
    let columns = vec![
        column(0, SchemaState::Public, false),
        column(1, SchemaState::Public, false),
    ];
    let meta = TableInfo {
        id: 9,
        columns: columns.iter().map(|column| column.info.clone()).collect(),
    };
    TableCommon::new(
        meta,
        19,
        columns,
        vec![index_info("unique_b", 1, true)],
        vec![],
        false,
    )
    .unwrap()
}

// 构造阶段必须尽早拒绝错位列和尚未进入任何模式状态的索引，
// 避免后续按偏移访问或维护索引时才暴露损坏的元数据。
#[test]
fn table_constructor_rejects_invalid_columns_and_none_state_indices() {
    let meta = TableInfo {
        id: 1,
        columns: vec![column(0, SchemaState::Public, false).info],
    };
    let mut invalid_column = column(0, SchemaState::Public, false);
    invalid_column.offset = 2;
    assert!(matches!(
        TableCommon::new(meta.clone(), 1, vec![invalid_column], vec![], vec![], false,),
        Err(TableError::InvalidColumnOffset(2))
    ));

    let mut none_index = index_info("not_ready", 0, false);
    none_index.state = SchemaState::None;
    assert!(matches!(
        TableCommon::new(
            meta,
            1,
            vec![column(0, SchemaState::Public, false)],
            vec![none_index],
            vec![],
            false,
        ),
        Err(TableError::IndexStateCannotNone(name)) if name == "not_ready"
    ));
}

#[test]
fn table_prefixes_and_metadata_use_physical_id() {
    let table = two_column_table();
    assert_eq!(table.meta().id, 9);
    assert_eq!(table.physical_id(), 19);
    assert!(!table.use_new_collation());
    assert_eq!(table.record_prefix(), b"t19_r");
    assert_eq!(table.index_prefix(), b"t19_i");
    assert_eq!(table.record_key(-7), b"t19_r-7");
}

#[test]
fn table_copy_clones_rows_and_handle_allocator_independently() {
    let mut original = two_column_table();
    original
        .add_record(vec![Datum::Int(1), Datum::Int(10)], None)
        .unwrap();
    let mut copied = original.copy();
    copied
        .add_record(vec![Datum::Int(2), Datum::Int(20)], None)
        .unwrap();

    assert_eq!(original.iter_records().count(), 1);
    assert_eq!(copied.iter_records().count(), 2);
    assert_eq!(original.alloc_handle_ids(2), Ok((1, 3)));
    assert_eq!(copied.alloc_handle_ids(1), Ok((2, 3)));
}

// 行写入与唯一索引更新应形成一个原子操作：冲突失败后，
// 原行和原索引项都必须保留，后续冲突检测仍应指向原记录。
#[test]
fn add_update_remove_and_iteration_preserve_unique_index_atomicity() {
    let mut table = two_column_table();
    table
        .add_record(vec![Datum::Int(10), Datum::Bytes(b"a".to_vec())], Some(2))
        .unwrap();
    table
        .add_record(vec![Datum::Int(20), Datum::Bytes(b"b".to_vec())], Some(1))
        .unwrap();
    assert_eq!(
        table.add_record(vec![Datum::Int(30), Datum::Bytes(b"a".to_vec())], Some(3),),
        Err(TableError::DuplicateIndex {
            index: "unique_b".to_owned(),
            handle: 2,
        })
    );
    assert_eq!(
        table
            .iter_records()
            .map(|(handle, _)| handle)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    assert_eq!(
        table.update_record(
            1,
            &[Datum::Int(20), Datum::Bytes(b"b".to_vec())],
            vec![Datum::Int(21), Datum::Bytes(b"a".to_vec())],
            &[true, true],
        ),
        Err(TableError::DuplicateIndex {
            index: "unique_b".to_owned(),
            handle: 2,
        })
    );
    assert_eq!(
        table.row_with_columns(1, &[0, 1]).unwrap(),
        vec![Datum::Int(20), Datum::Bytes(b"b".to_vec())]
    );
    assert_eq!(
        table.add_record(vec![Datum::Int(30), Datum::Bytes(b"b".to_vec())], Some(3),),
        Err(TableError::DuplicateIndex {
            index: "unique_b".to_owned(),
            handle: 1,
        }),
        "failed update must restore the original unique index entry"
    );

    table
        .update_record(
            1,
            &[Datum::Int(20), Datum::Bytes(b"b".to_vec())],
            vec![Datum::Int(21), Datum::Bytes(b"c".to_vec())],
            &[true, true],
        )
        .unwrap();
    table
        .remove_record(2, &[Datum::Int(10), Datum::Bytes(b"a".to_vec())])
        .unwrap();
    assert_eq!(
        table.row_with_columns(1, &[1]).unwrap(),
        vec![Datum::Bytes(b"c".to_vec())]
    );
    assert_eq!(
        table.remove_record(2, &[]),
        Err(TableError::RecordNotFound(2))
    );
}

#[test]
fn table_operations_validate_row_and_projection_widths() {
    let mut table = two_column_table();
    assert_eq!(
        table.add_record(vec![Datum::Int(1)], None),
        Err(TableError::RowLength {
            expected: 2,
            actual: 1,
        })
    );
    let handle = table
        .add_record(vec![Datum::Int(1), Datum::Int(2)], None)
        .unwrap();
    assert_eq!(handle, 1);
    assert_eq!(
        table.update_record(
            handle,
            &[Datum::Int(1), Datum::Int(2)],
            vec![Datum::Int(3), Datum::Int(4)],
            &[true],
        ),
        Err(TableError::RowLength {
            expected: 2,
            actual: 1,
        })
    );
    assert_eq!(
        table.row_with_columns(handle, &[2]),
        Err(TableError::InvalidColumnOffset(2))
    );
    assert_eq!(
        table.row_with_columns(99, &[0]),
        Err(TableError::RecordNotFound(99))
    );
}

#[test]
fn schema_state_filters_match_reset_columns_cache() {
    let columns = vec![
        column(0, SchemaState::None, false),
        column(1, SchemaState::DeleteOnly, false),
        column(2, SchemaState::WriteOnly, false),
        column(3, SchemaState::WriteReorganization, false),
        column(4, SchemaState::DeleteReorganization, true),
        column(5, SchemaState::Public, true),
    ];
    let meta = TableInfo {
        id: 1,
        columns: columns.iter().map(|column| column.info.clone()).collect(),
    };
    let constraints = vec![
        Constraint {
            name: "none".to_owned(),
            state: SchemaState::None,
            enforced: true,
        },
        Constraint {
            name: "delete".to_owned(),
            state: SchemaState::DeleteOnly,
            enforced: true,
        },
        Constraint {
            name: "public".to_owned(),
            state: SchemaState::Public,
            enforced: true,
        },
        Constraint {
            name: "not_enforced".to_owned(),
            state: SchemaState::Public,
            enforced: false,
        },
    ];
    let table = TableCommon::new(meta, 1, columns, vec![], constraints, false).unwrap();

    assert_eq!(
        table
            .writable_columns()
            .iter()
            .map(|column| column.offset)
            .collect::<Vec<_>>(),
        vec![0, 2, 3, 5]
    );
    assert_eq!(table.deletable_columns().len(), 6);
    assert_eq!(
        table
            .writable_constraints()
            .iter()
            .map(|constraint| constraint.name.as_str())
            .collect::<Vec<_>>(),
        vec!["none", "public"]
    );
}

// 用可编排的缓存分配与 rebase 结果隔离序列算法，便于覆盖跨缓存轮次和 setval 规则。
struct MockSequenceAllocator {
    caches: VecDeque<Result<(i64, i64, i64), TableError>>,
    rebases: Vec<(i64, Result<(i64, bool), TableError>)>,
}

impl SequenceAllocator for MockSequenceAllocator {
    fn alloc_cache(&mut self) -> Result<(i64, i64, i64), TableError> {
        self.caches.pop_front().unwrap()
    }

    fn rebase(&mut self, new_value: i64) -> Result<(i64, bool), TableError> {
        let position = self
            .rebases
            .iter()
            .position(|(value, _)| *value == new_value)
            .unwrap();
        self.rebases.remove(position).1
    }
}

// 同时验证正向缓存、偏移对齐、循环轮次和显式设值，确保缓存切换不跳号。
#[test]
fn sequence_uses_cache_offset_cycle_and_setval_rules() {
    let allocator = MockSequenceAllocator {
        caches: VecDeque::from([Ok((0, 9, 0)), Ok((9, 15, 1))]),
        rebases: vec![(100, Ok((100, false)))],
    };
    let mut sequence = SequenceCommon::new(
        SequenceInfo {
            increment: 3,
            start: 2,
            min_value: 1,
            max_value: 100,
            cycle: true,
        },
        Box::new(allocator),
    );
    assert_eq!(sequence.next_value(), Ok(2));
    assert_eq!(sequence.next_value(), Ok(5));
    assert_eq!(sequence.set_value(4), Ok((0, true)));
    assert_eq!(sequence.set_value(8), Ok((8, false)));
    assert_eq!(sequence.next_value(), Ok(10));
    assert_eq!(sequence.next_value(), Ok(13));
    assert_eq!(sequence.base_end_round(), (13, 15, 1));
    assert_eq!(sequence.set_value(100), Ok((100, false)));
    assert_eq!(sequence.base_end_round(), (100, 100, 1));
}

#[test]
fn sequence_helpers_cover_descending_and_runout_cases() {
    assert_eq!(seek_sequence_value(0, 3, 2, 9), Some(2));
    assert_eq!(seek_sequence_value(8, 3, 2, 9), None);
    assert_eq!(seek_sequence_value(9, 3, 2, 9), None);
    assert_eq!(seek_sequence_value(10, -3, 9, 0), Some(9));
    assert_eq!(seek_sequence_value(1, -3, 9, 0), Some(0));
    assert_eq!(seek_sequence_value(0, -3, 9, 0), None);
    assert_eq!(seek_sequence_value(0, 0, 0, 10), None);
}

#[test]
fn table_sequence_methods_require_and_use_bound_sequence() {
    let mut table = two_column_table();
    assert_eq!(
        table.sequence_next_value(),
        Err(TableError::SequenceMissing)
    );
    table.set_sequence(SequenceCommon::new(
        SequenceInfo {
            increment: 1,
            start: 1,
            min_value: 1,
            max_value: 10,
            cycle: false,
        },
        Box::new(MockSequenceAllocator {
            caches: VecDeque::from([Ok((0, 3, 0))]),
            rebases: vec![],
        }),
    ));
    assert_eq!(table.sequence_next_value(), Ok(1));
    assert_eq!(table.set_sequence_value(2), Ok((2, false)));
    assert_eq!(table.sequence_next_value(), Ok(3));
}

// 公共句柄列顺序必须跟随主索引定义，而不是表列或索引集合的存放顺序。
#[test]
fn index_and_common_handle_metadata_helpers_preserve_order() {
    let table = TableInfo {
        id: 1,
        columns: vec![
            column(0, SchemaState::Public, false).info,
            column(1, SchemaState::Public, false).info,
        ],
    };
    let mut primary = index_info("primary", 1, true);
    primary.primary = true;
    primary.columns.push(IndexColumn {
        name: "c0".to_owned(),
        offset: 0,
        length: Some(4),
    });
    primary.columns[0].length = Some(8);
    let secondary = index_info("secondary", 0, false);

    assert_eq!(
        find_primary_index(&[secondary.clone(), primary.clone()]),
        Some(&primary)
    );
    assert_eq!(
        try_get_common_pk_column_ids(&table, &[secondary.clone(), primary.clone()]),
        vec![2, 1]
    );
    assert_eq!(
        primary_prefix_column_ids(&table, &[secondary, primary]),
        vec![2, 1]
    );
}

#[test]
fn index_lookup_only_returns_public_single_column_index() {
    let table = two_column_table();
    assert_eq!(
        find_index_by_column_name(table.indices(), "C1")
            .unwrap()
            .index_info
            .name,
        "unique_b"
    );
    assert!(find_index_by_column_name(table.indices(), "missing").is_none());
}

#[test]
fn shard_overflow_checks_only_reserved_shard_field() {
    assert!(!overflow_shard_bits(123, 0, 64, true));
    assert!(!overflow_shard_bits(1, 4, 64, true));
    assert!(overflow_shard_bits(1_i64 << 59, 4, 64, true));
    assert!(overflow_shard_bits(1_i64 << 60, 4, 64, false));
}

#[test]
fn can_skip_matches_pk_default_and_generated_column_rules() {
    let mut candidate = column(0, SchemaState::Public, false);
    assert!(can_skip(&candidate, &Datum::Null, None));
    candidate.default_value = Some(Datum::Int(0));
    assert!(!can_skip(&candidate, &Datum::Null, None));
    candidate.primary_key = true;
    assert!(can_skip(&candidate, &Datum::Int(1), None));

    candidate.primary_key = false;
    candidate.generated = true;
    assert!(can_skip(&candidate, &Datum::Int(1), None));
    candidate.generated_stored = true;
    assert!(!can_skip(&candidate, &Datum::Int(1), None));

    candidate.generated = false;
    candidate.common_handle = true;
    candidate.default_value = None;
    let mut primary = index_info("primary", 0, true);
    primary.primary = true;
    assert!(can_skip(&candidate, &Datum::Int(1), Some(&primary)));
    primary.columns[0].length = Some(2);
    assert!(!can_skip(&candidate, &Datum::Int(1), Some(&primary)));
}

#[test]
fn restored_data_and_tail_space_helpers_match_index_rules() {
    let mut value = Datum::Bytes(b"abcdefghij".to_vec());
    try_truncate_restored_data(&mut value, Some(4), Some(7));
    assert_eq!(value, Datum::Bytes(b"abcdefg".to_vec()));

    let mut unbounded = Datum::Bytes(b"abcdefghij".to_vec());
    try_truncate_restored_data(&mut unbounded, None, Some(7));
    assert_eq!(unbounded, Datum::Bytes(b"abcdefghij".to_vec()));

    let mut spaces = Datum::Bytes(b"a   ".to_vec());
    convert_datum_to_tail_space_count(&mut spaces, true);
    assert_eq!(spaces, Datum::Int(3));
    let mut unchanged = Datum::Bytes(b"a   ".to_vec());
    convert_datum_to_tail_space_count(&mut unchanged, false);
    assert_eq!(unchanged, Datum::Bytes(b"a   ".to_vec()));
}

// 扫描描述需要同时携带表标识、主键列及前缀列信息；
// 下推列默认值则区分原始默认值与虚拟生成列的 NULL 占位。
#[test]
fn table_scan_and_pb_defaults_include_primary_metadata() {
    let columns = vec![
        column(0, SchemaState::Public, false),
        column(1, SchemaState::Public, false),
    ];
    let table = TableInfo {
        id: 77,
        columns: columns.iter().map(|column| column.info.clone()).collect(),
    };
    let mut primary = index_info("primary", 0, true);
    primary.primary = true;
    primary.columns[0].length = Some(3);
    let scan = build_table_scan(&table, &table.columns, &[primary.clone()], false);
    assert_eq!(scan.table_id, 77);
    assert_eq!(scan.primary_column_ids, vec![1]);
    assert_eq!(scan.primary_prefix_column_ids, vec![1]);
    assert!(!scan.tiflash);
    let partition_scan = build_partition_table_scan(&table, &table.columns, &[primary], true);
    assert!(partition_scan.scan.tiflash);
    assert!(partition_scan.fast_scan);

    let mut default_column = columns[0].clone();
    default_column.origin_default_value = Some(Datum::Bytes(b"x".to_vec()));
    let mut virtual_column = columns[1].clone();
    virtual_column.generated = true;
    let mut pb = vec![
        PbColumnInfo {
            id: 1,
            default_value: None,
        },
        PbColumnInfo {
            id: 2,
            default_value: None,
        },
    ];
    set_pb_columns_default_value(&mut pb, &[default_column, virtual_column]).unwrap();
    assert_eq!(pb[0].default_value, Some(vec![3, b'x']));
    assert_eq!(pb[1].default_value, Some(vec![0]));
    assert_eq!(
        set_pb_columns_default_value(&mut pb[..1], &[]),
        Err(TableError::RowLength {
            expected: 0,
            actual: 1,
        })
    );
}

#[test]
fn temporary_table_uses_atomic_modified_and_size_state() {
    let temporary = TemporaryTable::new(TableInfo {
        id: 8,
        columns: vec![],
    });
    assert!(!temporary.modified());
    assert_eq!(temporary.size(), 0);
    temporary.set_modified(true);
    temporary.set_size(123);
    assert!(temporary.modified());
    assert_eq!(temporary.size(), 123);
    assert_eq!(temporary.meta().id, 8);
}

// 分区定位覆盖边界语义：NULL、负数哈希、上界等值与默认列表分区。
#[test]
fn hash_key_range_and_list_partition_routing_cover_boundaries() {
    let hash = PartitionExpr::Hash {
        column_offset: 0,
        partition_count: 4,
    };
    assert_eq!(hash.locate_partition(&[Datum::Int(-5)]), Ok(1));
    assert_eq!(hash.locate_partition(&[Datum::Null]), Ok(0));
    assert_eq!(
        PartitionExpr::Hash {
            column_offset: 0,
            partition_count: 0,
        }
        .locate_partition(&[Datum::Int(1)]),
        Err(PartitionError::NoPartition)
    );

    let key = ForKeyPruning {
        column_offsets: vec![0, 1],
        partition_count: 17,
    };
    assert!(
        key.locate_key_partition(&[Datum::Int(1), Datum::Bytes(b"x".to_vec())])
            .is_ok_and(|partition| partition < 17)
    );

    let range = ForRangePruning {
        column_offset: 0,
        upper_bounds: vec![Some(10), Some(20), None],
    };
    assert_eq!(range.locate(&[Datum::Null]), Ok(0));
    assert_eq!(range.locate(&[Datum::Int(9)]), Ok(0));
    assert_eq!(range.locate(&[Datum::Int(10)]), Ok(1));
    assert_eq!(range.locate(&[Datum::Int(20)]), Ok(2));

    let list = ForListPruning {
        column_offset: 0,
        value_to_partition: [(Datum::Int(7), 1)].into_iter().collect(),
        default_partition: Some(2),
    };
    assert_eq!(list.locate(&[Datum::Int(7)]), Ok(1));
    assert_eq!(list.locate(&[Datum::Int(8)]), Ok(2));
}

// 多列范围按声明的列顺序做字典序比较，列表分区键也必须按列偏移重排。
#[test]
fn range_and_list_columns_route_lexicographically() {
    let range = ForRangeColumnsPruning {
        column_offsets: vec![0, 1],
        upper_bounds: vec![
            vec![Some(Datum::Int(1)), Some(Datum::Int(5))],
            vec![Some(Datum::Int(2)), None],
        ],
    };
    assert_eq!(range.locate(&[Datum::Int(1), Datum::Int(4)]), Ok(0));
    assert_eq!(range.locate(&[Datum::Int(1), Datum::Int(5)]), Ok(1));
    assert_eq!(
        range.locate(&[Datum::Int(3), Datum::Int(0)]),
        Err(PartitionError::NoPartitionForValue)
    );

    let list = ForListColumnPruning {
        column_offsets: vec![1, 0],
        value_to_partition: BTreeMap::from([(vec![Datum::Int(2), Datum::Int(1)], 3)]),
        default_partition: None,
    };
    assert_eq!(list.locate(&[Datum::Int(1), Datum::Int(2)]), Ok(3));
    assert_eq!(
        list.locate(&[Datum::Int(2), Datum::Int(1)]),
        Err(PartitionError::NoPartitionForValue)
    );
}

fn group(index: usize, partitions: &[usize]) -> ListPartitionGroup {
    ListPartitionGroup {
        group_index: index,
        partition_indexes: partitions.iter().copied().collect::<BTreeSet<_>>(),
    }
}

#[test]
fn list_partition_locations_union_and_intersect_by_group() {
    let mut location = ListPartitionLocation(vec![group(0, &[1, 2]), group(1, &[3])]);
    location.union(&ListPartitionLocation(vec![
        group(0, &[2, 4]),
        group(2, &[5]),
    ]));
    assert_eq!(
        location,
        ListPartitionLocation(vec![group(0, &[1, 2, 4]), group(1, &[3]), group(2, &[5]),])
    );
    assert!(location.intersect(&ListPartitionLocation(vec![
        group(0, &[2, 9]),
        group(2, &[5]),
    ])));
    assert_eq!(
        location,
        ListPartitionLocation(vec![group(0, &[2]), group(2, &[5])])
    );
}

fn partitioned(ids: &[i64]) -> PartitionedTable {
    let definitions = ids
        .iter()
        .map(|id| PartitionDefinition {
            id: *id,
            name: format!("p{id}"),
        })
        .collect::<Vec<_>>();
    let partitions = definitions
        .iter()
        .map(|definition| {
            (
                definition.id,
                Partition {
                    physical_id: definition.id,
                    definition: definition.clone(),
                },
            )
        })
        .collect();
    PartitionedTable {
        definitions,
        expression: PartitionExpr::Hash {
            column_offset: 0,
            partition_count: ids.len() as u64,
        },
        partitions,
        reorganize_partitions: HashMap::new(),
        double_write_partitions: HashMap::new(),
    }
}

#[test]
fn partitioned_table_locates_primary_and_sorted_double_writes() {
    let mut table = partitioned(&[10, 20, 30]);
    table.double_write_partitions.insert(
        40,
        Partition {
            physical_id: 40,
            definition: PartitionDefinition {
                id: 40,
                name: "p40".to_owned(),
            },
        },
    );
    assert_eq!(
        table
            .locate_partition(&[Datum::Int(4)])
            .unwrap()
            .physical_id,
        20
    );
    assert_eq!(
        table.writable_partition_ids(&[Datum::Int(4)]),
        Ok(vec![20, 40])
    );
    assert_eq!(partition_record_key(20, -2), b"t20_r-2");
}

// 分区重组交换的不只是定义和主分区，还包括重组、双写等过渡态映射。
#[test]
fn reorganization_swap_exchanges_all_fields_and_checks_dynamic_type() {
    let mut source = partitioned(&[1, 2]);
    let mut destination = partitioned(&[3, 4, 5]);
    source
        .reorganize_partitions
        .insert(10, source.partitions[&1].clone());
    destination
        .double_write_partitions
        .insert(20, destination.partitions[&3].clone());
    let source_before = source.clone();
    let destination_before = destination.clone();

    assert!(swap_reorg_part_fields(&mut source, &mut destination));
    assert_eq!(source, destination_before);
    assert_eq!(destination, source_before);

    let mut not_a_table = 7_i64;
    assert!(!swap_reorg_part_fields(&mut source, &mut not_a_table));
}
