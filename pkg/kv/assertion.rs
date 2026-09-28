// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// KV 断言操作：在事务预写（prewrite）阶段声明 key 必须存在或不存在。
//
// 断言（Assertion）是乐观事务路径上对 key 存在性的显式约束，编码在
// `KeyFlags` 的两个互斥位中。本模块定义 `AssertionOp` 类型，以及在保留
// 其他标志位的前提下更新断言位的 `ApplyAssertionOp`。

// 对齐 pkg/kv/assertion.go 的断言操作及 KeyFlags 位标记更新规则。

/// 对应 Go 独立 uint8 类型，保留未知操作值；不同于普通 FlagsOp。
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssertionOp(pub u8);

impl AssertionOp {
    /// 关联 key 必须已经存在。
    pub const AssertExist: Self = Self(0);
    /// 关联 key 必须不存在。
    pub const AssertNotExist: Self = Self(1);
    /// 存在性未知：两断言位同时置位编码。
    pub const AssertUnknown: Self = Self(2);
    /// 不附加任何断言（对已有断言位为 no-op）。
    pub const AssertNone: Self = Self(3);
}

// ApplyAssertionOp 对应 Go 的 switch：在保留其他 KeyFlags 位的同时更新两个断言位。
/// 按断言操作更新 `KeyFlags` 中的存在性断言位，保留其余标志。
pub fn ApplyAssertionOp(mut origin: KeyFlags, op: AssertionOp) -> KeyFlags {
    match op {
        AssertionOp::AssertExist => {
            // “必须存在”置位 exists，并清除互斥的 not-exists 位。
            origin |= flagAssertExists;
            origin &= !flagAssertNotExists;
        }
        AssertionOp::AssertNotExist => {
            // “必须不存在”与上一个分支相反，确保不会残留 exists 位。
            origin |= flagAssertNotExists;
            origin &= !flagAssertExists;
        }
        AssertionOp::AssertUnknown => {
            // Go 用两个断言位同时置位编码 unknown，而不是清除断言。
            origin |= flagAssertExists;
            origin |= flagAssertNotExists;
        }
        _ => {
            // AssertNone 和未知 uint8 值均为 no-op，与 Go switch 一致。
        }
    }

    origin
}
