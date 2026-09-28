// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 轻量构建信息打印：维护 Version / BuildTS / GitHash / GitBranch 全局态，
// 并格式化为欢迎横幅或原始信息字符串。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{LazyLock, RwLock};

/// 发布版本号字符串。
pub static Version: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("None".to_owned()));
/// UTC 构建时间戳。
pub static BuildTS: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("None".to_owned()));
/// Git commit hash。
pub static GitHash: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("None".to_owned()));
/// Git 分支名。
pub static GitBranch: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("None".to_owned()));

/// 写入全局构建元信息，供后续 GetRawInfo / PrintInfo 读取。
pub fn SetBuildInfo(version: &str, build_ts: &str, git_hash: &str, git_branch: &str) {
    *Version.write().expect("version lock poisoned") = version.to_owned();
    *BuildTS.write().expect("build timestamp lock poisoned") = build_ts.to_owned();
    *GitHash.write().expect("git hash lock poisoned") = git_hash.to_owned();
    *GitBranch.write().expect("git branch lock poisoned") = git_branch.to_owned();
}

/// 组装含应用名、版本、Git、构建时间与编译器版本的多行信息。
pub fn GetRawInfo(app: &str) -> String {
    format!(
        "App Name: {app}\nRelease Version: {}\nGit Commit Hash: {}\nGit Branch: {}\nUTC Build Time: {}\nRust Version: {}\n",
        Version.read().expect("version lock poisoned"),
        GitHash.read().expect("git hash lock poisoned"),
        GitBranch.read().expect("git branch lock poisoned"),
        BuildTS.read().expect("build timestamp lock poisoned"),
        rustc_version_runtime::version(),
    )
}

/// 以 info 日志打印欢迎语与构建信息字段。
pub fn PrintInfo(app: &str) {
    log::info!(
        "Welcome to {app} Release Version={} Git Commit Hash={} Git Branch={} UTC Build Time={} Rust Version={}",
        Version.read().expect("version lock poisoned"),
        GitHash.read().expect("git hash lock poisoned"),
        GitBranch.read().expect("git branch lock poisoned"),
        BuildTS.read().expect("build timestamp lock poisoned"),
        rustc_version_runtime::version(),
    );
}
