// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// DistSQL 内置表达式（TiPB → 内存表达式）可执行对等测试。
//
// 覆盖 protobuf FieldType 元数据拷贝、565 条 ScalarFuncSig 分派表、
// 各字面量族编解码、列/枚举/ValueList 边界，以及畸形 wire 错误返回。
// DistSQL 指下推到存储层的表达式片段，经 tipb.Expr 传输后在本侧重建。

use std::collections::HashSet;

use crate::expression_distsql_builtin::{
    BuildContext, Expression, PBToExpr, PBToExprs, PbTypeToFieldType, SIGNATURE_RECIPES, codec,
    getSignatureByPB, mysql, tipb, types,
};
use protobuf::RepeatedField;
use tipb::{Expr, ExprType, FieldType, ScalarFuncSig};

/// 构造带完整元数据（flag/flen/decimal/charset/collate/elems）的 tipb.FieldType。
fn field_type(tp: u8) -> FieldType {
    let mut value = FieldType::new();
    value.set_tp(tp as i32);
    value.set_flag(32);
    value.set_flen(21);
    value.set_decimal(4);
    value.set_charset("utf8mb4".to_owned());
    value.set_collate(46);
    value.set_elems(RepeatedField::from_vec(vec![
        "small".to_owned(),
        "large".to_owned(),
    ]));
    value
}

/// 组装 tipb.Expr 字面量：类型、编码后的 val，以及可选 FieldType。
fn literal(tp: ExprType, value: Vec<u8>, field_type: Option<FieldType>) -> Expr {
    let mut expr = Expr::new();
    expr.set_tp(tp);
    expr.set_val(value);
    if let Some(field_type) = field_type {
        expr.set_field_type(field_type);
    }
    expr
}

/// 从重建结果中取出 Constant，非常量则 panic（测试辅助）。
fn constant(expression: Expression) -> crate::expression_distsql_builtin::Constant {
    match expression {
        Expression::Constant(value) => value,
        _ => panic!("expected constant expression"),
    }
}

/// PbTypeToFieldType 须完整拷贝协议中的类型、标志、长度与排序规则等元数据。
#[test]
fn pb_field_type_copies_protocol_metadata() {
    let converted = PbTypeToFieldType(&field_type(mysql::TYPE_LONGLONG));
    assert_eq!(converted.GetType(), mysql::TYPE_LONGLONG);
    assert_eq!(converted.GetFlag(), 32);
    assert_eq!(converted.GetFlen(), 21);
    assert_eq!(converted.GetDecimal(), 4);
    assert_eq!(converted.GetCharset(), "utf8mb4");
    assert_eq!(converted.GetCollate(), "utf8mb4_bin");
    assert_eq!(
        converted.GetElems(),
        &["small".to_owned(), "large".to_owned()]
    );
}

