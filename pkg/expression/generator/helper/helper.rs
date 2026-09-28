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

// 向量化代码生成器共享的类型模板上下文，对应 Go `helper/helper.go`。
//
// `TypeContext` 描述每种 `types.EvalType` 在生成 Go 源码时所需的名称片段
// （VecEval、chunk.Column API、Go 具体类型、是否定长列）。

// 本文件由 pkg/expression/generator/helper/helper.go 迁移而来，提供各向量化代码生成器共享的类型模板上下文。
// Type and chunk column API names used when generating source.

/// TypeContext 对应 Go 的同名结构体，是每一种 types.EvalType 的模板上下文。
/// 所有字段都保留 Go 标识符拼写，因为它们最终会被插入生成的 Go 源码。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TypeContext {
    /// 对应 types.ET{{ .ETName }} 的名称片段。
    pub et_name: &'static str,
    /// 对应 expression.VecExpr.VecEval{{ .TypeName }} 的名称片段。
    pub type_name: &'static str,
    /// 对应 chunk.Column 的 Append/Resize/Reserve/Get 方法及切片访问器名称片段；未特化时与 type_name 相同。
    pub type_name_in_column: &'static str,
    /// 生成的 Go 代码中使用的具体类型名称。
    pub type_name_go: &'static str,
    /// 对应 chunk.getFixedLen：true 表示该列值可以使用固定长切片批量读写。
    pub fixed: bool,
}

/// TypeInt 对应 types.ETInt，chunk 中以 Int64 表示。
pub const TYPE_INT: TypeContext = TypeContext {
    et_name: "Int",
    type_name: "Int",
    type_name_in_column: "Int64",
    type_name_go: "int64",
    fixed: true,
};

/// TypeReal 对应 types.ETReal，chunk 中以 Float64 表示。
pub const TYPE_REAL: TypeContext = TypeContext {
    et_name: "Real",
    type_name: "Real",
    type_name_in_column: "Float64",
    type_name_go: "float64",
    fixed: true,
};

/// TypeDecimal 对应 types.ETDecimal，生成代码使用 types.MyDecimal。
pub const TYPE_DECIMAL: TypeContext = TypeContext {
    et_name: "Decimal",
    type_name: "Decimal",
    type_name_in_column: "Decimal",
    type_name_go: "types.MyDecimal",
    fixed: true,
};

/// TypeString 对应 types.ETString；字符串为变长列，生成器使用 Reserve/Append 路径。
pub const TYPE_STRING: TypeContext = TypeContext {
    et_name: "String",
    type_name: "String",
    type_name_in_column: "String",
    type_name_go: "string",
    fixed: false,
};

/// TypeDatetime 对应 types.ETDatetime，而 VecEval 与 chunk API 使用 Time。
pub const TYPE_DATETIME: TypeContext = TypeContext {
    et_name: "Datetime",
    type_name: "Time",
    type_name_in_column: "Time",
    type_name_go: "types.Time",
    fixed: true,
};

/// TypeDuration 对应 types.ETDuration，chunk 中暴露 GoDuration 切片。
pub const TYPE_DURATION: TypeContext = TypeContext {
    et_name: "Duration",
    type_name: "Duration",
    type_name_in_column: "GoDuration",
    type_name_go: "time.Duration",
    fixed: true,
};

/// TypeJSON 对应 types.ETJson；BinaryJSON 是变长值，生成器采用追加路径。
pub const TYPE_JSON: TypeContext = TypeContext {
    et_name: "Json",
    type_name: "JSON",
    type_name_in_column: "JSON",
    type_name_go: "json.BinaryJSON",
    fixed: false,
};
