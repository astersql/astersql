// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! version 包单元测试：对齐 Go `version_test.go` 的集群检查矩阵与解析用例。
//! 使用线程本地 ReleaseVersionGuard，避免 Rust 并行测试互相覆盖发布版本。
//! 表驱动用例覆盖 PiTR/BR/Backup/DDL 成功与失败路径，以及 FetchVersion 回退。
//! MockPdClient 断言 exclude_tombstone=true，锁定 CheckClusterVersion 调用约定。
//! 解析用例覆盖 removeVAndHash、ExtractTiDBVersion、NormalizeBackupVersion、Cloud 映射。
//! detect_server_info 通过 MockDb 期望队列验证 SQL 顺序与 Go 一致。
//! ensure_support_version 交叉检查 meta/model 常量，防止 TableInfo 版本漂移。
//! 错误断言使用正则而非全串相等，容忍地址等可变片段同时锁定关键措辞。
//! Cloud 用例验证 year-2000→major 映射及非法 TiDB-X-CLOUD 前缀回落 0.0.0。
//! 比较用例先 sanitize 再 semver cmp，验证 hash/dirty 剥离不影响序关系。
//! 所有修改发布版本的测试应持有 Guard 或在末尾显式清回，防止串扰。
//! 本文件不改生产逻辑，仅锁定契约；失败时优先对照 Go 同名测试。
//! CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION 与 model 常量交叉锁定防漂移。

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::Mutex;

use astersql_br_pkg_version_build as build;
use astersql_errors::SharedError;
use regex::Regex;
use semver::Version;

use crate::{
    CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION, CheckCheckpointSupport, CheckClusterVersion,
    CheckPITRSupportBatchKVFiles, CheckVersion, CheckVersionForBR, CheckVersionForBRPiTR,
    CheckVersionForBackup, CheckVersionForDDL, ExtractTiDBVersion, FetchVersion, NextMajorVersion,
    NormalizeBackupVersion, ParseServerInfo, PdClient, QueryExecutor, ServerType,
    SetPITRSupportBatchKVFilesForTest, SetReleaseVersionForTest, Store, StoreLabel, removeVAndHash,
};

/// RAII：构造时设置发布版本覆盖，Drop 时清回 None。
struct ReleaseVersionGuard;

impl ReleaseVersionGuard {
    /// 设置覆盖并返回守卫；作用域结束自动清理。
    fn set(version: &str) -> Self {
        SetReleaseVersionForTest(Some(version));
        Self
    }
}

impl Drop for ReleaseVersionGuard {
    /// 清除线程本地发布版本，避免泄漏到后续用例。
    fn drop(&mut self) {
        SetReleaseVersionForTest(None);
    }
}

/// PD 桩：返回固定 Store 列表，并断言排除 tombstone。
#[derive(Clone)]
struct MockPdClient {
    stores: Vec<Store>,
}

impl PdClient for MockPdClient {
    fn GetAllStores(&self, exclude_tombstone: bool) -> Result<Vec<Store>, SharedError> {
        // Go CheckClusterVersion 始终传 excludeTombstone=true。
        assert!(
            exclude_tombstone,
            "CheckClusterVersion must exclude tombstones"
        );
        Ok(self.stores.clone())
    }
}

/// 构造普通 TiKV Store 桩。
fn store(version: &str) -> Store {
    Store {
        Version: version.to_owned(),
        ..Store::default()
    }
}

/// 构造带 engine=tiflash 标签的 Store，走 TiFlash 检查分支。
fn tiflash(version: &str) -> Store {
    Store {
        Version: version.to_owned(),
        Labels: vec![StoreLabel {
            Key: "engine".to_owned(),
            Value: "tiflash".to_owned(),
        }],
        ..Store::default()
    }
}

