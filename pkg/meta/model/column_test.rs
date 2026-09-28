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

// 列默认值相关测试：BIT 列校验、JSON 往返兼容与额外物理表 ID 列元数据。

use crate::group_1::*;
use crate::group_2::serde_json;

// DefaultValueCase 对应 Go 匿名测试用例，记录待 JSON 往返的列与是否应保持默认值一致。
struct DefaultValueCase {
    col: ColumnInfo,
    is_consistent: bool,
}

fn round_trip_column(column: &ColumnInfo) -> ColumnInfo {
    let bytes = serde_json::to_vec(column).expect("marshal ColumnInfo compatibility shape");
    serde_json::from_slice(&bytes).expect("unmarshal ColumnInfo compatibility shape")
}

#[test]
fn test_default_value() {
    // 源列只设置 ID；各个分支都通过 Clone 复制基础元数据，保持 Go 的浅复制测试形状。
    let src_col = ColumnInfo {
        ID: 1,
        ..ColumnInfo::default()
    };
    let rand_plain_str = "random_plain_string";

    let mut old_plain_col = src_col.Clone();
    old_plain_col.Name = ast::NewCIStr("oldPlainCol");
    old_plain_col.FieldType = types::NewFieldType(mysql::TypeLong);
    // 旧 plain 列直接写 DefaultValue 与 OriginDefaultValue，模拟历史 JSON 中已有的动态默认值。
    old_plain_col.DefaultValue = Some(DefaultValue::String(rand_plain_str.as_bytes().to_vec()));
    old_plain_col.OriginDefaultValue =
        Some(DefaultValue::String(rand_plain_str.as_bytes().to_vec()));

    let mut new_plain_col = src_col.Clone();
    new_plain_col.Name = ast::NewCIStr("newPlainCol");
    new_plain_col.FieldType = types::NewFieldType(mysql::TypeLong);
    let mut err = new_plain_col.SetDefaultValue(Some(DefaultValue::Int(1)));
    assert!(err.is_ok());
    assert_eq!(Some(DefaultValue::Int(1)), new_plain_col.GetDefaultValue());
    err = new_plain_col.SetDefaultValue(Some(DefaultValue::String(
        rand_plain_str.as_bytes().to_vec(),
    )));
    assert!(err.is_ok());
    assert_eq!(
        Some(DefaultValue::String(rand_plain_str.as_bytes().to_vec())),
        new_plain_col.GetDefaultValue()
    );

    // Go 的 string([]byte{25, 185}) 保留非 UTF-8 字节形状；BIT 默认值要通过字节字段跨 JSON 保存。
    let rand_bit_str = vec![25_u8, 185_u8];

    let mut old_bit_col = src_col.Clone();
    old_bit_col.Name = ast::NewCIStr("oldBitCol");
    old_bit_col.FieldType = types::NewFieldType(mysql::TypeBit);
    // 旧 BIT 列直接写动态默认值，JSON 往返后会触发兼容性差异。
    old_bit_col.DefaultValue = Some(DefaultValue::String(rand_bit_str.clone()));
    old_bit_col.OriginDefaultValue = Some(DefaultValue::String(rand_bit_str.clone()));

    let mut new_bit_col = src_col.Clone();
    new_bit_col.Name = ast::NewCIStr("newBitCol");
    new_bit_col.FieldType = types::NewFieldType(mysql::TypeBit);
    err = new_bit_col.SetDefaultValue(Some(DefaultValue::Int(1)));
    // Only string type is allowed in BIT column.
    // BIT 列拒绝非字符串默认值，但 Go 测试确认出错后动态值仍保留为传入的 1。
    assert!(err.as_ref().is_err());
    assert!(
        err.unwrap_err()
            .to_string()
            .contains("Invalid default value")
    );
    assert_eq!(Some(DefaultValue::Int(1)), new_bit_col.GetDefaultValue());
    err = new_bit_col.SetDefaultValue(Some(DefaultValue::String(rand_bit_str.clone())));
    assert!(err.is_ok());
    assert_eq!(
        Some(DefaultValue::String(rand_bit_str.clone())),
        new_bit_col.GetDefaultValue()
    );

    let mut null_bit_col = src_col.Clone();
    null_bit_col.Name = ast::NewCIStr("nullBitCol");
    null_bit_col.FieldType = types::NewFieldType(mysql::TypeBit);
    err = null_bit_col.SetOriginDefaultValue(None);
    assert!(err.is_ok());
    assert!(null_bit_col.GetOriginDefaultValue().is_none());

    let test_cases = vec![
        DefaultValueCase {
            col: old_plain_col,
            is_consistent: true,
        },
        DefaultValueCase {
            col: old_bit_col,
            is_consistent: false,
        },
        DefaultValueCase {
            col: new_plain_col,
            is_consistent: true,
        },
        DefaultValueCase {
            col: new_bit_col,
            is_consistent: true,
        },
        DefaultValueCase {
            col: null_bit_col,
            is_consistent: true,
        },
    ];

    for tc in test_cases {
        let col = tc.col;
        let is_consistent = tc.is_consistent;
        let comment = format!("{} assertion failed", col.Name.O);
        let new_col = round_trip_column(&col);
        if is_consistent {
            // 普通列、新 BIT 写法与 nil BIT origin default 往返后应保持默认值与原始默认值一致。
            assert_eq!(
                col.GetDefaultValue(),
                new_col.GetDefaultValue(),
                "{comment}"
            );
            assert_eq!(
                col.GetOriginDefaultValue(),
                new_col.GetOriginDefaultValue(),
                "{comment}"
            );
        } else {
            // 旧 BIT 写法没有走 SetDefaultValue 的字节旁路，JSON 往返后默认值表现不同。
            assert_ne!(
                col.GetDefaultValue(),
                new_col.GetDefaultValue(),
                "{comment}"
            );
            assert_ne!(
                col.GetOriginDefaultValue(),
                new_col.GetOriginDefaultValue(),
                "{comment}"
            );
        }
    }

    let extra_phys_tbl_id_col = NewExtraPhysTblIDColInfo();
    // 额外物理表 ID 列必须是 NOT NULL BIGINT，保留 Go 测试最后的元数据断言。
    assert_eq!(mysql::NotNullFlag, extra_phys_tbl_id_col.GetFlag());
    assert_eq!(mysql::TypeLonglong, extra_phys_tbl_id_col.GetType());
}
use crate::group_1::{ColumnInfo, DefaultValue, ast, mysql};

