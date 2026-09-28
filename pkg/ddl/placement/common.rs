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

// Placement（放置策略）公共常量与辅助函数。
//
// Placement 用于把 Region（分布式存储层中一段连续 key 空间的数据分片）
// 调度到满足标签约束的 Store（存储节点）上。本文件集中定义：
// - Bundle（规则组）ID 前缀与特殊组名；
// - Rule（放置规则）的优先级索引（Index，数值越大优先级越高）；
// - Store 标签键值（如 zone/engine），供约束解析与规则构造复用。
//
// 这些常量与 PD（Placement Driver，集群调度中枢）的 placement rule API 对齐。

/// TiFlash（列存引擎）专用规则组 ID。
pub const TiFlashRuleGroupID: &str = "tiflash";
/// DDL 生成的 Bundle ID 前缀，后接对象 ID（表/分区 ID）。
pub const BundleIDPrefix: &str = "TiDB_DDL_";
/// PD 内置默认 Bundle 的 ID。
pub const PDBundleID: &str = "pd";
/// Placement 选项中表示"使用默认值"的关键字。
pub const DefaultKwd: &str = "default";
/// 覆盖全局 key 范围的 Bundle 名称前缀。
pub const TiDBBundleRangePrefixForGlobal: &str = "TiDB_GLOBAL";
/// 覆盖元数据（meta）key 范围的 Bundle 名称前缀。
pub const TiDBBundleRangePrefixForMeta: &str = "TiDB_META";
/// 全局 key 范围标识符。
pub const KeyRangeGlobal: &str = "global";
/// 元数据 key 范围标识符。
pub const KeyRangeMeta: &str = "meta";

/// 元数据键的字节前缀（`m`），用于编码 meta 范围起止键。
pub static metaPrefix: &[u8] = b"m";

/// 由对象 ID（表/分区等）生成 Bundle 组 ID，形如 `TiDB_DDL_<id>`。
pub fn GroupID(id: i64) -> String {
    format!("{BundleIDPrefix}{id}")
}

/// 全局 key 范围规则的 Index（优先级）。
pub const RuleIndexKeyRangeForGlobal: i32 = 20;
/// 元数据 key 范围规则的 Index。
pub const RuleIndexKeyRangeForMeta: i32 = 21;
/// 表级放置规则的 Index。
pub const RuleIndexTable: i32 = 40;
/// 分区级放置规则的 Index（高于表级，分区策略可覆盖表策略）。
pub const RuleIndexPartition: i32 = 80;
/// TiFlash 规则的 Index。
pub const RuleIndexTiFlash: i32 = 120;

/// 可用区（DC/Zone）标签键，常用于 Leader 所在区域约束。
pub const DCLabelKey: &str = "zone";
/// 存储引擎标签键（区分 TiKV / TiFlash 等）。
pub const EngineLabelKey: &str = "engine";
/// TiFlash 列存引擎的标签值。
pub const EngineLabelTiFlash: &str = "tiflash";
/// TiKV 行存引擎的标签值。
pub const EngineLabelTiKV: &str = "tikv";
/// TiFlash 计算节点引擎的标签值。
pub const EngineLabelTiFlashCompute: &str = "tiflash_compute";
/// 引擎角色标签键（如写节点角色）。
pub const EngineRoleLabelKey: &str = "engine_role";
/// 引擎写角色标签值。
pub const EngineRoleLabelWrite: &str = "write";
