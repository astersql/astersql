// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// TiPB（tipb）表达式反序列化到形式化表达式运行时的单元测试。
//
// 校验字段类型映射、字面量解码、列引用与标量函数边界，
// 以及全部 ScalarFuncSig 均有正式函数名映射。

use protobuf::{ProtobufEnum, RepeatedField};
use tipb::{Expr, ExprType, FieldType, ScalarFuncSig};

use astersql_expression::{
    PBSignatureFunctionName, PBToExpr, PBToExprs, PbTypeToFieldType, codec, mysql, types,
};

/// 构造带常见元数据的 tipb FieldType 桩。
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

/// 构造指定 ExprType / 载荷 / 可选字段类型的 tipb Expr。
fn literal(tp: ExprType, value: Vec<u8>, field_type: Option<FieldType>) -> Expr {
    let mut expression = Expr::new();
    expression.set_tp(tp);
    expression.set_val(value);
    if let Some(field_type) = field_type {
        expression.set_field_type(field_type);
    }
    expression
}

/// 字段类型与整型/字符串字面量解码应与 Go 对齐。
#[test]
fn field_type_and_literal_decoding_match_go() {
    let context = exprstatic::NewExprContext(Vec::new());
    let converted = PbTypeToFieldType(&field_type(mysql::TypeLonglong));
    assert_eq!(converted.GetType(), mysql::TypeLonglong);
    assert_eq!(converted.GetFlag(), 32);
    assert_eq!(converted.GetFlen(), 21);
    assert_eq!(converted.GetDecimal(), 4);
    assert_eq!(converted.GetCharset(), "utf8mb4");
    assert_eq!(converted.GetCollate(), "utf8mb4_bin");

    // Int64 载荷经 codec 编码后应还原为 Constant。
    let integer = literal(
        ExprType::Int64,
        codec::EncodeInt(Vec::new(), -42),
        Some(field_type(mysql::TypeLonglong)),
    );
    let decoded = PBToExpr(&context, &integer, &[]).unwrap();
    let constant = decoded
        .as_any()
        .downcast_ref::<astersql_expression::Constant>()
        .expect("integer PB must decode to Constant");
    assert_eq!(constant.Value.Kind(), types::KindInt64);
    assert_eq!(constant.Value.GetInt64(), -42);

    let string = literal(
        ExprType::String,
        b"hello".to_vec(),
        Some(field_type(mysql::TypeString)),
    );
    let decoded = PBToExpr(&context, &string, &[]).unwrap();
    let constant = decoded
        .as_any()
        .downcast_ref::<astersql_expression::Constant>()
        .expect("string PB must decode to Constant");
    assert_eq!(constant.Value.GetBytes(), b"hello");
    assert_eq!(constant.Value.Collation(), "utf8mb4_bin");
}

/// 列引用、空 ValueList 折叠与标量子表达式应保持 Go 边界语义。
#[test]
fn columns_lists_and_scalar_children_keep_go_boundaries() {
    let context = exprstatic::NewExprContext(Vec::new());
    let local_type = *types::NewFieldType(mysql::TypeLonglong);
    let column = literal(ExprType::ColumnRef, codec::EncodeInt(Vec::new(), 0), None);
    let decoded = PBToExpr(&context, &column, std::slice::from_ref(&local_type)).unwrap();
    let column_value = decoded
        .as_any()
        .downcast_ref::<astersql_expression::Column>()
        .expect("column PB must decode to Column");
    assert_eq!(column_value.Index, 0);
    assert_eq!(
        column_value.RetType.as_ref().unwrap().GetType(),
        mysql::TypeLonglong
    );
    assert_eq!(
        PBToExprs(&context, &[column], &[local_type]).unwrap().len(),
        1
    );

    // 空 ValueList 作为标量参数时折叠为假常量 0。
    let mut empty_list_scalar = Expr::new();
    empty_list_scalar.set_tp(ExprType::ScalarFunc);
    empty_list_scalar.set_sig(ScalarFuncSig::CastIntAsInt);
    empty_list_scalar.set_field_type(field_type(mysql::TypeLonglong));
    empty_list_scalar
        .mut_children()
        .push(literal(ExprType::ValueList, Vec::new(), None));
    let decoded = PBToExpr(&context, &empty_list_scalar, &[]).unwrap();
    let constant = decoded
        .as_any()
        .downcast_ref::<astersql_expression::Constant>()
        .expect("empty ValueList must fold to false Constant");
    assert_eq!(constant.Value.GetInt64(), 0);

    let mut truth = Expr::new();
    truth.set_tp(ExprType::ScalarFunc);
    truth.set_sig(ScalarFuncSig::IntIsTrue);
    truth.set_field_type(field_type(mysql::TypeTiny));
    truth.mut_children().push(literal(
        ExprType::Int64,
        codec::EncodeInt(Vec::new(), 1),
        Some(field_type(mysql::TypeLonglong)),
    ));
    let decoded = PBToExpr(&context, &truth, &[]).unwrap();
    let function = decoded
        .as_any()
        .downcast_ref::<astersql_expression::ScalarFunction>()
        .expect("scalar PB must decode to ScalarFunction");
    assert_eq!(
        function.FuncName.L,
        astersql_expression::ast::IsTruthWithoutNull
    );
}

/// 每个非 Unspecified 的 ScalarFuncSig 都必须能映射到正式函数名。
#[test]
fn every_existing_pb_signature_has_a_formal_function_mapping() {
    let signatures = ScalarFuncSig::values()
        .iter()
        .copied()
        .filter(|signature| *signature != ScalarFuncSig::Unspecified)
        .collect::<Vec<_>>();
    assert_eq!(signatures.len(), 639);
    let missing = signatures
        .iter()
        .map(|signature| format!("{signature:?}"))
        .filter(|signature| PBSignatureFunctionName(signature).is_none())
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "missing PB signature mappings: {missing:?}"
    );
}

/// Broad prefix families must not shadow more specific string signatures.
#[test]
fn string_signature_names_are_not_captured_by_broad_prefixes() {
    assert_eq!(
        PBSignatureFunctionName("InsertUTF8"),
        Some(astersql_expression::ast::InsertFunc)
    );
    assert_eq!(
        PBSignatureFunctionName("InstrUTF8"),
        Some(astersql_expression::ast::Instr)
    );
    assert_eq!(
        PBSignatureFunctionName("Substring2ArgsUTF8"),
        Some(astersql_expression::ast::Substring)
    );
}

/// 残缺线载载荷与越界列偏移应返回错误而非 panic。
#[test]
fn malformed_wire_values_return_errors() {
    let context = exprstatic::NewExprContext(Vec::new());
    let broken = literal(
        ExprType::Int64,
        vec![1, 2, 3],
        Some(field_type(mysql::TypeLonglong)),
    );
    assert!(PBToExpr(&context, &broken, &[]).is_err());

    // 列偏移 2 超出仅有 1 列的 field_types。
    let column = literal(ExprType::ColumnRef, codec::EncodeInt(Vec::new(), 2), None);
    assert!(
        PBToExpr(
            &context,
            &column,
            &[*types::NewFieldType(mysql::TypeLonglong)]
        )
        .is_err()
    );
}
