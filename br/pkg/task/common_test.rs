// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/task/common_test.go`.
//!
//! 公共配置测试：日志脱敏、OperationContext、PD URL、密钥校验、默认配置与 master-key。
//! 与 Go `common_test.go` 场景对齐；注明 Rust 桩/arm64 与 Go 的已知差异。
//! 不启动集群，全部在 FlagSet/Config 内存对象上断言。
//! 脱敏用例覆盖 storage query、cipher key、azure key、master-key 整串。
//! 默认配置用例同时覆盖 common/backup/restore 三套 Default* 工厂。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::backup::{BackupConfig, DefaultBackupConfig, DefineBackupFlags};
use crate::backup_raw::RawKvConfig;
use crate::common::*;
use crate::restore::{DefaultRestoreConfig, RestoreConfig};
use crate::stubs::backuppb::CipherInfo;
use crate::stubs::encryptionpb::{
    AwsKms, AzureKms, EncryptionMethod, GcpKms, MasterKey, MasterKeyBackend, MasterKeyFile,
    MasterKeyKms,
};
use crate::stubs::{
    DefaultBRGCSafePointTTL, EncloseDBAndTable, EncloseName, Flag, FlagKeyspaceName, FlagSet,
    FlagValue,
};

/// 对应 Go `TestUrlNoQuery`：存储剥 query、密钥类输出 `<redacted>`。
#[test]
fn test_url_no_query() {
    struct Case {
        input_name: &'static str,
        expected_name: &'static str,
        input_value: &'static str,
        expected_value: &'static str,
    }
    let cases = [
        Case {
            // 普通 bool flag：值原样进日志。
            input_name: flagSendCreds,
            expected_name: "send-credentials-to-tikv",
            input_value: "true",
            expected_value: "true",
        },
        Case {
            // S3 URL 必须剥掉 secret/key query。
            input_name: flagStorage,
            expected_name: "storage",
            input_value: "s3://some/what?secret=a123456789&key=987654321",
            expected_value: "s3://some/what",
        },
        Case {
            // 流全量存储同样剥 query。
            input_name: FlagStreamFullBackupStorage,
            expected_name: "full-backup-storage",
            input_value: "s3://bucket/prefix/?access-key=1&secret-key=2",
            expected_value: "s3://bucket/prefix/",
        },
        Case {
            // PiTR 加索引存储脱敏规则与 storage 相同。
            input_name: FlagPiTRAddIndexSQLStorage,
            expected_name: "pitr-add-index-sql-storage",
            input_value: "s3://bucket/pitr/add-index?access-key=1&secret-key=2",
            expected_value: "s3://bucket/pitr/add-index",
        },
        Case {
            // 全量密钥整值替换为 <redacted>。
            input_name: flagFullBackupCipherKey,
            expected_name: "crypter.key",
            input_value: "537570657253656372657456616C7565",
            expected_value: "<redacted>",
        },
        Case {
            // 日志备份密钥同样脱敏。
            input_name: flagLogBackupCipherKey,
            expected_name: "log.crypter.key",
            input_value: "537570657253656372657456616C7565",
            expected_value: "<redacted>",
        },
        Case {
            // Azure 加密密钥字段名硬编码匹配。
            input_name: "azblob.encryption-key",
            expected_name: "azblob.encryption-key",
            input_value: "SUPERSECRET_AZURE_ENCRYPTION_KEY",
            expected_value: "<redacted>",
        },
        Case {
            // 多段 master-key 整串脱敏，不暴露任何 query。
            input_name: flagMasterKeyConfig,
            expected_name: "master-key",
            input_value: "local:///path/abcd,aws-kms:///abcd?AWS_ACCESS_KEY_ID=SECRET1&AWS_SECRET_ACCESS_KEY=SECRET2&REGION=us-east-1,azure-kms:///abcd/v1?AZURE_TENANT_ID=tenant-id&AZURE_CLIENT_ID=client-id&AZURE_CLIENT_SECRET=client-secret&AZURE_VAULT_NAME=vault-name",
            expected_value: "<redacted>",
        },
    ];

    // 逐项比对 ZapField 的 Key 与脱敏后 String。
    for tc in cases {
        let field = flagToZapField(&Flag {
            Name: tc.input_name.into(),
            Value: tc.input_value.into(),
        });
        assert_eq!(field.Key, tc.expected_name, "name for {}", tc.input_name);
        assert_eq!(
            field.String, tc.expected_value,
            "value for {}",
            tc.input_name
        );
    }
}

