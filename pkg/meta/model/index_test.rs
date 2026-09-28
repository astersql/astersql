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

// 索引前缀覆盖与外键 partial index 条件规则测试。

use crate::group_1::*;
use crate::group_2::serde_json;
//

// new_column_for_test 对应 Go 的 newColumnForTest：生成带固定 ID、名称和 offset 的列元数据。
fn new_column_for_test(id: i64, offset: isize) -> ColumnInfo {
    ColumnInfo {
        ID: id,
        Name: ast::NewCIStr(&format!("c_{}", id)),
        Offset: offset,
        ..Default::default()
    }
}

// new_index_for_test 对应 Go 的 newIndexForTest：把列 offset/name 复制进 IndexColumn，模拟索引定义。
fn new_index_for_test(id: i64, cols: &[&ColumnInfo]) -> IndexInfo {
    let mut idx_cols = Vec::with_capacity(cols.len());
    for c in cols {
        idx_cols.push(IndexColumn {
            Offset: c.Offset,
            Name: c.Name.clone(),
            ..Default::default()
        });
    }
    IndexInfo {
        ID: id,
        Name: ast::NewCIStr(&format!("i_{}", id)),
        Columns: idx_cols,
        ..Default::default()
    }
}

#[test]
fn test_is_index_prefix_covered() {
    let c0 = new_column_for_test(0, 0);
    let c1 = new_column_for_test(1, 1);
    let c2 = new_column_for_test(2, 2);
    let c3 = new_column_for_test(3, 3);
    let c4 = new_column_for_test(4, 4);

    let i0 = new_index_for_test(0, &[&c0, &c1, &c2]);
    let i1 = new_index_for_test(1, &[&c4, &c2]);
    let tbl = TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        Columns: vec![c0.clone(), c1.clone(), c2.clone(), c3.clone(), c4.clone()],
        Indices: vec![i0.clone(), i1.clone()],
        ..Default::default()
    };

    // 普通索引只允许从第一列开始按顺序覆盖；跳过前缀或调换顺序都应返回 false。
    assert_eq!(
        true,
        IsIndexPrefixCovered(&tbl, &i0, &[ast::NewCIStr("c_0")])
    );
    assert_eq!(
        true,
        IsIndexPrefixCovered(
            &tbl,
            &i0,
            &[
                ast::NewCIStr("c_0"),
                ast::NewCIStr("c_1"),
                ast::NewCIStr("c_2")
            ]
        )
    );
    assert_eq!(
        false,
        IsIndexPrefixCovered(&tbl, &i0, &[ast::NewCIStr("c_1")])
    );
    assert_eq!(
        false,
        IsIndexPrefixCovered(&tbl, &i0, &[ast::NewCIStr("c_2")])
    );
    assert_eq!(
        false,
        IsIndexPrefixCovered(&tbl, &i0, &[ast::NewCIStr("c_1"), ast::NewCIStr("c_2")])
    );
    assert_eq!(
        false,
        IsIndexPrefixCovered(&tbl, &i0, &[ast::NewCIStr("c_0"), ast::NewCIStr("c_2")])
    );

    // 第二个索引的首列是 c_4，测试覆盖逻辑不能默认按表列顺序判断。
    assert_eq!(
        true,
        IsIndexPrefixCovered(&tbl, &i1, &[ast::NewCIStr("c_4")])
    );
    assert_eq!(
        true,
        IsIndexPrefixCovered(&tbl, &i1, &[ast::NewCIStr("c_4"), ast::NewCIStr("c_2")])
    );
    assert_eq!(
        false,
        IsIndexPrefixCovered(&tbl, &i0, &[ast::NewCIStr("c_2")])
    );

    let mut safe_partial = new_index_for_test(2, &[&c0, &c1]);
    safe_partial.ConditionExprString = "`c_1` is not null".to_string();
    assert!(IsIndexPrefixCoveredForForeignKey(
        &tbl,
        &safe_partial,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")]
    ));

    let mut safe_partial_on_first_fk_col = new_index_for_test(3, &[&c0, &c1]);
    safe_partial_on_first_fk_col.ConditionExprString = "`c_0` is not null".to_string();
    assert!(IsIndexPrefixCoveredForForeignKey(
        &tbl,
        &safe_partial_on_first_fk_col,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")]
    ));

    // partial index 的条件若引用非外键列、使用 IS NULL、二元比较或无法解析，都不能作为外键覆盖索引。
    let mut unsafe_partial_on_non_fk_col = new_index_for_test(4, &[&c0, &c1]);
    unsafe_partial_on_non_fk_col.ConditionExprString = "`c_2` is not null".to_string();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &tbl,
        &unsafe_partial_on_non_fk_col,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")]
    ));

    let mut unsafe_partial_is_null = new_index_for_test(5, &[&c0]);
    unsafe_partial_is_null.ConditionExprString = "`c_0` is null".to_string();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &tbl,
        &unsafe_partial_is_null,
        &[ast::NewCIStr("c_0")]
    ));

    let mut unsafe_partial_binary_condition = new_index_for_test(6, &[&c0]);
    unsafe_partial_binary_condition.ConditionExprString = "`c_0` > 0".to_string();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &tbl,
        &unsafe_partial_binary_condition,
        &[ast::NewCIStr("c_0")]
    ));

    let mut bad_condition = new_index_for_test(7, &[&c0]);
    bad_condition.ConditionExprString = "`c_0` is".to_string();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &tbl,
        &bad_condition,
        &[ast::NewCIStr("c_0")]
    ));

    // Go 的 require.Same 校验返回的是候选切片里的同一个 safePartial 指针。
    let candidates = [unsafe_partial_on_non_fk_col.clone(), safe_partial.clone()];
    let found = FindIndexByColumnsForForeignKey(
        &tbl,
        &candidates,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")],
    );
    assert!(std::ptr::eq(found.unwrap(), &candidates[1]));
}

