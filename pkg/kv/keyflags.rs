// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 事务内键元数据位标志（KeyFlags）及 FlagsOp 应用逻辑。
//
// 每一位记录延迟存在性检查（Presume Key Not Exists）、加锁需求、存在性断言、
// 以及是否将约束检查推迟到 prewrite（两阶段提交的预写阶段）。
// 对齐 pkg/kv/keyflags.go 的键元数据位与修改操作顺序。

// 对齐 pkg/kv/keyflags.go 的键元数据位与修改操作顺序。

/// 事务内单个键的附加语义位集（对应 Go uint8）。
// KeyFlags 对应 Go uint8 位集，每一位记录键在当前事务中的附加语义。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyFlags(pub u8);

impl std::ops::BitOrAssign for KeyFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAndAssign for KeyFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl std::ops::Not for KeyFlags {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

pub(crate) const flagPresumeKNE: KeyFlags = KeyFlags(1 << 0);
pub(crate) const flagNeedLocked: KeyFlags = KeyFlags(1 << 1);

// 以下两位共同表示断言状态：00 未设置、01 存在、10 不存在、11 未知。
pub(crate) const flagAssertExists: KeyFlags = KeyFlags(1 << 2);
pub(crate) const flagAssertNotExists: KeyFlags = KeyFlags(1 << 3);
// 冲突与约束检查延后到下一次悲观锁或 prewrite 请求。
pub(crate) const flagNeedConstraintCheckInPrewrite: KeyFlags = KeyFlags(1 << 4);
pub(crate) const flagPreviousPresumeKNE: KeyFlags = KeyFlags(1 << 5);

impl KeyFlags {
    // HasPresumeKeyNotExists 判断关联键是否采用延迟存在性检查。
    pub fn HasPresumeKeyNotExists(self) -> bool {
        self.0 & flagPresumeKNE.0 != 0
    }

    // HasNeedLocked 判断关联键是否需要加锁。
    pub fn HasNeedLocked(self) -> bool {
        self.0 & flagNeedLocked.0 != 0
    }

    // HasAssertExists 仅在“存在”位置位且“不存在”位未置位时返回 true。
    pub fn HasAssertExists(self) -> bool {
        self.0 & flagAssertExists.0 != 0 && self.0 & flagAssertNotExists.0 == 0
    }

    // HasAssertNotExists 与 HasAssertExists 对称，表示事务前键不存在。
    pub fn HasAssertNotExists(self) -> bool {
        self.0 & flagAssertNotExists.0 != 0 && self.0 & flagAssertExists.0 == 0
    }

    // HasAssertUnknown 对应两个断言位同时置位，表示无法对该键作断言。
    pub fn HasAssertUnknown(self) -> bool {
        self.0 & flagAssertExists.0 != 0 && self.0 & flagAssertNotExists.0 != 0
    }

    // HasAssertionFlags 判断断言是否已经设置；事务内设置后预期不可改变。
    pub fn HasAssertionFlags(self) -> bool {
        self.0 & flagAssertExists.0 != 0 || self.0 & flagAssertNotExists.0 != 0
    }

    // HasNeedConstraintCheckInPrewrite 判断约束检查是否需要推迟到 prewrite。
    pub fn HasNeedConstraintCheckInPrewrite(self) -> bool {
        self.0 & flagNeedConstraintCheckInPrewrite.0 != 0
    }
}

/// FlagsOp 对应 Go 的独立 uint16 类型，保留未知操作值。
// FlagsOp 对应 Go 的 KeyFlags 修改操作类型，保持 iota 数值顺序和未知值语义。
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlagsOp(pub u16);

impl FlagsOp {
    // 标记关联键的存在性采用延迟检查。
    pub const SetPresumeKeyNotExists: Self = Self(0);
    // 标记关联键需要获取锁。
    pub const SetNeedLocked: Self = Self(1);
    // 标记约束与冲突检查延后至 prewrite。
    pub const SetNeedConstraintCheckInPrewrite: Self = Self(2);
    // 标记 PNE 来自之前语句，重试或回滚当前语句时不能清除。
    pub const SetPreviousPresumeKeyNotExists: Self = Self(3);
}

/// 按传入顺序把已知操作对应位并入 origin；未知 uint16 值与 Go 一样为 no-op。
// ApplyFlagsOps 按传入顺序应用操作；Go switch 没有 default，因此未知值保持 origin 不变。
pub fn ApplyFlagsOps(mut origin: KeyFlags, ops: &[FlagsOp]) -> KeyFlags {
    for op in ops {
        let flag = match *op {
            FlagsOp::SetPresumeKeyNotExists => Some(flagPresumeKNE),
            FlagsOp::SetNeedLocked => Some(flagNeedLocked),
            FlagsOp::SetNeedConstraintCheckInPrewrite => Some(flagNeedConstraintCheckInPrewrite),
            FlagsOp::SetPreviousPresumeKeyNotExists => Some(flagPreviousPresumeKNE),
            _ => None,
        };
        if let Some(flag) = flag {
            origin.0 |= flag.0;
        }
    }
    origin
}