/// 对应 Go `TestParseStreamRestoreFlagsPiTRAddIndexSQLStorage`：解析 PiTR 存储 flag。
#[test]
fn test_parse_stream_restore_flags_pitr_add_index_sql_storage() {
    let mut flags = FlagSet::new();
    crate::restore::DefineStreamRestoreFlags(&mut flags);
    // 写入 local PiTR 路径并断言回读。
    flags.Set(
        FlagPiTRAddIndexSQLStorage,
        FlagValue::String("local:///tmp/pitr-add-index".into()),
    );
    let mut cfg = RestoreConfig::default();
    cfg.ParseStreamRestoreFlags(&flags).unwrap();
    // 解析结果应与 Set 的路径一致。
    assert_eq!(cfg.PiTRAddIndexSQLStorage, "local:///tmp/pitr-add-index");
}

/// 对应 Go `TestTiDBConfigUnchanged`：restore 闭包可安全调用。
#[test]
fn test_tidb_config_unchanged() {
    // Go 会改全局 tidb 配置再用 defer 恢复。
    // Rust arm64 上 tweakLocalConfForRestore 返回空操作闭包。
    // Go mutates global tidb config then restores via defer.
    // Rust `tweakLocalConfForRestore` is a no-op restore closure on arm64.
    let restore = crate::restore::tweakLocalConfForRestore();
    // 调用恢复闭包，确认不 panic。
    restore();
}

/// 对应 Go `TestEnsureOperationContext`：ID/时间成对约束。
#[test]
fn test_ensure_operation_context() {
    {
        // 空上下文应自动生成 ID 与 StartedAt。
        let mut cfg = Config::default();
        cfg.EnsureOperationContext("log-restore").unwrap();
        // 新生成的 OperationID 非空。
        assert!(!cfg.OperationContext.OperationID.is_empty());
        assert_ne!(cfg.OperationContext.StartedAt, UNIX_EPOCH);
    }
    {
        let started = UNIX_EPOCH + Duration::from_secs(1_750_000_000);
        let mut cfg = Config::default();
        // 已完整的上下文不得被覆盖。
        cfg.OperationContext.OperationID = "operation-id".into();
        cfg.OperationContext.StartedAt = started;
        cfg.EnsureOperationContext("log-restore").unwrap();
        // 预置 ID 保持不变。
        assert_eq!(cfg.OperationContext.OperationID, "operation-id");
        assert_eq!(cfg.OperationContext.StartedAt, started);
    }
    {
        let mut cfg = Config::default();
        // 有 ID 无时间 → 错误。
        cfg.OperationContext.OperationID = "operation-id".into();
        cfg.OperationContext.StartedAt = UNIX_EPOCH;
        let err = cfg.EnsureOperationContext("log-restore").unwrap_err();
        // 错误信息应提示 started time。
        assert!(err.msg.contains("operation started time"));
    }
    {
        let mut cfg = Config::default();
        // 有时间无 ID → 错误。
        cfg.OperationContext.StartedAt = SystemTime::now();
        let err = cfg.EnsureOperationContext("log-restore").unwrap_err();
        // 错误信息应提示 operation ID。
        assert!(err.msg.contains("operation ID"));
    }
}

/// 对应 Go `TestStripingPDURL`：http(s) 与 TLS 交叉校验。
#[test]
fn test_striping_pd_url() {
    // https + TLS：剥前缀保留 host:port。
    assert_eq!(normalizePDURL("https://pd:5432", true).unwrap(), "pd:5432");
    // https 但 TLS 关闭应失败。
    let err = normalizePDURL("https://pd.pingcap.com", false).unwrap_err();
    assert!(err.msg.contains("https while TLS disabled"));
    // http 但 TLS 开启应失败。
    let err = normalizePDURL("http://127.0.0.1:2379", true).unwrap_err();
    assert!(err.msg.contains("http while TLS enabled"));
    // http + 无 TLS：剥前缀。
    assert_eq!(
        normalizePDURL("http://127.0.0.1", false).unwrap(),
        "127.0.0.1"
    );
    // 无 scheme 原样返回。
    assert_eq!(
        normalizePDURL("127.0.0.1:2379", false).unwrap(),
        "127.0.0.1:2379"
    );
}