#[test]
fn test_global_index_v1_supported_for_next_gen() {
    struct RestoreGlobalIndexV1Support(bool);

    impl Drop for RestoreGlobalIndexV1Support {
        fn drop(&mut self) {
            SetGlobalIndexV1Supported(self.0);
        }
    }

    let initial = GetGlobalIndexV1Supported();
    {
        let _restore = RestoreGlobalIndexV1Support(initial);

        assert_eq!(initial, kerneltype::IsNextGen());

        SetGlobalIndexV1Supported(!initial);
        assert_eq!(GetGlobalIndexV1Supported(), !initial);
        SetGlobalIndexV1Supported(initial);
        assert_eq!(GetGlobalIndexV1Supported(), initial);

        SetGlobalIndexV1Supported(!initial);
    }
    assert_eq!(GetGlobalIndexV1Supported(), initial);
}

#[test]
fn columnar_index_type_json_matches_go_uint8_encoding() {
    assert_eq!(serde_json::to_string(&ColumnarIndexType::NA).unwrap(), "0");
    assert_eq!(
        serde_json::to_string(&ColumnarIndexType::Inverted).unwrap(),
        "1"
    );
    assert_eq!(
        serde_json::to_string(&ColumnarIndexType::Vector).unwrap(),
        "2"
    );
    assert_eq!(
        serde_json::to_string(&ColumnarIndexType::Fulltext).unwrap(),
        "3"
    );
    assert_eq!(
        serde_json::from_str::<ColumnarIndexType>("2").unwrap(),
        ColumnarIndexType::Vector
    );
}
use crate::group_1::{ColumnInfo, IndexColumn, IndexInfo, IsIndexPrefixCovered, TableInfo, ast};

/// 精简断言：索引前缀覆盖要求从左侧按序匹配列，不能跳过前缀。
#[test]
fn index_prefix_requires_ordered_leading_columns() {
    let columns = vec![
        ColumnInfo {
            ID: 1,
            Name: ast::NewCIStr("a"),
            Offset: 0,
            ..Default::default()
        },
        ColumnInfo {
            ID: 2,
            Name: ast::NewCIStr("b"),
            Offset: 1,
            ..Default::default()
        },
    ];
    let index = IndexInfo {
        Columns: columns
            .iter()
            .map(|column| IndexColumn {
                Name: column.Name.clone(),
                Offset: column.Offset,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let table = TableInfo {
        Columns: columns,
        Indices: vec![index.clone()],
        ..Default::default()
    };
    assert!(IsIndexPrefixCovered(&table, &index, &[ast::NewCIStr("a")]));
    assert!(IsIndexPrefixCovered(
        &table,
        &index,
        &[ast::NewCIStr("a"), ast::NewCIStr("b")]
    ));
    assert!(!IsIndexPrefixCovered(&table, &index, &[ast::NewCIStr("b")]));
}
