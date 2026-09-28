// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 表达式求值的可选属性键与位图集合。
//
// 对应 Go `optional.go`：部分内置函数依赖会话侧可选能力（当前用户、InfoSchema、
// KV 存储、咨询锁等）。键用整数编号，集合用 `u64` 位图表示；未使用的高位不参与
// 空/满判断，与 Go 位集语义对齐。

use std::fmt;

/// 可选属性键；数值顺序必须与 OPTIONAL_PROPERTY_DESC_LIST 下标一致。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OptionalEvalPropKey(pub usize);

impl OptionalEvalPropKey {
    /// 当前用户与活跃角色。
    pub const OptPropCurrentUser: Self = Self(0);
    /// 会话变量快照。
    pub const OptPropSessionVars: Self = Self(1);
    /// 元数据 InfoSchema（库表结构视图）。
    pub const OptPropInfoSchema: Self = Self(2);
    /// KV 存储句柄。
    pub const OptPropKVStore: Self = Self(3);
    /// 内部 SQL 执行器。
    pub const OptPropSQLExecutor: Self = Self(4);
    /// 序列（SEQUENCE）操作接口。
    pub const OptPropSequenceOperator: Self = Self(5);
    /// 咨询锁（GET_LOCK / RELEASE_LOCK）。
    pub const OptPropAdvisoryLock: Self = Self(6);
    /// DDL Owner 信息。
    pub const OptPropDDLOwnerInfo: Self = Self(7);
    /// 权限检查器。
    pub const OptPropPrivilegeChecker: Self = Self(8);
}

/// 已注册可选属性个数；位图有效宽度。
pub const OPT_PROPS_CNT: usize = 9;
/// 与 Go 导出名对齐的别名。
pub const OptPropsCnt: usize = OPT_PROPS_CNT;
/// 覆盖全部有效属性位的掩码。
const ALL_OPT_PROPS_MASK: u64 = (1_u64 << OPT_PROPS_CNT) - 1;

/// 包级键常量，便于 `use` 时直接引用。
pub const OptPropCurrentUser: OptionalEvalPropKey = OptionalEvalPropKey::OptPropCurrentUser;
pub const OptPropSessionVars: OptionalEvalPropKey = OptionalEvalPropKey::OptPropSessionVars;
pub const OptPropInfoSchema: OptionalEvalPropKey = OptionalEvalPropKey::OptPropInfoSchema;
pub const OptPropKVStore: OptionalEvalPropKey = OptionalEvalPropKey::OptPropKVStore;
pub const OptPropSQLExecutor: OptionalEvalPropKey = OptionalEvalPropKey::OptPropSQLExecutor;
pub const OptPropSequenceOperator: OptionalEvalPropKey =
    OptionalEvalPropKey::OptPropSequenceOperator;
pub const OptPropAdvisoryLock: OptionalEvalPropKey = OptionalEvalPropKey::OptPropAdvisoryLock;
pub const OptPropDDLOwnerInfo: OptionalEvalPropKey = OptionalEvalPropKey::OptPropDDLOwnerInfo;
pub const OptPropPrivilegeChecker: OptionalEvalPropKey =
    OptionalEvalPropKey::OptPropPrivilegeChecker;

impl OptionalEvalPropKey {
    /// 返回只包含当前键的一位集合。
    pub fn AsPropKeySet(self) -> OptionalEvalPropKeySet {
        if self.0 >= u64::BITS as usize {
            return OptionalEvalPropKeySet(0);
        }
        OptionalEvalPropKeySet(1_u64 << self.0)
    }

    /// 查描述表，取得键对应的静态描述。
    pub fn Desc(self) -> &'static OptionalEvalPropDesc {
        &OPTIONAL_PROPERTY_DESC_LIST[self.0]
    }
}

impl fmt::Display for OptionalEvalPropKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < OPT_PROPS_CNT {
            f.write_str(self.Desc().str)
        } else {
            write!(f, "UnknownOptionalEvalPropKey({})", self.0)
        }
    }
}

