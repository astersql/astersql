// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Coprocessor（协处理器）上下文构建逻辑的单元测试。
//
// Coprocessor 是将部分计算下推到存储节点（TiKV）执行的机制；DDL 在
// 添加索引等操作时会构建 CopContext，用于描述需要从存储层读取哪些列、
// 以及如何解析行句柄（handle，即行的唯一定位标识）。
// 本测试覆盖三个方面：
// - 不同句柄模式（隐式 `_tidb_rowid`、整型主键即句柄、公共句柄）下
//   索引 Coprocessor 上下文选取的列集合；
// - 根据句柄列 ID 解析其在表达式列中的下标；
// - 虚拟生成列（virtual generated column）偏移与类型的收集。

use crate::{
    BuildContext, ColumnInfo, CopContext, ExprColumn, FieldType, IndexColumn, IndexInfo,
    NewCopContextMultiIndex, NewCopContextSingleIndex, TableInfo,
    collectVirtualColumnOffsetsAndTypes, resolveIndicesForHandle,
};

/// 构造 6 个名为 `c0`..`c5` 的 tinyint 测试列，列 ID 与偏移量一致。
fn columns() -> Vec<ColumnInfo> {
    (0..6)
        .map(|offset| ColumnInfo {
            ID: offset as i64,
            Name: format!("c{offset}"),
            Offset: offset,
            FieldType: FieldType {
                TypeName: "tinyint".to_owned(),
            },
            ..Default::default()
        })
        .collect()
}

/// 构造指定 ID 的索引信息，`offsets` 为索引各列在表中的列偏移。
fn index(id: i64, offsets: &[usize]) -> IndexInfo {
    IndexInfo {
        ID: id,
        Columns: offsets
            .iter()
            .copied()
            .map(|Offset| IndexColumn { Offset })
            .collect(),
        ..Default::default()
    }
}

/// 验证不同句柄模式下，单索引 Coprocessor 上下文选取的列集合与
/// Go 版 TiDB 的行为一致。
///
/// 三种句柄模式：
/// - 隐式句柄：表无显式主键，使用内部隐藏列 `_tidb_rowid` 定位行；
/// - PKIsHandle：整型主键列本身充当句柄；
/// - 公共句柄（common handle）：聚簇索引表，主键索引列组合作为句柄。
#[test]
fn surrogate_context_selects_go_equivalent_columns_for_handle_modes() {
    /// 单个测试用例：句柄模式配置、索引列偏移与期望选出的列名。
    struct Case {
        primary_key_is_handle: bool,
        common_handle: bool,
        index_offsets: &'static [usize],
        expected_columns: &'static [&'static str],
    }

    let cases = [
        // 隐式句柄：索引列之外还需追加隐藏的 _tidb_rowid 列。
        Case {
            primary_key_is_handle: false,
            common_handle: false,
            index_offsets: &[1],
            expected_columns: &["c1", "_tidb_rowid"],
        },
        Case {
            primary_key_is_handle: false,
            common_handle: false,
            index_offsets: &[1, 3],
            expected_columns: &["c1", "c3", "_tidb_rowid"],
        },
        // 整型主键即句柄：主键列 c0 作为句柄列被选入。
        Case {
            primary_key_is_handle: true,
            common_handle: false,
            index_offsets: &[1],
            expected_columns: &["c0", "c1"],
        },
        // 公共句柄：主键索引列 (c2, c4) 与索引列合并去重后按偏移排序。
        Case {
            primary_key_is_handle: false,
            common_handle: true,
            index_offsets: &[4, 1],
            expected_columns: &["c1", "c2", "c4"],
        },
    ];

    for (id, case) in cases.into_iter().enumerate() {
        let tested_index = index(id as i64, case.index_offsets);
        // 按用例配置构造表信息；公共句柄模式下额外提供主键索引。
        let table = TableInfo {
            Name: "t".to_owned(),
            Columns: columns(),
            PKIsHandle: case.primary_key_is_handle,
            IsCommonHandle: case.common_handle,
            PrimaryIndex: case.common_handle.then(|| index(99, &[2, 4])),
            ..Default::default()
        };

        // 构建单索引 Coprocessor 上下文，并校验其基础字段：
        // 列信息、字段类型、表达式列三者的数量与列名顺序均须匹配期望。
        let context =
            NewCopContextSingleIndex(BuildContext, 0, table, tested_index, "", false).unwrap();
        let base = context.CopContextBase;
        assert_eq!(base.TableInfo.Name, "t");
        assert_eq!(base.PrimaryKeyInfo.is_some(), case.common_handle);
        assert_eq!(base.ColumnInfos.len(), case.expected_columns.len());
        assert_eq!(base.FieldTypes.len(), case.expected_columns.len());
        assert_eq!(base.ExprColumnInfos.len(), case.expected_columns.len());
        assert_eq!(
            base.ColumnInfos
                .iter()
                .map(|column| column.Name.as_str())
                .collect::<Vec<_>>(),
            case.expected_columns
        );
    }
}

