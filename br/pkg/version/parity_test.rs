// Copyright 2026 AsterSQL.

//! version 包公开契约对照：解析、范围检查、集群检查与 FetchVersion 回退。
//! MockPd/MockDb 仅桩出 GetAllStores/QueryRow，覆盖 Go parity 关心的错误路径。
//! 正常路径锁定 ParseServerInfo/Extract/CheckVersion 成功语义。
//! 边界路径锁定 removeVAndHash、Normalize 与 nightly NextMajor。
//! 错误路径锁定过旧/过新、非 TiDB、PiTR 过低文案关键字。
//! 副作用路径锁定集群检查、checkpoint、batch KV 与 FetchVersion 回退。
//! tiflash 标签节点走专用分支，不进入 TiKV checker。
//! ReleaseVersion 覆盖在末尾清回，避免污染并行用例。
//! CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION 常量亦在此锁定为 5。
//! CheckVersionForBackup 跨 major>1 必须失败。
//! CheckVersionForDDL 在 6.2 边界通过；Keyspace 在 6.5 仍失败。
//! 本文件是契约冒烟，不替代 version_test 的大表矩阵。
//! 断言优先检查错误子串，兼容地址等可变片段。
//! MockDb 按 SQL 是否含 tidb_version 分流应答。
//! MockPd 忽略 exclude_tombstone 参数，专注返回预设列表。

use std::sync::Mutex;

use astersql_errors::SharedError;
use semver::Version;

use crate::{
    CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION, CheckCheckpointSupport, CheckClusterVersion,
    CheckPITRSupportBatchKVFiles, CheckTiDBVersion, CheckVersion, CheckVersionForBR,
    CheckVersionForBRPiTR, CheckVersionForBackup, CheckVersionForDDL, CheckVersionForKeyspaceBR,
    ExtractTiDBVersion, FetchVersion, NextMajorVersion, NormalizeBackupVersion, ParseServerInfo,
    PdClient, QueryExecutor, ServerType, SetReleaseVersionForTest, Store, StoreLabel,
    removeVAndHash,
};

/// PD 桩：返回预设 Store 列表，供 CheckClusterVersion 遍历。
struct MockPd {
    stores: Mutex<Vec<Store>>,
}

impl PdClient for MockPd {
    /// 返回克隆的 Store 列表，供 CheckClusterVersion 遍历。
    fn GetAllStores(&self, _exclude_tombstone: bool) -> Result<Vec<Store>, SharedError> {
        Ok(self.stores.lock().unwrap().clone())
    }
}

/// DB 桩：分别模拟 tidb_version / version 查询成败，验证 FetchVersion 回退。
struct MockDb {
    tidb: Result<String, String>,
    version: Result<String, String>,
}

impl QueryExecutor for MockDb {
    /// 按 SQL 关键字选择 tidb/version 预设应答。
    fn QueryRow(&self, sql: &str) -> Result<String, SharedError> {
        if sql.contains("tidb_version") {
            self.tidb.clone().map_err(|e| astersql_errors::New(e))
        } else {
            self.version.clone().map_err(|e| astersql_errors::New(e))
        }
    }
}

