// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/task` public contracts vs Go sources.
//!
//! br/pkg/task 层公开 API 与 Go 的 parity 契约测试。
//! 核对 flag 常量、默认配置、估算空间、DDL 过滤规则等。
//! 不启动真实恢复；用构造数据断言纯函数与配置解析。
//! 样例数据尽量贴近 Go 单测构造，避免魔法数无对照。
//! 配置默认值变更时需同步更新本文件期望。
//! 空间估算公式与副本数边界（0 副本）需特别覆盖。

use std::sync::Arc;

use url::Url;

use crate::backup::{
    BackupConfig, FullBackupCmd, ParseTSString, RunBackupWithDefaults, isFullBackup,
    parseCompressionType, parseReplicaReadLabelFlag,
};
use crate::backup_ebs::{RunBackupEBS, isRegionsHasHole};
use crate::backup_raw::{RawKvConfig, RunBackupRawWithDefaults};
use crate::backup_txn::{RunBackupTxnWithDefaults, TxnKvConfig};
use crate::common::{
    Config, FlagPiTRAddIndexSQLStorage, FlagStreamFullBackupStorage, FullBackupType,
    FullBackupTypeEBS, FullBackupTypeKV, GetCipherKeyContent, NewMgr, TLSConfig, checkCipherKey,
    checkCipherKeyMatch, flagFullBackupCipherKey, flagLogBackupCipherKey, flagMasterKeyConfig,
    flagSendCreds, flagStorage, flagToZapField, normalizePDURL, parseCipherType,
};
use crate::encryption::{
    parseAwsKmsConfig, parseAzureKmsConfig, parseGcpKmsConfig, parseLocalDiskConfig,
    validateAndParseMasterKeyString,
};
use crate::restore::{
    ActionAddIndex, ActionLockTable, BinlogInfo, CIStr, CheckDDLJobByRules, CheckStoreSpace,
    DBInfo, DDLJobBlockListRule, EstimateTikvUsage, FilterDDLJobByRules, FilterDDLJobs,
    FullRestoreCmd, IsStreamRestore, Job, PointRestoreCmd, RestoreConfig, RunRestore, Table,
    TableInfo, encodeCompactAndCheckKey, isFullRestore, rewriteKeyRanges,
};
use crate::restore_data::{ReadBackupMetaData, RunResolveKvData};
use crate::restore_ebs_meta::RunRestoreEBSMetaWithDefaults;
use crate::restore_raw::{RunRestoreRaw, getEndKeys};
use crate::restore_txn::RunRestoreTxn;
use crate::stream::{
    RunStreamTruncate, ShiftTS, StreamConfig, buildKeyRangesFromSchemasReplace,
    buildPauseSafePointName, buildRewriteRules, checkLogRange, getCheckpointFromResumeState,
    getGlobalCheckpointFromStorage, isCurrentIdMapSaved, isValidSnapshotRange, verifyContiguousIDs,
};
use crate::stubs::backuppb::{CipherInfo, CompressionType};
use crate::stubs::encryptionpb::{EncryptionMethod, MasterKeyBackend};
use crate::stubs::metapb::Region;
use crate::stubs::{
    CaseInsensitive, DBReplace, EncloseDBAndTable, EncloseName, EncodeBytes, EncodeTablePrefix,
    Flag, FlagSet, FlagValue, GetStreamBackupGlobalCheckpointPrefix, Glue, IsSysOrTempSysDB,
    LogRestoreProgressIdMapSaved, MemGlue, MemStorage, MetaFile, NormalVersionChecker, ParseFilter,
    ParseKey, PersistentState, RangeStats, SchemasReplace, Storage, TableReplace,
    TaskInfoForLogRestore, resumeStateFileName,
};

