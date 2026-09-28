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

// Hash Join 版本选择相关的迁移单元测试。
//
// 核对与 Go 一致的大小写折叠、版本常量取值、非移动运行时下 v2 可用性，
// 以及非 GA join 开关的初始值与可写性。

use super::join_version::{
    HASH_JOIN_VERSION_LEGACY, HASH_JOIN_VERSION_OPTIMIZED, TIFLASH_HASH_JOIN_VERSION_DEFAULT,
    is_hash_join_v2_supported, is_optimized_version, set_use_hash_join_v2_for_non_ga_join,
    use_hash_join_v2_for_non_ga_join,
};

/// 版本字符串忽略大小写匹配 `optimized`；带空格或其它别名不得命中。
#[test]
fn optimized_version_matches_go_case_folding_behavior() {
    for value in ["optimized", "OPTIMIZED", "OpTiMiZeD"] {
        assert!(
            is_optimized_version(value),
            "{value} should select hash join v2"
        );
    }

    for value in ["", "legacy", "optimized ", " optimized", "v2"] {
        assert!(
            !is_optimized_version(value),
            "{value:?} must not select hash join v2"
        );
    }
}

/// 常量取值与 Go 包字符串字面量一致；TiFlash 默认等于 legacy。
#[test]
fn version_constants_match_go_values() {
    assert_eq!(HASH_JOIN_VERSION_LEGACY, "legacy");
    assert_eq!(HASH_JOIN_VERSION_OPTIMIZED, "optimized");
    assert_eq!(TIFLASH_HASH_JOIN_VERSION_DEFAULT, HASH_JOIN_VERSION_LEGACY);
}

/// 当前 Rust 运行时满足 v2 的非移动堆假设，故应报告支持。
#[test]
fn hash_join_v2_support_matches_current_non_moving_runtime_assumption() {
    assert!(is_hash_join_v2_supported());
}

/// 非 GA 开关默认为 true，且可通过 setter 翻转后再恢复。
#[test]
fn non_ga_join_switch_is_initialized_and_mutable() {
    assert!(use_hash_join_v2_for_non_ga_join());
    set_use_hash_join_v2_for_non_ga_join(false);
    assert!(!use_hash_join_v2_for_non_ga_join());
    set_use_hash_join_v2_for_non_ga_join(true);
}