/// 签名表长度/唯一性与 Go 一致，并拒绝 Unspecified；构造器名字符串对齐草稿。
#[test]
fn signature_table_preserves_go_dispatch_and_rejects_unspecified() {
    assert_eq!(SIGNATURE_RECIPES.len(), 565);
    let unique = SIGNATURE_RECIPES
        .iter()
        .map(|recipe| recipe.signature)
        .collect::<HashSet<_>>();
    assert_eq!(unique.len(), SIGNATURE_RECIPES.len());
    assert_eq!(SIGNATURE_RECIPES.first().unwrap().signature, "CastIntAsInt");
    assert_eq!(
        SIGNATURE_RECIPES.last().unwrap().signature,
        "FTSMatchExpression"
    );

    let ctx = BuildContext::new(chrono_tz::UTC, 64 << 20);
    let builtin = getSignatureByPB(
        &ctx,
        ScalarFuncSig::CastIntAsInt,
        &field_type(mysql::TYPE_LONGLONG),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(builtin.signature, ScalarFuncSig::CastIntAsInt);
    assert_eq!(builtin.max_allowed_packet, 64 << 20);
    assert!(builtin.constructor.contains("builtinCastIntAsIntSig"));

    let comparison = getSignatureByPB(
        &ctx,
        ScalarFuncSig::LtInt,
        &field_type(mysql::TYPE_LONGLONG),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(comparison.constructor, "&builtinLTIntSig{base}");

    let error = getSignatureByPB(
        &ctx,
        ScalarFuncSig::Unspecified,
        &field_type(mysql::TYPE_LONGLONG),
        Vec::new(),
    )
    .err()
    .expect("unspecified signature must fail");
    assert!(error.to_string().contains("Unspecified"));
}

/// Int64/Uint64/Float32 字面量经 TiDB codec 解码后 Kind 与取值正确。
#[test]
fn literal_decoding_matches_tidb_codec_and_preserves_types() {
    let ctx = BuildContext::new(chrono_tz::UTC, 1024);
    let integer = literal(
        ExprType::Int64,
        codec::EncodeInt(Vec::new(), -42),
        Some(field_type(mysql::TYPE_LONGLONG)),
    );
    let integer = constant(PBToExpr(&ctx, &integer, &[]).unwrap().unwrap());
    assert_eq!(integer.value.Kind(), types::KindInt64);
    assert_eq!(integer.value.GetInt64(), -42);
    assert_eq!(integer.ret_type.GetFlen(), 21);

    let unsigned = literal(
        ExprType::Uint64,
        codec::EncodeUint(Vec::new(), u64::MAX),
        Some(field_type(mysql::TYPE_LONGLONG)),
    );
    let unsigned = constant(PBToExpr(&ctx, &unsigned, &[]).unwrap().unwrap());
    assert_eq!(unsigned.value.Kind(), types::KindUint64);
    assert_eq!(unsigned.value.GetUint64(), u64::MAX);

    let float = literal(
        ExprType::Float32,
        codec::EncodeFloat(Vec::new(), 1.25),
        None,
    );
    let float = constant(PBToExpr(&ctx, &float, &[]).unwrap().unwrap());
    assert_eq!(float.value.Kind(), types::KindFloat32);
    assert_eq!(float.value.GetFloat32(), 1.25);
}

/// 列引用下标、ENUM 名称/序号，以及空 ValueList 折叠为 0 的边界与 Go 一致。
#[test]
fn columns_enums_and_empty_value_lists_keep_go_boundaries() {
    let ctx = BuildContext::new(chrono_tz::UTC, 1024);
    let local_type = PbTypeToFieldType(&field_type(mysql::TYPE_LONGLONG));
    let column = literal(ExprType::ColumnRef, codec::EncodeInt(Vec::new(), 0), None);
    match PBToExpr(&ctx, &column, &[local_type.clone()])
        .unwrap()
        .unwrap()
    {
        Expression::Column(column) => {
            assert_eq!(column.index, 0);
            assert_eq!(column.ret_type.GetType(), mysql::TYPE_LONGLONG);
        }
        _ => panic!("expected column"),
    }

    let enum_expr = literal(
        ExprType::MysqlEnum,
        codec::EncodeUint(Vec::new(), 2),
        Some(field_type(mysql::TypeEnum)),
    );
    let enum_value = constant(PBToExpr(&ctx, &enum_expr, &[]).unwrap().unwrap());
    assert_eq!(enum_value.value.GetMysqlEnum().Name, "large");
    assert_eq!(enum_value.value.GetMysqlEnum().Value, 2);

    let mut scalar = Expr::new();
    scalar.set_tp(ExprType::ScalarFunc);
    scalar.set_sig(ScalarFuncSig::CastIntAsInt);
    scalar.set_field_type(field_type(mysql::TYPE_LONGLONG));
    scalar
        .mut_children()
        .push(literal(ExprType::ValueList, Vec::new(), None));
    let folded = constant(PBToExpr(&ctx, &scalar, &[]).unwrap().unwrap());
    assert_eq!(folded.value.GetInt64(), 0);

    let decoded = PBToExprs(&ctx, &[column], &[local_type]).unwrap();
    assert_eq!(decoded.len(), 1);
}

/// String/Bytes/Bit/Duration/Time 字面量 Kind 与时区相关 Time 类型对齐 Go。
#[test]
fn string_bytes_bit_duration_and_time_literals_match_go_kinds() {
    let ctx = BuildContext::new(chrono_tz::Asia::Shanghai, 1024);
    let string = literal(
        ExprType::String,
        b"hello".to_vec(),
        Some(field_type(mysql::TYPE_STRING)),
    );
    let string = constant(PBToExpr(&ctx, &string, &[]).unwrap().unwrap());
    assert_eq!(string.value.Kind(), types::KindString);
    assert_eq!(string.value.GetBytes(), b"hello");
    assert_eq!(string.value.Collation(), "utf8mb4_bin");

    let bytes = literal(ExprType::Bytes, b"raw".to_vec(), None);
    let bytes = constant(PBToExpr(&ctx, &bytes, &[]).unwrap().unwrap());
    assert_eq!(bytes.value.Kind(), types::KindBytes);
    assert_eq!(bytes.value.GetBytes(), b"raw");

    let bit = literal(ExprType::MysqlBit, vec![0x12, 0x34], None);
    let bit = constant(PBToExpr(&ctx, &bit, &[]).unwrap().unwrap());
    assert_eq!(bit.value.Kind(), types::KindMysqlBit);

    let duration = literal(
        ExprType::MysqlDuration,
        codec::EncodeInt(Vec::new(), 1_500_000_000),
        None,
    );
    let duration = constant(PBToExpr(&ctx, &duration, &[]).unwrap().unwrap());
    assert_eq!(duration.value.GetMysqlDuration().Duration, 1_500_000_000);
    assert_eq!(duration.value.GetMysqlDuration().Fsp, types::MaxFsp);

    let time = literal(
        ExprType::MysqlTime,
        codec::EncodeUint(Vec::new(), 0),
        Some(field_type(mysql::TypeDatetime)),
    );
    let time = constant(PBToExpr(&ctx, &time, &[]).unwrap().unwrap());
    assert_eq!(time.value.Kind(), types::KindMysqlTime);
    assert_eq!(time.value.GetMysqlTime().Type(), mysql::TypeDatetime);
}

/// Decimal/JSON/Vector 与非空 ValueList 按真实 wire 格式解码并挂到标量函数参数。
#[test]
fn decimal_json_vector_and_nonempty_value_lists_decode_real_wire_formats() {
    let ctx = BuildContext::new(chrono_tz::UTC, 1024);

    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"12.34").unwrap();
    let decimal = literal(
        ExprType::MysqlDecimal,
        codec::EncodeDecimal(Vec::new(), &decimal, 4, 2).unwrap(),
        Some(field_type(mysql::TypeNewDecimal)),
    );
    let decimal = constant(PBToExpr(&ctx, &decimal, &[]).unwrap().unwrap());
    assert_eq!(decimal.value.Kind(), types::KindMysqlDecimal);
    assert_eq!(decimal.value.GetMysqlDecimal().ToString(), b"12.34");
    assert_eq!(decimal.value.Length(), 4);
    assert_eq!(decimal.value.Frac(), 2);

    let json = types::ParseBinaryJSONFromString(r#"{"answer":42}"#).unwrap();
    let mut json_wire = vec![10, json.TypeCode];
    json_wire.extend_from_slice(&json.Value);
    let json = literal(ExprType::MysqlJson, json_wire, None);
    let json = constant(PBToExpr(&ctx, &json, &[]).unwrap().unwrap());
    assert_eq!(json.value.Kind(), types::KindMysqlJSON);
    assert_eq!(json.value.GetMysqlJSON().String(), r#"{"answer": 42}"#);

    let vector = types::ParseVectorFloat32("[1.5,-2]").unwrap();
    let vector = literal(
        ExprType::TiDbVectorFloat32,
        vector.SerializeTo(Vec::new()),
        None,
    );
    let vector = constant(PBToExpr(&ctx, &vector, &[]).unwrap().unwrap());
    assert_eq!(vector.value.Kind(), types::KindVectorFloat32);
    assert_eq!(vector.value.GetVectorFloat32().Elements(), &[1.5, -2.0]);

    let mut scalar = Expr::new();
    scalar.set_tp(ExprType::ScalarFunc);
    scalar.set_sig(ScalarFuncSig::CastIntAsInt);
    scalar.set_field_type(field_type(mysql::TYPE_LONGLONG));
    let mut encoded_list = vec![3];
    encoded_list = codec::EncodeInt(encoded_list, 7);
    scalar
        .mut_children()
        .push(literal(ExprType::ValueList, encoded_list, None));
    match PBToExpr(&ctx, &scalar, &[]).unwrap().unwrap() {
        Expression::ScalarFunction(function) => {
            assert_eq!(function.function.base.args().len(), 1);
            assert_eq!(function.function.signature, ScalarFuncSig::CastIntAsInt);
        }
        _ => panic!("expected scalar function"),
    }
}

/// 截断/非法编码的整数、列引用与 ValueList 必须返回错误而非部分结果。
#[test]
fn malformed_literals_return_errors_instead_of_partial_values() {
    let ctx = BuildContext::new(chrono_tz::UTC, 1024);
    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"12.34").unwrap();
    let malformed = [
        literal(
            ExprType::Int64,
            codec::EncodeInt(Vec::new(), -42),
            Some(field_type(mysql::TYPE_LONGLONG)),
        ),
        literal(
            ExprType::Uint64,
            codec::EncodeUint(Vec::new(), 42),
            Some(field_type(mysql::TYPE_LONGLONG)),
        ),
        literal(
            ExprType::Float64,
            codec::EncodeFloat(Vec::new(), 1.25),
            None,
        ),
        literal(
            ExprType::MysqlDecimal,
            codec::EncodeDecimal(Vec::new(), &decimal, 4, 2).unwrap(),
            Some(field_type(mysql::TypeNewDecimal)),
        ),
        literal(
            ExprType::MysqlDuration,
            codec::EncodeInt(Vec::new(), 1_500_000_000),
            None,
        ),
    ];
    for mut expression in malformed {
        let truncated = expression.get_val()[..expression.get_val().len() / 2].to_vec();
        expression.set_val(truncated);
        assert!(
            PBToExpr(&ctx, &expression, &[]).is_err(),
            "truncated {:?} must fail",
            expression.get_tp()
        );
    }

    let broken_column = literal(ExprType::ColumnRef, vec![0x80], None);
    assert!(PBToExpr(&ctx, &broken_column, &[]).is_err());

    let mut scalar = Expr::new();
    scalar.set_tp(ExprType::ScalarFunc);
    scalar.set_sig(ScalarFuncSig::CastIntAsInt);
    scalar.set_field_type(field_type(mysql::TYPE_LONGLONG));
    scalar
        .mut_children()
        .push(literal(ExprType::ValueList, vec![1, 2, 3], None));
    assert!(PBToExpr(&ctx, &scalar, &[]).is_err());
}
