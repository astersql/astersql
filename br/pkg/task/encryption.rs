// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under the Apache License, Version 2.0.

//! Master-key URL parsing matching `br/pkg/task/encryption.go`.
//!
//! 将 `--master-key` URL 解析为 `encryptionpb::MasterKey`，供 common 配置装配。
//! 支持 scheme：`local` / `aws-kms` / `azure-kms` / `gcp-kms`，与 Go 校验规则一一对应。
//! 敏感凭据来自 query；缺必填项或路径不合规则立即失败，避免半配置进入备份流程。
//! 多主密钥由 common 用逗号拆分后逐条调用本入口，本文件只负责单条 URL。
//! 明文数据密钥与 master-key 互斥校验不在此文件，而在 `parseAndValidateMasterKeyInfo`。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::stubs::encryptionpb::{
    AwsKms, AzureKms, GcpKms, MasterKey, MasterKeyBackend, MasterKeyFile, MasterKeyKms,
};
use crate::stubs::{Error, Result};

/// local 盘主密钥 scheme；path 必须是绝对路径（无 host）。
pub const SchemeLocal: &str = "local";
/// AWS KMS scheme。
pub const SchemeAWS: &str = "aws-kms";
/// Azure Key Vault KMS scheme。
pub const SchemeAzure: &str = "azure-kms";
/// GCP Cloud KMS scheme。
pub const SchemeGCP: &str = "gcp-kms";

/// 写入 MasterKeyKms.Vendor 的 AWS 标识。
pub const AWSVendor: &str = "aws";
/// AWS 区域 query 键；缺失则解析失败。
pub const AWSRegion: &str = "REGION";
/// 可选自定义 endpoint（私有化/兼容实现）。
pub const AWSEndpoint: &str = "ENDPOINT";
/// 显式 AK；与 Secret 必须成对。
pub const AWSAccessKeyId: &str = "AWS_ACCESS_KEY_ID";
/// 显式 SK；缺一即报错。
pub const AWSSecretKey: &str = "AWS_SECRET_ACCESS_KEY";

/// Azure vendor 字符串。
pub const AzureVendor: &str = "azure";
/// AAD 租户 ID。
pub const AzureTenantID: &str = "AZURE_TENANT_ID";
/// AAD 应用 client id。
pub const AzureClientID: &str = "AZURE_CLIENT_ID";
/// AAD client secret。
pub const AzureClientSecret: &str = "AZURE_CLIENT_SECRET";
/// query 名沿用 Go：值为 vault 名/URL，填入 AzureKms.KeyVaultUrl。
pub const AzureVaultName: &str = "AZURE_VAULT_NAME";

/// GCP vendor 字符串。
pub const GCPVendor: &str = "gcp";
/// GCP 服务账号凭据 JSON/路径的 query 键。
pub const GCPCredentials: &str = "CREDENTIALS";

// path 仅允许单段 key id：`/key-id`。
static AWS_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/([^/]+)$").unwrap());
// Azure 允许 `key-name/key-version` 等多段 path。
static AZURE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/(.+)$").unwrap());
// GCP 资源名四段：projects/locations/keyRings/cryptoKeys。
static GCP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/projects/([^/]+)/locations/([^/]+)/keyRings/([^/]+)/cryptoKeys/([^/]+)/?$")
        .unwrap()
});

/// 入口：按 URL scheme 分发到各云解析器；未知 scheme 报 unsupported。
pub fn validateAndParseMasterKeyString(keyString: &str) -> Result<MasterKey> {
    let u = Url::parse(keyString).map_err(|e| Error::Trace(Error::new(e.to_string())))?;
    match u.scheme() {
        SchemeLocal => parseLocalDiskConfig(&u),
        SchemeAWS => parseAwsKmsConfig(&u),
        SchemeAzure => parseAzureKmsConfig(&u),
        SchemeGCP => parseGcpKmsConfig(&u),
        scheme => Err(Error::Errorf(format!(
            "unsupported master key type: {scheme}"
        ))),
    }
}

/// `local:///abs/path`：有 host 视为相对/非绝对路径，与 Go 一样拒绝。
pub fn parseLocalDiskConfig(u: &Url) -> Result<MasterKey> {
    if u.host_str().is_some_and(|host| !host.is_empty()) {
        return Err(Error::new("local master key path must be absolute"));
    }
    Ok(MasterKey {
        Backend: Some(MasterKeyBackend::File(MasterKeyFile {
            Path: u.path().to_owned(),
        })),
    })
}

