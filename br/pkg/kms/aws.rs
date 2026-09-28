// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! AWS KMS backend matching `br/pkg/kms/aws.go`.
//
// 本模块实现 AWS KMS 主密钥解密后端，语义对齐 Go `br/pkg/kms/aws.go`。
// 真实 AWS SDK 由调用方注入 `AwsDecryptClient`；本文件只做密钥元数据持有与错误分类。

use crate::kms::{Context, Provider};
use crate::stubs::{AwsDecryptClient, AwsDecryptError, MasterKeyKms};
use aws_sdk_kms::error::ProvideErrorMetadata;
use std::sync::Arc;

/// Must match TiKV ENCRYPTION_VENDOR_NAME_AWS_KMS.
// 厂商名必须与 TiKV 侧 ENCRYPTION_VENDOR_NAME_AWS_KMS 字面量一致，供加密元数据比对。
pub const EncryptionVendorNameAwsKms: &str = "AWS";

// 持有注入的解密客户端与主密钥定位信息（KeyId/Region/Endpoint）。
pub struct AwsKms<C: AwsDecryptClient> {
    pub client: C,
    pub currentKeyID: String,
    pub region: String,
    pub endpoint: String,
}

/// Build AwsKms with an injected decrypt client (real AWS SDK wired by callers).
// 构造时仅拷贝 MasterKeyKms 字段；凭证校验留给 SDK 默认链，对齐 Go 宽松策略。
pub fn NewAwsKmsWithClient<C: AwsDecryptClient>(
    masterKeyConfig: &MasterKeyKms,
    client: C,
) -> Result<AwsKms<C>, String> {
    // Mirror Go: optional static credentials only when both access+secret are set.
    // Incomplete pairs fall through to default credential chain (no error here).
    // Go：仅当 access+secret 齐全才用静态凭证；残缺对不报错，走默认凭证链。
    let _ = &masterKeyConfig.AwsKms;
    Ok(AwsKms {
        client,
        currentKeyID: masterKeyConfig.KeyId.clone(),
        region: masterKeyConfig.Region.clone(),
        endpoint: masterKeyConfig.Endpoint.clone(),
    })
}

/// Production AWS SDK adapter. The runtime is retained because the public
/// KMS boundary is synchronous, matching the Go interface.
pub struct AwsSdkClient {
    client: aws_sdk_kms::Client,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl AwsDecryptClient for AwsSdkClient {
    fn Decrypt(
        &self,
        ctx: &Context,
        ciphertext: &[u8],
        key_id: &str,
    ) -> Result<Vec<u8>, AwsDecryptError> {
        let result = self.runtime.block_on(async {
            tokio::select! {
                result = self.client
                .decrypt()
                .ciphertext_blob(aws_sdk_kms::primitives::Blob::new(ciphertext))
                .key_id(key_id)
                .send() => result.map_err(|error| AwsDecryptError {
                    code: error.as_service_error().and_then(|service| service.code()).unwrap_or("KMS error").to_string(),
                    message: error.to_string(),
                }),
                _ = ctx.token().cancelled() => Err(AwsDecryptError {
                    code: "KMS error".into(),
                    message: "context canceled".into(),
                }),
            }
        });
        result.map(|output| output.plaintext.unwrap_or_default().into_inner())
    }
}

/// Construct the production AWS KMS provider using the default credential
/// chain, with the same static-credential and endpoint overrides as Go.
pub fn NewAwsKms(masterKeyConfig: &MasterKeyKms) -> Result<AwsKms<AwsSdkClient>, String> {
    let runtime = Arc::new(
        tokio::runtime::Runtime::new().map_err(|e| format!("failed to load AWS config: {e}"))?,
    );
    // Use portable compiled-in roots while retaining normal certificate validation.
    // `https_or_http` also preserves Go-compatible custom HTTP endpoint support.
    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let http_client =
        aws_smithy_http_client::hyper_014::HyperClientBuilder::new().build(https_connector);
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_types::region::Region::new(
            masterKeyConfig.Region.clone(),
        ))
        .http_client(http_client);
    if let Some(credentials) = masterKeyConfig.AwsKms.as_ref().filter(|credentials| {
        !credentials.AccessKey.is_empty() && !credentials.SecretAccessKey.is_empty()
    }) {
        loader = loader.credentials_provider(aws_credential_types::Credentials::new(
            credentials.AccessKey.clone(),
            credentials.SecretAccessKey.clone(),
            None,
            None,
            "master-key-config",
        ));
    }
    let sdk_config = runtime.block_on(loader.load());
    let mut config = aws_sdk_kms::config::Builder::from(&sdk_config);
    if !masterKeyConfig.Endpoint.is_empty() {
        config = config.endpoint_url(masterKeyConfig.Endpoint.clone());
    }
    NewAwsKmsWithClient(
        masterKeyConfig,
        AwsSdkClient {
            client: aws_sdk_kms::Client::from_conf(config.build()),
            runtime,
        },
    )
}

impl<C: AwsDecryptClient> AwsKms<C> {
    pub fn Name(&self) -> &'static str {
        EncryptionVendorNameAwsKms
    }

    // 用 currentKeyID 解密 data key；SDK 错误经 classifyDecryptError 归一化文案。
    pub fn DecryptDataKey(&self, dataKey: &[u8]) -> Result<Vec<u8>, String> {
        self.DecryptDataKeyWithContext(&Context::default(), dataKey)
    }

    pub fn DecryptDataKeyWithContext(
        &self,
        ctx: &Context,
        dataKey: &[u8],
    ) -> Result<Vec<u8>, String> {
        match self.client.Decrypt(ctx, dataKey, &self.currentKeyID) {
            Ok(pt) => Ok(pt),
            Err(err) => Err(classifyDecryptError(&err)),
        }
    }

    // AWS 客户端无显式关闭资源；保留空实现以对齐 Provider/Go Close 签名。
    pub fn Close(&self) {}
}

impl<C: AwsDecryptClient> Provider for AwsKms<C> {
    fn DecryptDataKey(&self, ctx: &Context, dataKey: &[u8]) -> Result<Vec<u8>, String> {
        AwsKms::DecryptDataKeyWithContext(self, ctx, dataKey)
    }
    fn Name(&self) -> &str {
        EncryptionVendorNameAwsKms
    }
    fn Close(&mut self) {}
}

/// classifyDecryptError matches Go AWS SDK error classification.
// 将 AWS SDK 错误码映射为与 Go 相同的前缀，便于上层统一识别主密钥错误/超时。
pub fn classifyDecryptError(err: &AwsDecryptError) -> String {
    match err.code.as_str() {
        // NotFound / InvalidKeyUsage 视为选错主密钥，而非瞬时故障。
        "NotFoundException" | "InvalidKeyUsageException" => {
            format!("wrong master key: {err}")
        }
        // 超时与内部错误单独前缀，便于调用方区分可重试与不可重试。
        "DependencyTimeoutException" => format!("API timeout: {err}"),
        "KMSInternalException" => format!("API internal error: {err}"),
        // The SDK adapter uses this sentinel when no service error code exists.
        // Go annotates the underlying transport/dispatch error only once.
        "KMS error" => format!("KMS error: {}", err.message),
        _ => format!("KMS error: {err}"),
    }
}
