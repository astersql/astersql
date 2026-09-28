// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! GCP KMS backend matching `br/pkg/kms/gcp.go`.
//
// 本模块实现 GCP Cloud KMS 解密后端，语义对齐 Go `br/pkg/kms/gcp.go`。
// 依赖注入的 `GcpDecryptClient`；构造时解析 KeyId 路径并在解密路径做 CRC32C 完整性校验。

use crate::kms::{Context, Provider};
use crate::stubs::{GcpDecryptClient, MasterKeyKms};
use std::sync::Arc;

/// Must match TiKV STORAGE_VENDOR_NAME_GCP.
// 厂商名必须与 TiKV STORAGE_VENDOR_NAME_GCP 一致（小写 "gcp"）。
pub const StorageVendorNameGcp: &str = "gcp";

// 保存完整 MasterKeyKms、解析出的 location 前缀，以及可替换的解密客户端。
pub struct GcpKms<C: GcpDecryptClient> {
    pub config: MasterKeyKms,
    /// location prefix: projects/{project}/locations/{location}
    // KeyId 前四段构成 location，供后续资源路径拼装/校验。
    pub location: String,
    pub client: C,
}

// 要求 GcpKms 配置存在；规范化 KeyId（去尾斜杠）并至少含 projects/p/locations/l 四段。
pub fn NewGcpKmsWithClient<C: GcpDecryptClient>(
    mut config: MasterKeyKms,
    client: C,
) -> Result<GcpKms<C>, String> {
    // 缺少 GcpKms 配置块时直接失败，与 Go NewGcpKms 前置检查一致。
    if config.GcpKms.is_none() {
        return Err("GCP config is missing".into());
    }
    // Go strings.TrimSuffix 只去掉一个尾部 '/'。
    if let Some(trimmed) = config.KeyId.strip_suffix('/') {
        config.KeyId = trimmed.to_string();
    }
    let parts: Vec<&str> = config.KeyId.split('/').collect();
    // 少于四段无法形成合法 resource name，拒绝进入解密流程。
    if parts.len() < 4 {
        return Err(format!("invalid GCP key id: {}", config.KeyId));
    }
    let location = parts[..4].join("/");
    Ok(GcpKms {
        config,
        location,
        client,
    })
}

/// Production Google Cloud KMS adapter backed by Application Default
/// Credentials or an explicitly configured Google credential JSON file.
pub struct GcpSdkClient {
    client: google_cloud_kms_v1::client::KeyManagementService,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl GcpDecryptClient for GcpSdkClient {
    fn Decrypt(
        &self,
        ctx: &Context,
        name: &str,
        ciphertext: &[u8],
        ciphertext_crc32c: i64,
    ) -> Result<crate::stubs::GcpDecryptResponse, String> {
        let response = self.runtime.block_on(async {
            tokio::select! {
                response = self.client
                    .decrypt()
                    .set_name(name)
                    .set_ciphertext(bytes::Bytes::copy_from_slice(ciphertext))
                    .set_ciphertext_crc32c(ciphertext_crc32c)
                    .send() => response.map_err(|e| e.to_string()),
                _ = ctx.token().cancelled() => Err("context canceled".into()),
            }
        })?;
        Ok(crate::stubs::GcpDecryptResponse {
            Plaintext: response.plaintext.to_vec(),
            PlaintextCrc32C: response.plaintext_crc32c.unwrap_or_default(),
        })
    }

    fn Close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// Construct the production GCP KMS provider.
pub fn NewGcpKms(config: &MasterKeyKms) -> Result<GcpKms<GcpSdkClient>, String> {
    if config.GcpKms.is_none() {
        return Err("GCP config is missing".into());
    }
    let runtime = Arc::new(
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to create GCP KMS client: {e}"))?,
    );
    let mut builder = google_cloud_kms_v1::client::KeyManagementService::builder();
    if let Some(path) = config
        .GcpKms
        .as_ref()
        .map(|gcp| gcp.Credential.as_str())
        .filter(|path| !path.is_empty())
    {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to create GCP KMS client: {e}"))?;
        let value: serde_json::Value = serde_json::from_str(json.as_str())
            .map_err(|e| format!("failed to create GCP KMS client: {e}"))?;
        let credential_type = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                "failed to create GCP KMS client: credential type is missing".to_string()
            })?;
        let credentials = match credential_type {
            "service_account" => {
                google_cloud_auth::credentials::service_account::Builder::new(value).build()
            }
            "authorized_user" => {
                google_cloud_auth::credentials::user_account::Builder::new(value).build()
            }
            "external_account" => {
                google_cloud_auth::credentials::external_account::Builder::new(value).build()
            }
            "impersonated_service_account" => {
                google_cloud_auth::credentials::impersonated::Builder::new(value).build()
            }
            other => {
                return Err(format!(
                    "failed to create GCP KMS client: unsupported credential type {other}"
                ));
            }
        }
        .map_err(|e| format!("failed to create GCP KMS client: {e}"))?;
        builder = builder.with_credentials(credentials);
    }
    let client = runtime
        .block_on(builder.build())
        .map_err(|e| format!("failed to create GCP KMS client: {e}"))?;
    NewGcpKmsWithClient(config.clone(), GcpSdkClient { client, runtime })
}

