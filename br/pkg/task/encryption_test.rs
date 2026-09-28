// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/task/encryption_test.go`.
//!
//! 表驱动校验各云 master-key URL 解析：合法样例字段对齐，非法样例必须失败。
//! 仅测 `parse*KmsConfig`/`parseLocalDiskConfig`，不走 `validateAndParseMasterKeyString` 入口，
//! 以便在 scheme 分发之外单独覆盖 path/query 规则。

use url::Url;

// 直接测各 parse* 函数，绕过 scheme 分发。
use crate::encryption::{
    parseAwsKmsConfig, parseAzureKmsConfig, parseGcpKmsConfig, parseLocalDiskConfig,
};
use crate::stubs::encryptionpb::{
    AwsKms, AzureKms, GcpKms, MasterKey, MasterKeyBackend, MasterKeyFile, MasterKeyKms,
};

/// 对应 Go `TestParseLocalDiskConfig`：绝对路径成功，带 host 的相对路径失败。
#[test]
fn test_parse_local_disk_config() {
    struct Case {
        name: &'static str,
        input: &'static str,
        expected: Option<MasterKey>,
        expect_error: bool,
    }
    // 表驱动：合法绝对路径 vs 相对/带 host。
    let tests = [
        Case {
            name: "Valid local path",
            // `local:///path` → File.Path=/path/to/key。
            input: "local:///path/to/key",
            expected: Some(MasterKey {
                Backend: Some(MasterKeyBackend::File(MasterKeyFile {
                    Path: "/path/to/key".into(),
                })),
            }),
            // 期望 File backend。
            expect_error: false,
        },
        Case {
            name: "Invalid local path",
            // `local://relative` 含 host，Go/Rust 均拒绝。
            input: "local://relative/path",
            expected: None,
            // relative path 含 host → 失败。
            expect_error: true,
        },
    ];

    // 先 Url::parse 再交给业务解析器。
    for tt in tests {
        let u = Url::parse(tt.input).expect("parse local url");
        let result = parseLocalDiskConfig(&u);
        // 错误样例只关心 is_err；成功样例比对整个 MasterKey。
        // GCP KeyId 必须是完整资源名字符串。
        if tt.expect_error {
            assert!(result.is_err(), "{}", tt.name);
        } else {
            assert_eq!(result.unwrap(), tt.expected.unwrap(), "{}", tt.name);
        }
    }
}

/// 对应 Go `TestParseAwsKmsConfig`：完整凭据、缺 key id、AK/SK 不成对。
#[test]
fn test_parse_aws_kms_config() {
    struct Case {
        name: &'static str,
        input: &'static str,
        expected: Option<MasterKey>,
        expect_error: bool,
    }
    // AWS：完整凭据、空 key id、AK/SK 不成对三类。
    let tests = [
        Case {
            name: "Valid AWS config",
            // path=/key-id，query 含 REGION 与成对 AK/SK。
            input: "aws-kms:///key-id?AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE&AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY&REGION=us-west-2",
            expected: Some(MasterKey {
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
            }),
            // Region/Vendor/KeyId/AwsKms 全匹配。
            expect_error: false,
        },
        Case {
            name: "Missing key ID",
            // path 为空不符合 `/([^/]+)`。
            input: "aws-kms:///?AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE&AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY&REGION=us-west-2",
            expected: None,
            // 空 key id。
            expect_error: true,
        },
        Case {
            name: "Missing required parameter",
            // 只有 AK 无 SK，应报成对缺失。
            input: "aws-kms:///key-id?AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE&REGION=us-west-2",
            expected: None,
            // AK/SK 不成对。
            expect_error: true,
        },
    ];

    // AWS 用例循环。
    for tt in tests {
        let u = Url::parse(tt.input).expect("parse aws kms url");
        let result = parseAwsKmsConfig(&u);
        // 与 local 相同的错误/成功分支。
        if tt.expect_error {
            assert!(result.is_err(), "{}", tt.name);
        } else {
            assert_eq!(result.unwrap(), tt.expected.unwrap(), "{}", tt.name);
        }
    }
}