/// 验证 `resolveIndicesForHandle` 按请求的句柄列 ID 顺序返回下标。
///
/// 该函数根据句柄列 ID 在表达式列列表中查找位置，结果顺序必须
/// 与传入的 ID 顺序一致，而不是列在表中的物理顺序。
#[test]
fn surrogate_handle_indices_preserve_requested_id_order() {
    // 构造 ID 为 1、2、3 的三个表达式列，下标依次为 0、1、2。
    let columns = [1, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(Index, ID)| ExprColumn {
            ID,
            Index,
            ..Default::default()
        })
        .collect::<Vec<_>>();

    for (handles, expected) in [
        (vec![2], vec![1]),
        (vec![3, 2, 1], vec![2, 1, 0]),
        (vec![1, 3], vec![0, 2]),
    ] {
        assert_eq!(resolveIndicesForHandle(&columns, &handles), expected);
    }
}

/// 验证 `collectVirtualColumnOffsetsAndTypes` 只收集虚拟生成列，
/// 并保持它们的偏移顺序与字段类型对应关系。
///
/// 虚拟生成列（virtual generated column）的值由表达式实时计算、
/// 不落盘存储，因此 Coprocessor 读取时需要单独记录其位置与类型
/// 以便在读取后重新求值。
#[test]
fn surrogate_virtual_columns_preserve_offsets_and_types() {
    for (columns, expected_offsets, expected_types) in [
        // 用例 1：第 0、2 列为虚拟列，期望偏移 [0, 2]、类型 ["1", "2"]。
        (
            vec![
                ExprColumn {
                    VirtualExpr: true,
                    FieldType: FieldType {
                        TypeName: "1".to_owned(),
                    },
                    ..Default::default()
                },
                ExprColumn::default(),
                ExprColumn {
                    VirtualExpr: true,
                    FieldType: FieldType {
                        TypeName: "2".to_owned(),
                    },
                    ..Default::default()
                },
            ],
            vec![0, 2],
            vec!["1", "2"],
        ),
        // 用例 2：仅第 1 列为虚拟列，期望偏移 [1]、类型 ["1"]。
        (
            vec![
                ExprColumn::default(),
                ExprColumn {
                    VirtualExpr: true,
                    FieldType: FieldType {
                        TypeName: "1".to_owned(),
                    },
                    ..Default::default()
                },
                ExprColumn::default(),
            ],
            vec![1],
            vec!["1"],
        ),
    ] {
        let (offsets, field_types) = collectVirtualColumnOffsetsAndTypes(&columns);
        assert_eq!(offsets, expected_offsets);
        assert_eq!(
            field_types
                .iter()
                .map(|field_type| field_type.TypeName.as_str())
                .collect::<Vec<_>>(),
            expected_types
        );
    }
}

