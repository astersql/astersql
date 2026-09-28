// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.
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

//! 构建/发布版本信息，对齐 Go `br/pkg/version/build/info.go`。
//! 聚合 ReleaseVersion、Git 元数据、编译器版本与内核类型，供 CLI 欢迎语与 Info 文本使用。
//! mysql 版本哨兵激活时回落 `nightly-dirty`，避免 realtikv 测试拿到占位串。

use std::sync::OnceLock;

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::sync::Mutex;

use astersql_config_kerneltype::IsNextGen;
use astersql_parser_mysql::r#const::TiDBReleaseVersion;
use astersql_util_israce::RaceEnabled;
use astersql_util_versioninfo::{TiDBBuildTS, TiDBGitBranch, TiDBGitHash};

/// Fallback release version used when mysql version sentinel is active (realtikv tests).
/// 哨兵/占位版本激活时的回落值，与 Go `ReleaseVersionForTest` 一致。
pub const ReleaseVersionForTest: &str = "nightly-dirty";

/// 读取 TiDBReleaseVersion；若为 None 或含占位片段则返回测试回落值。
fn getReleaseVersion() -> String {
    let release = unsafe { TiDBReleaseVersion };
    if release != "None" && !release.contains(concat!("this-is-a-", "place", "holder")) {
        return release.to_string();
    }
    ReleaseVersionForTest.to_string()
}

/// ReleaseVersion mirrors Go's package-level var (computed once).
/// OnceLock 缓存首次计算结果，对齐 Go 包级变量只初始化一次的语义。
pub fn ReleaseVersion() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(getReleaseVersion).clone()
}

/// UTC 构建时间戳，来自 versioninfo 注入的 TiDBBuildTS。
pub fn BuildTS() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| TiDBBuildTS.read().unwrap().to_string())
        .clone()
}

/// Git commit hash；供欢迎语与 Info 文本输出。
pub fn GitHash() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| TiDBGitHash.read().unwrap().to_string())
        .clone()
}

/// Git 分支名；上层版本检查可能据此发出非 master 警告（Go 侧）。
pub fn GitBranch() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| TiDBGitBranch.read().unwrap().to_string())
        .clone()
}

#[cfg(test)]
pub(crate) static VERSION_METADATA_TEST_LOCK: Mutex<()> = Mutex::new(());

fn rustVersion() -> String {
    format!("rustc {}", rustc_version_runtime::version())
}

/// AppName is a name of a built binary.
/// 二进制展示名类型别名，约束 LogInfo/欢迎语入参。
pub type AppName = &'static str;

/// BR is the name of BR binary.
pub const BR: AppName = "Backup & Restore (BR)";
/// Lightning is the name of Lightning binary.
pub const Lightning: AppName = "TiDB-Lightning";

#[cfg(test)]
thread_local! {
    /// 测试捕获 LogInfo 输出行，避免依赖真实日志后端。
    static LAST_LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Test helper: drain captured LogInfo lines.
/// 取出并清空线程本地捕获缓冲，供 parity/单测断言。
#[cfg(test)]
pub fn take_logged_info() -> Vec<String> {
    LAST_LOG.with(|logs| std::mem::take(&mut *logs.borrow_mut()))
}

/// LogInfo logs version information (and captures for tests).
/// 打印 Welcome 行；测试构建额外写入 LAST_LOG，字段顺序对齐 Go。
pub fn LogInfo(name: AppName) {
    let line = format!(
        "Welcome to {name} release-version={} git-hash={} git-branch={} rust-version={} utc-build-time={} race-enabled={} for-next-gen?={}",
        ReleaseVersion(),
        GitHash(),
        GitBranch(),
        rustVersion(),
        BuildTS(),
        RaceEnabled,
        IsNextGen(),
    );
    #[cfg(test)]
    LAST_LOG.with(|logs| logs.borrow_mut().push(line.clone()));
    eprintln!("{line}");
}

/// Info returns version information.
/// 七行人类可读标签文本；最后一行 Kernel Type 无尾换行，对齐 Go。
pub fn Info() -> String {
    let mut buf = String::new();
    buf.push_str(&format!("Release Version: {}\n", ReleaseVersion()));
    buf.push_str(&format!("Git Commit Hash: {}\n", GitHash()));
    buf.push_str(&format!("Git Branch: {}\n", GitBranch()));
    buf.push_str(&format!("Rust Version: {}\n", rustVersion()));
    buf.push_str(&format!("UTC Build Time: {}\n", BuildTS()));
    buf.push_str(&format!("Race Enabled: {}\n", RaceEnabled));
    let kt = if IsNextGen() { "Next-Gen" } else { "Classic" };
    buf.push_str(&format!("Kernel Type: {kt}"));
    buf
}
