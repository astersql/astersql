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

// helper 模块迁移回归测试：对齐 Go `TypeContext` 预置值与 fixed 语义。
//
// 向量化代码生成器依赖这些常量拼出 `VecEval*` / `chunk.Column` API 名称；
// 任一字段漂移都会导致生成的 Go 源码调用错误方法或选错固定长/变长路径。

use super::{
    TYPE_DATETIME, TYPE_DECIMAL, TYPE_DURATION, TYPE_INT, TYPE_JSON, TYPE_REAL, TYPE_STRING,
    TypeContext,
};

/// 逐字段比对七种 EvalType 预置上下文与 Go helper 包字面量。
#[test]
fn predefined_type_contexts_match_go_values() {
    // (常量, (ETName, TypeName, TypeNameInColumn, TypeNameGo, Fixed))
    let cases = [
        (TYPE_INT, ("Int", "Int", "Int64", "int64", true)),
        (TYPE_REAL, ("Real", "Real", "Float64", "float64", true)),
        (
            TYPE_DECIMAL,
            ("Decimal", "Decimal", "Decimal", "types.MyDecimal", true),
        ),
        (TYPE_STRING, ("String", "String", "String", "string", false)),
        (
            TYPE_DATETIME,
            ("Datetime", "Time", "Time", "types.Time", true),
        ),
        (
            TYPE_DURATION,
            ("Duration", "Duration", "GoDuration", "time.Duration", true),
        ),
        (
            TYPE_JSON,
            ("Json", "JSON", "JSON", "json.BinaryJSON", false),
        ),
    ];

    for (actual, expected) in cases {
        assert_eq!(
            (
                actual.et_name,
                actual.type_name,
                actual.type_name_in_column,
                actual.type_name_go,
                actual.fixed,
            ),
            expected
        );
    }
}

/// 固定长类型走切片批量读写；String/JSON 为变长，走 Get/Append 路径。
#[test]
fn fixed_flag_selects_the_same_chunk_storage_classes_as_go() {
    let fixed = [
        TYPE_INT,
        TYPE_REAL,
        TYPE_DECIMAL,
        TYPE_DATETIME,
        TYPE_DURATION,
    ];
    let variable = [TYPE_STRING, TYPE_JSON];

    assert!(fixed.into_iter().all(|context| context.fixed));
    assert!(variable.into_iter().all(|context| !context.fixed));
}

/// `Default` 对应 Go 结构体零值：空名称且 fixed=false。
#[test]
fn type_context_default_matches_go_zero_value() {
    assert_eq!(
        TypeContext::default(),
        TypeContext {
            et_name: "",
            type_name: "",
            type_name_in_column: "",
            type_name_go: "",
            fixed: false,
        }
    );
}