/// 表驱动用例选用的检查策略枚举。
#[derive(Clone, Copy)]
enum Checker {
    /// 标准 PiTR 检查。
    PiTr,
    /// 先预设 batch KV 标志再跑 PiTR，验证标志被覆盖/保留。
    PiTrWithBatchKvFilesEnabled,
    /// 普通 BR 兼容检查。
    Br,
    /// 带备份版本的恢复检查。
    Backup(&'static str),
    /// DDL 表模式最低版本检查。
    Ddl,
}

/// 单条集群版本用例：发布版本、节点、策略与期望错误/标志。
struct ClusterVersionCase {
    /// BR 侧发布版本覆盖。
    release: &'static str,
    /// 集群 Store 集合。
    stores: Vec<Store>,
    /// 选用的检查器。
    checker: Checker,
    /// 期望错误正则；None 表示必须成功。
    error_pattern: Option<&'static str>,
    /// 可选：断言 PiTR batch KV 标志终态。
    batch_kv_files: Option<bool>,
    /// 可选：断言 checkpoint 是否支持。
    checkpoint_supported: Option<bool>,
}

/// 执行单条集群用例：设置发布版本、跑检查、断言错误与副作用标志。
fn run_cluster_case(case: &ClusterVersionCase) {
    SetReleaseVersionForTest(Some(case.release));
    let pd = MockPdClient {
        stores: case.stores.clone(),
    };
    // 按 Checker 枚举选择策略；Batch 变体先注入 true 再跑以观察覆盖。
    let result = match case.checker {
        Checker::PiTr => CheckClusterVersion(&pd, &CheckVersionForBRPiTR),
        Checker::PiTrWithBatchKvFilesEnabled => {
            SetPITRSupportBatchKVFilesForTest(true);
            CheckClusterVersion(&pd, &CheckVersionForBRPiTR)
        }
        Checker::Br => CheckClusterVersion(&pd, &CheckVersionForBR),
        Checker::Ddl => CheckClusterVersion(&pd, &CheckVersionForDDL),
        Checker::Backup(version) => {
            let checker = CheckVersionForBackup(Version::parse(version).unwrap());
            CheckClusterVersion(&pd, checker.as_ref())
        }
    };

    if let Some(pattern) = case.error_pattern {
        // 失败路径：错误文案须匹配 Go 侧正则。
        let error = result.expect_err("case must reject the cluster version");
        assert!(
            Regex::new(pattern).unwrap().is_match(&error.to_string()),
            "error {:?} does not match {pattern:?}",
            error.to_string()
        );
    } else {
        // 成功路径：不得返回错误。
        result.expect("case must accept the cluster version");
    }
    if let Some(expected) = case.batch_kv_files {
        // 校验 PiTR 副作用标志。
        assert_eq!(CheckPITRSupportBatchKVFiles(), expected);
    }
    if let Some(expected) = case.checkpoint_supported {
        // 校验 checkpoint 支持缓存。
        assert_eq!(CheckCheckpointSupport().is_ok(), expected);
    }
}

/// 大表驱动：覆盖 PiTR 过低/错配、TiFlash 不兼容、BR major、Backup 跨 major、DDL 门槛。
#[test]
fn test_check_cluster_version() {
    let _release = ReleaseVersionGuard::set("v6.5.0");
    // 用例开始前 batch KV 标志默认为 false。
    assert!(
        !CheckPITRSupportBatchKVFiles(),
        "PiTR batch-KV support must default to false"
    );

    // 下列用例顺序与字段对齐 Go TestCheckClusterVersion。
    let cases = [
        // PiTR：TiKV 5.4 过低。
        ClusterVersionCase {
            release: "v6.2.0",
            stores: vec![store("v5.4.2")],
            checker: Checker::PiTr,
            error_pattern: Some(r"^TiKV .* is too low when use PiTR, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // PiTR：TiKV 6.0 仍过低（需 ≥6.1）。
        ClusterVersionCase {
            release: "v6.2.0",
            stores: vec![store("v6.0.0")],
            checker: Checker::PiTr,
            error_pattern: Some(r"^TiKV .* is too low when use PiTR, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 6.2+ 配 TiKV 6.1 → 错配。
        ClusterVersionCase {
            release: "v6.2.0",
            stores: vec![store("v6.1.0")],
            checker: Checker::PiTr,
            error_pattern: Some(r"^TiKV .* version mismatch when use PiTR v6\.2\.0\+, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // TiKV 6.2 通过，但 <6.5 故 batch KV=false。
        ClusterVersionCase {
            release: "v6.2.0",
            stores: vec![store("v6.2.0")],
            checker: Checker::PiTr,
            error_pattern: None,
            batch_kv_files: Some(false),
            checkpoint_supported: None,
        },
        // 预设 batch=true 后跑 6.4，应被覆盖为 false。
        ClusterVersionCase {
            release: "v6.2.0",
            stores: vec![store("v6.4.0")],
            checker: Checker::PiTrWithBatchKvFilesEnabled,
            error_pattern: None,
            batch_kv_files: Some(false),
            checkpoint_supported: None,
        },
        // TiKV 6.5 → batch KV 支持为 true。
        ClusterVersionCase {
            release: "v6.2.0",
            stores: vec![store("v6.5.0")],
            checker: Checker::PiTrWithBatchKvFilesEnabled,
            error_pattern: None,
            batch_kv_files: Some(true),
            checkpoint_supported: None,
        },
        // BR 6.1 配 TiKV 5.4 → 过低。
        ClusterVersionCase {
            release: "v6.1.0",
            stores: vec![store("v5.4.2")],
            checker: Checker::PiTr,
            error_pattern: Some(r"^TiKV .* is too low when use PiTR, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 6.1 与 TiKV 6.1 对齐通过。
        ClusterVersionCase {
            release: "v6.1.0",
            stores: vec![store("v6.1.0")],
            checker: Checker::PiTr,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 6.1 配更高 TiKV 6.2 → 错配。
        ClusterVersionCase {
            release: "v6.1.0",
            stores: vec![store("v6.2.0")],
            checker: Checker::PiTr,
            error_pattern: Some(r"^TiKV .* version mismatch when use PiTR v6\.1\.0, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // TiFlash 4.0.0-rc.1 低于 4.0.0。
        ClusterVersionCase {
            release: "v4.0.5",
            stores: vec![tiflash("v4.0.0-rc.1")],
            checker: Checker::Br,
            error_pattern: Some(r"^incompatible.*version v4\.0\.0-rc\.1, try update it to 4\.0\.0"),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // TiFlash 3.1.0-beta.1 低于 3.1.0。
        ClusterVersionCase {
            release: "v3.0.14",
            stores: vec![tiflash("v3.1.0-beta.1")],
            checker: Checker::Br,
            error_pattern: Some(
                r"^incompatible.*version v3\.1\.0-beta\.1, try update it to 3\.1\.0",
            ),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // TiFlash 3.0.15 低于 3.1.0。
        ClusterVersionCase {
            release: "v3.1.1",
            stores: vec![tiflash("v3.0.15")],
            checker: Checker::Br,
            error_pattern: Some(r"^incompatible.*version v3\.0\.15, try update it to 3\.1\.0"),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // 最低支持 TiKV 版本边界通过。
        ClusterVersionCase {
            release: "v3.1.0-beta.2",
            stores: vec![store("3.1.0-beta.2")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // TiKV 2.1 不支持 BR。
        ClusterVersionCase {
            release: "v3.1.0-beta.2",
            stores: vec![store("v2.1.0")],
            checker: Checker::Br,
            error_pattern: Some(r"TiKV .* don't support BR, please upgrade cluster "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 3.1.0 配旧 3.1.0-beta.2 → 细粒度不兼容。
        ClusterVersionCase {
            release: "v3.1.0",
            stores: vec![store("3.1.0-beta.2")],
            checker: Checker::Br,
            error_pattern: Some(r"^TiKV .* mismatch, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR major 落后 TiKV → major mismatch。
        ClusterVersionCase {
            release: "v3.1.0",
            stores: vec![store("v4.0.0-rc")],
            checker: Checker::Br,
            error_pattern: Some(r"^TiKV .* major version mismatch, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR≥4.0.0-rc.1 配旧 4.0.0-beta.1 → 不兼容。
        ClusterVersionCase {
            release: "v4.0.0-rc.2",
            stores: vec![store("v4.0.0-beta.1")],
            checker: Checker::Br,
            error_pattern: Some(r"^TiKV .* mismatch, please "),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // 4.x 兼容通过，但 checkpoint 未达 6.5。
        ClusterVersionCase {
            release: "v4.0.0-rc.2",
            stores: vec![store("v4.0.0-rc.1")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: Some(false),
        },
        // 6.0 通过但 checkpoint 仍 false。
        ClusterVersionCase {
            release: "v6.0.0-rc.2",
            stores: vec![store("v6.0.0-rc.1")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: Some(false),
        },
        // 6.5 起 checkpoint 支持为 true。
        ClusterVersionCase {
            release: "v6.5.0-rc.2",
            stores: vec![store("v6.5.0-rc.1")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: Some(true),
        },
        // Backup：同 major 可恢复。
        ClusterVersionCase {
            release: "v4.0.12",
            stores: vec![store("v4.0.0-rc.1")],
            checker: Checker::Backup("4.0.12"),
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // Backup：major 差 1 仍允许。
        ClusterVersionCase {
            release: "v5.0.0-rc",
            stores: vec![store("v4.0.0-rc.1")],
            checker: Checker::Backup("5.0.0-rc"),
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // Backup：major 差 >1 拒绝。
        ClusterVersionCase {
            release: "v6.0.0",
            stores: vec![store("v4.0.0-rc.1")],
            checker: Checker::Backup("6.0.0"),
            error_pattern: Some("major version mismatches"),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 略旧于 TiKV 同 major 可通过。
        ClusterVersionCase {
            release: "v4.0.0-rc.1",
            stores: vec![store("v4.0.0-rc.2")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 领先一个 major 可通过。
        ClusterVersionCase {
            release: "v6.0.0",
            stores: vec![store("v5.4.0")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // 8.x 邻近 major 兼容。
        ClusterVersionCase {
            release: "v8.2.0",
            stores: vec![store("v8.1.0")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR 略旧于同系列 TiKV 可通过。
        ClusterVersionCase {
            release: "v8.1.0",
            stores: vec![store("v8.2.0")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // BR major 领先超过 2 → mismatch。
        ClusterVersionCase {
            release: "v26.3.0",
            stores: vec![store("v8.5.4")],
            checker: Checker::Br,
            error_pattern: Some("major version mismatch"),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // 测试回落版本短路所有真实检查。
        ClusterVersionCase {
            release: build::ReleaseVersionForTest,
            stores: vec![store("v8.5.4")],
            checker: Checker::Br,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // DDL：6.4 ≥ 6.2 通过。
        ClusterVersionCase {
            release: "v8.1.0",
            stores: vec![store("v6.4.0")],
            checker: Checker::Ddl,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // DDL：恰在 6.2.0 边界通过。
        ClusterVersionCase {
            release: "v8.1.0",
            stores: vec![store("v6.2.0")],
            checker: Checker::Ddl,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // DDL：6.2.0-alpha 满足 require。
        ClusterVersionCase {
            release: "v8.1.0",
            stores: vec![store("v6.2.0-alpha")],
            checker: Checker::Ddl,
            error_pattern: None,
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // DDL：6.1 过旧。
        ClusterVersionCase {
            release: "v8.1.0",
            stores: vec![store("v6.1.0")],
            checker: Checker::Ddl,
            error_pattern: Some("detected the old version"),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
        // DDL：5.4 过旧。
        ClusterVersionCase {
            release: "v8.1.0",
            stores: vec![store("v5.4.0")],
            checker: Checker::Ddl,
            error_pattern: Some("detected the old version"),
            batch_kv_files: None,
            checkpoint_supported: None,
        },
    ];

    // 逐条执行，任何一条失败即暴露 Go/Rust 分歧。
    for case in &cases {
        run_cluster_case(case);
    }
}

/// TiFlash 错误应使用 protobuf `GetPeerAddress()`，不能回落到客户端地址。
#[test]
fn test_tiflash_error_uses_peer_address() {
    let pd = MockPdClient {
        stores: vec![Store {
            Address: "client-address".to_owned(),
            PeerAddress: "peer-address".to_owned(),
            Version: "invalid".to_owned(),
            Labels: vec![StoreLabel {
                Key: "engine".to_owned(),
                Value: "tiflash".to_owned(),
            }],
            ..Store::default()
        }],
    };
    let error = CheckClusterVersion(&pd, &CheckVersionForBR).unwrap_err();
    assert!(error.to_string().contains("peer-address"), "{error}");
}

/// 验证 removeVAndHash 后 semver 比较序与 Go 用例一致。
#[test]
fn test_compare_version() {
    let cases = [
        // rc < rc.2
        (Ordering::Less, "4.0.0-rc", "4.0.0-rc.2"),
        // beta < rc
        (Ordering::Less, "4.0.0-beta.3", "4.0.0-rc.2"),
        // rc.1 < 正式版
        (Ordering::Less, "4.0.0-rc.1", "4.0.0"),
        // beta < 正式版
        (Ordering::Less, "4.0.0-beta.1", "4.0.0"),
        // 去 hash 后 rc < rc.2
        (Ordering::Less, "4.0.0-rc-35-g31dae220", "4.0.0-rc.2"),
        // 去 hash 后正式构建 > rc.1
        (Ordering::Greater, "4.0.0-9-g30f0b014", "4.0.0-rc.1"),
        // dirty+hash 剥离后等于 beta
        (
            Ordering::Equal,
            "v3.0.0-beta-211-g09beefbe0-dirty",
            "3.0.0-beta",
        ),
        // 仅 dirty 后缀
        (Ordering::Equal, "v3.0.5-dirty", "3.0.5"),
        // beta.12 + dirty
        (Ordering::Equal, "v3.0.5-beta.12-dirty", "3.0.5-beta.12"),
        // rc.1 + hash + dirty
        (
            Ordering::Equal,
            "v2.1.0-rc.1-7-g38c939f-dirty",
            "2.1.0-rc.1",
        ),
    ];
    // 左侧先 sanitize，右侧已是干净 semver。
    for (expected, left, right) in cases {
        let left = removeVAndHash(left);
        // sanitize 后的序关系必须与期望 Ordering 一致。
        assert_eq!(
            Version::parse(&left)
                .unwrap()
                .cmp(&Version::parse(right).unwrap()),
            expected,
            "{left} compared with {right}"
        );
    }
}

/// NextMajorVersion：正常 +1 major；不可解析则 nightly。
#[test]
fn test_next_major_version() {
    let _release = ReleaseVersionGuard::set("v4.0.0-rc.1");
    // 含 rc/hash/master 后缀的发布串均应进位到下一 major。
    for (release, expected) in [
        ("v4.0.0-rc.1", "5.0.0"),
        // 含 hash 的 rc 串
        ("4.0.0-rc-35-g31dae220", "5.0.0"),
        // 正式构建 + hash
        ("4.0.0-9-g30f0b014", "5.0.0"),
        // 5→6
        ("v5.0.0-rc.2", "6.0.0"),
        // master 后缀仍可 parse 进位
        ("v5.0.0-master", "6.0.0"),
    ] {
        SetReleaseVersionForTest(Some(release));
        assert_eq!(NextMajorVersion().to_string(), expected);
    }

    // 脏 commit 串不可 parse → 无穷新 nightly。
    SetReleaseVersionForTest(Some("b7ed87d-dirty"));
    assert_eq!(NextMajorVersion().pre.as_str(), "nightly");
}

/// ExtractTiDBVersion：合法形态提取与非法形态错误前缀。
#[test]
fn test_extract_tidb_version() {
    // 覆盖有/无 hash、有/无 dirty、带 beta/rc 的提取结果。
    for (input, expected) in [
        ("5.7.10-TiDB-v2.1.0-rc.1-7-g38c939f", "2.1.0-rc.1"),
        // 有 hash 取核心版本
        ("5.7.10-TiDB-v2.0.4-1-g06a0bf5", "2.0.4"),
        // 无 hash 直接取
        ("5.7.10-TiDB-v2.0.7", "2.0.7"),
        // 保留 beta.12
        ("8.0.12-TiDB-v3.0.5-beta.12", "3.0.5-beta.12"),
        // dirty+hash → beta
        ("5.7.25-TiDB-v3.0.0-beta-211-g09beefbe0-dirty", "3.0.0-beta"),
        // 仅 dirty
        ("8.0.12-TiDB-v3.0.5-dirty", "3.0.5"),
        // beta + dirty
        ("8.0.12-TiDB-v3.0.5-beta.12-dirty", "3.0.5-beta.12"),
        // rc + hash + dirty
        ("5.7.10-TiDB-v2.1.0-rc.1-7-g38c939f-dirty", "2.1.0-rc.1"),
    ] {
        assert_eq!(
            ExtractTiDBVersion(input).unwrap(),
            Version::parse(expected).unwrap()
        );
    }

    // 空串与纯 MySQL 版本非法；乱序串亦应失败。
    for input in ["", "8.0.12"] {
        let error = ExtractTiDBVersion(input).unwrap_err();
        assert!(
            error.to_string().starts_with("not a valid TiDB version"),
            "{error}"
        );
    }
    assert!(ExtractTiDBVersion("not-a-valid-version").is_err());
}

/// CheckVersion 半开区间：过旧/过新（含 pre）错误前缀对齐 Go。
#[test]
fn test_check_version() {
    let cases = [
        // 区间内成功
        ("2.3.5", "2.1.0", "3.0.0", None),
        // 过旧
        ("2.1.0", "2.3.5", "3.0.0", Some("TiNB version too old")),
        // major 触达上界
        ("3.1.0", "2.3.5", "3.0.0", Some("TiNB version too new")),
        // pre-release 亦算过新
        ("3.0.0-beta", "2.3.5", "3.0.0", Some("TiNB version too new")),
    ];
    // 组件名 TiNB 仅作占位，锁定错误前缀文案。
    for (actual, min, max, expected_error) in cases {
        let result = CheckVersion(
            "TiNB",
            &Version::parse(actual).unwrap(),
            &Version::parse(min).unwrap(),
            &Version::parse(max).unwrap(),
        );
        match expected_error {
            Some(prefix) => {
                // 仅校验错误前缀，与 Go strings.HasPrefix 一致。
                assert!(
                    result.unwrap_err().to_string().starts_with(prefix),
                    "{actual}"
                )
            }
            None => result.unwrap(),
        }
    }
}

/// NormalizeBackupVersion：去引号换行；空串返回 None。
#[test]
fn test_normalize_backup_version() {
    // 含引号/换行的 backupmeta 版本串规范化。
    for (expected, input) in [
        (Some("4.0.0"), r#""4.0.0\n""#),
        // 引号+换行内的 rc.x
        (Some("5.0.0-rc.x"), r#""5.0.0-rc.x\n""#),
        // 无引号直通
        (Some("5.0.0-rc.x"), "5.0.0-rc.x"),
        // 引号后多余换行
        (Some("4.0.12"), "\"4.0.12\"\n"),
        // 空串失败
        (None, ""),
    ] {
        assert_eq!(
            NormalizeBackupVersion(input),
            expected.map(|version| Version::parse(version).unwrap()),
            "input={input:?}"
        );
    }
}

/// PD HTTP API 返回 JSON 引号串时，Go `strconv.Unquote` 会还原转义字符。
#[test]
fn test_normalize_backup_version_unquotes_escaped_newline() {
    assert_eq!(
        NormalizeBackupVersion(r#""4.0.0\n""#),
        Some(Version::parse("4.0.0").unwrap())
    );
}

/// MockDb 单次查询应答：成功值或错误。
#[derive(Clone)]
enum QueryReply {
    Value(&'static str),
    Error(&'static str),
}

/// 期望的 SQL 与应答；顺序必须与 Go 一致。
struct QueryExpectation {
    sql: &'static str,
    reply: QueryReply,
}

/// 按队列弹出期望的查询桩；多余/缺失调用都会失败。
struct MockDb {
    expectations: Mutex<VecDeque<QueryExpectation>>,
}

impl MockDb {
    fn new(expectations: impl IntoIterator<Item = QueryExpectation>) -> Self {
        Self {
            expectations: Mutex::new(expectations.into_iter().collect()),
        }
    }

    /// 断言所有期望 SQL 均已消费。
    fn assert_exhausted(&self) {
        let remaining = self.expectations.lock().unwrap();
        assert!(
            remaining.is_empty(),
            "{} expected SQL calls were not made",
            remaining.len()
        );
    }
}

impl QueryExecutor for MockDb {
    fn QueryRow(&self, sql: &str) -> Result<String, SharedError> {
        let expectation = self
            .expectations
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected SQL query");
        // SQL 文本与顺序必须与 Go 测试期望完全一致。
        assert_eq!(sql, expectation.sql, "SQL query order differs from Go");
        match expectation.reply {
            QueryReply::Value(value) => Ok(value.to_owned()),
            QueryReply::Error(error) => Err(astersql_errors::New(error)),
        }
    }
}

/// detect_server_info 单条：tag 对应 Go 用例编号便于对照。
struct DetectServerInfoCase {
    tag: u8,
    version: &'static str,
    expected_type: ServerType,
    expected_version: &'static str,
}

/// MySQL/MariaDB/Unknown/无版本 TiDB 等公共检测用例。
fn common_detect_server_info_cases() -> Vec<DetectServerInfoCase> {
    vec![
        // tag1：纯 MySQL。
        DetectServerInfoCase {
            tag: 1,
            version: "8.0.18",
            expected_type: ServerType::MySQL,
            expected_version: "8.0.18",
        },
        // tag2：MariaDB；版本截断对齐 Go。
        DetectServerInfoCase {
            tag: 2,
            version: "10.4.10-MariaDB-1:10.4.10+maria~bionic",
            expected_type: ServerType::MariaDB,
            expected_version: "10.4.10-MariaDB-1",
        },
        // tag6：无法识别 → Unknown/0.0.0。
        DetectServerInfoCase {
            tag: 6,
            version: "invalid version",
            expected_type: ServerType::Unknown,
            expected_version: "0.0.0",
        },
        // tag9：TiDB 但无合法 semver → 0.0.0。
        DetectServerInfoCase {
            tag: 9,
            version: "5.7.25-TiDB-5584f12",
            expected_type: ServerType::TiDB,
            expected_version: "0.0.0",
        },
    ]
}

/// TiDB 常规、Release Version、Cloud 年月映射及非法 X-CLOUD 前缀用例。
fn tidb_detect_server_info_cases() -> Vec<DetectServerInfoCase> {
    vec![
        // tag3：带 alpha+hash 的 TiDB version()。
        DetectServerInfoCase {
            tag: 3,
            version: "5.7.25-TiDB-v4.0.0-alpha-1263-g635f2e1af",
            expected_type: ServerType::TiDB,
            expected_version: "4.0.0-alpha-1263-g635f2e1af",
        },
        // tag4：3.0.7 + 提交计数/hash。
        DetectServerInfoCase {
            tag: 4,
            version: "5.7.25-TiDB-v3.0.7-58-g6adce2367",
            expected_type: ServerType::TiDB,
            expected_version: "3.0.7-58-g6adce2367",
        },
        // tag5：无 v 前缀的 TiDB 段。
        DetectServerInfoCase {
            tag: 5,
            version: "5.7.25-TiDB-3.0.6",
            expected_type: ServerType::TiDB,
            expected_version: "3.0.6",
        },
        // tag7：tidb_version 多行 Release Version。
        DetectServerInfoCase {
            tag: 7,
            version: "Release Version: v5.2.1\nEdition: Community\nGit Commit Hash: cd8fb24c5f7ebd9d479ed228bb41848bd5e97445",
            expected_type: ServerType::TiDB,
            expected_version: "5.2.1",
        },
        // tag8：带 alpha/hash 的 Release Version。
        DetectServerInfoCase {
            tag: 8,
            version: "Release Version: v5.4.0-alpha-21-g86caab907\nEdition: Community\nGit Commit Hash: 86caab907c481bbc4243b5a3346ec13907cc8721\nGit Branch: master",
            expected_type: ServerType::TiDB,
            expected_version: "5.4.0-alpha-21-g86caab907",
        },
        // tag10：CLOUD.202603.0 → 26.3.0。
        DetectServerInfoCase {
            tag: 10,
            version: "8.0.11-TiDB-CLOUD.202603.0",
            expected_type: ServerType::TiDB,
            expected_version: "26.3.0",
        },
        // tag11：CLOUD 带 dirty 后缀保留。
        DetectServerInfoCase {
            tag: 11,
            version: "8.0.11-TiDB-CLOUD.202603.3-1c7827b003-dirty",
            expected_type: ServerType::TiDB,
            expected_version: "26.3.3-1c7827b003-dirty",
        },
        // tag12：Release 行 CLOUD 映射。
        DetectServerInfoCase {
            tag: 12,
            version: "Release Version: CLOUD.202603.2\nEdition: Community",
            expected_type: ServerType::TiDB,
            expected_version: "26.3.2",
        },
        // tag12 变体：CLOUD dirty。
        DetectServerInfoCase {
            tag: 12,
            version: "Release Version: CLOUD.202603.5-1c7827b003-dirty\nEdition: Community",
            expected_type: ServerType::TiDB,
            expected_version: "26.3.5-1c7827b003-dirty",
        },
        // tag13：非法 X-CLOUD 前缀不映射，回落 0.0.0。
        DetectServerInfoCase {
            tag: 13,
            version: "8.0.11-TiDB-X-CLOUD.202603.0",
            expected_type: ServerType::TiDB,
            expected_version: "0.0.0",
        },
    ]
}

/// 通过 FetchVersion+ParseServerInfo 锁定类型与版本，并校验 SQL 回退顺序。
#[test]
fn test_detect_server_info() {
    let cases = common_detect_server_info_cases()
        .into_iter()
        .chain(tidb_detect_server_info_cases());
    for case in cases {
        let is_release = case.version.starts_with("Release Version:");
        // Release Version 输出走 tidb_version 直通；否则先失败再回退 version()。
        let expectations = if is_release {
            vec![QueryExpectation {
                sql: "SELECT tidb_version();",
                reply: QueryReply::Value(case.version),
            }]
        } else {
            vec![
                QueryExpectation {
                    sql: "SELECT tidb_version();",
                    reply: QueryReply::Error("mock error"),
                },
                QueryExpectation {
                    sql: "SELECT version();",
                    reply: QueryReply::Value(case.version),
                },
            ]
        };
        let db = MockDb::new(expectations);
        let fetched = FetchVersion(&db)
            .unwrap_or_else(|error| panic!("case {} failed to fetch version: {error}", case.tag));
        let info = ParseServerInfo(&fetched);
        // 类型与版本均须与 Go 用例 tag 对齐。
        assert_eq!(info.ServerType, case.expected_type, "case {}", case.tag);
        assert_eq!(
            info.ServerVersion.unwrap(),
            Version::parse(case.expected_version).unwrap(),
            "case {}",
            case.tag
        );
        // 期望队列必须耗尽，防止漏调用或多余查询。
        db.assert_exhausted();
    }
}

/// 规范 tidb_version 多行输出样例，含 Release Version 行。
const TIDB_VERSION: &str = "Release Version: v5.2.1\nEdition: Community\nGit Commit Hash: cd8fb24c5f7ebd9d479ed228bb41848bd5e97445\nGit Branch: heads/refs/tags/v5.2.1\nUTC Build Time: 2021-09-08 02:32:56\nGoVersion: go1.16.4\nRace Enabled: false\nTiKV Min Version: v3.0.0-60965b006877ca7234adaced7890d7b029ed1306\nCheck Table Before Drop: false";

/// FetchVersion：直通 / 回退成功 / 双失败错误路径。
#[test]
fn test_fetch_version() {
    let db = MockDb::new([
        // 调用1：直通
        QueryExpectation {
            sql: "SELECT tidb_version();",
            reply: QueryReply::Value(TIDB_VERSION),
        },
        // 调用2：tidb 失败
        QueryExpectation {
            sql: "SELECT tidb_version();",
            reply: QueryReply::Error("mock failure"),
        },
        // 调用2 回退成功
        QueryExpectation {
            sql: "SELECT version();",
            reply: QueryReply::Value("5.7.25"),
        },
        // 调用3：双失败
        QueryExpectation {
            sql: "SELECT tidb_version();",
            reply: QueryReply::Error("mock failure"),
        },
        QueryExpectation {
            sql: "SELECT version();",
            reply: QueryReply::Error("mock failure"),
        },
    ]);

    // 1) 直通 tidb_version 2) 回退 version 3) 双失败。
    assert_eq!(FetchVersion(&db).unwrap(), TIDB_VERSION);
    assert_eq!(FetchVersion(&db).unwrap(), "5.7.25");
    let error = FetchVersion(&db).unwrap_err();
    assert!(error.to_string().ends_with("mock failure"), "{error}");
    db.assert_exhausted();
}

/// Release Version 为 commit-id 时不匹配正则，应回退 version()。
#[test]
fn test_fetch_version_with_commit_id() {
    const COMMIT_RELEASE: &str = "Release Version: bcf61918\nGit Commit Hash: cd8fb24c5f7ebd9d479ed228bb41848bd5e97445\nGit Branch: heads/refs/tags/v5.2.1\nUTC Build Time: 2021-09-08 02:32:56\nGoVersion: go1.16.4\nRace Enabled: false\nTiKV Min Version: v3.0.0-60965b006877ca7234adaced7890d7b029ed1306\nCheck Table Before Drop: false";
    let db = MockDb::new([
        // 先成功直通
        QueryExpectation {
            sql: "SELECT tidb_version();",
            reply: QueryReply::Value(TIDB_VERSION),
        },
        // commit-id Release 不匹配正则
        QueryExpectation {
            sql: "SELECT tidb_version();",
            reply: QueryReply::Value(COMMIT_RELEASE),
        },
        // 因而回退 version()
        QueryExpectation {
            sql: "SELECT version();",
            reply: QueryReply::Value("5.7.25"),
        },
    ]);

    // 第二次 Release 为 commit-id，应跳过并回退到 version()。
    assert_eq!(FetchVersion(&db).unwrap(), TIDB_VERSION);
    assert_eq!(FetchVersion(&db).unwrap(), "5.7.25");
    db.assert_exhausted();
}

/// 交叉锁定 BR 支持版本与 meta/model TableInfoVersion5 常量。
#[test]
fn test_ensure_support_version() {
    // include 源码文本，防止模型版本升级时 BR 未同步审查。
    const MODEL_TABLE_SOURCE: &str = include_str!("../../../pkg/meta/model/table.rs");
    assert_eq!(CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION, 5);
    assert!(
        MODEL_TABLE_SOURCE
            .contains("pub const CurrLatestTableInfoVersion: u16 = TableInfoVersion5;"),
        "model latest table-info alias changed; review BR compatibility"
    );
    assert!(
        MODEL_TABLE_SOURCE.contains("pub const TableInfoVersion5: u16 = 5;"),
        "model table-info version 5 changed; review BR compatibility"
    );
}
