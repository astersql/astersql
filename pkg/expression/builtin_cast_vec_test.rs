// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// CAST 表达式向量化单元测试入口。
//
// 经 `#[path]` 挂载 `builtin_cast_vec_3_aster_unit_test.rs`，
// 校验列式 CAST（vectorized cast）与 Go 签名矩阵、NULL 传播及舍入语义一致。

/// 与 Go `builtin_cast_vec_test.go` 对齐的向量化 CAST 用例。
#[path = "builtin_cast_vec_3_aster_unit_test.rs"]
mod go_parity;

use crate::expression_builtin_cast_vec::{
    CastContext, CastSpec, EvalKind, ScalarValue, cast_column,
};
use types_dependency::datum::IsBinaryStr;

/// Go `padZeroForBinaryType` pads CAST(... AS BINARY(N)), while JSON→String
/// intentionally only applies `ProduceStrWithSpecifiedTp` and stays unpadded.
#[test]
fn binary_string_targets_match_go_padding_by_signature() {
    let mut binary = CastSpec::string(EvalKind::Int, 5);
    binary.target.SetType(crate::mysql::TypeString);
    binary
        .target
        .SetCharset(crate::charset::CharsetBin.to_owned());
    binary
        .target
        .SetCollate(crate::charset::CollationBin.to_owned());
    binary.target.AddFlag(crate::mysql::BinaryFlag);
    assert!(IsBinaryStr(&binary.target));

    let mut ctx = CastContext::warning();
    let padded = cast_column(&mut ctx, &binary, &[Some(ScalarValue::Int(12))]).unwrap();
    assert_eq!(
        padded,
        vec![Some(ScalarValue::String("12\0\0\0".to_owned()))]
    );

    let mut json_binary = CastSpec::string(EvalKind::Json, 5);
    json_binary.target.SetType(crate::mysql::TypeString);
    json_binary
        .target
        .SetCharset(crate::charset::CharsetBin.to_owned());
    json_binary
        .target
        .SetCollate(crate::charset::CollationBin.to_owned());
    json_binary.target.AddFlag(crate::mysql::BinaryFlag);
    let unpadded = cast_column(
        &mut ctx,
        &json_binary,
        &[Some(ScalarValue::Json(
            crate::expression_builtin_cast_vec::json("12"),
        ))],
    )
    .unwrap();
    assert_eq!(unpadded, vec![Some(ScalarValue::String("12".to_owned()))]);
}