/// 精简断言：BIT 列拒绝整型默认值，接受字节串默认值。
#[test]
fn column_default_value_validation_matches_go() {
    let mut column = ColumnInfo::New(1, ast::NewCIStr("bit_col"));
    column.SetType(mysql::TypeBit);
    assert!(column.SetDefaultValue(Some(DefaultValue::Int(1))).is_err());
    let bytes = vec![0x19, 0xb9];
    column
        .SetDefaultValue(Some(DefaultValue::String(bytes.clone())))
        .unwrap();
    assert_eq!(column.GetDefaultValue(), Some(DefaultValue::String(bytes)));
}

#[test]
fn column_json_uses_go_default_and_byte_shapes() {
    let mut column = ColumnInfo::New(1, ast::NewCIStr("bit_col"));
    column.SetType(mysql::TypeBit);
    column
        .SetDefaultValue(Some(DefaultValue::String(vec![0x19, 0xb9])))
        .unwrap();

    let json = serde_json::to_value(&column).expect("serialize ColumnInfo");
    assert_eq!(json["default"], "\u{19}\u{fffd}");
    assert_eq!(json["default_bit"], "Gbk=");

    let decoded: ColumnInfo = serde_json::from_value(json).expect("deserialize ColumnInfo");
    assert_eq!(decoded.DefaultValueBit, Some(vec![0x19, 0xb9]));
    assert_eq!(
        decoded.GetDefaultValue(),
        Some(DefaultValue::String(vec![0x19, 0xb9]))
    );
}
