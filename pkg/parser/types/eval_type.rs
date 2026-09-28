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
// See the License for the specific language governing permissions and
// limitations under the License.

// 表达式求值类型（EvalType）定义，对照 Go 的 `eval_type.go`。
//
// EvalType 把 MySQL 列存储类型归并到表达式引擎内部使用的少数几种求值形态
//（整型、浮点、Decimal、字符串、时间、JSON、向量等），供类型推断与算子选择。

// 本文件对照 pkg/parser/types/eval_type.go，保留求值类型常量与方法顺序；fmt.Stringer 对应 Rust Display。

use std::fmt;

/// EvalType 是表达式求值类型的新类型包装；底层为 u8，对应 Go 的 byte 别名。
// EvalType 对应 Go 的 byte 别名；使用新类型而非 enum，以保留 Go 可构造未知数值的语义。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct EvalType(pub u8);

// 以下常量保持 Go iota 从 0 开始的顺序与数值。
/// INT 求值类型。
pub const ETInt: EvalType = EvalType(0); // INT 求值类型。
/// REAL（双精度浮点）求值类型。
pub const ETReal: EvalType = EvalType(1); // REAL 求值类型。
/// DECIMAL（定点十进制）求值类型。
pub const ETDecimal: EvalType = EvalType(2); // DECIMAL 求值类型。
/// STRING（字符串）求值类型。
pub const ETString: EvalType = EvalType(3); // STRING 求值类型。
/// DATETIME 求值类型。
pub const ETDatetime: EvalType = EvalType(4); // DATETIME 求值类型。
/// TIMESTAMP 求值类型。
pub const ETTimestamp: EvalType = EvalType(5); // TIMESTAMP 求值类型。
/// DURATION（TIME）求值类型。
pub const ETDuration: EvalType = EvalType(6); // DURATION 求值类型。
/// JSON 求值类型。
pub const ETJson: EvalType = EvalType(7); // JSON 求值类型。
/// VectorFloat32（向量）求值类型。
pub const ETVectorFloat32: EvalType = EvalType(8); // VectorFloat32 求值类型。

impl EvalType {
    /// IsStringKind 判断是否按字符串形态参与求值；时间、JSON 与向量也归入此类。
    // IsStringKind 对应 Go 的分类规则：时间、JSON 与向量也走字符串形态的求值路径。
    pub fn IsStringKind(self) -> bool {
        matches!(
            self,
            ETString | ETDatetime | ETTimestamp | ETDuration | ETJson | ETVectorFloat32
        )
    }

    /// IsVectorKind 判断是否为向量求值类型；当前仅 VectorFloat32。
    // IsVectorKind 对应 Go 的向量分类；当前仅 VectorFloat32 命中。
    pub fn IsVectorKind(self) -> bool {
        self == ETVectorFloat32
    }

    /// String 返回可读名称，形状对齐 Go 的 `fmt.Stringer`。
    // String 保留 Go fmt.Stringer 的方法形状，并委托给 Display 的同一映射。
    pub fn String(self) -> String {
        self.to_string()
    }
}

/// Display 将 EvalType 映射为固定英文名；未知数值 panic，与 Go 一致。
// Display 对应 Go String 方法；未知值继续 panic，避免把无效 EvalType 静默格式化。
impl fmt::Display for EvalType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            ETInt => "Int",
            ETReal => "Real",
            ETDecimal => "Decimal",
            ETString => "String",
            ETDatetime => "Datetime",
            ETTimestamp => "Timestamp",
            ETDuration => "Time",
            ETJson => "Json",
            ETVectorFloat32 => "VectorFloat32",
            // Go 使用 panic(fmt.Sprintf(...))；这里保留相同的非法输入失败策略。
            EvalType(value) => panic!("invalid EvalType {value}"),
        };
        f.write_str(name)
    }
}
