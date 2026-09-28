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

// 表达式求值类型（EvalType）的再导出。
//
// EvalType 是向量化表达式执行时使用的粗粒度类型分类
//（整数、实数、Decimal、字符串、时间、JSON、向量等），
// 比 MySQL 字段类型更少、更适合运行时运算分派。

/// 表达式求值类型别名，转发自 parser/ast。
pub type EvalType = ast::EvalType;
/// 整数求值类型。
pub const ETInt: EvalType = ast::ETInt;
/// 浮点实数求值类型。
pub const ETReal: EvalType = ast::ETReal;
/// 精确小数（Decimal）求值类型。
pub const ETDecimal: EvalType = ast::ETDecimal;
/// 字符串求值类型。
pub const ETString: EvalType = ast::ETString;
/// DATETIME 求值类型。
pub const ETDatetime: EvalType = ast::ETDatetime;
/// TIMESTAMP 求值类型。
pub const ETTimestamp: EvalType = ast::ETTimestamp;
/// TIME/Duration 求值类型。
pub const ETDuration: EvalType = ast::ETDuration;
/// JSON 求值类型。
pub const ETJson: EvalType = ast::ETJson;
/// 向量（float32）求值类型。
pub const ETVectorFloat32: EvalType = ast::ETVectorFloat32;
