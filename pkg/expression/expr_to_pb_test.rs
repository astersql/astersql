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

// `expr_to_pb` 字段类型编解码与 TiFlash Decimal 校验的单元测试。
//
// 验证 FieldType 往返保留协议元数据，TiFlash 拒绝非法 Decimal 而 TiKV 保持 Go 兼容放行，
// 以及合法 Decimal 边界与非 Decimal 类型不受 flen/decimal 约束。

use crate::*;
use std::any::Any;

struct CapabilityClient {
    basic_dag: bool,
}

impl kv::Client for CapabilityClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _req: &kv::Request,
        _vars: &dyn Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("expression-to-PB tests never send requests")
    }

    fn IsRequestTypeSupported(&self, request_type: i64, sub_type: i64) -> bool {
        request_type == kv::ReqTypeSelect
            || (request_type == kv::ReqTypeDAG
                && (sub_type != kv::ReqSubTypeBasic || self.basic_dag))
    }
}

const MODERN_CLIENT: CapabilityClient = CapabilityClient { basic_dag: true };
const LEGACY_CLIENT: CapabilityClient = CapabilityClient { basic_dag: false };

/// FieldType → PB → FieldType 往返应保留类型、flag、长度、小数位、字符集、排序规则与 elems。
#[test]
fn field_type_pb_round_trip_preserves_protocol_metadata() {
    let mut field_type = *types::NewFieldType(mysql::TypeVarchar);
    field_type.SetFlag(mysql::NotNullFlag | mysql::UnsignedFlag);
    field_type.SetFlen(255);
    field_type.SetDecimal(3);
    field_type.SetCharset(charset::CharsetUTF8MB4.to_owned());
    field_type.SetCollate(charset::CollationUTF8MB4.to_owned());
    field_type.SetElems(vec!["normal".to_owned(), "边界".to_owned()]);

    let encoded = ToPBFieldType(&field_type);
    let decoded = FieldTypeFromPB(&encoded);

    assert_eq!(decoded.GetType(), field_type.GetType());
    assert_eq!(decoded.GetFlag(), field_type.GetFlag());
    assert_eq!(decoded.GetFlen(), 255);
    assert_eq!(decoded.GetDecimal(), 3);
    assert_eq!(decoded.GetCharset(), charset::CharsetUTF8MB4);
    assert_eq!(decoded.GetCollate(), charset::CollationUTF8MB4);
    assert_eq!(decoded.GetElems(), field_type.GetElems());
}

/// TiFlash 对非法 Decimal（scale > precision）报错；TiKV 仍编码以兼容 Go。
#[test]
fn tiflash_rejects_invalid_decimal_but_tikv_keeps_go_compatibility() {
    let mut invalid = *types::NewFieldType(mysql::TypeNewDecimal);
    invalid.SetFlen(5);
    invalid.SetDecimal(6);
    assert!(!invalid.IsDecimalValid());

    let error = ToPBFieldTypeWithCheck(&invalid, kv::StoreType::TiFlash).unwrap_err();
    assert!(error.to_string().contains("invalid decimal"));
    let encoded = ToPBFieldTypeWithCheck(&invalid, kv::StoreType::TiKV).unwrap();
    assert_eq!(encoded.get_flen(), 5);
    assert_eq!(encoded.get_decimal(), 6);
}

/// 合法 Decimal 边界可通过；非 Decimal 类型即使 flen/decimal 为 -1 也不被 TiFlash 拒绝。
#[test]
fn decimal_validation_accepts_limit_relationship_and_non_decimal_types() {
    let mut decimal = *types::NewFieldType(mysql::TypeNewDecimal);
    decimal.SetFlen(mysql::MaxDecimalWidth as isize);
    decimal.SetDecimal(mysql::MaxDecimalScale as isize);
    assert!(decimal.IsDecimalValid());
    assert!(ToPBFieldTypeWithCheck(&decimal, kv::StoreType::TiFlash).is_ok());

    let mut varchar = *types::NewFieldType(mysql::TypeVarchar);
    varchar.SetFlen(-1);
    varchar.SetDecimal(-1);
    assert!(ToPBFieldTypeWithCheck(&varchar, kv::StoreType::TiFlash).is_ok());
}