/// 属性描述与 Go 结构体字段顺序一致；后续扩展字段可继续加入此结构。
pub struct OptionalEvalPropDesc {
    key: OptionalEvalPropKey,
    str: &'static str,
}

impl OptionalEvalPropDesc {
    /// 返回描述绑定的属性键。
    pub fn Key(&self) -> OptionalEvalPropKey {
        self.key
    }
}

/// 所有可选属性提供者至少能够报告自己的属性描述。
pub trait OptionalEvalPropProvider {
    fn Desc(&self) -> &'static OptionalEvalPropDesc;

    /// Returns a type-erased view when the provider owns `'static` state.
    /// Borrowing providers may keep the default and remain valid trait objects.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// 固定注册表保留 Go 初始化顺序；SequenceOperator 的字符串也按原源码保留。
pub static OPTIONAL_PROPERTY_DESC_LIST: [OptionalEvalPropDesc; OPT_PROPS_CNT] = [
    OptionalEvalPropDesc {
        key: OptPropCurrentUser,
        str: "OptPropCurrentUser",
    },
    OptionalEvalPropDesc {
        key: OptPropSessionVars,
        str: "OptPropSessionVars",
    },
    OptionalEvalPropDesc {
        key: OptPropInfoSchema,
        str: "OptPropInfoSchema",
    },
    OptionalEvalPropDesc {
        key: OptPropKVStore,
        str: "OptPropKVStore",
    },
    OptionalEvalPropDesc {
        key: OptPropSQLExecutor,
        str: "OptPropSQLExecutor",
    },
    OptionalEvalPropDesc {
        key: OptPropSequenceOperator,
        str: "OptPropDDLOwnerInfo",
    },
    OptionalEvalPropDesc {
        key: OptPropAdvisoryLock,
        str: "OptPropAdvisoryLock",
    },
    OptionalEvalPropDesc {
        key: OptPropDDLOwnerInfo,
        str: "OptPropDDLOwnerInfo",
    },
    OptionalEvalPropDesc {
        key: OptPropPrivilegeChecker,
        str: "OptPropPrivilegeChecker",
    },
];

/// 对应 Go init 的启动期一致性检查；Rust 固定数组已在类型层保证长度，再检查键和下标。
pub fn validateOptionalProperties() {
    assert!(
        OPT_PROPS_CNT <= u64::BITS as usize,
        "optional property count exceeds bit-set width"
    );
    for (index, desc) in OPTIONAL_PROPERTY_DESC_LIST.iter().enumerate() {
        assert_eq!(
            desc.Key().0,
            index,
            "optional property description index mismatch"
        );
    }
}

/// 以 u64 位图保存可选属性集合。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OptionalEvalPropKeySet(pub u64);

impl OptionalEvalPropKeySet {
    /// 并入一个键；非法键原样返回。
    pub fn Add(self, key: OptionalEvalPropKey) -> Self {
        if key.0 >= OPT_PROPS_CNT {
            return self;
        }
        Self(self.0 | key.AsPropKeySet().0)
    }

    /// 去掉一个键；非法键原样返回。
    pub fn Remove(self, key: OptionalEvalPropKey) -> Self {
        if key.0 >= OPT_PROPS_CNT {
            return self;
        }
        Self(self.0 & !key.AsPropKeySet().0)
    }

    /// 是否包含指定键。
    pub fn Contains(self, key: OptionalEvalPropKey) -> bool {
        if key.0 >= OPT_PROPS_CNT {
            return false;
        }
        self.0 & key.AsPropKeySet().0 != 0
    }

    /// 有效位全为 0 时为空（忽略未使用高位）。
    pub fn IsEmpty(self) -> bool {
        self.0 & ALL_OPT_PROPS_MASK == 0
    }
    /// 有效位全部置位时为满。
    pub fn IsFull(self) -> bool {
        self.0 & ALL_OPT_PROPS_MASK == ALL_OPT_PROPS_MASK
    }
}
