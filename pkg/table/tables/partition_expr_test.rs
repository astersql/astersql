// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 分区表达式（partition expression）单元测试。
//
// 覆盖 KEY / LIST / RANGE 分区裁剪（partition pruning）辅助结构的克隆、
// 重索引、CRC32 定位、集合交并与有符号/无符号比较语义，对齐 Go 侧行为。

use super::*;

/// 构造用于分区测试的整型表达式列：指定 UniqueID、行内 Index 与显示长度 Flen。
fn column(unique_id: i64, index: isize, flen: isize) -> expression::Column {
    let mut field_type = expression::types::NewFieldType(expression::mysql::TypeLonglong);
    field_type.SetFlen(flen);
    expression::Column::new(*field_type, unique_id, unique_id, index)
}

/// 验证 KEY 分区按 ColumnOffset 选取列后会克隆并重编号 Index，且原地写回选中列指针。
#[test]
fn key_partition_columns_are_cloned_and_reindexed() {
    let mut columns = vec![column(10, 7, 11), column(20, 8, 22), column(30, 9, 33)];
    let partition = PartitionExpr {
        ColumnOffset: vec![2, 0],
        ..Default::default()
    };

    // 按偏移 [2,0] 取出列并重排为分区键列，同时返回各列 Flen。
    let (partition_columns, lengths) = partition.GetPartColumnsForKeyPartition(&mut columns);

    assert_eq!(lengths, vec![33, 11]);
    assert_eq!(partition_columns[0].UniqueID, 30);
    assert_eq!(partition_columns[0].Index, 0);
    assert_eq!(partition_columns[1].UniqueID, 10);
    assert_eq!(partition_columns[1].Index, 1);
    assert_eq!(
        columns[2].Index, 0,
        "Go mutates the selected column pointer"
    );
    assert_eq!(
        columns[0].Index, 1,
        "Go mutates the selected column pointer"
    );
}

/// 验证 KEY 分区定位使用 IEEE CRC32：NULL 列写入单字节 0 标记，再对非空列哈希键累加。
#[test]
fn key_partition_uses_ieee_crc32_and_null_marker() {
    let pruning = ForKeyPruning {
        KeyPartCols: vec![column(1, 0, 8), column(2, 1, 8)],
    };
    let row = vec![
        expression::types::Datum::default(),
        expression::types::NewStringDatum("aster".to_owned()),
    ];

    // 手工复算 CRC32 % 分区数，与 LocateKeyPartition 对齐。
    let mut expected = crc32fast::Hasher::new();
    expected.update(&[0]);
    expected.update(&row[1].ToHashKey().unwrap());
    let expected = (expected.finalize() % 17) as usize;

    assert_eq!(pruning.LocateKeyPartition(17, &row).unwrap(), expected);
}

/// 验证 LIST 分区位置集合的 Union 保持 Go 侧 append 语义（含重复 GroupIdx）。
#[test]
fn list_location_union_preserves_go_append_semantics() {
    let mut helper = NewListPartitionLocationHelper();
    helper.Union(&ListPartitionLocation(vec![
        ListPartitionGroup {
            PartIdx: 0,
            GroupIdxs: vec![0, 1],
        },
        ListPartitionGroup {
            PartIdx: 1,
            GroupIdxs: vec![0],
        },
    ]));
    helper.UnionPartitionGroup(&ListPartitionGroup {
        PartIdx: 0,
        GroupIdxs: vec![1, 2],
    });

    assert_eq!(helper.GetLocation().0[0].GroupIdxs, vec![0, 1, 1, 2]);
}

