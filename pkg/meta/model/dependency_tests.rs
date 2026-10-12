// Copyright 2026 AsterSQL.

// 验证列、索引、表三者共享同一列名身份时，前缀覆盖判定
// `IsIndexPrefixCovered` 成立。

use crate::{ColumnInfo, IndexColumn, IndexInfo, IsIndexPrefixCovered, TableInfo, ast};

/// 列、索引、表引用同一列名时，索引前缀应覆盖该列名列表。
#[test]
fn canonical_model_dependencies_share_one_identity() {
    // 单列 id，索引包含该列，表同时挂上列与索引。
    let column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("id"),
        Offset: 0,
        ..Default::default()
    };
    let index = IndexInfo {
        Columns: vec![IndexColumn {
            Name: column.Name.clone(),
            Offset: column.Offset,
            ..Default::default()
        }],
        ..Default::default()
    };
    let table = TableInfo {
        Columns: vec![column],
        Indices: vec![index.clone()],
        ..Default::default()
    };
    // IsIndexPrefixCovered：查询列名是否被索引列前缀完整覆盖（可用于索引选择）。
    assert!(IsIndexPrefixCovered(&table, &index, &[ast::NewCIStr("id")]));
}
