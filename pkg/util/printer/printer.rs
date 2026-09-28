// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// TiDB 版本信息打印与简易 ASCII 表格格式化。
//
// `PrintTiDBInfo` / `GetTiDBInfo` 按 Classic 与 NextGen 分支输出版本、部署模式、
// 企业扩展 hash 与全局配置；`GetPrintResult` 将列与行渲染为与 Go 一致的表框。

#![allow(non_snake_case)]

use std::sync::OnceLock;

use crate::{config, deploymode, israce, kerneltype, mysql, versioninfo};

/// 缓存 rustc 版本字符串，避免重复查询。
static buildVersion: OnceLock<String> = OnceLock::new();

/// 从 versioninfo 全局态读取的版本快照。
struct VersionInfo {
    /// 发行版标识。
    edition: String,
    /// Git commit。
    git_hash: String,
    /// Git 分支。
    git_branch: String,
    /// UTC 构建时间。
    build_ts: String,
    /// 企业扩展 commit；空表示无。
    enterprise_extension_git_hash: String,
}

/// 惰性初始化并返回编译器版本字符串。
fn build_version() -> &'static str {
    buildVersion
        .get_or_init(|| rustc_version_runtime::version().to_string())
        .as_str()
}

/// 计算展示用发布版本：Classic 用原占位；NextGen 尝试转为 TiDBX CLOUD 版本。
/// 返回 `(展示版本, component 版本)`；失败或非 NextGen 时第二项为空。
fn getReleaseVersionsForDisplay() -> (String, String) {
    let release_version = unsafe { mysql::TiDBReleaseVersion };
    let releaseVersion = release_version.to_owned();
    if !kerneltype::IsNextGen() {
        return (releaseVersion, String::new());
    }

    let normalizedReleaseVersion = mysql::NormalizeTiDBReleaseVersionForNextGen(release_version);
    match mysql::BuildTiDBXReleaseVersion(&normalizedReleaseVersion) {
        Ok(tidbXReleaseVersion) => (tidbXReleaseVersion, normalizedReleaseVersion),
        Err(_) => (releaseVersion, String::new()),
    }
}

/// 从全局 versioninfo 读出 VersionInfo 快照。
fn version_info() -> VersionInfo {
    VersionInfo {
        edition: (*versioninfo::TiDBEdition.read().unwrap()).to_owned(),
        git_hash: (*versioninfo::TiDBGitHash.read().unwrap()).to_owned(),
        git_branch: (*versioninfo::TiDBGitBranch.read().unwrap()).to_owned(),
        build_ts: (*versioninfo::TiDBBuildTS.read().unwrap()).to_owned(),
        enterprise_extension_git_hash: (*versioninfo::TiDBEnterpriseExtensionGitHash
            .read()
            .unwrap())
        .to_owned(),
    }
}

/// Prints the TiDB version information and the loaded configuration.
/// 打印欢迎日志（按 NextGen / 企业扩展 hash 选择字段），再打印全局配置 JSON。
pub fn PrintTiDBInfo() {
    let (releaseVersion, componentVersion) = getReleaseVersionsForDisplay();
    let info = version_info();
    let check_table_before_drop = config::CheckTableBeforeDrop();
    let kernel_type = kerneltype::Name();
    let enterprise_hash = info.enterprise_extension_git_hash.as_str();

    // NextGen：额外带 component_version 与 deploy_mode；企业版再带 extension hash。
    if kerneltype::IsNextGen() {
        let deploy_mode = deploymode::Get().String();
        if enterprise_hash.is_empty() {
            tracing::info!(
                release_version = %releaseVersion,
                edition = %info.edition,
                git_commit_hash = %info.git_hash,
                git_branch = %info.git_branch,
                utc_build_time = %info.build_ts,
                go_version = %build_version(),
                race_enabled = israce::RaceEnabled,
                check_table_before_drop,
                component_version = %componentVersion,
                deploy_mode = %deploy_mode,
                kernel_type = %kernel_type,
                "Welcome to TiDB."
            );
        } else {
            tracing::info!(
                release_version = %releaseVersion,
                edition = %info.edition,
                git_commit_hash = %info.git_hash,
                git_branch = %info.git_branch,
                utc_build_time = %info.build_ts,
                go_version = %build_version(),
                race_enabled = israce::RaceEnabled,
                check_table_before_drop,
                component_version = %componentVersion,
                deploy_mode = %deploy_mode,
                kernel_type = %kernel_type,
                enterprise_extension_commit_hash = %enterprise_hash,
                "Welcome to TiDB."
            );
        }
    } else if enterprise_hash.is_empty() {
        tracing::info!(
            release_version = %releaseVersion,
            edition = %info.edition,
            git_commit_hash = %info.git_hash,
            git_branch = %info.git_branch,
            utc_build_time = %info.build_ts,
            go_version = %build_version(),
            race_enabled = israce::RaceEnabled,
            check_table_before_drop,
            kernel_type = %kernel_type,
            "Welcome to TiDB."
        );
    } else {
        tracing::info!(
            release_version = %releaseVersion,
            edition = %info.edition,
            git_commit_hash = %info.git_hash,
            git_branch = %info.git_branch,
            utc_build_time = %info.build_ts,
            go_version = %build_version(),
            race_enabled = israce::RaceEnabled,
            check_table_before_drop,
            kernel_type = %kernel_type,
            enterprise_extension_commit_hash = %enterprise_hash,
            "Welcome to TiDB."
        );
    }

    let configJSON = serde_json::to_string(&config::GetGlobalConfig())
        .expect("global TiDB configuration must be serializable");
    tracing::info!(config = %configJSON, "loaded config");
}