impl<C: GcpDecryptClient> GcpKms<C> {
    // 返回固定厂商名，供加密元数据与 TiKV 侧交叉校验。
    pub fn Name(&self) -> &'static str {
        StorageVendorNameGcp
    }

    // 请求携带密文 CRC；响应明文 CRC 必须与本地计算一致，否则判定传输损坏。
    pub fn DecryptDataKey(&self, dataKey: &[u8]) -> Result<Vec<u8>, String> {
        self.DecryptDataKeyWithContext(&Context::default(), dataKey)
    }

    pub fn DecryptDataKeyWithContext(
        &self,
        ctx: &Context,
        dataKey: &[u8],
    ) -> Result<Vec<u8>, String> {
        // 先算请求侧 CRC，再调用注入客户端；网络错误包装为统一文案。
        let crc = self.calculateCRC32C(dataKey) as i64;
        let resp = self
            .client
            .Decrypt(ctx, &self.config.KeyId, dataKey, crc)
            .map_err(|e| format!("gcp kms decrypt request failed: {e}"))?;
        // 与 Go 相同：明文 CRC 不匹配即拒绝返回，防止静默使用损坏密钥。
        if self.calculateCRC32C(&resp.Plaintext) as i64 != resp.PlaintextCrc32C {
            return Err("response corrupted in-transit".into());
        }
        Ok(resp.Plaintext)
    }

    // 独立 CRC 校验入口，供测试与潜在请求侧复用。
    pub fn checkCRC32(&self, data: &[u8], expected: i64) -> Result<(), String> {
        let crc = self.calculateCRC32C(data) as i64;
        if crc != expected {
            return Err(format!("crc32c mismatch, expected: {expected}, got: {crc}"));
        }
        Ok(())
    }

    // 薄封装，便于方法调用风格与 Go 接收者方法对齐。
    pub fn calculateCRC32C(&self, data: &[u8]) -> u32 {
        crc32c(data)
    }

    // 关闭底层客户端；与 Go 一样不向调用方返回错误，但必须记录关闭失败。
    pub fn Close(&mut self) {
        if let Err(error) = self.client.Close() {
            eprintln!("failed to close gcp kms client: {error}");
        }
    }
}

impl<C: GcpDecryptClient> Provider for GcpKms<C> {
    // Provider 适配：委托具体方法，保持 trait 对象可调度。
    fn DecryptDataKey(&self, ctx: &Context, dataKey: &[u8]) -> Result<Vec<u8>, String> {
        GcpKms::DecryptDataKeyWithContext(self, ctx, dataKey)
    }
    fn Name(&self) -> &str {
        StorageVendorNameGcp
    }
    fn Close(&mut self) {
        GcpKms::Close(self);
    }
}

/// Castagnoli CRC32C matching Go hash/crc32.MakeTable(crc32.Castagnoli).
// 位反射 Castagnoli 多项式实现，结果需与 Go hash/crc32.Castagnoli 一致。
// 初始值全 1、最终取反，是 CRC32C 标准约定，不可改成 IEEE 多项式。
pub fn crc32c(data: &[u8]) -> u32 {
    const POLY: u32 = 0x82F6_3B78; // reflected Castagnoli
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}
