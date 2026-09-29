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

// ENUM 解析、类型分类、错误码、EvalType 与字段元数据的迁移期单元测试。
//
// 覆盖校对规则下的名称匹配、数值/进制回退、类型谓词、
// types 错误 errno、Explain 格式列表、FieldName/FieldTypeBuilder 语义。

use std::sync::Arc;

use super::*;

#[test]
/// 名称校对、十进制/十六进制序号回退及越界错误。
fn enum_parsing_matches_name_collation_numeric_fallback_and_bounds() {
    let elems = vec!["a".to_owned(), "b".to_owned(), "啊".to_owned()];

    for collation in [mysql::DefaultCollationName, "utf8_unicode_ci"] {
        for (name, expected) in [
            ("a", Some(("a", 1.0))),
            ("b", Some(("b", 2.0))),
            ("missing", None),
        ] {
            match expected {
                Some((expected_name, expected_number)) => {
                    let parsed = ParseEnum(&elems, name, collation).unwrap();
                    assert_eq!(parsed.String(), expected_name);
                    assert_eq!(parsed.ToNumber(), expected_number);
                }
                None => assert!(ParseEnum(&elems, name, collation).is_err()),
            }
        }
    }

    for (name, expected) in [
        ("A     ", Some(("a", 1.0))),
        ("A", Some(("a", 1.0))),
        ("啊", Some(("啊", 3.0))),
        ("missing", None),
    ] {
        match expected {
            Some((expected_name, expected_number)) => {
                let parsed = ParseEnum(&elems, name, "utf8_general_ci").unwrap();
                assert_eq!(parsed.String(), expected_name);
                assert_eq!(parsed.ToNumber(), expected_number);
            }
            None => assert!(ParseEnum(&elems, name, "utf8_general_ci").is_err()),
        }
    }

    let by_name = ParseEnum(&elems, "A     ", "utf8_general_ci").unwrap();

    let by_decimal = ParseEnum(&elems, "2", mysql::DefaultCollationName).unwrap();
    assert_eq!(
        by_decimal,
        Enum {
            Name: "b".to_owned(),
            Value: 2
        }
    );

    let by_hex = ParseEnum(&elems, "0x3", mysql::DefaultCollationName).unwrap();
    assert_eq!(by_hex.String(), "啊");
    assert_eq!(by_hex.Value, 3);

    let err = ParseEnum(&elems, "missing", mysql::DefaultCollationName).unwrap_err();
    assert!(
        err.to_string()
            .contains("item missing is not in enum [a b 啊]")
    );
    assert!(ParseEnumValue(&elems, 0).is_err());
    assert!(ParseEnumValue(&elems, 4).is_err());

    let copied = by_name.Copy();
    assert_eq!(copied, by_name);
    assert_ne!(copied.Name.as_ptr(), by_name.Name.as_ptr());
}

