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

// DistSQL 内置表达式历史测试入口。
//
// DistSQL（分布式 SQL 片段）侧职责是 protobuf 解码与签名构造；本文件复用
// 权威可执行对等测试套件，覆盖字面量族、字段元数据、签名分派、ValueList
// 边界与畸形线格式错误路径。

// DistSQL's production responsibility is protobuf decoding and signature
// construction. Reuse the canonical executable parity suite so this historical
// test target exercises every supported literal family, field metadata,
// signature dispatch, value-list boundary, and malformed-wire error path.
// 直接 include 权威 parity 套件，保持与 Go 历史测试目标同名入口。
include!("distsql_builtin_34_aster_unit_test.rs");

#[test]
fn bytes_bit_and_empty_value_list_keep_go_return_types() {
    let ctx = BuildContext::new(chrono_tz::UTC, 1024);

    let bytes = constant(
        PBToExpr(&ctx, &literal(ExprType::Bytes, b"raw".to_vec(), None), &[])
            .unwrap()
            .unwrap(),
    );
    assert_eq!(bytes.ret_type.GetType(), mysql::TYPE_STRING);

    let bit = constant(
        PBToExpr(
            &ctx,
            &literal(ExprType::MysqlBit, vec![0x12, 0x34], None),
            &[],
        )
        .unwrap()
        .unwrap(),
    );
    assert_eq!(bit.ret_type.GetType(), mysql::TYPE_STRING);

    let mut scalar = Expr::new();
    scalar.set_tp(ExprType::ScalarFunc);
    scalar.set_sig(ScalarFuncSig::CastIntAsInt);
    scalar.set_field_type(field_type(mysql::TYPE_LONGLONG));
    scalar
        .mut_children()
        .push(literal(ExprType::ValueList, Vec::new(), None));
    let folded = constant(PBToExpr(&ctx, &scalar, &[]).unwrap().unwrap());
    assert_eq!(folded.ret_type.GetType(), mysql::TYPE_LONGLONG);
}
