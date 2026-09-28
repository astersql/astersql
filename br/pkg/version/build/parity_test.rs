// Copyright 2026 AsterSQL.

//! build 包公开契约对照：Info 标签序、哨兵回落、LogInfo 捕获与常量。
//! 对应 Go `br/pkg/version/build` 对外可见字段与欢迎语格式。

use astersql_config_kerneltype::IsNextGen;
use astersql_util_israce::RaceEnabled;
use astersql_util_versioninfo::{TiDBBuildTS, TiDBGitBranch, TiDBGitHash};

use crate::info::VERSION_METADATA_TEST_LOCK;
use crate::{
    BR, BuildTS, GitBranch, GitHash, Info, Lightning, LogInfo, ReleaseVersion,
    ReleaseVersionForTest, take_logged_info,
};

/// 聚合正常标签、边界哨兵与 LogInfo 副作用，锁定 Go/Rust 公开契约。
#[test]
fn go_rust_public_contract_matches() {
    let _guard = VERSION_METADATA_TEST_LOCK.lock().unwrap();
    // normal: Info line order and labels match Go
    // Info 七行标签顺序与 Go 完全一致，变更即契约破坏。
    let info = Info();
    let lines: Vec<_> = info.lines().collect();
    assert!(lines[0].starts_with("Release Version: "));
    assert!(lines[1].starts_with("Git Commit Hash: "));
    assert!(lines[2].starts_with("Git Branch: "));
    assert!(lines[3].starts_with("Rust Version: "));
    assert!(lines[4].starts_with("UTC Build Time: "));
    assert_eq!(lines[5], format!("Race Enabled: {RaceEnabled}"));
    let expected_kt = if IsNextGen() { "Next-Gen" } else { "Classic" };
    assert_eq!(lines[6], format!("Kernel Type: {expected_kt}"));

    // boundary: default versioninfo sentinels
    // BuildTS/Git* 必须透传 versioninfo 注入值，不做二次改写。
    assert_eq!(BuildTS(), *TiDBBuildTS.read().unwrap());
    assert_eq!(GitHash(), *TiDBGitHash.read().unwrap());
    assert_eq!(GitBranch(), *TiDBGitBranch.read().unwrap());

    // sentinel mysql release → nightly-dirty (current default contains the sentinel)
    // 含占位哨兵时回落 nightly-dirty；真实版本则不得仍含占位片段。
    let rv = ReleaseVersion();
    assert!(
        rv == ReleaseVersionForTest || !rv.contains(concat!("this-is-a-", "place", "holder")),
        "unexpected release version {rv}"
    );

    // resource / side-effect: LogInfo captures welcome line then restores (no panic)
    // 先排空缓冲再 LogInfo，断言恰好一行且含 BR 欢迎语关键字段。
    let _ = take_logged_info();
    LogInfo(BR);
    let logged = take_logged_info();
    assert_eq!(logged.len(), 1);
    assert!(logged[0].contains("Welcome to Backup & Restore (BR)"));
    assert!(logged[0].contains(&format!("release-version={rv}")));
    assert!(logged[0].contains(&format!("race-enabled={RaceEnabled}")));
    assert!(logged[0].contains(&format!("for-next-gen?={}", IsNextGen())));

    // constants
    // 应用名与测试回落常量与 Go 字面量对齐。
    assert_eq!(Lightning, "TiDB-Lightning");
    assert_eq!(ReleaseVersionForTest, "nightly-dirty");
}
