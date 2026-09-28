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
// See the License for the specific language governing permissions and
// limitations under the License.

// TiDB 专有语法特性标识（feature ID）与可解析白名单。
//
// 对照 Go `features.go`：DDL/语法扩展用字符串 ID 标记能力（如 AUTO_RANDOM、
// 聚簇索引、TTL、Placement 等）；`CanParseFeature` 检查给定 ID 是否均在允许列表中。
// 空白的 `FEATURE_ID_TIDB` 表示「通用 TiDB」占位，不在可解析白名单内。

#![allow(non_upper_case_globals, non_snake_case)]

/// 通用 TiDB 占位特性 ID；值为空串，不参与可解析白名单。
pub const FEATURE_ID_TIDB: &str = "";
/// AUTO_RANDOM：按随机位分配自增主键，减轻热点写入。
pub const FEATURE_ID_AUTO_RANDOM: &str = "auto_rand";
/// AUTO_ID_CACHE：控制自增 ID 预分配缓存大小相关语法。
pub const FEATURE_ID_AUTO_ID_CACHE: &str = "auto_id_cache";
/// AUTO_RANDOM_BASE：AUTO_RANDOM 基值相关语法。
pub const FEATURE_ID_AUTO_RANDOM_BASE: &str = "auto_rand_base";
/// 聚簇索引（clustered index）：主键与行数据同组织存储。
pub const FEATURE_ID_CLUSTERED_INDEX: &str = "clustered_index";
/// 强制自增相关语法扩展。
pub const FEATURE_ID_FORCE_AUTO_INC: &str = "force_inc";
/// Placement：表/分区的副本放置策略（与 PD 调度相关）。
pub const FEATURE_ID_PLACEMENT: &str = "placement";
/// TTL：按时间自动清理过期行的表属性。
pub const FEATURE_ID_TTL: &str = "ttl";
/// 资源组（resource group）：请求限流与优先级隔离。
pub const FEATURE_ID_RESOURCE_GROUP: &str = "resource_group";
/// 全局索引：分区表上跨分区的唯一/二级索引。
pub const FEATURE_ID_GLOBAL_INDEX: &str = "global_index";
/// 预分裂（pre-split）：建表时预先切分 Region，减轻热点。
pub const FEATURE_ID_PRESPLIT: &str = "pre_split";
/// 亲和性（affinity）相关表属性。
pub const FEATURE_ID_AFFINITY: &str = "affinity";
/// Region 分裂相关语法；Region 是 TiKV 的数据分片与调度单位。
pub const FEATURE_ID_SPLIT_REGION: &str = "region_split";

/// 解析器当前允许识别的特性 ID 白名单（不含 TIDB 空串与 RESOURCE_GROUP）。
const FEATURE_IDS: &[&str] = &[
    FEATURE_ID_AUTO_RANDOM,
    FEATURE_ID_AUTO_ID_CACHE,
    FEATURE_ID_AUTO_RANDOM_BASE,
    FEATURE_ID_CLUSTERED_INDEX,
    FEATURE_ID_FORCE_AUTO_INC,
    FEATURE_ID_PLACEMENT,
    FEATURE_ID_TTL,
    FEATURE_ID_GLOBAL_INDEX,
    FEATURE_ID_PRESPLIT,
    FEATURE_ID_AFFINITY,
    FEATURE_ID_SPLIT_REGION,
];

/// 检查给定特性 ID 列表是否全部落在白名单内；空列表视为可解析。
pub fn can_parse_feature(features: &[&str]) -> bool {
    features.iter().all(|feature| FEATURE_IDS.contains(feature))
}

/// Go 风格导出名：与 FEATURE_ID_TIDB 相同。
pub const FeatureIDTiDB: &str = FEATURE_ID_TIDB;
/// Go 风格导出名：与 FEATURE_ID_AUTO_RANDOM 相同。
pub const FeatureIDAutoRandom: &str = FEATURE_ID_AUTO_RANDOM;
/// Go 风格导出名：与 FEATURE_ID_AUTO_ID_CACHE 相同。
pub const FeatureIDAutoIDCache: &str = FEATURE_ID_AUTO_ID_CACHE;
/// Go 风格导出名：与 FEATURE_ID_AUTO_RANDOM_BASE 相同。
pub const FeatureIDAutoRandomBase: &str = FEATURE_ID_AUTO_RANDOM_BASE;
/// Go 风格导出名：与 FEATURE_ID_CLUSTERED_INDEX 相同。
pub const FeatureIDClusteredIndex: &str = FEATURE_ID_CLUSTERED_INDEX;
/// Go 风格导出名：与 FEATURE_ID_FORCE_AUTO_INC 相同。
pub const FeatureIDForceAutoInc: &str = FEATURE_ID_FORCE_AUTO_INC;
/// Go 风格导出名：与 FEATURE_ID_PLACEMENT 相同。
pub const FeatureIDPlacement: &str = FEATURE_ID_PLACEMENT;
/// Go 风格导出名：与 FEATURE_ID_TTL 相同。
pub const FeatureIDTTL: &str = FEATURE_ID_TTL;
/// Go 风格导出名：与 FEATURE_ID_RESOURCE_GROUP 相同。
pub const FeatureIDResourceGroup: &str = FEATURE_ID_RESOURCE_GROUP;
/// Go 风格导出名：与 FEATURE_ID_GLOBAL_INDEX 相同。
pub const FeatureIDGlobalIndex: &str = FEATURE_ID_GLOBAL_INDEX;
/// Go 风格导出名：与 FEATURE_ID_PRESPLIT 相同。
pub const FeatureIDPresplit: &str = FEATURE_ID_PRESPLIT;
/// Go 风格导出名：与 FEATURE_ID_AFFINITY 相同。
pub const FeatureIDAffinity: &str = FEATURE_ID_AFFINITY;
/// Go 风格导出名：与 FEATURE_ID_SPLIT_REGION 相同。
pub const FeatureIDSplitRegion: &str = FEATURE_ID_SPLIT_REGION;

/// Go 风格导出名：委托给 `can_parse_feature`。
pub fn CanParseFeature(features: &[&str]) -> bool {
    can_parse_feature(features)
}