/// 条件索引构造时必须像 Go 实现一样解析条件列，并在条件非法或引用未知列时
/// 直接返回错误，而不是悄悄构造一个缺少扫描列的上下文。
#[test]
fn condition_indexes_validate_go_expression_inputs() {
    let table = TableInfo {
        Name: "t".to_owned(),
        Columns: columns(),
        ..Default::default()
    };

    let context = NewCopContextSingleIndex(
        BuildContext,
        0,
        table.clone(),
        IndexInfo {
            ID: 1,
            Columns: vec![IndexColumn { Offset: 1 }],
            ConditionExprString: "c2 > c0".to_owned(),
        },
        "",
        false,
    )
    .unwrap();
    assert_eq!(
        context
            .CopContextBase
            .ColumnInfos
            .iter()
            .map(|column| column.Name.as_str())
            .collect::<Vec<_>>(),
        vec!["c0", "c1", "c2", "_tidb_rowid"]
    );
    assert_eq!(context.GetCondition().unwrap().unwrap().Text, "c2 > c0");

    let mut reordered_pk_columns = columns();
    reordered_pk_columns[2].PrimaryKey = true;
    let reordered_pk = NewCopContextSingleIndex(
        BuildContext,
        0,
        TableInfo {
            Name: "t".to_owned(),
            Columns: reordered_pk_columns,
            PKIsHandle: true,
            ..Default::default()
        },
        index(5, &[1]),
        "",
        false,
    )
    .unwrap();
    assert_eq!(reordered_pk.CopContextBase.HandleOutputOffsets, vec![1]);
    assert_eq!(
        reordered_pk
            .CopContextBase
            .ColumnInfos
            .iter()
            .map(|column| column.Name.as_str())
            .collect::<Vec<_>>(),
        vec!["c1", "c2"]
    );

    let quoted_and_literal = NewCopContextSingleIndex(
        BuildContext,
        0,
        table.clone(),
        IndexInfo {
            ID: 3,
            Columns: vec![IndexColumn { Offset: 1 }],
            ConditionExprString: "c1 = 'c0' AND `c2` IS NULL".to_owned(),
        },
        "",
        false,
    )
    .unwrap();
    assert_eq!(
        quoted_and_literal
            .CopContextBase
            .ColumnInfos
            .iter()
            .map(|column| column.Name.as_str())
            .collect::<Vec<_>>(),
        vec!["c1", "c2", "_tidb_rowid"]
    );

    let mut virtual_columns = columns();
    virtual_columns[2].VirtualExpr = true;
    let virtual_condition = NewCopContextSingleIndex(
        BuildContext,
        0,
        TableInfo {
            Name: "t".to_owned(),
            Columns: virtual_columns,
            ..Default::default()
        },
        IndexInfo {
            ID: 4,
            Columns: vec![IndexColumn { Offset: 1 }],
            ConditionExprString: "c2 > 0".to_owned(),
        },
        "",
        false,
    )
    .unwrap();
    assert!(virtual_condition.GetCondition().unwrap().is_none());

    for condition in ["missing = 1", "c1 >", "   "] {
        let result = NewCopContextSingleIndex(
            BuildContext,
            0,
            table.clone(),
            IndexInfo {
                ID: 2,
                Columns: vec![IndexColumn { Offset: 1 }],
                ConditionExprString: condition.to_owned(),
            },
            "",
            false,
        );
        assert!(
            result.is_err(),
            "condition should be rejected: {condition:?}"
        );
    }
}

/// 多索引上下文需要保留每个索引的查找与偏移映射，并在任一索引没有条件时
/// 按 Go 语义放弃整体条件下推。
#[test]
fn multi_index_context_matches_go_condition_and_lookup_semantics() {
    let indexes = vec![
        IndexInfo {
            ID: 10,
            Columns: vec![IndexColumn { Offset: 1 }],
            ConditionExprString: "c1 > 0".to_owned(),
        },
        IndexInfo {
            ID: 11,
            Columns: vec![IndexColumn { Offset: 2 }],
            ConditionExprString: "c2 < 10".to_owned(),
        },
    ];
    let context = NewCopContextMultiIndex(
        BuildContext,
        0,
        TableInfo {
            Name: "t".to_owned(),
            Columns: columns(),
            ..Default::default()
        },
        indexes,
        "",
        false,
    )
    .unwrap();

    assert_eq!(context.IndexInfo(10).unwrap().ID, 10);
    assert!(context.IndexInfo(99).is_none());
    assert_eq!(context.IndexColumnOutputOffsets(10), vec![0]);
    assert_eq!(context.IndexColumnOutputOffsets(11), vec![1]);
    assert!(context.GetCondition().unwrap().is_some());

    let no_condition = NewCopContextMultiIndex(
        BuildContext,
        0,
        TableInfo {
            Name: "t".to_owned(),
            Columns: columns(),
            ..Default::default()
        },
        vec![
            IndexInfo {
                ID: 10,
                Columns: vec![IndexColumn { Offset: 1 }],
                ConditionExprString: "c1 > 0".to_owned(),
            },
            index(11, &[2]),
        ],
        "",
        false,
    )
    .unwrap();
    assert!(no_condition.GetCondition().unwrap().is_none());
}