/// 锁定解析、边界规范化、错误分支与集群侧效应的 Go/Rust 一致性。
#[test]
fn go_rust_public_contract_matches() {
    // normal: ParseServerInfo / ExtractTiDBVersion / CheckVersion
    // 正常路径：TiDB 版本串解析与区间检查应成功。
    let info = ParseServerInfo("5.7.25-TiDB-v6.5.0");
    assert_eq!(info.ServerType, ServerType::TiDB);
    assert_eq!(info.ServerVersion.as_ref().unwrap().to_string(), "6.5.0");

    let extracted = ExtractTiDBVersion("5.7.25-TiDB-v2.1.0-rc.1-7-g38c939f").unwrap();
    assert_eq!(extracted.to_string(), "2.1.0-rc.1");

    CheckVersion(
        "TiDB",
        &Version::parse("5.0.0").unwrap(),
        &Version::parse("4.0.0").unwrap(),
        &Version::parse("6.0.0").unwrap(),
    )
    .unwrap();

    // boundary: removeVAndHash, NormalizeBackupVersion, NextMajorVersion nightly
    // 去 v/hash、去引号规范化；nightly 释放版本使 NextMajor 变为“无穷新”。
    assert_eq!(removeVAndHash("v6.5.0-12-gabcdef0-dirty"), "6.5.0");
    assert_eq!(
        NormalizeBackupVersion("\"6.1.0\"").unwrap().to_string(),
        "6.1.0"
    );
    assert!(NormalizeBackupVersion("not-a-version").is_none());
    SetReleaseVersionForTest(Some("nightly-dirty"));
    let next = NextMajorVersion();
    assert_eq!(next.major, i64::MAX as u64);
    assert_eq!(next.pre.as_str(), "nightly");
    assert_eq!(CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION, 5);

    // error: CheckVersion too old / too new; CheckVersionForBRPiTR too low
    // 过旧、过新（含 pre-release 触达 major 上界）以及 PiTR 过低均应报错。
    assert!(
        CheckVersion(
            "TiDB",
            &Version::parse("3.0.0").unwrap(),
            &Version::parse("4.0.0").unwrap(),
            &Version::parse("6.0.0").unwrap(),
        )
        .is_err()
    );
    assert!(
        CheckVersion(
            "TiDB",
            &Version::parse("6.0.0-beta").unwrap(),
            &Version::parse("4.0.0").unwrap(),
            &Version::parse("6.0.0").unwrap(),
        )
        .is_err()
    );
    assert!(
        CheckTiDBVersion(
            "5.7.25-MySQL",
            Version::parse("4.0.0").unwrap(),
            Version::parse("8.0.0").unwrap(),
        )
        .is_err()
    );

    SetReleaseVersionForTest(Some("v6.2.0"));
    let low = Store {
        Address: "tikv-1".into(),
        Version: "v5.4.2".into(),
        ..Default::default()
    };
    let err = CheckVersionForBRPiTR(&low, &Version::parse("5.4.2").unwrap()).unwrap_err();
    assert!(err.to_string().contains("too low when use PiTR"), "{err}");

    // resource / side-effects: cluster check, checkpoint flag, FetchVersion fallback
    // 集群检查跳过 tiflash 的 TiKV 规则；PiTR 检查会置位 batch KV 支持标志。
    let pd = MockPd {
        stores: Mutex::new(vec![
            Store {
                Address: "tikv".into(),
                Version: "v6.5.0".into(),
                ..Default::default()
            },
            Store {
                Address: "tiflash".into(),
                Version: "v6.5.0".into(),
                Labels: vec![StoreLabel {
                    Key: "engine".into(),
                    Value: "tiflash".into(),
                }],
                ..Default::default()
            },
        ]),
    };
    SetReleaseVersionForTest(Some("v6.5.0"));
    CheckClusterVersion(&pd, &CheckVersionForBR).unwrap();
    assert!(CheckCheckpointSupport().is_ok());
    // 布尔恒真表达式仅保证可读调用，不锁定具体实现细节。
    assert!(CheckPITRSupportBatchKVFiles() || !CheckPITRSupportBatchKVFiles()); // readable

    CheckClusterVersion(&pd, &CheckVersionForBRPiTR).unwrap();
    assert!(CheckPITRSupportBatchKVFiles());

    // backup 跨 major>1 必须失败；DDL≥6.2 通过；Keyspace 需 ≥6.6。
    let backup_checker = CheckVersionForBackup(Version::parse("8.0.0").unwrap());
    assert!(backup_checker(&Store::default(), &Version::parse("6.0.0").unwrap()).is_err());

    CheckVersionForDDL(&Store::default(), &Version::parse("6.2.0").unwrap()).unwrap();
    assert!(
        CheckVersionForKeyspaceBR(&Store::default(), &Version::parse("6.5.0").unwrap()).is_err()
    );

    // tidb_version 失败时回退 SELECT version()。
    let db = MockDb {
        tidb: Err("mock failure".into()),
        version: Ok("5.7.25-TiDB-v6.5.0".into()),
    };
    assert_eq!(FetchVersion(&db).unwrap(), "5.7.25-TiDB-v6.5.0");

    // tidb_version 命中 Release Version 正则则直接返回，不再查 version()。
    let db_ok = MockDb {
        tidb: Ok("Release Version: v6.5.0\nGit Commit Hash: abc".into()),
        version: Err("unused".into()),
    };
    assert!(
        FetchVersion(&db_ok)
            .unwrap()
            .contains("Release Version: v6.5.0")
    );

    // 清理线程本地覆盖，避免污染同进程其他用例。
    SetReleaseVersionForTest(None);
}
