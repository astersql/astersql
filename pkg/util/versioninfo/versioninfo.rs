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

// 构建版本信息：发行版常量与可被发布构建覆盖的包级静态字符串。
//
// 对应 Go `pkg/util/versioninfo`。`RwLock` 模拟 Go package 变量的可变性，
// 便于 `-X`/注入式替换构建时间戳、git 分支/哈希与发行版标识。

#![allow(non_upper_case_globals)]

use std::sync::RwLock;

/// CommunityEdition is the default edition for building.
/// 社区版默认发行版名称。
pub const CommunityEdition: &str = "Community";

/// Version information. These values are mutable because release builds may
/// replace the defaults, matching the Go package variables.
/// 构建时间戳（字符串），发布构建可覆盖。
pub static TiDBBuildTS: RwLock<&str> = RwLock::new("None");
/// 当前二进制对应的 git commit hash。
pub static TiDBGitHash: RwLock<&str> = RwLock::new("None");
/// 当前二进制对应的 git 分支名。
pub static TiDBGitBranch: RwLock<&str> = RwLock::new("None");
/// 发行版标识（默认 Community）。
pub static TiDBEdition: RwLock<&str> = RwLock::new(CommunityEdition);
/// 企业扩展组件的 git hash；社区构建通常为空。
pub static TiDBEnterpriseExtensionGitHash: RwLock<&str> = RwLock::new("");
