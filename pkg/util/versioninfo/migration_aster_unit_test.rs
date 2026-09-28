// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// versioninfo 迁移回归：包级版本变量默认值与可写覆盖。
//
// 对齐 Go `pkg/util/versioninfo` 的 package 变量语义（可被发布构建替换）。

use super::{
    CommunityEdition, TiDBBuildTS, TiDBEdition, TiDBEnterpriseExtensionGitHash, TiDBGitBranch,
    TiDBGitHash,
};

/// 断言发行版常量与各静态量的初始默认值与 Go 一致。
#[test]
fn version_information_defaults_match_go() {
    assert_eq!(CommunityEdition, "Community");
    assert_eq!(*TiDBBuildTS.read().unwrap(), "None");
    assert_eq!(*TiDBGitBranch.read().unwrap(), "None");
    assert_eq!(*TiDBEdition.read().unwrap(), CommunityEdition);
    assert_eq!(*TiDBEnterpriseExtensionGitHash.read().unwrap(), "");
}

/// 断言 `TiDBGitHash` 可像 Go package 变量一样读写覆盖，并恢复原值。
#[test]
fn version_information_can_be_overridden_like_go_package_variables() {
    let original = *TiDBGitHash.read().unwrap();
    assert_eq!(original, "None");
    // 模拟发布构建注入 git hash，再写回以免污染其它测试。
    *TiDBGitHash.write().unwrap() = "test-git-hash";
    assert_eq!(*TiDBGitHash.read().unwrap(), "test-git-hash");
    *TiDBGitHash.write().unwrap() = original;
}
