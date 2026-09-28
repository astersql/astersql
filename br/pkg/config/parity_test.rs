// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Parity checks proving the Go public contract of `br/pkg/config`
//! (ebs.go/kv.go) is reflected by the Rust port.
//!
//! 契约测试：锁定 EBS 元数据与 TiKV 配置解析的公开行为与 Go 一致。
//! 覆盖卷类型合法性、fixture 加载、setter 回填、checkEBSBRMeta 错误顺序，
//! 以及 import/merge/log-backup 解析与 RAMInBytes 单位换算。
//! 本文件只断言公开契约，不模拟外部存储；`NewMetaFromStorage` 仅作签名锚定。

use std::collections::HashMap;

use crate::ebs::{EBSBasedBRMeta, EBSVolumeType_Valid, NewMetaFromStorage};
use crate::kv::{
    ParseImportThreadsFromConfig, ParseLogBackupEnableFromConfig, ParseMergeRegionSizeFromConfig,
    units,
};

#[test]
fn go_rust_public_contract_matches() {
    // EBSVolumeType.Valid parity.
    // 仅 gp3/io1/io2 合法；gp2 等旧类型必须拒绝。
    // gp3 / io1 / io2 分别对应通用与预置 IOPS 类型。
    assert!(EBSVolumeType_Valid("gp3"));
    assert!(EBSVolumeType_Valid("io1"));
    assert!(EBSVolumeType_Valid("io2"));
    // gp2 不在支持集合内。
    assert!(!EBSVolumeType_Valid("gp2"));

    // TestParseConfig parity: load the checked-in ebs_backup.json fixture.
    // 使用 crate 清单目录下固定 fixture，断言 store/卷数与 resolved-ts。
    let mut config = EBSBasedBRMeta::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ebs_backup.json");
    config.ConfigFromFile(path).expect("config from file");
    // fixture 期望：3 store、每 store 2 卷、固定 resolved-ts。
    assert_eq!(config.GetStoreCount(), 3);
    assert_eq!(config.GetTiKVVolumeCount(), 2);
    assert_eq!(config.GetResolvedTS(), 456745777823347);
    // 完整元数据须通过 checkEBSBRMeta。
    config.checkEBSBRMeta().expect("valid meta");

    // Missing file surfaces an error like Go errors.Trace(os error).
    // 不存在路径须返回错误，不能静默成功。
    let mut broken = EBSBasedBRMeta::default();
    assert!(
        broken
            .ConfigFromFile("/nonexistent/ebs_backup.json")
            .is_err()
    );

    // Setters keep Go behavior: rewriting ids across all stores/volumes.
    // 三个 setter 均按 volume.ID 查表回填；缺键在 Rust 侧写空串。
    let ids: Vec<String> = config
        .TiKVComponent
        .as_ref()
        .expect("tikv")
        .Stores
        .iter()
        .flat_map(|store| store.Volumes.iter().map(|volume| volume.ID.clone()))
        .collect();
    let snapshot_map: HashMap<String, String> = ids
        .iter()
        .map(|id| (id.clone(), format!("snap-of-{id}")))
        .collect();
    // 依次回填 SnapshotID / RestoreVolumeId / VolumeAZ。
    config.SetSnapshotIDs(&snapshot_map);
    config.SetRestoreVolumeIDs(&snapshot_map);
    config.SetVolumeAZs(&snapshot_map);
    for store in &config.TiKVComponent.as_ref().expect("tikv").Stores {
        for volume in &store.Volumes {
            assert_eq!(volume.SnapshotID, format!("snap-of-{}", volume.ID));
            assert_eq!(volume.RestoreVolumeId, format!("snap-of-{}", volume.ID));
            assert_eq!(volume.VolumeAZ, format!("snap-of-{}", volume.ID));
        }
    }

    // CheckClusterInfo lazily initializes; SetResolvedTS works on empty meta.
    // 空元数据上 setter 应先惰性建 ClusterInfo，再读写成功。
    let mut empty = EBSBasedBRMeta::default();
    empty.SetResolvedTS(42);
    assert_eq!(empty.GetResolvedTS(), 42);
    empty.SetFullBackupType("aws-ebs".to_string());
    assert_eq!(empty.GetFullBackupType(), "aws-ebs");
    // 带 v 前缀的版本字符串在校验路径可被规范化。
    empty.SetClusterVersion("v7.5.0".to_string());

    // Masterminds/semver NewVersion accepts partial versions and numeric
    // components with leading zeroes; the Rust validator must do the same.
    let mut loose_versions = config.clone();
    for version in ["7.5", "7", "01.2.3"] {
        loose_versions.SetClusterVersion(version.to_string());
        loose_versions
            .checkEBSBRMeta()
            .unwrap_or_else(|error| panic!("Go accepts version {version}: {error}"));
    }

    // checkEBSBRMeta error paths in Go order.
    // 错误文案与顺序：无集群 → 坏版本 → 零 ts → 无 TiKV。
    let no_cluster = EBSBasedBRMeta::default();
    assert!(
        no_cluster
            .checkEBSBRMeta()
            .expect_err("no cluster info")
            .to_string()
            .contains("no cluster info")
    );
    let mut bad_version = EBSBasedBRMeta::default();
    bad_version.SetClusterVersion("not-a-version".to_string());
    assert!(
        bad_version
            .checkEBSBRMeta()
            .expect_err("invalid version")
            .to_string()
            .contains("invalid cluster version")
    );
    let mut zero_ts = EBSBasedBRMeta::default();
    zero_ts.SetClusterVersion("v6.1.0".to_string());
    assert!(
        zero_ts
            .checkEBSBRMeta()
            .expect_err("zero ts")
            .to_string()
            .contains("invalid resolved ts")
    );
    let mut no_tikv = EBSBasedBRMeta::default();
    no_tikv.SetClusterVersion("v6.1.0".to_string());
    no_tikv.SetResolvedTS(1);
    assert!(
        no_tikv
            .checkEBSBRMeta()
            .expect_err("no tikv")
            .to_string()
            .contains("tikv info is empty")
    );

    // String() marshals to JSON.
    // 合法元数据的 String 须含 cluster_info 键。
    assert!(config.String().contains("cluster_info"));
}

