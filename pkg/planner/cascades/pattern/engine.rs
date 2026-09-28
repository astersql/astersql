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

// 执行引擎类型位掩码：描述算子/规则可落在 TiDB、TiKV 或 TiFlash 上。
//
// Cascades pattern 匹配时用 `EngineTypeSet` 约束候选引擎；位运算组合对应
// Go 侧 `EngineTypeSet` 常量（Only / Or / All）。

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// 单一执行引擎标识（按位标志，便于与集合做与运算）。
pub struct EngineType(pub u32);

/// TiDB 行存计算引擎（coordinator / 本地执行）。
pub const EngineTiDB: EngineType = EngineType(1 << 0);
/// TiKV 存储引擎（coprocessor 下推目标之一）。
pub const EngineTiKV: EngineType = EngineType(1 << 1);
/// TiFlash 列存引擎（分析型下推目标）。
pub const EngineTiFlash: EngineType = EngineType(1 << 2);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// 引擎类型集合：多个 `EngineType` 位或组合。
pub struct EngineTypeSet(pub u32);

/// 仅允许 TiDB 引擎。
pub const EngineTiDBOnly: EngineTypeSet = EngineTypeSet(EngineTiDB.0);
/// 仅允许 TiKV 引擎。
pub const EngineTiKVOnly: EngineTypeSet = EngineTypeSet(EngineTiKV.0);
/// 仅允许 TiFlash 引擎。
pub const EngineTiFlashOnly: EngineTypeSet = EngineTypeSet(EngineTiFlash.0);
/// 允许 TiKV 或 TiFlash（常见于可下推到存储层的算子）。
pub const EngineTiKVOrTiFlash: EngineTypeSet = EngineTypeSet(EngineTiKV.0 | EngineTiFlash.0);
/// 允许全部引擎。
pub const EngineAll: EngineTypeSet = EngineTypeSet(EngineTiDB.0 | EngineTiKV.0 | EngineTiFlash.0);

impl EngineTypeSet {
    /// 判断集合是否包含指定引擎。
    pub fn Contains(self, engine: EngineType) -> bool {
        self.0 & engine.0 != 0
    }
}

/// 打印引擎名，未知位返回 `UnknownEngineType`。
impl fmt::Display for EngineType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match *self {
            EngineTiDB => "EngineTiDB",
            EngineTiKV => "EngineTiKV",
            EngineTiFlash => "EngineTiFlash",
            _ => "UnknownEngineType",
        })
    }
}