#[test]
/// 类型分类谓词、TypeStr/KindStr 与 NeedRestoredData 校对规则。
fn type_helpers_match_go_classification_and_restored_data_rules() {
    for tp in [
        mysql::TypeTinyBlob,
        mysql::TypeMediumBlob,
        mysql::TypeBlob,
        mysql::TypeLongBlob,
    ] {
        assert!(IsTypeBlob(tp));
    }
    assert!(!IsTypeBlob(mysql::TypeInt24));
    assert!(IsTypeChar(mysql::TypeString));
    assert!(IsTypeChar(mysql::TypeVarchar));
    assert!(!IsTypeChar(mysql::TypeLong));
    assert!(IsTypeVarchar(mysql::TypeVarString));
    assert!(IsTypeInteger(mysql::TypeYear));
    assert!(!IsTypeStoredAsInteger(mysql::TypeEnum));
    assert!(IsTypeStoredAsInteger(mysql::TypeDatetime));
    assert!(IsTypeNumeric(mysql::TypeNewDecimal));
    assert!(!IsTypeNumeric(mysql::TypeUnspecified));
    assert!(IsTypeTemporal(mysql::TypeNewDate));
    assert_eq!(TypeStr(mysql::TypeYear), "year");
    assert_eq!(TypeStr(0xdd), "");
    for (tp, charset, expected) in [
        (mysql::TypeBlob, "utf8", "text"),
        (mysql::TypeLongBlob, "utf8", "longtext"),
        (mysql::TypeTinyBlob, "utf8", "tinytext"),
        (mysql::TypeMediumBlob, "utf8", "mediumtext"),
        (mysql::TypeVarchar, "binary", "varbinary"),
        (mysql::TypeString, "binary", "binary"),
        (mysql::TypeTiny, "binary", "tinyint"),
        (mysql::TypeBlob, "binary", "blob"),
        (mysql::TypeLongBlob, "binary", "longblob"),
        (mysql::TypeTinyBlob, "binary", "tinyblob"),
        (mysql::TypeMediumBlob, "binary", "mediumblob"),
        (mysql::TypeVarchar, "utf8", "varchar"),
        (mysql::TypeString, "utf8", "char"),
        (mysql::TypeShort, "binary", "smallint"),
        (mysql::TypeInt24, "binary", "mediumint"),
        (mysql::TypeLong, "binary", "int"),
        (mysql::TypeLonglong, "binary", "bigint"),
        (mysql::TypeFloat, "binary", "float"),
        (mysql::TypeDouble, "binary", "double"),
        (mysql::TypeYear, "binary", "year"),
        (mysql::TypeDuration, "binary", "time"),
        (mysql::TypeDatetime, "binary", "datetime"),
        (mysql::TypeDate, "binary", "date"),
        (mysql::TypeTimestamp, "binary", "timestamp"),
        (mysql::TypeNewDecimal, "binary", "decimal"),
        (mysql::TypeUnspecified, "binary", "unspecified"),
        (0xdd, "binary", ""),
        (mysql::TypeBit, "binary", "bit"),
        (mysql::TypeEnum, "binary", "enum"),
        (mysql::TypeSet, "binary", "set"),
    ] {
        assert_eq!(TypeToStr(tp, charset), expected);
    }
    assert_eq!(KindStr(KindMysqlJSON), "json");
    assert_eq!(KindStr(0xff), "");

    let restored_cases = [
        (mysql::TypeString, "binary", "binary", false),
        (mysql::TypeVarString, "binary", "binary", false),
        (mysql::TypeString, "utf8mb4", "utf8mb4_bin", false),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_bin", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_general_ci", true),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_general_ci", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_unicode_ci", true),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_unicode_ci", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_0900_ai_ci", true),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_0900_ai_ci", true),
        (mysql::TypeString, "utf8mb4", "utf8mb4_0900_bin", false),
        (mysql::TypeVarString, "utf8mb4", "utf8mb4_0900_bin", false),
        (mysql::TypeString, "gbk", "gbk_bin", true),
        (mysql::TypeVarString, "gbk", "gbk_bin", true),
        (mysql::TypeString, "gbk", "gbk_chinese_ci", true),
        (mysql::TypeVarString, "gbk", "gbk_chinese_ci", true),
        (mysql::TypeString, "gb18030", "gb18030_bin", true),
        (mysql::TypeVarString, "gb18030", "gb18030_bin", true),
        (mysql::TypeString, "gb18030", "gb18030_chinese_ci", true),
        (mysql::TypeVarString, "gb18030", "gb18030_chinese_ci", true),
    ];
    for (tp, charset, collate, expected) in restored_cases {
        let field = parser_types_field(tp, charset, collate);
        assert_eq!(NeedRestoredDataWithCollate(&field, true), expected);
        assert!(!NeedRestoredDataWithCollate(&field, false));
    }

    let binary_char = parser_types_field(mysql::TypeString, "binary", "binary");
    assert!(IsBinaryStr(&binary_char));
}

