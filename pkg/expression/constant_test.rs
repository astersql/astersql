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

// Constant 构造与基础行为单元测试。
//
// 覆盖整数/字符串/NULL 构造器的 MySQL 字段元数据、克隆与哈希隔离、
// 常量级别（ConstLevel）以及 prepared 参数标记序号。

use crate::*;

/// 校验有符号/无符号整数构造器保留取值与 UnsignedFlag 等元数据。
#[test]
fn integer_constructors_preserve_value_sign_and_mysql_metadata() {
    let one = NewOne();
    let signed_one = NewSignedOne();
    let zero = NewZero();

    assert_eq!(one.Value.GetInt64(), 1);
    assert_eq!(signed_one.Value.GetInt64(), 1);
    assert_eq!(zero.Value.GetInt64(), 0);
    assert_ne!(
        one.RetType.as_ref().unwrap().GetFlag() & mysql::UnsignedFlag,
        0
    );
    assert_eq!(
        signed_one.RetType.as_ref().unwrap().GetFlag() & mysql::UnsignedFlag,
        0
    );

    let unsigned = NewUInt64Const(usize::MAX);
    assert_ne!(
        unsigned.RetType.as_ref().unwrap().GetFlag() & mysql::UnsignedFlag,
        0
    );
    let typed_unsigned =
        NewUInt64ConstWithFieldType(u64::MAX, *types::NewFieldType(mysql::TypeLonglong));
    assert_eq!(typed_unsigned.Value.GetUint64(), u64::MAX);
}

/// 字符串常量长度与 NULL/带类型 NULL 的 Datum 行为。
#[test]
fn string_null_and_typed_null_cover_normal_and_null_values() {
    let text = NewStrConst("TiDB");
    assert_eq!(text.Value.GetString(), "TiDB");
    assert_eq!(text.RetType.as_ref().unwrap().GetFlen(), 4);

    assert!(NewNull().Value.IsNull());
    let field_type = *types::NewFieldType(mysql::TypeDatetime);
    let typed_null = NewNullWithFieldType(field_type.clone());
    assert!(typed_null.Value.IsNull());
    assert_eq!(typed_null.RetType.as_ref(), Some(&field_type));
}

/// 克隆后改值不得污染原对象；Equals / HashCode 在改值前后语义正确。
#[test]
fn cloning_hashing_and_replacing_value_do_not_alias() {
    let mut original = NewInt64Const(7);
    let mut cloned = original.Clone();
    assert!(original.Equals(&cloned));
    assert_eq!(original.HashCode(), cloned.HashCode());

    let replaced = cloned.clone_with_value(types::NewDatum(&8_i64));
    assert!(!original.Equals(&replaced));
    assert_eq!(original.Value.GetInt64(), 7);
    assert_eq!(replaced.Value.GetInt64(), 8);
}

/// 严格常量级别、向量化能力、跨会话共享与 ParamMarker 序号。
#[test]
fn constants_report_strict_level_and_parameter_marker_order() {
    let constant = NewSignedZero();
    assert_eq!(constant.ConstLevel(), ConstStrict);
    assert!(constant.Vectorized());
    assert!(constant.SafeToShareAcrossSession());
    assert!(constant.MemoryUsage() > 0);

    let marker = ParamMarker::new(usize::MAX);
    assert_eq!(marker.order(), usize::MAX);
}

/// Go 的所有具体类型求值器都优先尊重 `TypeNull`，不得读取非空的占位 Datum。
#[test]
fn null_return_type_short_circuits_typed_evaluation() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    let null_type = *types::NewFieldType(mysql::TypeNull);
    let time = Constant::with_type(types::NewDatum(&types::ZeroTime), null_type.clone());
    let (value, is_null) = time
        .EvalTime(&ctx, chunk::Row::default())
        .expect("TypeNull must not attempt a mismatched conversion");
    assert!(is_null);
    assert_eq!(value, types::ZeroTime);

    let duration = Constant::with_type(
        types::NewDatum(&types::Duration::default()),
        null_type.clone(),
    );
    assert!(
        duration
            .EvalDuration(&ctx, chunk::Row::default())
            .unwrap()
            .1
    );

    let json = Constant::with_type(
        types::NewDatum(&types::BinaryJSON::default()),
        null_type.clone(),
    );
    assert!(json.EvalJSON(&ctx, chunk::Row::default()).unwrap().1);

    let vector = Constant::with_type(types::NewDatum(&types::ZeroVectorFloat32()), null_type);
    assert!(
        vector
            .EvalVectorFloat32(&ctx, chunk::Row::default())
            .unwrap()
            .1
    );
}
