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

// misc 模块单元测试。
//
// 覆盖列替换时返回类型/IN 操作数标记保留、表达式与投影按 schema 位置替换、
// 外/内表列归属判断、最大一行（Max One Row）等值条件，以及唯一索引可否作为键。

use std::collections::{HashMap, HashSet};

use crate::*;
use expression::{Column, ExprBox, Expression, NewSchema};

/// 构造仅含 UniqueID/ID 的测试列。
fn column(unique_id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = unique_id;
    column.ID = unique_id;
    column
}

/// 按表达式 HashCode 建立 origin → destination 的列替换表。
fn replacement_map(origin: &Column, destination: Column) -> ColumnReplaceMap {
    HashMap::from([(Expression::HashCode(origin), destination)])
}

/// 验证替换后保留原点列的 RetType 与 InOperand 标记。
#[test]
fn column_replacement_preserves_origin_type_markers() {
    let mut origin = column(1);
    origin.InOperand = true;
    let destination = column(9);
    let replacements = replacement_map(&origin, destination);

    let replaced = ResolveColumnAndReplace(&origin, &replacements);
    assert_eq!(replaced.UniqueID, 9);
    assert_eq!(replaced.RetType, origin.RetType);
    assert!(replaced.InOperand);

    let unchanged = ResolveColumnAndReplace(&column(2), &replacements);
    assert_eq!(unchanged.UniqueID, 2);
}

/// 验证 ResolveExprAndReplace 与 ReplaceColumnOfExpr 按 schema 下标替换。
#[test]
fn expression_and_projection_replacement_match_schema_positions() {
    let origin = column(1);
    let replacements = replacement_map(&origin, column(8));
    let rewritten = ResolveExprAndReplace(Box::new(origin.clone()), &replacements);
    assert_eq!(
        rewritten
            .as_any()
            .downcast_ref::<Column>()
            .unwrap()
            .UniqueID,
        8
    );

    let schema = NewSchema(vec![origin]);
    let projection: Vec<ExprBox> = vec![Box::new(column(11))];
    let projected = ReplaceColumnOfExpr(Box::new(column(1)), &projection, &schema);
    assert_eq!(
        projected
            .as_any()
            .downcast_ref::<Column>()
            .unwrap()
            .UniqueID,
        11
    );
}

/// 验证空列集、部分命中外层列，以及 PK/UK/可空 UK 上的最大一行条件。
#[test]
fn outer_inner_and_max_one_row_checks_cover_go_edge_cases() {
    let mut outer = intset::FastIntSet::default();
    outer.Insert(1);
    outer.Insert(2);
    assert!(!IsColsAllFromOuterTable(&[], &outer));
    assert!(IsColsAllFromOuterTable(&[column(1), column(2)], &outer));
    assert!(!IsColsAllFromOuterTable(&[column(1), column(3)], &outer));
    assert!(IsColFromInnerTable(&[column(3), column(2)], &outer));
    assert!(!IsColFromInnerTable(&[column(3)], &outer));

    let mut schema = NewSchema(vec![column(1), column(2), column(3)]);
    schema.PKOrUK = vec![vec![column(1), column(2)]];
    schema.NullableUK = vec![vec![column(3)]];
    assert!(!CheckMaxOneRowCond(&HashSet::new(), &schema));
    assert!(!CheckMaxOneRowCond(&HashSet::from([1]), &schema));
    assert!(CheckMaxOneRowCond(&HashSet::from([1, 2]), &schema));
    assert!(CheckMaxOneRowCond(&HashSet::from([3]), &schema));
}

/// 构造带名称与可选 NOT NULL 标志的表列元信息。
fn named_column(name: &str, not_null: bool) -> model::ColumnInfo {
    let mut column = model::ColumnInfo::default();
    column.Name.O = name.to_owned();
    column.Name.L = name.to_ascii_lowercase();
    if not_null {
        column.FieldType.SetFlag(mysql::r#type::NotNullFlag);
    }
    column
}

/// 构造仅含列名列表的唯一索引元信息。
fn unique_index(names: &[&str]) -> model::IndexInfo {
    let mut index = model::IndexInfo {
        Unique: true,
        ..Default::default()
    };
    index.Columns = names
        .iter()
        .map(|name| {
            let mut column = model::IndexColumn::default();
            column.Name.O = (*name).to_owned();
            column.Name.L = name.to_ascii_lowercase();
            column
        })
        .collect();
    index
}

/// 区分全非空唯一键、可空唯一键，以及索引列在表中找不到的情况。
#[test]
fn index_key_classification_distinguishes_nullable_and_missing_columns() {
    let schema = NewSchema(vec![column(10), column(20)]);
    let columns = vec![named_column("a", true), named_column("b", true)];
    let (nullable, key) = CheckIndexCanBeKey(&unique_index(&["a", "b"]), &columns, &schema);
    assert!(nullable.is_none());
    assert_eq!(
        key.unwrap()
            .into_iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>(),
        vec![10, 20]
    );

    let nullable_columns = vec![named_column("a", true), named_column("b", false)];
    let (nullable, key) =
        CheckIndexCanBeKey(&unique_index(&["a", "b"]), &nullable_columns, &schema);
    assert!(key.is_none());
    assert_eq!(nullable.unwrap().len(), 2);

    let (nullable, key) = CheckIndexCanBeKey(&unique_index(&["missing"]), &columns, &schema);
    assert!(nullable.is_none());
    assert!(key.is_none());
}