/// 对应 Go `TestCheckCipherKeyMatch`：各算法密钥长度与 UNKNOWN。
#[test]
fn test_check_cipher_key_match() {
    struct Case {
        name: &'static str,
        cipher: CipherInfo,
        expect_err: bool,
        err_msg: &'static str,
    }
    let cases = [
        Case {
            // 明文允许空密钥。
            name: "PLAINTEXT",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::PLAINTEXT,
                CipherKey: vec![],
            },
            expect_err: false,
            err_msg: "",
        },
        Case {
            // UNKNOWN 必须拒绝。
            name: "UNKNOWN",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::UNKNOWN,
                CipherKey: vec![],
            },
            expect_err: true,
            err_msg: "Unknown encryption method",
        },
        Case {
            // AES-128 需要精确 16 字节。
            name: "AES128_CTR valid",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::AES128_CTR,
                CipherKey: vec![0; crypterAES128KeyLen],
            },
            expect_err: false,
            err_msg: "",
        },
        Case {
            // 长度不足应报 mismatch。
            name: "AES128_CTR invalid length",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::AES128_CTR,
                CipherKey: vec![0; crypterAES128KeyLen - 1],
            },
            expect_err: true,
            err_msg: "AES-128 key length mismatch",
        },
        Case {
            // AES-192 需要 24 字节。
            name: "AES192_CTR valid",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::AES192_CTR,
                CipherKey: vec![0; crypterAES192KeyLen],
            },
            expect_err: false,
            err_msg: "",
        },
        Case {
            // 超长同样失败。
            name: "AES192_CTR invalid length",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::AES192_CTR,
                CipherKey: vec![0; crypterAES192KeyLen + 1],
            },
            expect_err: true,
            err_msg: "AES-192 key length mismatch",
        },
        Case {
            // AES-256 需要 32 字节。
            name: "AES256_CTR valid",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::AES256_CTR,
                CipherKey: vec![0; crypterAES256KeyLen],
            },
            expect_err: false,
            err_msg: "",
        },
        Case {
            // 空密钥对 AES-256 非法。
            name: "AES256_CTR invalid length",
            cipher: CipherInfo {
                CipherType: EncryptionMethod::AES256_CTR,
                CipherKey: vec![],
            },
            expect_err: true,
            err_msg: "AES-256 key length mismatch",
        },
    ];

    // 按 expect_err 分支断言错误子串。
    for c in cases {
        let result = checkCipherKeyMatch(&c.cipher);
        if c.expect_err {
            let err = result.unwrap_err();
            assert!(err.msg.contains(c.err_msg), "{}: {}", c.name, err.msg);
        } else {
            assert!(result.is_ok(), "{}", c.name);
        }
    }
}

/// 对应 Go `TestCheckCipherKey`：密钥与文件二选一。
#[test]
fn test_check_cipher_key() {
    // (key, file, ok)：二者恰有一个非空才通过。
    let cases = [
        ("0123456789abcdef0123456789abcdef", "", true),
        ("0123456789abcdef0123456789abcdef", "/tmp/abc", false),
        ("", "/tmp/abc", true),
        ("", "", false),
    ];
    // 逐用例核对 is_ok 与期望。
    for (key, file, ok) in cases {
        let result = checkCipherKey(key, file);
        assert_eq!(result.is_ok(), ok, "key={key:?} file={file:?}");
    }
}

/// 对应 Go `TestGetCipherKey`：非 hex 密钥报统一文案。
#[test]
fn test_get_cipher_key() {
    // 非 hex 明文密钥触发统一错误文案。
    let err = GetCipherKeyContent("this is not a hex string", "").unwrap_err();
    assert!(err.msg.contains(cipherKeyNonHexErrorMsg));

    // Go hex.DecodeString 不会裁剪直接传入密钥的首尾空白。
    let err = GetCipherKeyContent(" 00 ", "").unwrap_err();
    assert!(err.msg.contains(cipherKeyNonHexErrorMsg));

    // Go 只移除密钥文件末尾的一个 `\n`，CRLF 中的 `\r` 仍应令解码失败。
    let key_file = std::env::temp_dir().join(format!(
        "astersql-common-cipher-key-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("unnamed")
    ));
    std::fs::write(&key_file, b"00\r\n").unwrap();
    let err = GetCipherKeyContent("", key_file.to_str().unwrap()).unwrap_err();
    let _ = std::fs::remove_file(key_file);
    assert!(err.msg.contains(cipherKeyNonHexErrorMsg));
}