#[test]
fn stub_key_and_filter_contracts_match_go_dependencies() {
    assert_eq!(ParseKey("escaped", r"\a\x1").unwrap(), b"\x07\x01");
    assert_eq!(
        ParseKey("escaped", r"\b\f\n\r\t\v\'").unwrap(),
        b"\x08\x0c\n\r\t\x0b'"
    );
    assert!(ParseKey("escaped", r"trailing\").is_err());
    assert_eq!(
        ParseKey("unsupported", "key").unwrap_err().msg,
        "unknown format: invalid argument"
    );

    assert_eq!(EncodeBytes(b""), vec![0, 0, 0, 0, 0, 0, 0, 0, 247]);
    assert_eq!(EncodeBytes(&[1, 2, 3]), vec![1, 2, 3, 0, 0, 0, 0, 0, 250]);
    assert_eq!(
        EncodeBytes(&[1, 2, 3, 4, 5, 6, 7, 8]),
        vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 0, 0, 0, 0, 0, 0, 0, 0, 247]
    );

    let filter = CaseInsensitive(ParseFilter(vec!["Sales.Orders".into()]).unwrap());
    assert!(filter.MatchTable("sales", "orders"));
    assert!(filter.MatchTable("SALES", "ORDERS"));
    assert!(!filter.MatchTable("sales", "customers"));

    assert_eq!(EncloseName("a`b"), "`a``b`");
    assert_eq!(EncloseDBAndTable("d`b", "t`b"), "`d``b`.`t``b`");
    assert_eq!(EncodeTablePrefix(1), vec![b't', 0x80, 0, 0, 0, 0, 0, 0, 1]);
    assert!(IsSysOrTempSysDB("mysql"));
    assert!(IsSysOrTempSysDB("__TiDB_BR_Temporary_sys"));
    assert!(IsSysOrTempSysDB("workload_schema"));
    assert!(!IsSysOrTempSysDB("information_schema"));
    assert!(!IsSysOrTempSysDB("MYSQL"));
}

#[test]
/// 总入口：驱动 task 层契约断言。
fn go_rust_public_contract_matches() {
    // --- encryption: normal + error ---
    // 绑定 `u`，供后续步骤使用。
    let u = Url::parse("local:///path/to/key").unwrap();
    // 绑定 `mk`，供后续步骤使用。
    let mk = parseLocalDiskConfig(&u).unwrap();
    // 匹配分支：按枚举/结果形态分流。
    match mk.Backend.unwrap() {
        // 赋值更新状态。
        MasterKeyBackend::File(f) => assert_eq!(f.Path, "/path/to/key"),
        // 赋值更新状态。
        _ => panic!("expected file backend"),
    }
    // 绑定 `u_bad`，供后续步骤使用。
    let u_bad = Url::parse("local://relative/path").unwrap();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(parseLocalDiskConfig(&u_bad).is_err());

    // 绑定 `aws`，供后续步骤使用。
    let aws = validateAndParseMasterKeyString(
        // 赋值更新状态。
        "aws-kms:///key-id?AWS_ACCESS_KEY_ID=AKIA&AWS_SECRET_ACCESS_KEY=SECRET&REGION=us-west-2",
    )
    // 期望成功；失败则测试直接崩，便于定位。
    .unwrap();
    // 匹配分支：按枚举/结果形态分流。
    match aws.Backend.unwrap() {
        // 赋值更新状态。
        MasterKeyBackend::Kms(k) => {
            // 断言：核对与 Go 契约一致的期望值/错误。
            assert_eq!(k.Vendor, "aws");
            // 断言：核对与 Go 契约一致的期望值/错误。
            assert_eq!(k.KeyId, "key-id");
            // 断言：核对与 Go 契约一致的期望值/错误。
            assert_eq!(k.Region, "us-west-2");
            // 断言：核对与 Go 契约一致的期望值/错误。
            assert!(k.AwsKms.is_some());
        }
        // 赋值更新状态。
        _ => panic!("kms"),
    }
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        validateAndParseMasterKeyString(
            // 赋值更新状态。
            "aws-kms:///key-id?AWS_ACCESS_KEY_ID=AKIA&REGION=us-west-2"
        )
        .is_err()
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(parseAwsKmsConfig(&Url::parse("aws-kms:///?REGION=us-west-2").unwrap()).is_err());

    // 绑定 `azure`，供后续步骤使用。
    let azure = parseAzureKmsConfig(
        &Url::parse(
            // 赋值更新状态。
            "azure-kms:///abcd/v1?AZURE_TENANT_ID=t&AZURE_CLIENT_ID=c&AZURE_CLIENT_SECRET=s&AZURE_VAULT_NAME=v",
        )
        // 期望成功；失败则测试直接崩，便于定位。
        .unwrap(),
    )
    // 期望成功；失败则测试直接崩，便于定位。
    .unwrap();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(matches!(azure.Backend, Some(MasterKeyBackend::Kms(_))));
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        // 期望成功；失败则测试直接崩，便于定位。
        parseAzureKmsConfig(&Url::parse("azure-kms:///abcd/v1?AZURE_TENANT_ID=t").unwrap())
            .is_err()
    );

    // 绑定 `gcp`，供后续步骤使用。
    let gcp = parseGcpKmsConfig(
        // 赋值更新状态。
        &Url::parse("gcp-kms:///projects/p/locations/l/keyRings/r/cryptoKeys/k?CREDENTIALS=creds")
            // 期望成功；失败则测试直接崩，便于定位。
            .unwrap(),
    )
    // 期望成功；失败则测试直接崩，便于定位。
    .unwrap();
    // 匹配分支：按枚举/结果形态分流。
    match gcp.Backend.unwrap() {
        // 赋值更新状态。
        MasterKeyBackend::Kms(k) => {
            // 断言：核对与 Go 契约一致的期望值/错误。
            assert_eq!(k.KeyId, "projects/p/locations/l/keyRings/r/cryptoKeys/k");
        }
        // 赋值更新状态。
        _ => panic!("gcp"),
    }
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(validateAndParseMasterKeyString("unknown://x").is_err());

    // --- common: PD URL, cipher, flags redaction, operation context ---
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(normalizePDURL("https://pd:5432", true).unwrap(), "pd:5432");
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        normalizePDURL("https://pd.pingcap.com", false)
            .unwrap_err()
            .msg
            .contains("https while TLS disabled")
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        normalizePDURL("http://127.0.0.1:2379", true)
            .unwrap_err()
            .msg
            .contains("http while TLS enabled")
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        // 期望成功；失败则测试直接崩，便于定位。
        normalizePDURL("http://127.0.0.1", false).unwrap(),
        "127.0.0.1"
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        // 期望成功；失败则测试直接崩，便于定位。
        normalizePDURL("127.0.0.1:2379", false).unwrap(),
        "127.0.0.1:2379"
    );

    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(matches!(
        // 期望成功；失败则测试直接崩，便于定位。
        parseCipherType("aes256-ctr").unwrap(),
        EncryptionMethod::AES256_CTR
    ));
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(parseCipherType("nope").is_err());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(checkCipherKey("", "").is_err());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(checkCipherKey("aa", "file").is_err());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(checkCipherKey("aabb", "").is_ok());
    // 绑定 `key`，供后续步骤使用。
    let key = GetCipherKeyContent("00112233445566778899aabbccddeeff", "").unwrap();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(key.len(), 16);
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(GetCipherKeyContent("zz", "").is_err());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        checkCipherKeyMatch(&CipherInfo {
            CipherType: EncryptionMethod::AES128_CTR,
            CipherKey: key,
        })
        .is_ok()
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        checkCipherKeyMatch(&CipherInfo {
            CipherType: EncryptionMethod::AES128_CTR,
            // 构造集合承载中间数据。
            CipherKey: vec![1, 2, 3],
        })
        .is_err()
    );

    // 绑定 `field`，供后续步骤使用。
    let field = flagToZapField(&Flag {
        Name: flagStorage.into(),
        // 赋值更新状态。
        Value: "s3://some/what?secret=a123456789&key=987654321".into(),
    });
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(field.Key, "storage");
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(field.String, "s3://some/what");
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        flagToZapField(&Flag {
            Name: FlagStreamFullBackupStorage.into(),
            // 赋值更新状态。
            Value: "s3://bucket/prefix/?access-key=1&secret-key=2".into(),
        })
        .String,
        "s3://bucket/prefix/"
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        flagToZapField(&Flag {
            Name: FlagPiTRAddIndexSQLStorage.into(),
            // 赋值更新状态。
            Value: "s3://bucket/pitr/add-index?access-key=1&secret-key=2".into(),
        })
        .String,
        "s3://bucket/pitr/add-index"
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        flagToZapField(&Flag {
            Name: flagFullBackupCipherKey.into(),
            Value: "537570657253656372657456616C7565".into(),
        })
        .String,
        "<redacted>"
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        flagToZapField(&Flag {
            Name: flagLogBackupCipherKey.into(),
            Value: "x".into()
        })
        .String,
        "<redacted>"
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        flagToZapField(&Flag {
            Name: flagMasterKeyConfig.into(),
            Value: "local:///path".into()
        })
        .String,
        "<redacted>"
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        flagToZapField(&Flag {
            Name: flagSendCreds.into(),
            Value: "true".into()
        })
        .String,
        "true"
    );

    // 绑定 `cfg`，供后续步骤使用。
    let mut cfg = Config::default();
    // 期望成功；失败则测试直接崩，便于定位。
    cfg.EnsureOperationContext("log-restore").unwrap();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(!cfg.OperationContext.OperationID.is_empty());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(cfg.OperationContext.StartedAt != std::time::SystemTime::UNIX_EPOCH);

    // 绑定 `bad`，供后续步骤使用。
    let mut bad = Config::default();
    // 赋值更新状态。
    bad.OperationContext.OperationID = "operation-id".into();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        bad.EnsureOperationContext("log-restore")
            .unwrap_err()
            .msg
            .contains("operation started time")
    );

    // 绑定 `bad2`，供后续步骤使用。
    let mut bad2 = Config::default();
    // 赋值更新状态。
    bad2.OperationContext.StartedAt = std::time::SystemTime::now();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        bad2.EnsureOperationContext("log-restore")
            .unwrap_err()
            .msg
            .contains("operation ID")
    );

    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(TLSConfig::default().IsEnabled() == false);
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(FullBackupType(FullBackupTypeKV.into()).Valid());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(FullBackupType(FullBackupTypeEBS.into()).Valid());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(!FullBackupType("nope".into()).Valid());

    // NewMgr empty PD error
    // 绑定 `g`，供后续步骤使用。
    let g = MemGlue {
        version: "br-test".into(),
        ..Default::default()
    };
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(
        NewMgr(
            &g,
            "",
            &[],
            &TLSConfig::default(),
            crate::stubs::KeepaliveParams::default(),
            true,
            false,
            NormalVersionChecker,
        )
        .is_err()
    );

    // --- backup helpers ---
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(isFullBackup(FullBackupCmd));
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(!isFullBackup("x"));
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(ParseTSString("", false).unwrap(), 0);
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(
        // 期望成功；失败则测试直接崩，便于定位。
        ParseTSString("400036290571534337", false).unwrap(),
        400036290571534337
    );
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(ParseTSString("2018-05-11 01:42:23", true).is_err());
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(matches!(
        // 期望成功；失败则测试直接崩，便于定位。
        parseCompressionType("zstd").unwrap(),
        CompressionType::ZSTD
    ));
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(parseCompressionType("gzip").is_err());
    // 绑定 `fs`，供后续步骤使用。
    let mut fs = FlagSet::new();
    // 执行语句，推进流程。
    fs.DefineString("replica-read-label", "zone:tikv");
    // 绑定 `labels`，供后续步骤使用。
    let labels = parseReplicaReadLabelFlag(&fs).unwrap();
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert_eq!(labels.get("zone").map(|s| s.as_str()), Some("tikv"));
    // 执行语句，推进流程。
    fs.Set("replica-read-label", FlagValue::String("bad".into()));
    // 断言：核对与 Go 契约一致的期望值/错误。
    assert!(parseReplicaReadLabelFlag(&fs).is_err());

    // 绑定 `bcfg`，供后续步骤使用。
    let mut bcfg = BackupConfig::default();
    // 构造集合承载中间数据。
    bcfg.Config.PD = vec!["127.0.0.1:2379".into()];
    // 赋值更新状态。
    bcfg.Config.Storage = "local:///tmp/br".into();
    RunBackupWithDefaults(&g, FullBackupCmd, &mut bcfg).unwrap();
    assert!(crate::stubs::TakeSuccessStatus());

    // raw / txn range validation
    let mut raw = RawKvConfig::default();
    raw.StartKey = vec![2];
    raw.EndKey = vec![1];
    let mut rfs = FlagSet::new();
    crate::common::DefineCommonFlags(&mut rfs);
    crate::backup_raw::DefineRawBackupFlags(&mut rfs);
    // ParseFromFlags will fail on range after reading keys — set via fields + ParseFromFlags path
    assert!(
        raw.ParseFromFlags(&{
            let mut f = FlagSet::new();
            crate::common::DefineCommonFlags(&mut f);
            crate::backup_raw::DefineRawBackupFlags(&mut f);
            f.Set("start", FlagValue::String("02".into()));
            f.Set("end", FlagValue::String("01".into()));
            f.Set("storage", FlagValue::String("local:///tmp".into()));
            f
        })
        .is_err()
    );

    let mut txn = TxnKvConfig {
        StartKey: vec![5],
        EndKey: vec![1],
        ..Default::default()
    };
    assert!(
        txn.ParseFromFlags(&{
            let mut f = FlagSet::new();
            crate::common::DefineCommonFlags(&mut f);
            f.Set("storage", FlagValue::String("local:///tmp".into()));
            f
        })
        .is_err()
    );

    let mut raw_ok = RawKvConfig::default();
    raw_ok.Config.PD = vec!["127.0.0.1:2379".into()];
    raw_ok.Config.Storage = "local:///tmp".into();
    raw_ok.CF = "default".into();
    RunBackupRawWithDefaults(&g, "Raw Backup", &mut raw_ok).unwrap();

    let mut txn_ok = TxnKvConfig::default();
    txn_ok.Config.PD = vec!["127.0.0.1:2379".into()];
    txn_ok.Config.Storage = "local:///tmp".into();
    RunBackupTxnWithDefaults(&g, "Txn Backup", &mut txn_ok).unwrap();

    // EBS hole detection
    let mut regions = vec![
        Region {
            Id: 1,
            StartKey: vec![0],
            EndKey: vec![1],
        },
        Region {
            Id: 2,
            StartKey: vec![2],
            EndKey: vec![3],
        },
    ];
    assert!(isRegionsHasHole(&mut regions));
    let mut contiguous = vec![
        Region {
            Id: 1,
            StartKey: vec![0],
            EndKey: vec![1],
        },
        Region {
            Id: 2,
            StartKey: vec![1],
            EndKey: vec![2],
        },
    ];
    assert!(!isRegionsHasHole(&mut contiguous));

    let mut ebs_cfg = BackupConfig::default();
    ebs_cfg.FullBackupType = FullBackupType(FullBackupTypeEBS.into());
    ebs_cfg.Config.PD = vec!["127.0.0.1:2379".into()];
    ebs_cfg.SkipAWS = true;
    let storage = Arc::new(MemStorage::new());
    RunBackupEBS(&g, &mut ebs_cfg, storage.clone()).unwrap();
    assert!(storage.as_ref().FileExists("backupmeta.json").unwrap());

    // --- restore helpers ---
    assert!(isFullRestore(FullRestoreCmd));
    assert!(IsStreamRestore(PointRestoreCmd));
    assert_eq!(EstimateTikvUsage(100, 3, 3), 100);
    assert_eq!(EstimateTikvUsage(100, 5, 2), 100); // replica capped to store
    assert_eq!(EstimateTikvUsage(100, 3, 0), 0);
    assert!(CheckStoreSpace(10, 9, 1).is_err());
    assert!(CheckStoreSpace(10, 11, 1).is_ok());
    assert!(CheckStoreSpace(10, 0, 1).is_err());

    let (a, b) = encodeCompactAndCheckKey([1, 3]);
    assert!(!a.is_empty() && !b.is_empty());
    assert!(rewriteKeyRanges([0, 0]).is_empty());
    assert_eq!(rewriteKeyRanges([1, 2]).len(), 1);

    let tables = vec![Table {
        DB: DBInfo {
            ID: 1,
            Name: CIStr("db".into()),
        },
        Info: TableInfo {
            ID: 10,
            Name: CIStr("t".into()),
            ..Default::default()
        },
    }];
    let mut jobs = vec![
        Job {
            SchemaID: 1,
            TableID: 10,
            SchemaName: "db".into(),
            Type: ActionLockTable,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 2,
                DBInfo: Some(DBInfo {
                    ID: 1,
                    Name: CIStr("db".into()),
                }),
                TableInfo: Some(TableInfo {
                    ID: 10,
                    Name: CIStr("t".into()),
                    ..Default::default()
                }),
            },
        },
        Job {
            SchemaID: 99,
            TableID: 99,
            SchemaName: "other".into(),
            Type: ActionAddIndex,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 1,
                ..Default::default()
            },
        },
    ];
    let filtered = FilterDDLJobs(&mut jobs, &tables);
    assert!(!filtered.is_empty());
    assert!(CheckDDLJobByRules(&filtered, &[DDLJobBlockListRule]).is_err());
    let passed = FilterDDLJobByRules(&filtered, &[DDLJobBlockListRule]);
    assert!(passed.is_empty());

    let ends = getEndKeys(&[
        RangeStats {
            EndKey: vec![1],
            ..Default::default()
        },
        RangeStats {
            EndKey: vec![],
            ..Default::default()
        },
        RangeStats {
            EndKey: vec![2],
            ..Default::default()
        },
    ]);
    assert_eq!(ends, vec![vec![1], vec![2]]);

    let mut rcfg = RestoreConfig::default();
    rcfg.Config.PD = vec!["127.0.0.1:2379".into()];
    rcfg.Config.Storage = "local:///tmp".into();
    let restore_storage = MemStorage::new();
    restore_storage.put(
        MetaFile,
        serde_json::to_vec(&crate::stubs::backuppb::BackupMeta {
            EndVersion: 100,
            Files: vec![crate::stubs::backuppb::File {
                Name: "s1".into(),
                Size_: 100,
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap(),
    );
    rcfg.RestoreStorage = Some(restore_storage);
    RunRestore(&g, FullRestoreCmd, &mut rcfg).unwrap();

    let mut raw_restore = crate::restore_raw::RestoreRawConfig::default();
    raw_restore.RawKvConfig.Config.PD = vec!["127.0.0.1:2379".into()];
    raw_restore.RawKvConfig.Config.Storage = "local:///tmp".into();
    raw_restore.RawKvConfig.CF = "default".into();
    RunRestoreRaw(&g, "Raw Restore", &mut raw_restore).unwrap();

    let mut txn_cfg = Config::default();
    txn_cfg.PD = vec!["127.0.0.1:2379".into()];
    txn_cfg.Storage = "local:///tmp".into();
    RunRestoreTxn(&g, "Txn Restore", &mut txn_cfg).unwrap();

    // restore_data meta validation
    let ebs_store = MemStorage::new();
    ebs_store.put(
        "backupmeta.json",
        serde_json::to_vec(&serde_json::json!({
            "full_backup_type": "aws-ebs",
            "resolved_ts": 123u64,
            "tikv": {"replicas": 3, "stores": []}
        }))
        .unwrap(),
    );
    let (ts, replicas) = ReadBackupMetaData(&ebs_store).unwrap();
    assert_eq!(ts, 123);
    assert_eq!(replicas, 3);
    let bad_store = MemStorage::new();
    bad_store.put(
        "backupmeta.json",
        serde_json::to_vec(&serde_json::json!({"full_backup_type": "kv"})).unwrap(),
    );
    assert!(ReadBackupMetaData(&bad_store).is_err());
    let mut rcfg2 = RestoreConfig::default();
    rcfg2.Config.PD = vec!["127.0.0.1:2379".into()];
    RunResolveKvData(&g, "resolve", &mut rcfg2, Arc::new(ebs_store)).unwrap();

    let mut ebs_restore = RestoreConfig::default();
    ebs_restore.SkipAWS = true;
    ebs_restore.Prepare = true;
    ebs_restore.Config.PD = vec!["127.0.0.1:2379".into()];
    RunRestoreEBSMetaWithDefaults(&g, "ebs-meta", &mut ebs_restore).unwrap();

    // --- stream pure helpers ---
    assert!(checkLogRange(10, 20, 5, 30).is_ok());
    assert!(checkLogRange(4, 20, 5, 30).is_err());
    assert!(checkLogRange(10, 40, 5, 30).is_err());
    assert!(checkLogRange(25, 20, 5, 30).is_err());

    let ts = oracle_compose_sample();
    let shifted = ShiftTS(ts);
    assert!(shifted < ts || shifted == 0);
    assert_eq!(ShiftTS(0), 0);
    assert_eq!(buildPauseSafePointName("task1"), "task1_pause_safepoint");
    assert!(isValidSnapshotRange([1, 5]));
    assert!(!isValidSnapshotRange([0, 0]));
    assert!(!isValidSnapshotRange([5, 5]));
    assert!(verifyContiguousIDs(&[1, 2, 3]));
    assert!(!verifyContiguousIDs(&[1, 3]));
    assert!(!isCurrentIdMapSaved(None));
    assert!(isCurrentIdMapSaved(Some(&TaskInfoForLogRestore {
        Progress: LogRestoreProgressIdMapSaved,
    })));

    let mut schemas = SchemasReplace::default();
    let mut db = DBReplace {
        Name: "test".into(),
        ..Default::default()
    };
    db.TableMap.insert(
        100,
        TableReplace {
            Name: "t".into(),
            TableID: 200,
            PartitionMap: HashMap::from([(101, 201)]),
            ..Default::default()
        },
    );
    schemas.DbReplaceMap.insert(1, db);
    let rules = buildRewriteRules(&schemas);
    assert!(rules.contains_key(&100));
    assert!(rules.contains_key(&101));
    let key_ranges = buildKeyRangesFromSchemasReplace(&schemas, [10, 50]);
    assert!(!key_ranges.is_empty());

    // checkpoint resume state resource cleanup / IO boundary
    let s = MemStorage::new();
    s.put(
        MetaFile,
        serde_json::to_vec(&crate::stubs::backuppb::BackupMeta {
            StartVersion: 10,
            EndVersion: 0,
            ClusterId: 7,
            ..Default::default()
        })
        .unwrap(),
    );
    let prefix = GetStreamBackupGlobalCheckpointPrefix();
    s.put(&format!("{prefix}/a.ts"), 50u64.to_le_bytes().to_vec());
    assert_eq!(getGlobalCheckpointFromStorage(&s).unwrap(), 50);
    assert_eq!(getCheckpointFromResumeState(&s).unwrap(), (0, false));
    s.put(
        resumeStateFileName,
        serde_json::to_vec(&PersistentState { LastCheckpoint: 99 }).unwrap(),
    );
    assert_eq!(getCheckpointFromResumeState(&s).unwrap(), (99, true));

    let mut scfg = StreamConfig::default();
    scfg.Config.PD = vec!["127.0.0.1:2379".into()];
    scfg.Config.Storage = "local:///tmp".into();
    scfg.UntilTS = 20;
    scfg.DryRun = true;
    // truncate dry-run still validates range against storage meta
    // seed min/max via meta + checkpoint already in `s` — use makeStorage mem stub:
    // GetStorage returns empty MemStorage; dry-run with UntilTS uses getLogInfoFromStorage which needs meta.
    // Exercise error path when meta missing:
    assert!(RunStreamTruncate(&g, "truncate", &mut scfg).is_err());
}

/// 构造样例 TS，供与 Go oracle 组合结果对照。
fn oracle_compose_sample() -> u64 {
    crate::stubs::oracle::ComposeTS(1_000_000, 7)
}

use std::collections::HashMap;
