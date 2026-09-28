// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! 构建信息单测：Info 七行标签与 LogInfo 无 panic 契约，对齐 Go。
//! 不校验具体版本字符串内容，只锁结构与前缀。
//! AppName 常量 BR/Lightning 仅用于 LogInfo 可调用性，不断言文案内容。

use super::{BR, BuildTS, GitBranch, GitHash, Info, Lightning, LogInfo};
use crate::info::VERSION_METADATA_TEST_LOCK;
use astersql_util_versioninfo::{TiDBBuildTS, TiDBGitBranch, TiDBGitHash};

// TestInfo verifies the same seven labels as Go's TestInfo.
// 断言 Info() 恰好七行，且前缀与 Go 标签文案一致。
#[test]
fn test_info() {
    let info = Info();
    let lines: Vec<_> = info.split('\n').collect();

    assert_eq!(lines.len(), 7, "unexpected Info output: {info:?}");
    // 标签顺序与 Go TestInfo 固定，变更即视为契约破坏。
    for (line, prefix) in lines.iter().zip([
        "Release Version",
        "Git Commit Hash",
        "Git Branch",
        "Rust Version",
        "UTC Build Time",
        "Race Enabled",
        "Kernel Type",
    ]) {
        assert!(
            line.starts_with(prefix),
            "line {line:?} does not start with {prefix:?}"
        );
    }
}

// TestLogInfo keeps Go's two concrete application-name calls and no-panic contract.
// 对 BR/Lightning 两个应用名各打一次日志，仅要求不 panic。
// 不检查日志内容，与 Go 侧“可调用即通过”一致。
#[test]
fn test_log_info() {
    LogInfo(BR);
    LogInfo(Lightning);
}

// Go copies versioninfo globals into package variables during package initialization.
// Later writes to the source package must therefore not change build.Info output.
#[test]
fn version_metadata_is_snapshotted_once() {
    let _guard = VERSION_METADATA_TEST_LOCK.lock().unwrap();
    let initial_build_ts = BuildTS();
    let initial_git_hash = GitHash();
    let initial_git_branch = GitBranch();

    let old_build_ts = *TiDBBuildTS.read().unwrap();
    let old_git_hash = *TiDBGitHash.read().unwrap();
    let old_git_branch = *TiDBGitBranch.read().unwrap();
    *TiDBBuildTS.write().unwrap() = "changed-build-ts";
    *TiDBGitHash.write().unwrap() = "changed-git-hash";
    *TiDBGitBranch.write().unwrap() = "changed-git-branch";

    let observed = (BuildTS(), GitHash(), GitBranch());

    *TiDBBuildTS.write().unwrap() = old_build_ts;
    *TiDBGitHash.write().unwrap() = old_git_hash;
    *TiDBGitBranch.write().unwrap() = old_git_branch;

    assert_eq!(observed.0, initial_build_ts);
    assert_eq!(observed.1, initial_git_hash);
    assert_eq!(observed.2, initial_git_branch);
}