/// Returns the git hash and build time of this tidb-server binary.
/// 组装多行纯文本版本信息，供 SHOW 或诊断输出使用。
pub fn GetTiDBInfo() -> String {
    let (releaseVersion, _) = getReleaseVersionsForDisplay();
    let version = version_info();
    let enterpriseVersion = if version.enterprise_extension_git_hash.is_empty() {
        String::new()
    } else {
        format!(
            "\nEnterprise Extension Commit Hash: {}",
            version.enterprise_extension_git_hash
        )
    };

    format!(
        "Release Version: {releaseVersion}\n\
         Edition: {}\n\
         Git Commit Hash: {}\n\
         Git Branch: {}\n\
         UTC Build Time: {}\n\
         RustVersion: {}\n\
         Race Enabled: {}\n\
         Check Table Before Drop: {}\n\
         Store: {}{}\n\
         Kernel Type: {}",
        version.edition,
        version.git_hash,
        version.git_branch,
        version.build_ts,
        build_version(),
        israce::RaceEnabled,
        config::CheckTableBeforeDrop(),
        config::GetGlobalConfig().store,
        enterpriseVersion,
        kerneltype::Name(),
    )
}

/// 校验列非空、行非空，且每行列数与列头一致。
fn checkValidity(cols: &[String], datas: &[Vec<String>]) -> bool {
    !cols.is_empty() && !datas.is_empty() && datas.iter().all(|data| data.len() == cols.len())
}

/// 按字节长度计算每列最大宽度（与 Go `len(string)` 一致，非显示宽度）。
fn getMaxColLen(cols: &[String], datas: &[Vec<String>]) -> Vec<usize> {
    let mut maxColLen: Vec<_> = cols.iter().map(String::len).collect();
    for data in datas {
        for (index, value) in data.iter().enumerate() {
            maxColLen[index] = maxColLen[index].max(value.len());
        }
    }
    maxColLen
}

/// 生成表框分隔行，形如 `+----+----+`。
fn getPrintDivLine(maxColLen: &[usize]) -> String {
    let mut value = String::new();
    for width in maxColLen {
        value.push('+');
        value.extend(std::iter::repeat('-').take(width + 2));
    }
    value.push_str("+\n");
    value
}

/// 按列宽填充一行单元格。
fn getPrintRow(data: &[String], maxColLen: &[usize]) -> String {
    let mut value = String::new();
    for (index, cell) in data.iter().enumerate() {
        value.push_str("| ");
        value.push_str(cell);
        value.extend(std::iter::repeat(' ').take(maxColLen[index] + 1 - cell.len()));
    }
    value.push_str("|\n");
    value
}

/// 打印表头行（复用 getPrintRow）。
fn getPrintCol(cols: &[String], maxColLen: &[usize]) -> String {
    getPrintRow(cols, maxColLen)
}

/// 拼接所有数据行。
fn getPrintRows(datas: &[Vec<String>], maxColLen: &[usize]) -> String {
    datas
        .iter()
        .map(|data| getPrintRow(data, maxColLen))
        .collect()
}

/// Gets a result formatted as the table emitted by the Go implementation.
/// 将列与数据格式化为 ASCII 表；校验失败返回空串与 false。
pub fn GetPrintResult(cols: &[String], datas: &[Vec<String>]) -> (String, bool) {
    if !checkValidity(cols, datas) {
        return (String::new(), false);
    }

    let maxColLen = getMaxColLen(cols, datas);
    let divider = getPrintDivLine(&maxColLen);
    let mut value = String::new();
    value.push_str(&divider);
    value.push_str(&getPrintCol(cols, &maxColLen));
    value.push_str(&divider);
    value.push_str(&getPrintRows(datas, &maxColLen));
    value.push_str(&divider);
    (value, true)
}
