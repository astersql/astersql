// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Dumpling CLI version metadata, matching Go `dumpling/cli`.
//!
//! 本模块集中保存 dumpling CLI 在构建时注入的版本元数据，
//! 并统一提供字符串展示与日志输出两个对外入口。
//! Rust 实现保留公共元数据顺序，并展示实际 Rust 编译器版本。

use std::sync::RwLock;

use astersql_dumpling_log::{Field, Logger};

/// ReleaseVersion is the current program version.
/// 默认值使用 `"Unknown"`，与 Go 的未注入构建产物保持一致。
pub static ReleaseVersion: RwLock<&'static str> = RwLock::new("Unknown");
/// BuildTimestamp is the UTC date time when the program is compiled.
/// `LongVersion` 会无条件补上 `Z`，因此这里存的是不带后缀的主体值。
pub static BuildTimestamp: RwLock<&'static str> = RwLock::new("Unknown");
/// GitHash is the git commit hash when the program is compiled.
pub static GitHash: RwLock<&'static str> = RwLock::new("Unknown");
/// GitBranch is the active git branch when the program is compiled.
pub static GitBranch: RwLock<&'static str> = RwLock::new("Unknown");
/// Optional compiler version override for tests and build metadata injection.
pub static RustVersion: RwLock<Option<&'static str>> = RwLock::new(None);

fn rust_version() -> String {
    RustVersion
        .read()
        .expect("version var poisoned")
        .map(str::to_owned)
        .unwrap_or_else(|| rustc_version_runtime::version().to_string())
}

fn read_var(v: &RwLock<&'static str>) -> &'static str {
    // 读锁封装成单点辅助函数，便于所有导出路径共享同一套 poisoning 处理。
    *v.read().expect("version var poisoned")
}

/// LongVersion returns the version information of this program as a string.
pub fn LongVersion() -> String {
    // 保持元数据顺序，编译器字段明确标识 Rust。
    format!(
        "Release version: {}\n\
Git commit hash: {}\n\
Git branch:      {}\n\
Build timestamp: {}Z\n\
Rust version:    {}\n",
        read_var(&ReleaseVersion),
        read_var(&GitHash),
        read_var(&GitBranch),
        read_var(&BuildTimestamp),
        rust_version(),
    )
}

/// LogLongVersion logs the version information of this program to the logger.
pub fn LogLongVersion(logger: &Logger) {
    // 日志字段与版本文本使用同一编译器信息。
    logger.Info(
        "Welcome to dumpling",
        [
            Field::string("Release Version", read_var(&ReleaseVersion)),
            Field::string("Git Commit Hash", read_var(&GitHash)),
            Field::string("Git Branch", read_var(&GitBranch)),
            Field::string("Build timestamp", read_var(&BuildTimestamp)),
            Field::string("Rust Version", rust_version()),
        ],
    );
}

/// Reset version metadata and clear the compiler version override.
pub fn reset_version_vars() {
    // 测试会直接覆写这些全局值，因此需要提供显式恢复入口来隔离用例。
    *ReleaseVersion.write().expect("poisoned") = "Unknown";
    *BuildTimestamp.write().expect("poisoned") = "Unknown";
    *GitHash.write().expect("poisoned") = "Unknown";
    *GitBranch.write().expect("poisoned") = "Unknown";
    *RustVersion.write().expect("poisoned") = None;
}