/// 对应 Go `TestDefault`：DefaultConfig 与 keyspace 解析。
#[test]
fn test_default() {
    // 默认 PD/并发/checksum/凭据/检查开关等。
    let def = DefaultConfig();
    // 默认单节点本地 PD。
    assert_eq!(def.PD, vec!["127.0.0.1:2379".to_string()]);
    // 默认 checksum 并发 4。
    assert_eq!(def.ChecksumConcurrency, 4);
    // 默认不做 checksum。
    assert!(!def.Checksum);
    // 默认向 TiKV 发送凭据。
    assert!(def.SendCreds);
    // 默认检查集群前提。
    assert!(def.CheckRequirements);
    // switch-mode 默认 5 分钟。
    assert_eq!(def.SwitchModeInterval, defaultSwitchInterval);
    // gRPC keepalive 默认 time。
    assert_eq!(def.GRPCKeepaliveTime, defaultGRPCKeepaliveTime);
    // gRPC keepalive 默认 timeout。
    assert_eq!(def.GRPCKeepaliveTimeout, defaultGRPCKeepaliveTimeout);
    // 全量加密默认明文。
    assert_eq!(def.CipherInfo.CipherType, EncryptionMethod::PLAINTEXT);
    assert_eq!(
        def.LogBackupCipherInfo.CipherType,
        EncryptionMethod::PLAINTEXT
    );
    // 128=0x80，与 defaultMetadataDownloadBatchSize 一致。
    assert_eq!(def.MetadataDownloadBatchSize, 0x80);

    {
        // 显式 keyspace 应写入 Config。
        let mut flags = FlagSet::new();
        DefineCommonFlags(&mut flags);
        flags.DefineString(FlagKeyspaceName, "");
        flags.Set(
            FlagKeyspaceName,
            FlagValue::String("restore-keyspace".into()),
        );
        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).unwrap();
        // restore 场景 keyspace 回读。
        assert_eq!(cfg.KeyspaceName, "restore-keyspace");
    }

    {
        let mut flags = FlagSet::new();
        DefineCommonFlags(&mut flags);
        // Raw 备份路径也能解析 keyspace-name。
        crate::backup_raw::DefineRawBackupFlags(&mut flags);
        flags.DefineInt32("compression-level", 0);
        flags.Set("keyspace-name", FlagValue::String("backup-keyspace".into()));
        let mut cfg = RawKvConfig::default();
        cfg.ParseBackupConfigFromFlags(&flags).unwrap();
        // raw backup 配置路径 keyspace 回读。
        assert_eq!(cfg.Config.KeyspaceName, "backup-keyspace");
    }
}

/// 对应 Go `TestDefaultBackup`：备份默认 GCTTL/统计/meta/checkpoint。
#[test]
fn test_default_backup() {
    // 备份覆盖默认：关闭 checksum，再套 DefaultBackupConfig。
    let mut common = DefaultConfig();
    common.OverrideDefaultForBackup();
    let def = DefaultBackupConfig(common);
    // GC TTL 使用 BR 默认安全点 TTL。
    assert_eq!(def.GCTTL, DefaultBRGCSafePointTTL);
    // 备份默认忽略统计收集。
    assert!(def.IgnoreStats);
    // 默认启用 backupmeta v2。
    assert!(def.UseBackupMetaV2);
    // 默认启用 checkpoint。
    assert!(def.UseCheckpoint);
    assert_eq!(
        def.CompressionConfig.CompressionType as i32,
        crate::stubs::backuppb::CompressionType::ZSTD as i32
    );
}

/// 对应 Go `TestDefaultRestore`：恢复默认与系统用户重置列表。
#[test]
fn test_default_restore() {
    let common = DefaultConfig();
    let def = DefaultRestoreConfig(common);
    // NoSchema 默认 false。
    assert!(!def.NoSchema);
    // 与 Go DefineRestoreFlags 默认值一致。
    assert!(def.LoadStats);
    assert!(def.FastLoadSysTables);
    assert!(def.UseCheckpoint);
    // 系统表默认包含（来自 RestoreCommonConfig 解析）。
    assert!(def.RestoreCommonConfig.WithSysTable);
    // 重置系统用户列表含 cloud_admin/root。
    assert_eq!(
        def.RestoreCommonConfig.ResetSysUsers,
        vec!["cloud_admin".to_string(), "root".to_string()]
    );
}