/// 对应 Go `TestParseAzureKmsConfig`：四参数齐全 vs 缺 ClientSecret。
#[test]
fn test_parse_azure_kms_config() {
    struct Case {
        name: &'static str,
        input: &'static str,
        expected: Option<MasterKey>,
        expect_error: bool,
    }
    // Azure：四参数齐全 vs 缺 secret。
    let tests = [
        Case {
            name: "Valid Azure config",
            // KeyId 保留 `key-name/key-version`；Vault 进 KeyVaultUrl。
            input: "azure-kms:///key-name/key-version?AZURE_TENANT_ID=tenant-id&AZURE_CLIENT_ID=client-id&AZURE_CLIENT_SECRET=client-secret&AZURE_VAULT_NAME=vault-name",
            expected: Some(MasterKey {
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
            }),
            // Tenant/Client/Secret/Vault 齐全。
            expect_error: false,
        },
        Case {
            name: "Missing required parameter",
            // 缺 AZURE_CLIENT_SECRET。
            input: "azure-kms:///key-name/key-version?AZURE_TENANT_ID=tenant-id&AZURE_CLIENT_ID=client-id&AZURE_VAULT_NAME=vault-name",
            expected: None,
            // 缺 ClientSecret。
            expect_error: true,
        },
    ];

    // Azure 用例循环。
    for tt in tests {
        let u = Url::parse(tt.input).expect("parse azure kms url");
        let result = parseAzureKmsConfig(&u);
        // Azure 成功时 Vendor/KeyId/AzureKms 全字段比对。
        if tt.expect_error {
            assert!(result.is_err(), "{}", tt.name);
        } else {
            assert_eq!(result.unwrap(), tt.expected.unwrap(), "{}", tt.name);
        }
    }
}

/// 对应 Go `TestParseGcpKmsConfig`：标准资源路径、非法 path、缺 CREDENTIALS。
#[test]
fn test_parse_gcp_kms_config() {
    struct Case {
        name: &'static str,
        input: &'static str,
        expected: Option<MasterKey>,
        expect_error: bool,
    }
    // GCP：合法资源路径、非法 path、缺凭据。
    let tests = [
        Case {
            name: "Valid GCP config",
            // KeyId 归一为完整 projects/.../cryptoKeys/... 资源名。
            input: "gcp-kms:///projects/project-id/locations/global/keyRings/ring-name/cryptoKeys/key-name?CREDENTIALS=credentials",
            expected: Some(MasterKey {
                Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
                    Vendor: "gcp".into(),
                    KeyId: "projects/project-id/locations/global/keyRings/ring-name/cryptoKeys/key-name".into(),
                    GcpKms: Some(GcpKms {
                        Credential: "credentials".into(),
                    }),
                    ..Default::default()
                })),
            }),
            // Credential 与完整 KeyId。
            expect_error: false,
        },
        Case {
            name: "Invalid path format",
            // 非四段资源路径应失败。
            input: "gcp-kms:///invalid/path?CREDENTIALS=credentials",
            expected: None,
            // path 不符合四段资源名。
            expect_error: true,
        },
        Case {
            name: "Missing credentials",
            // 缺 CREDENTIALS query。
            input: "gcp-kms:///projects/project-id/locations/global/keyRings/ring-name/cryptoKeys/key-name",
            expected: None,
            // 无 CREDENTIALS query。
            expect_error: true,
        },
    ];

    // GCP 用例循环。
    for tt in tests {
        let u = Url::parse(tt.input).expect("parse gcp kms url");
        let result = parseGcpKmsConfig(&u);
        if tt.expect_error {
            assert!(result.is_err(), "{}", tt.name);
        } else {
            assert_eq!(result.unwrap(), tt.expected.unwrap(), "{}", tt.name);
        }
    }
}
