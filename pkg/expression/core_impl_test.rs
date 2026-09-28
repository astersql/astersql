// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 表达式核心实现（core_impl）相关冒烟测试。
//
// 覆盖 Schema 按 UniqueID 的列查找规则、整数/NULL 常量元数据，
// 以及 `ExprBox` 深度克隆后列身份保持不变。

use crate::*;

/// 构造带 UniqueID 与 Index 的简易 Longlong 列，供 Schema 测试使用。
fn column(unique_id: i64, index: isize) -> Column {
    Column {
        RetType: Some(*types::NewFieldType(mysql::TypeLonglong)),
        UniqueID: unique_id,
        Index: index,
        ..Default::default()
    }
}

/// Schema 克隆与按列身份查找：前缀列不抢占同 UniqueID 的完整列下标。
#[test]
fn schema_clone_and_lookup_keep_go_column_identity_rules() {
    let first = column(11, 0);
    let prefix = Column {
        IsPrefix: true,
        ..column(22, 1)
    };
    let full = column(22, 2);
    let schema = NewSchema(vec![first.clone(), prefix, full]);

    assert_eq!(schema.ColumnIndex(&first), Some(0));
    assert_eq!(schema.ColumnIndex(&column(22, -1)), Some(2));
    let cloned = schema.Clone();
    assert!(schema.Equal(&cloned));
    assert_eq!(cloned.Len(), 3);
}

/// NewOne / NewNull 保留 UnsignedFlag、TypeTiny 与 ConstStrict 级别。
#[test]
fn integer_and_null_constants_keep_mysql_metadata() {
    let one = NewOne();
    assert_eq!(one.Value.GetInt64(), 1);
    assert!(mysql::HasUnsignedFlag(
        one.RetType.as_ref().unwrap().GetFlag()
    ));
    assert_eq!(one.ConstLevel(), ConstStrict);

    let null = NewNull();
    assert!(null.Value.IsNull());
    assert_eq!(null.RetType.as_ref().unwrap().GetType(), mysql::TypeTiny);
}

/// ExprBox 经 CloneExpr 后仍可还原为列并保留 UniqueID / Index。
#[test]
fn expression_boxes_are_deeply_cloneable() {
    let original: ExprBox = Box::new(column(42, 3));
    let cloned = original.CloneExpr();
    let cloned_column = cloned.as_column().expect("column clone");
    assert_eq!(cloned_column.UniqueID, 42);
    assert_eq!(cloned_column.Index, 3);
}