/// 对应 Go `TestParseAndValidateMasterKeyInfo`：单/多云 URL 与非法 scheme。
#[test]
fn test_parse_and_validate_master_key_info() {
    struct Case {
        name: &'static str,
        input: &'static str,
        expected: Vec<MasterKey>,
        expect_error: bool,
    }
    let tests = [
        Case {
            // 空串：不报错，MasterKeys 为空。
            name: "Empty input",
            input: "",
            expected: vec![],
            expect_error: false,
        },
        Case {
            // 单条 local 文件主密钥。
            name: "Single local config",
            input: "local:///path/to/key",
            expected: vec![MasterKey {
                Backend: Some(MasterKeyBackend::File(MasterKeyFile {
                    Path: "/path/to/key".into(),
                })),
            }],
            expect_error: false,
        },
        Case {
            // 单条 AWS KMS。
            name: "Single AWS config",
            input: "aws-kms:///key-id?AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE&AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY&REGION=us-west-2",
            expected: vec![MasterKey {
                Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
                    Vendor: "aws".into(),
                    KeyId: "key-id".into(),
                    Region: "us-west-2".into(),
                    AwsKms: Some(AwsKms {
                        AccessKey: "AKIAIOSFODNN7EXAMPLE".into(),
                        SecretAccessKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
                    }),
                    ..Default::default()
                })),
            }],
            expect_error: false,
        },
        Case {
            // 单条 Azure KMS。
            name: "Single Azure config",
            input: "azure-kms:///key-name/key-version?AZURE_TENANT_ID=tenant-id&AZURE_CLIENT_ID=client-id&AZURE_CLIENT_SECRET=client-secret&AZURE_VAULT_NAME=vault-name",
            expected: vec![MasterKey {
                Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
                    Vendor: "azure".into(),
                    KeyId: "key-name/key-version".into(),
                    AzureKms: Some(AzureKms {
                        TenantId: "tenant-id".into(),
                        ClientId: "client-id".into(),
                        ClientSecret: "client-secret".into(),
                        KeyVaultUrl: "vault-name".into(),
                    }),
                    ..Default::default()
                })),
            }],
            expect_error: false,
        },
        Case {
            // 单条 GCP KMS，KeyId 为完整资源名。
            name: "Single GCP config",
            input: "gcp-kms:///projects/project-id/locations/global/keyRings/ring-name/cryptoKeys/key-name?CREDENTIALS=credentials",
            expected: vec![MasterKey {
                Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
                    Vendor: "gcp".into(),
                    KeyId: "projects/project-id/locations/global/keyRings/ring-name/cryptoKeys/key-name".into(),
                    GcpKms: Some(GcpKms {
                        Credential: "credentials".into(),
                    }),
                    ..Default::default()
                })),
            }],
            expect_error: false,
        },
        Case {
            // 逗号连接多主密钥，顺序保持。
            name: "Multiple configs",
            input: "local:///path/to/key,aws-kms:///key-id?AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE&AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY&REGION=us-west-2",
            expected: vec![
                MasterKey {
                    Backend: Some(MasterKeyBackend::File(MasterKeyFile {
                        Path: "/path/to/key".into(),
                    })),
                },
                MasterKey {
                    Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
                        Vendor: "aws".into(),
                        KeyId: "key-id".into(),
                        Region: "us-west-2".into(),
                        AwsKms: Some(AwsKms {
                            AccessKey: "AKIAIOSFODNN7EXAMPLE".into(),
                            SecretAccessKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
                        }),
                        ..Default::default()
                    })),
                },
            ],
            expect_error: false,
        },
        Case {
            // 未知 scheme 应失败。
            name: "Invalid config",
            input: "invalid:///config",
            expected: vec![],
            expect_error: true,
        },
    ];

    // 每个 master-key 样例独立 FlagSet，避免串扰。
    for tt in tests {
        let mut cfg = Config::default();
        let mut flags = FlagSet::new();
        flags.DefineString(flagMasterKeyConfig, "");
        // 包装算法固定 aes256-ctr，与 Go 用例一致。
        flags.DefineString(flagMasterKeyCipherType, "aes256-ctr");
        flags.Set(flagMasterKeyConfig, FlagValue::String(tt.input.into()));
        // hasPlaintextKey=false：允许配置 master-key。
        let result = cfg.parseAndValidateMasterKeyInfo(false, &flags);
        if tt.expect_error {
            assert!(result.is_err(), "{}", tt.name);
        } else {
            result.unwrap();
            assert_eq!(cfg.MasterKeyConfig.MasterKeys, tt.expected, "{}", tt.name);
        }
    }

    // 触达 Enclose* 辅助，与 Go 默认 schema/table 封闭格式一致。
    assert_eq!(EncloseName("test"), "`test`");
    assert_eq!(EncloseDBAndTable("test", "t"), "`test`.`t`");
}
