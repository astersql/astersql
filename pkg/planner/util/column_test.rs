// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `IndexInfo2PrefixCols` / `IndexInfo2FullCols` / `IndexInfo2Cols` 单元测试。
//
// 覆盖：前缀截断、缺失列填 None、以及 FullCols 与 IndexInfo2Cols 完整侧一致。

use super::*;
use ::expression::{Column, mysql, types};

/// 构造带给定 UniqueID 的测试列。
fn column(id: i64) -> Column {
    Column::new(*types::NewFieldType(mysql::TypeLonglong), id, id, 0)
}

/// 构造名称与 ID 对应的列元信息。
fn column_info(id: i64) -> model::ColumnInfo {
    model::ColumnInfo {
        ID: id,
        Name: parser_ast::NewCIStr(&id.to_string()),
        FieldType: *types::NewFieldType(mysql::TypeLonglong),
        ..Default::default()
    }
}

/// 验证前缀提取、缺失列截断/占位，以及 FullCols 与 Cols 完整侧对齐。
#[test]
fn test_index_info2_cols() {
    crate::main_test::setup_for_planner_util_test();
    let col0 = column(0);
    let col1 = column(1);
    let col2 = column(2);
    let col_info0 = column_info(0);
    let col_info1 = column_info(1);
    let col_info2 = column_info(2);
    // 索引含列 0/1/2，Length=-1 表示整列非前缀。
    let index_info = model::IndexInfo {
        Columns: (0..3)
            .map(|id| model::IndexColumn {
                Name: parser_ast::NewCIStr(&id.to_string()),
                Length: -1,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    // 仅提供列 0：前缀长度为 1。
    let (columns, lengths) =
        IndexInfo2PrefixCols(&[col_info0.clone()], &[col0.clone()], &index_info);
    assert_eq!(columns.len(), 1);
    assert_eq!(lengths, vec![-1]);
    assert!(columns[0].EqualColumn(&col0));

    // 仅提供列 1：首列缺失，前缀为空。
    let (columns, lengths) =
        IndexInfo2PrefixCols(&[col_info1.clone()], &[col1.clone()], &index_info);
    assert!(columns.is_empty());
    assert!(lengths.is_empty());

    // 提供列 0、1：前缀长度为 2。
    let (columns, lengths) = IndexInfo2PrefixCols(
        &[col_info0.clone(), col_info1],
        &[col0.clone(), col1],
        &index_info,
    );
    assert_eq!(columns.len(), 2);
    assert_eq!(lengths, vec![-1, -1]);
    assert!(columns[0].EqualColumn(&col0));

    // 提供列 0、2：前缀截断在缺失的列 1；完整侧列 1 为 None。
    let infos = [col_info0, col_info2];
    let source = [col0.clone(), col2.clone()];
    let (prefix, prefix_lengths, full, full_lengths) = IndexInfo2Cols(&infos, &source, &index_info);
    assert_eq!(prefix.len(), 1);
    assert_eq!(prefix_lengths, vec![-1]);
    assert!(prefix[0].EqualColumn(&col0));
    assert_eq!(full.len(), 3);
    assert_eq!(full_lengths, vec![-1, -1, -1]);
    assert!(
        full[0]
            .as_ref()
            .is_some_and(|column| column.EqualColumn(&col0))
    );
    assert!(full[1].is_none());
    assert!(
        full[2]
            .as_ref()
            .is_some_and(|column| column.EqualColumn(&col2))
    );

    // IndexInfo2FullCols 应与 IndexInfo2Cols 的完整侧一致。
    let (direct_full, direct_lengths) = IndexInfo2FullCols(&infos, &source, &index_info);
    assert_eq!(direct_lengths, full_lengths);
    assert_eq!(direct_full.len(), full.len());
    for (left, right) in direct_full.iter().zip(&full) {
        assert_eq!(left.is_some(), right.is_some());
        if let (Some(left), Some(right)) = (left, right) {
            assert!(left.EqualColumn(right));
        }
    }
}