#[test]
fn constants_encode_go_literal_types_and_keep_negative_zero_at_root() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let expressions: Vec<ExprBox> = vec![
        Box::new(NewNull()),
        Box::new(NewInt64Const(-7)),
        Box::new(Constant::with_type(
            types::NewUintDatum(9),
            *types::NewFieldType(mysql::TypeLonglong),
        )),
        Box::new(NewStrConst("TiDB")),
    ];

    let encoded = ExpressionsToPBList(eval, &expressions, &MODERN_CLIENT).unwrap();
    assert_eq!(
        encoded.iter().map(|item| item.get_tp()).collect::<Vec<_>>(),
        vec![
            tipb::ExprType::Null,
            tipb::ExprType::Int64,
            tipb::ExprType::Uint64,
            tipb::ExprType::String,
        ]
    );
    assert_eq!(codec::DecodeInt(encoded[1].get_val()).unwrap().1, -7);
    assert_eq!(codec::DecodeUint(encoded[2].get_val()).unwrap().1, 9);
    assert_eq!(encoded[3].get_val(), b"TiDB");

    let negative_zero = Constant::with_type(
        types::NewFloat64Datum(-0.0),
        *types::NewFieldType(mysql::TypeDouble),
    );
    assert!(
        NewPBConverter(&MODERN_CLIENT, eval)
            .ExprToPB(&negative_zero)
            .is_none()
    );
}

#[test]
fn columns_follow_type_checks_projection_exception_and_protocol_versions() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let unsupported = Column::new(*types::NewFieldType(mysql::TypeSet), 41, 141, 3);
    assert!(
        NewPBConverter(&MODERN_CLIENT, eval)
            .ExprToPB(&unsupported)
            .is_none()
    );
    let projected: Vec<ExprBox> = vec![Box::new(unsupported)];
    let encoded = ProjectionExpressionsToPBList(eval, &projected, &MODERN_CLIENT).unwrap();
    assert_eq!(codec::DecodeInt(encoded[0].get_val()).unwrap().1, 3);
    assert!(encoded[0].has_field_type());

    let modern = Column::new(*types::NewFieldType(mysql::TypeLonglong), 42, 142, 4);
    let modern_pb = NewPBConverter(&MODERN_CLIENT, eval)
        .ExprToPB(&modern)
        .unwrap();
    assert_eq!(codec::DecodeInt(modern_pb.get_val()).unwrap().1, 4);
    assert!(modern_pb.has_field_type());

    let legacy_pb = NewPBConverter(&LEGACY_CLIENT, eval)
        .ExprToPB(&modern)
        .unwrap();
    assert_eq!(codec::DecodeInt(legacy_pb.get_val()).unwrap().1, 42);
    assert!(!legacy_pb.has_field_type());
    for invalid_id in [0, -1] {
        let column = Column::new(*types::NewFieldType(mysql::TypeLonglong), invalid_id, 1, 4);
        assert!(
            NewPBConverter(&LEGACY_CLIENT, eval)
                .ExprToPB(&column)
                .is_none()
        );
    }
}

#[test]
fn scalar_functions_encode_signature_children_return_type_and_list_errors() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let scalar = NewFunctionBase(
        &context,
        ast::Plus,
        *types::NewFieldType(mysql::TypeLonglong),
        vec![Box::new(NewInt64Const(2)), Box::new(NewInt64Const(3))],
    )
    .unwrap();
    let encoded = NewPBConverter(&MODERN_CLIENT, eval)
        .ExprToPB(scalar.as_ref())
        .unwrap();
    assert_eq!(encoded.get_tp(), tipb::ExprType::ScalarFunc);
    assert_eq!(encoded.get_sig(), tipb::ScalarFuncSig::PlusInt);
    assert_eq!(encoded.get_children().len(), 2);
    assert_eq!(
        encoded.get_field_type().get_tp(),
        mysql::TypeLonglong as i32
    );

    let invalid: Vec<ExprBox> = vec![Box::new(Column::new(
        *types::NewFieldType(mysql::TypeSet),
        7,
        17,
        0,
    ))];
    let error = ExpressionsToPBList(eval, &invalid, &MODERN_CLIENT).unwrap_err();
    assert!(error.to_string().contains("cannot be pushed down"));
}

#[test]
fn group_and_sort_items_preserve_expression_and_direction() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let expression = NewInt64Const(11);

    let group = GroupByItemToPB(eval, &MODERN_CLIENT, &expression).unwrap();
    assert!(!group.get_desc());
    assert_eq!(group.get_expr().get_tp(), tipb::ExprType::Int64);
    let sort = SortByItemToPB(eval, &MODERN_CLIENT, &expression, true).unwrap();
    assert!(sort.get_desc());
    assert_eq!(codec::DecodeInt(sort.get_expr().get_val()).unwrap().1, 11);
}
