// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 位运算聚合函数：BIT_OR / BIT_XOR / BIT_AND。
//
// 对分组内各行的整数值做按位累积；NULL 跳过。
// 初始值：OR/XOR 为 0，AND 为全 1（`u64::MAX`），保证空输入时 AND 语义正确。
// 仅 BIT_XOR 可逆，故只有它支持滑动窗口 `slide`（移出=再 XOR 一次）。

use crate::aggfuncs::PartialResult4BitFunc;
use std::mem::size_of;

/// 位运算聚合部分结果的固定内存占用。
pub const DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE: i64 = size_of::<PartialResult4BitFunc>() as i64;

/// 位运算聚合种类：按位或、异或、与。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitAggKind {
    Or,
    Xor,
    And,
}

/// 统一的位运算累加器；按 `kind` 选择初始值与 `apply` 运算。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitAggregator {
    kind: BitAggKind,
    value: PartialResult4BitFunc,
}

/// BIT_OR 类型别名（对应 Go 的 BitOrUint64 求值器）。
pub type BitOrUint64 = BitAggregator;
/// BIT_XOR 类型别名。
pub type BitXorUint64 = BitAggregator;
/// BIT_AND 类型别名。
pub type BitAndUint64 = BitAggregator;

impl BitAggregator {
    /// 按种类构造；AND 初始为 `u64::MAX`，其余为 0。
    pub fn new(kind: BitAggKind) -> Self {
        let value = if kind == BitAggKind::And { u64::MAX } else { 0 };
        Self { kind, value }
    }

    /// 重置为该种类的初始值。
    pub fn reset(&mut self) {
        self.value = if self.kind == BitAggKind::And {
            u64::MAX
        } else {
            0
        };
    }

    /// 当前累积的位运算结果。
    pub fn value(&self) -> u64 {
        self.value
    }

    /// 用原始行值更新；`Option::None`（SQL NULL）被跳过。
    pub fn update<I>(&mut self, values: I)
    where
        I: IntoIterator<Item = Option<i64>>,
    {
        for value in values.into_iter().flatten() {
            self.apply(value as u64);
        }
    }

    /// 合并同种类的部分结果；种类不一致则 panic。
    pub fn merge(&mut self, source: &Self) {
        assert_eq!(
            self.kind, source.kind,
            "cannot merge different bit aggregate kinds"
        );
        self.apply(source.value);
    }

    /// BIT_XOR is the only invertible bit aggregate in the Go implementation.
    /// 滑动窗口：对移出行再 XOR 一次等价于撤销，再累加移入行。
    pub fn slide<I, J>(&mut self, outgoing: I, incoming: J)
    where
        I: IntoIterator<Item = Option<i64>>,
        J: IntoIterator<Item = Option<i64>>,
    {
        assert_eq!(self.kind, BitAggKind::Xor, "only BIT_XOR supports sliding");
        self.update(outgoing);
        self.update(incoming);
    }

    /// 按种类对当前值应用一次位运算。
    fn apply(&mut self, value: u64) {
        match self.kind {
            BitAggKind::Or => self.value |= value,
            BitAggKind::Xor => self.value ^= value,
            BitAggKind::And => self.value &= value,
        }
    }
}