#[test]
/// EOFAsNil、InvOp2 文案以及 types 错误与 MySQL errno 对应关系。
fn error_helpers_preserve_eof_invalid_operation_and_mysql_codes() {
    let eof = errors::SharedError::new(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    assert!(EOFAsNil(Some(eof)).is_none());

    let ordinary = errors::New("test");
    assert_eq!(EOFAsNil(Some(ordinary)).unwrap().to_string(), "test");

    let invalid = InvOp2(&1_i32, &"x", opcode::Op::Plus).unwrap_err();
    assert!(
        invalid
            .to_string()
            .contains("Invalid operation: 1 plus \"x\"")
    );
    assert!(invalid.to_string().contains("i32 and &str"));

    let errors_and_codes = [
        (&**ErrInvalidDefault, errno::ErrInvalidDefault),
        (&**ErrDataTooLong, errno::ErrDataTooLong),
        (&**ErrIllegalValueForType, errno::ErrIllegalValueForType),
        (&**ErrTruncated, errno::WarnDataTruncated),
        (&**ErrOverflow, errno::ErrDataOutOfRange),
        (&**ErrDivByZero, errno::ErrDivisionByZero),
        (&**ErrTooBigDisplayWidth, errno::ErrTooBigDisplaywidth),
        (&**ErrTooBigFieldLength, errno::ErrTooBigFieldlength),
        (&**ErrTooBigSet, errno::ErrTooBigSet),
        (&**ErrTooBigScale, errno::ErrTooBigScale),
        (&**ErrTooBigPrecision, errno::ErrTooBigPrecision),
        (&**ErrBadNumber, errno::ErrBadNumber),
        (&**ErrInvalidFieldSize, errno::ErrInvalidFieldSize),
        (&**ErrMBiggerThanD, errno::ErrMBiggerThanD),
        (&**ErrWarnDataOutOfRange, errno::ErrWarnDataOutOfRange),
        (&**ErrDuplicatedValueInType, errno::ErrDuplicatedValueInType),
        (
            &**ErrDatetimeFunctionOverflow,
            errno::ErrDatetimeFunctionOverflow,
        ),
        (&**ErrCastAsSignedOverflow, errno::ErrCastAsSignedOverflow),
        (&**ErrCastNegIntAsUnsigned, errno::ErrCastNegIntAsUnsigned),
        (&**ErrInvalidYearFormat, errno::ErrInvalidYearFormat),
        (&**ErrInvalidYear, errno::ErrInvalidYear),
        (&**ErrTruncatedWrongVal, errno::ErrTruncatedWrongValue),
        (&**ErrInvalidWeekModeFormat, errno::ErrInvalidWeekModeFormat),
        (&**ErrWrongFieldSpec, errno::ErrWrongFieldSpec),
        (&**ErrSyntax, errno::ErrParse),
        (&**ErrWrongValue, errno::ErrTruncatedWrongValue),
        (&**ErrWrongValue2, errno::ErrWrongValue),
        (&**ErrWrongValueForType, errno::ErrWrongValueForType),
        (&**ErrPartitionStatsMissing, errno::ErrPartitionStatsMissing),
        (
            &**ErrPartitionColumnStatsMissing,
            errno::ErrPartitionColumnStatsMissing,
        ),
        (
            &**ErrIncorrectDatetimeValue,
            errno::ErrIncorrectDatetimeValue,
        ),
        (&**ErrJSONBadOneOrAllArg, errno::ErrJSONBadOneOrAllArg),
        (&**ErrJSONVacuousPath, errno::ErrJSONVacuousPath),
        (
            &**ErrTimestampInDSTTransition,
            errno::ErrTimeStampInDSTTransition,
        ),
    ];
    for (err, code) in errors_and_codes {
        assert_eq!(err.Code(), i32::from(code));
    }
    assert_eq!(
        (DateTimeStr, DateStr, TimeStr, TimestampStr),
        ("datetime", "date", "time", "timestamp")
    );
}

#[test]
/// EvalType 别名/IsStringKind 与 ExplainFormats 顺序不变。
fn eval_types_and_explain_formats_preserve_aliases_values_and_order() {
    assert_eq!(ETInt, ast_types::ETInt);
    assert_eq!(ETVectorFloat32, ast_types::ETVectorFloat32);
    assert!(ETDatetime.IsStringKind());
    assert!(!ETInt.IsStringKind());

    assert_eq!(
        ExplainFormats,
        &[
            "brief",
            "dot",
            "hint",
            "json",
            "row",
            "verbose",
            "traditional",
            "true_card_cost",
            "binary",
            "tidb_json",
            "cost_trace",
            "plan_cache",
            "plan_tree",
            "ru",
        ]
    );
}

#[test]
/// FieldName 渲染、内存估算、Clone/Shallow 与 AST 列名查找。
fn field_names_match_rendering_memory_clone_shallow_and_lookup_semantics() {
    let name = FieldName {
        OrigTblName: ast::NewCIStr("OrigT"),
        OrigColName: ast::NewCIStr("OrigC"),
        DBName: ast::NewCIStr("DB"),
        TblName: ast::NewCIStr("Tbl"),
        ColName: ast::NewCIStr("Col"),
        Hidden: false,
        NotExplicitUsable: true,
        Redundant: false,
    };
    assert_eq!(name.String(), "db.tbl.col");
    assert_eq!(
        name.MemoryUsage(),
        32 * 5 + 10 + 10 + 4 + 6 + 6 + size::SizeOfBool * 3
    );

    let cloned = name.Clone();
    assert_eq!(cloned.String(), name.String());
    assert_ne!(cloned.DBName.L.as_ptr(), name.DBName.L.as_ptr());

    let shared = Arc::new(name);
    let names = NameSlice(vec![Some(shared.clone()), None]);
    let shallow = names.Shallow();
    assert!(Arc::ptr_eq(shallow.0[0].as_ref().unwrap(), &shared));

    let wildcard = ast::ColumnName {
        Schema: ast::CIStr::default(),
        Table: ast::CIStr::default(),
        Name: ast::NewCIStr("COL"),
    };
    assert!(names.FindAstColName(&wildcard));

    let mismatch = ast::ColumnName {
        Schema: ast::NewCIStr("other"),
        ..wildcard
    };
    assert!(!names.FindAstColName(&mismatch));
    assert_eq!(EmptyName.String(), "EMPTY_NAME");
}

#[test]
/// FieldTypeBuilder 链式改标志/长度，Build 返回快照，BuildP 返回内部字段。
fn field_type_builder_forwards_all_fields_and_buildp_aliases_builder_field() {
    let mut builder = NewFieldTypeBuilder();
    builder
        .SetType(mysql::TypeVarchar)
        .SetFlag(mysql::NotNullFlag)
        .AddFlag(mysql::UnsignedFlag)
        .ToggleFlag(mysql::BinaryFlag)
        .DelFlag(mysql::UnsignedFlag)
        .SetFlen(42)
        .SetDecimal(3)
        .SetCharset("utf8mb4".to_owned())
        .SetCollate("utf8mb4_general_ci".to_owned())
        .SetElems(vec!["a".to_owned(), "b".to_owned()])
        .SetArray(false);

    assert_eq!(builder.GetType(), mysql::TypeVarchar);
    assert_eq!(builder.GetFlag(), mysql::NotNullFlag | mysql::BinaryFlag);
    assert_eq!(builder.GetFlen(), 42);
    assert_eq!(builder.GetDecimal(), 3);
    assert_eq!(builder.GetCharset(), "utf8mb4");
    assert_eq!(builder.GetCollate(), "utf8mb4_general_ci");

    let value = builder.Build();
    let pointer = builder.BuildP();
    assert_eq!(value, *pointer);
    pointer.SetType(mysql::TypeLong);
    assert_eq!(builder.GetType(), mysql::TypeLong);
    assert_eq!(value.GetType(), mysql::TypeVarchar);
}

/// 构造带 charset/collate 的简易 FieldType 测试夹具。
fn parser_types_field(tp: u8, charset: &str, collate: &str) -> FieldType {
    let mut field = FieldType::default();
    field.SetType(tp);
    field.SetCharset(charset.to_owned());
    field.SetCollate(collate.to_owned());
    field
}