/// AWS：强制 REGION；AK/SK 必须成对出现，或全部省略（走默认链）。
pub fn parseAwsKmsConfig(u: &Url) -> Result<MasterKey> {
    let matches = AWS_REGEX
        .captures(u.path())
        .ok_or_else(|| Error::new("invalid AWS KMS key ID format"))?;
    let keyID = matches.get(1).map(|m| m.as_str()).unwrap_or_default();

    let q: HashMap<String, String> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let region = q.get(AWSRegion).map(|s| s.as_str()).unwrap_or_default();
    let accessKey = q
        .get(AWSAccessKeyId)
        .map(|s| s.as_str())
        .unwrap_or_default();
    let secretAccessKey = q.get(AWSSecretKey).map(|s| s.as_str()).unwrap_or_default();

    if region.is_empty() {
        return Err(Error::new("missing AWS KMS region info"));
    }

    // 只给一侧密钥会静默落到默认凭据链，故显式报错。
    let awsKms = if !accessKey.is_empty() && !secretAccessKey.is_empty() {
        Some(AwsKms {
            AccessKey: accessKey.to_owned(),
            SecretAccessKey: secretAccessKey.to_owned(),
        })
    } else if !accessKey.is_empty() || !secretAccessKey.is_empty() {
        return Err(Error::new(
            "missing AWS KMS(access key or secret access key)",
        ));
    } else {
        None
    };

    Ok(MasterKey {
        Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
            Vendor: AWSVendor.to_owned(),
            KeyId: keyID.to_owned(),
            Region: region.to_owned(),
            Endpoint: q.get(AWSEndpoint).cloned().unwrap_or_default(),
            AwsKms: awsKms,
            ..Default::default()
        })),
    })
}

/// Azure：tenant/client/secret/vault 四项均必填。
pub fn parseAzureKmsConfig(u: &Url) -> Result<MasterKey> {
    let matches = AZURE_REGEX
        .captures(u.path())
        .ok_or_else(|| Error::new("invalid Azure KMS path format"))?;
    let keyID = matches.get(1).map(|m| m.as_str()).unwrap_or_default();
    let q: HashMap<String, String> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let azureKms = AzureKms {
        TenantId: q.get(AzureTenantID).cloned().unwrap_or_default(),
        ClientId: q.get(AzureClientID).cloned().unwrap_or_default(),
        ClientSecret: q.get(AzureClientSecret).cloned().unwrap_or_default(),
        KeyVaultUrl: q.get(AzureVaultName).cloned().unwrap_or_default(),
    };

    if azureKms.TenantId.is_empty()
        || azureKms.ClientId.is_empty()
        || azureKms.ClientSecret.is_empty()
        || azureKms.KeyVaultUrl.is_empty()
    {
        return Err(Error::new("missing required Azure KMS parameters"));
    }

    Ok(MasterKey {
        Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
            Vendor: AzureVendor.to_owned(),
            KeyId: keyID.to_owned(),
            AzureKms: Some(azureKms),
            ..Default::default()
        })),
    })
}

/// GCP：path 拆成标准资源名写入 KeyId；CREDENTIALS 必填。
pub fn parseGcpKmsConfig(u: &Url) -> Result<MasterKey> {
    let matches = GCP_REGEX
        .captures(u.path())
        .ok_or_else(|| Error::new("invalid GCP KMS path format"))?;
    let projectID = matches.get(1).map(|m| m.as_str()).unwrap_or_default();
    let location = matches.get(2).map(|m| m.as_str()).unwrap_or_default();
    let keyRing = matches.get(3).map(|m| m.as_str()).unwrap_or_default();
    let keyName = matches.get(4).map(|m| m.as_str()).unwrap_or_default();
    let q: HashMap<String, String> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let credential = q
        .get(GCPCredentials)
        .map(|s| s.as_str())
        .unwrap_or_default();
    // 无凭据则无法调 GCP API，与 Go 同步拒绝。
    if credential.is_empty() {
        return Err(Error::new("missing credential"));
    }

    // KeyId 归一为完整资源路径，与 GCP API 期望一致。
    Ok(MasterKey {
        Backend: Some(MasterKeyBackend::Kms(MasterKeyKms {
            Vendor: GCPVendor.to_owned(),
            KeyId: format!(
                "projects/{projectID}/locations/{location}/keyRings/{keyRing}/cryptoKeys/{keyName}"
            ),
            GcpKms: Some(GcpKms {
                Credential: credential.to_owned(),
            }),
            ..Default::default()
        })),
    })
}