#[test]
fn config_from_file_preserves_fields_absent_from_json_like_go_unmarshal() {
    let path = std::env::temp_dir().join(format!(
        "astersql-br-config-{}-{}.json",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::write(&path, br#"{"region":"new-region"}"#).expect("write config fixture");

    let mut config = EBSBasedBRMeta::default();
    config.SetResolvedTS(42);
    config
        .ConfigFromFile(path.to_str().expect("utf-8 temp path"))
        .expect("load partial config");
    std::fs::remove_file(path).expect("remove config fixture");

    assert_eq!(config.Region, "new-region");
    assert_eq!(config.GetResolvedTS(), 42);
}

#[test]
fn kv_config_parsers_match_go() {
    // ParseImportThreadsFromConfig.
    // 正常值 / 缺省 0 / 非法 JSON 报错。
    // 显式 num-threads=8。
    assert_eq!(
        ParseImportThreadsFromConfig(br#"{"import":{"num-threads":8}}"#).expect("threads"),
        8
    );
    // 空对象缺省为 0。
    assert_eq!(
        ParseImportThreadsFromConfig(br#"{}"#).expect("default zero"),
        0
    );
    assert_eq!(
        ParseImportThreadsFromConfig(br#"{"import":null}"#).expect("null import is zero"),
        0
    );
    assert_eq!(
        ParseImportThreadsFromConfig(br#"{"import":{"num-threads":4294967296}}"#)
            .expect("Go uint is 64-bit on supported targets"),
        4_294_967_296
    );
    // 非法 JSON 必须失败。
    assert!(ParseImportThreadsFromConfig(b"not json").is_err());

    // ParseMergeRegionSizeFromConfig with human-readable size (like Go go-units).
    // 96MiB → 96*1024^2；错误后缀与非法 JSON 均失败。
    let (size, keys) = ParseMergeRegionSizeFromConfig(
        br#"{"coprocessor":{"region-split-size":"96MiB","region-split-keys":960000}}"#,
    )
    .expect("merge region size");
    assert_eq!(size, 96 * 1024 * 1024);
    assert_eq!(keys, 960000);
    assert!(ParseMergeRegionSizeFromConfig(b"not json").is_err());
    assert!(
        ParseMergeRegionSizeFromConfig(
            br#"{"coprocessor":{"region-split-size":"96XB","region-split-keys":1}}"#
        )
        .is_err()
    );

    // RAMInBytes parity with docker/go-units binary multiples.
    // 纯数字、KB、小数 GiB 均按 1024 进制。
    assert_eq!(units::RAMInBytes("32").expect("plain"), 32);
    assert_eq!(units::RAMInBytes("1KB").expect("kb"), 1024);
    assert_eq!(units::RAMInBytes("1.5GiB").expect("gib"), 1610612736);
    assert_eq!(units::RAMInBytes("32b").expect("byte suffix"), 32);
    assert_eq!(units::RAMInBytes("32 B").expect("spaced suffix"), 32);
    assert_eq!(units::RAMInBytes("32.3").expect("fractional bytes"), 32);
    assert_eq!(units::RAMInBytes("1e3MB").expect("exponent"), 1_048_576_000);
    assert!(units::RAMInBytes("-32").is_err());
    assert!(units::RAMInBytes(" 32 ").is_err());

    // ParseLogBackupEnableFromConfig.
    // enable=true；缺省 false；非法 JSON 报错。
    assert!(ParseLogBackupEnableFromConfig(br#"{"log-backup":{"enable":true}}"#).expect("enabled"));
    assert!(!ParseLogBackupEnableFromConfig(br#"{}"#).expect("default false"));
    assert!(
        !ParseLogBackupEnableFromConfig(br#"{"log-backup":null}"#)
            .expect("null log-backup is false")
    );
    assert!(ParseLogBackupEnableFromConfig(b"not json").is_err());

    // NewMetaFromStorage is exercised via the storage trait; reference it so
    // signature drift breaks this parity test at compile time.
    // 编译期锚定公开签名，防止重构时静默漂移。
    let _ = NewMetaFromStorage;
}