/// 验证 LIST 分区位置 Intersect：按 PartIdx 与 GroupIdxs 取交集，空集时返回 false。
#[test]
fn list_location_intersection_filters_partitions_and_groups() {
    let mut helper = NewListPartitionLocationHelper();
    assert!(helper.Intersect(&ListPartitionLocation(vec![
        ListPartitionGroup {
            PartIdx: 0,
            GroupIdxs: vec![0, 1]
        },
        ListPartitionGroup {
            PartIdx: 1,
            GroupIdxs: vec![0, 2]
        },
    ])));
    assert!(helper.Intersect(&ListPartitionLocation(vec![
        ListPartitionGroup {
            PartIdx: 1,
            GroupIdxs: vec![2, 3]
        },
        ListPartitionGroup {
            PartIdx: 2,
            GroupIdxs: vec![0]
        },
    ])));

    assert_eq!(
        helper.GetLocation(),
        &ListPartitionLocation(vec![ListPartitionGroup {
            PartIdx: 1,
            GroupIdxs: vec![2]
        }])
    );
    assert!(!helper.GetLocation().IsEmpty());
    assert!(!helper.Intersect(&ListPartitionLocation(vec![])));
}

/// 验证 RANGE 分区 Compare：有符号/无符号解释 LessThan，以及 MaxValue 分区恒大于普通上界。
#[test]
fn range_comparison_preserves_signed_unsigned_and_maxvalue_rules() {
    let signed = ForRangePruning {
        LessThan: vec![-1, 10, 0],
        MaxValue: true,
        Unsigned: false,
    };
    assert_eq!(signed.Compare(0, 0, false), -1);
    assert_eq!(signed.Compare(1, 10, false), 0);
    assert_eq!(signed.Compare(1, 9, false), 1);
    // 末分区为 MAXVALUE 时，与任意有限值比较结果为大于。
    assert_eq!(signed.Compare(2, i64::MAX, false), 1);

    let unsigned = ForRangePruning {
        LessThan: vec![-1],
        MaxValue: false,
        Unsigned: true,
    };
    // LessThan 存 -1 时按无符号应为 u64::MAX，故大于 i64::MAX。
    assert_eq!(unsigned.Compare(0, i64::MAX, true), 1);
}

/// 验证 LIST 裁剪结构 clone：列深拷贝，ValueMap / Sorted 等查找树通过 Arc 共享。
#[test]
fn list_pruning_clone_deep_clones_columns_and_shares_lookup_trees() {
    let pruning = ForListPruning {
        PruneExprCols: vec![column(7, 0, 8)],
        ValueToPartitionIdx: std::sync::Arc::new([(12, 3)].into_iter().collect()),
        ColPrunes: vec![ForListColumnPruning {
            ExprCol: Some(column(9, 1, 8)),
            ValueMap: std::sync::Arc::new(Default::default()),
            Sorted: std::sync::Arc::new(Default::default()),
            DefaultPartID: 42,
            ..Default::default()
        }],
        DefaultPartitionIdx: 5,
        ..Default::default()
    };

    let mut cloned = pruning.clone();
    cloned.PruneExprCols[0].Index = 4;
    cloned.ColPrunes[0].ExprCol.as_mut().unwrap().Index = 6;

    assert_eq!(pruning.PruneExprCols[0].Index, 0);
    assert_eq!(pruning.ColPrunes[0].ExprCol.as_ref().unwrap().Index, 1);
    assert!(std::sync::Arc::ptr_eq(
        &pruning.ValueToPartitionIdx,
        &cloned.ValueToPartitionIdx
    ));
    assert!(pruning.ColPrunes[0].HasDefault());
    assert_eq!(pruning.GetDefaultIdx(), 5);
}

/// Go uses zero as the uninitialized partition ID, so only positive IDs denote DEFAULT.
#[test]
fn list_column_default_requires_positive_partition_id() {
    assert!(!ForListColumnPruning::default().HasDefault());
    assert!(
        !ForListColumnPruning {
            DefaultPartID: -1,
            ..Default::default()
        }
        .HasDefault()
    );
    assert!(
        ForListColumnPruning {
            DefaultPartID: 1,
            ..Default::default()
        }
        .HasDefault()
    );
}
