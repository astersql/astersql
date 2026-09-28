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

// Aster 单元测试：BDR 映射、列默认值、库拷贝与索引前缀覆盖等元数据行为。
//
// 补充覆盖 BDR、列、DB 与索引模型的跨文件行为。

use crate::group_1::*;
use std::sync::Arc;

#[test]
fn bdr_maps_all_actions_and_converts_tso_millis() {
    let total: usize = BDRActionMap.values().map(Vec::len).sum();
    assert_eq!(total, ActionBDRMap.len());
    assert_eq!(ActionBDRMap.get(&ACTION_CREATE_TABLE), Some(&SafeDDL));
    assert_eq!(ActionBDRMap.get(&ACTION_DROP_TABLE), Some(&UnsafeDDL));
    assert_eq!(
        ActionBDRMap.get(&ACTION_CREATE_RESOURCE_GROUP),
        Some(&UnmanagementDDL)
    );
    assert_eq!(
        ActionBDRMap.get(&DEPRECATED_ACTION_ALTER_TABLE_ALTER_PARTITION),
        Some(&UnknownDDL)
    );

    let millis = 1_720_000_123_456_i64;
    assert_eq!(
        TSConvert2Time((millis as u64) << 18).timestamp_millis(),
        millis
    );
}

#[test]
fn column_defaults_names_flags_and_extra_columns_match_go() {
    let mut bit = ColumnInfo::New(1, ast::NewCIStr("bit_col"));
    bit.SetType(mysql::TypeBit);
    let error = bit.SetDefaultValue(Some(DefaultValue::Int(1))).unwrap_err();
    assert!(error.to_string().contains("Invalid default value"));
    assert_eq!(bit.GetDefaultValue(), Some(DefaultValue::Int(1)));

    let raw = vec![25, 185];
    bit.SetDefaultValue(Some(DefaultValue::String(raw.clone())))
        .unwrap();
    assert_eq!(bit.GetDefaultValue(), Some(DefaultValue::String(raw)));

    let old = ColumnInfo::New(2, ast::NewCIStr("Mixed"));
    let occupied = ColumnInfo::New(3, ast::NewCIStr("_COL$_MIXED_0"));
    let table = TableInfo {
        Columns: vec![occupied],
        ..Default::default()
    };
    assert_eq!(GenUniqueChangingColumnName(&table, &old), "_Col$_Mixed_1");
    assert_eq!(GenRemovingObjName("c"), "_Tombstone$_c");
    assert_eq!(GenRemovingObjName("_Tombstone$_c"), "_Tombstone$_c");

    let extra = NewExtraPhysTblIDColInfo();
    assert_eq!(extra.GetType(), mysql::TypeLonglong);
    assert_eq!(extra.GetFlag(), mysql::NotNullFlag);
    assert_eq!(FLAG_IN_SELECT_STMT, 1 << 5);
    assert_eq!(FlagInRestrictedSQL, 1 << 11);
}

#[test]
fn db_clone_is_deep_copy_while_copy_shares_tables() {
    let table = Arc::new(TableInfo {
        ID: 7,
        Revision: 11,
        ..Default::default()
    });
    let db = DBInfo {
        ID: 1,
        Name: ast::NewCIStr("Zoo"),
        Deprecated: DeprecatedDBInfo {
            Tables: vec![table.clone()],
        },
        ..Default::default()
    };

    let deep = db.Clone();
    let shallow = db.Copy();
    assert!(!Arc::ptr_eq(
        &db.Deprecated.Tables[0],
        &deep.Deprecated.Tables[0]
    ));
    assert!(Arc::ptr_eq(
        &db.Deprecated.Tables[0],
        &shallow.Deprecated.Tables[0]
    ));
    assert_eq!(
        LessDBInfo(
            &db,
            &DBInfo {
                Name: ast::NewCIStr("zoo"),
                ..Default::default()
            }
        ),
        0
    );
}

#[test]
fn index_metadata_and_partial_foreign_key_rules_match_go() {
    let mut int_type = types::NewFieldType(mysql::TypeLong);
    int_type.AddFlag(mysql::UnsignedFlag);
    let inverted = FieldTypeToInvertedIndexInfo(&int_type, 9).unwrap();
    assert_eq!(
        (inverted.ColumnID, inverted.IsSigned, inverted.TypeSize),
        (9, false, 4)
    );
    assert!(FieldTypeToInvertedIndexInfo(&types::NewFieldType(mysql::TypeVarchar), 9).is_none());
    assert_eq!(
        GetFullTextParserTypeBySQLName("standard").SQLName(),
        "STANDARD"
    );

    let mut c0 = ColumnInfo::New(0, ast::NewCIStr("c_0"));
    c0.Offset = 0;
    c0.SetFlen(20);
    let mut c1 = ColumnInfo::New(1, ast::NewCIStr("c_1"));
    c1.Offset = 1;
    c1.SetFlen(20);
    let table = TableInfo {
        Columns: vec![c0, c1],
        ..Default::default()
    };
    let mut index = IndexInfo::default();
    index.Columns = vec![
        IndexColumn {
            Name: ast::NewCIStr("c_0"),
            Offset: 0,
            Length: types::UnspecifiedLength,
            ..Default::default()
        },
        IndexColumn {
            Name: ast::NewCIStr("c_1"),
            Offset: 1,
            Length: types::UnspecifiedLength,
            ..Default::default()
        },
    ];
    assert!(IsIndexPrefixCovered(
        &table,
        &index,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")]
    ));

    index.ConditionExprString = "`c_1` is not null".into();
    assert!(IsIndexPrefixCoveredForForeignKey(
        &table,
        &index,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")]
    ));
    index.ConditionExprString = "`other` is not null".into();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &table,
        &index,
        &[ast::NewCIStr("c_0"), ast::NewCIStr("c_1")]
    ));
    index.ConditionExprString = "`c_0` > 0".into();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &table,
        &index,
        &[ast::NewCIStr("c_0")]
    ));
    index.ConditionExprString = "`c_0` is".into();
    assert!(!IsIndexPrefixCoveredForForeignKey(
        &table,
        &index,
        &[ast::NewCIStr("c_0")]
    ));
}
use crate::group_1::{ACTION_CREATE_TABLE, ACTION_DROP_TABLE, ActionBDRMap, SafeDDL, UnsafeDDL};

/// 校验 CREATE_TABLE 归为 SafeDDL、DROP_TABLE 归为 UnsafeDDL。
#[test]
fn bdr_classifies_safe_and_unsafe_table_actions() {
    assert_eq!(ActionBDRMap.get(&ACTION_CREATE_TABLE), Some(&SafeDDL));
    assert_eq!(ActionBDRMap.get(&ACTION_DROP_TABLE), Some(&UnsafeDDL));
}
