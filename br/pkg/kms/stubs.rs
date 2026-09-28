// Copyright 2026 AsterSQL.
//! Local stand-ins for encryptionpb MasterKeyKms configs.
//
// 本文件是 KMS 对 protobuf/SDK 的本地替身边界，不是完整云厂商实现。
// 提供配置结构与可注入解密 trait，供 aws/gcp 模块与单测在无真实 SDK 时编译运行。
// 不得将此处桩能力解读为已接入生产 AWS/GCP API。

// 对齐 encryptionpb 中 AWS 静态凭证字段；残缺对由上层/SDK 链处理。
#[derive(Clone, Debug, Default)]
pub struct AwsKmsConfig {
    pub AccessKey: String,
    pub SecretAccessKey: String,
}

// GCP 侧仅暴露凭证文件路径占位，真实加载由未来 SDK 接线完成。
#[derive(Clone, Debug, Default)]
pub struct GcpKmsConfig {
    pub Credential: String,
}

// 主密钥 KMS 配置聚合体，字段名保持与 Go/proto 一致便于对照迁移。
#[derive(Clone, Debug, Default)]
pub struct MasterKeyKms {
    pub KeyId: String,
    pub Region: String,
    pub Endpoint: String,
    pub AwsKms: Option<AwsKmsConfig>,
    pub GcpKms: Option<GcpKmsConfig>,
}

/// Decrypt client trait — production uses AWS SDK; tests inject fakes.
// 生产应接 AWS SDK；测试注入假实现。此处不包含网络或签名逻辑。
pub trait AwsDecryptClient: Send {
    fn Decrypt(
        &self,
        ctx: &crate::kms::Context,
        ciphertext: &[u8],
        key_id: &str,
    ) -> Result<Vec<u8>, AwsDecryptError>;
}

// 携带 SDK 风格 code/message，供 classifyDecryptError 按码分类。
#[derive(Clone, Debug)]
pub struct AwsDecryptError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for AwsDecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// GCP decrypt client trait.
// GCP 解密接口要求传入密文 CRC；Close 允许返回错误供上层记录。
pub trait GcpDecryptClient: Send {
    fn Decrypt(
        &self,
        ctx: &crate::kms::Context,
        name: &str,
        ciphertext: &[u8],
        ciphertext_crc32c: i64,
    ) -> Result<GcpDecryptResponse, String>;
    fn Close(&mut self) -> Result<(), String>;
}

// 响应同时携带明文与服务端 CRC，供调用方做传输完整性校验。
#[derive(Clone, Debug, Default)]
pub struct GcpDecryptResponse {
    pub Plaintext: Vec<u8>,
    pub PlaintextCrc32C: i64,
}
