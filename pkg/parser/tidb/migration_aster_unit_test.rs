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

// TiDB 特性标识与 `CanParseFeature` 迁移对齐单元测试。
//
// 对照 Go 侧常量字符串与白名单语义：校验 ID 字面量、已注册集合、
// 空参/重复项/未知项，以及 Go 风格别名导出是否与 snake_case 常量一致。

use super::{
    CanParseFeature, FEATURE_ID_AFFINITY, FEATURE_ID_AUTO_ID_CACHE, FEATURE_ID_AUTO_RANDOM,
    FEATURE_ID_AUTO_RANDOM_BASE, FEATURE_ID_CLUSTERED_INDEX, FEATURE_ID_FORCE_AUTO_INC,
    FEATURE_ID_GLOBAL_INDEX, FEATURE_ID_PLACEMENT, FEATURE_ID_PRESPLIT, FEATURE_ID_RESOURCE_GROUP,
    FEATURE_ID_SPLIT_REGION, FEATURE_ID_TIDB, FEATURE_ID_TTL, FeatureIDAutoRandom,
    FeatureIDResourceGroup, can_parse_feature,
};

/// 断言各 FEATURE_ID_* 字符串字面量与 Go 源文件一致。
#[test]
fn feature_identifiers_match_go_values() {
    assert_eq!(FEATURE_ID_TIDB, "");
    assert_eq!(FEATURE_ID_AUTO_RANDOM, "auto_rand");
    assert_eq!(FEATURE_ID_AUTO_ID_CACHE, "auto_id_cache");
    assert_eq!(FEATURE_ID_AUTO_RANDOM_BASE, "auto_rand_base");
    assert_eq!(FEATURE_ID_CLUSTERED_INDEX, "clustered_index");
    assert_eq!(FEATURE_ID_FORCE_AUTO_INC, "force_inc");
    assert_eq!(FEATURE_ID_PLACEMENT, "placement");
    assert_eq!(FEATURE_ID_TTL, "ttl");
    assert_eq!(FEATURE_ID_RESOURCE_GROUP, "resource_group");
    assert_eq!(FEATURE_ID_GLOBAL_INDEX, "global_index");
    assert_eq!(FEATURE_ID_PRESPLIT, "pre_split");
    assert_eq!(FEATURE_ID_AFFINITY, "affinity");
    assert_eq!(FEATURE_ID_SPLIT_REGION, "region_split");
}

/// 白名单内 ID 可解析；TIDB 空串、RESOURCE_GROUP 与未知 ID 均拒绝。
#[test]
fn registered_features_match_the_go_allowlist() {
    let registered = [
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

    assert!(can_parse_feature(&registered));
    // FEATURE_ID_TIDB 为空串，不在 FEATURE_IDS 表中。
    assert!(!can_parse_feature(&[FEATURE_ID_TIDB]));
    // RESOURCE_GROUP 有常量但未列入可解析白名单，与 Go 一致。
    assert!(!can_parse_feature(&[FEATURE_ID_RESOURCE_GROUP]));
    assert!(!can_parse_feature(&["unknown"]));
}

/// 保留 Go 可变参数语义：空列表通过、重复合法 ID 通过、夹杂未知 ID 失败。
#[test]
fn feature_validation_preserves_go_variadic_semantics() {
    assert!(can_parse_feature(&[]));
    assert!(can_parse_feature(&[
        FEATURE_ID_AUTO_RANDOM,
        FEATURE_ID_AUTO_RANDOM,
    ]));
    assert!(!can_parse_feature(&[
        FEATURE_ID_AUTO_RANDOM,
        "unknown",
        FEATURE_ID_TTL,
    ]));

    // Go 风格别名与 snake_case 常量指向同一字符串。
    assert!(CanParseFeature(&[FeatureIDAutoRandom]));
    assert!(!CanParseFeature(&[FeatureIDResourceGroup]));
}
