// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 内核类型（kernel type）判定逻辑的单元测试。
//
// 数据库内核存在两种构建形态：Classic（经典版）与 Next Generation（下一代版），
// 通过 Cargo feature `nextgen` 在编译期选择。本测试验证
// `IsClassic`/`IsNextGen`/`Name`/`IsMatch` 等接口在两种形态下的行为一致性，
// 以及与 PD（Placement Driver，集群元数据与调度中心）侧内核类型字符串匹配
// 时保持与 Go 版本相同的兼容性语义。

// 引入被测的内核类型查询函数：
// - IsClassic/IsNextGen：判断当前编译的内核形态
// - Name：返回当前内核形态的显示名称
// - IsMatch：判断 PD 上报的内核类型字符串是否与当前内核匹配
use super::{IsClassic, IsMatch, IsNextGen, Name};

/// 验证两个内核形态标志互斥且互补：任意时刻恰好一个为真。
#[test]
fn kernel_type_flags_are_complements() {
    assert_eq!(!IsClassic(), IsNextGen());
    assert_eq!(IsClassic(), !IsNextGen());
}

/// 验证内核名称与编译期选择的构建形态一致。
///
/// `cfg!(feature = "nextgen")` 在编译期展开为布尔值，
/// 据此断言运行时接口返回的形态标志与名称字符串相匹配。
#[test]
fn current_kernel_name_matches_the_selected_build() {
    // 启用 nextgen feature 时应为 Next Generation 内核，否则为 Classic 内核
    if cfg!(feature = "nextgen") {
        assert!(IsNextGen());
        assert_eq!("Next Generation", Name());
    } else {
        assert!(IsClassic());
        assert_eq!("Classic", Name());
    }
}

/// 验证与 PD 内核类型字符串的匹配规则保持 Go 版本兼容语义。
///
/// 规则要点：匹配区分大小写；空字符串仅在 Classic 内核下视为匹配
/// （历史上旧版 PD 不上报内核类型，等价于 Classic），Next Generation
/// 内核则要求 PD 明确上报对应名称。
#[test]
fn pd_kernel_type_matching_preserves_go_compatibility() {
    // 当前内核自身的名称必然匹配；未知名称与大小写不符的名称均不匹配
    assert!(IsMatch(Name()));
    assert!(!IsMatch("Unknown"));
    assert!(!IsMatch("classic"));

    // 分内核形态验证空字符串与对方形态名称的匹配结果
    if IsClassic() {
        assert!(IsMatch(""));
        assert!(IsMatch("Classic"));
        assert!(!IsMatch("Next Generation"));
    } else {
        assert!(!IsMatch(""));
        assert!(IsMatch("Next Generation"));
        assert!(!IsMatch("Classic"));
    }
}
