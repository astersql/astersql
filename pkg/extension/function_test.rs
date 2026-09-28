// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 扩展自定义函数（extension function）定义校验的单元测试。
//
// 覆盖 `FunctionDef::Validate`：函数名不能为空，且 `OptionalArgsLen`
//（可选参数个数）必须落在合法区间内。

use crate::FunctionDef;

/// 校验空名与可选参数的上下界，并保持 Go 的精确错误文本。
#[test]
fn canonical_extension_function_validates_name_and_optional_arity() {
    // 默认 FunctionDef 名称为空，Validate 应报错。
    assert_eq!(
        FunctionDef::default().Validate().unwrap_err().to_string(),
        "extension function name should not be empty"
    );
    // 构造带名称、两个参数类型且可选参数长度为 1 的合法定义。
    let valid = FunctionDef {
        Name: "aster_fn".into(),
        ArgTps: vec![crate::types::EvalType(1), crate::types::EvalType(1)],
        OptionalArgsLen: 1,
        ..FunctionDef::default()
    };
    assert!(valid.Validate().is_ok());

    // OptionalArgsLen 为负数时非法。
    let invalid = FunctionDef {
        OptionalArgsLen: -1,
        ..valid
    };
    assert_eq!(
        invalid.Validate().unwrap_err().to_string(),
        "invalid OptionalArgsLen: -1"
    );

    // OptionalArgsLen 大于参数列表长度时非法。
    let invalid = FunctionDef {
        OptionalArgsLen: 3,
        ..invalid
    };
    assert_eq!(
        invalid.Validate().unwrap_err().to_string(),
        "invalid OptionalArgsLen: 3"
    );
}
