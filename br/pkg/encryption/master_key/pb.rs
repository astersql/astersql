// Copyright 2026 AsterSQL.
//! 主密钥相关 protobuf 消息的 Rust 轻量镜像（对应 `encryptionpb`）。
//!
//! Go 侧通过 `encryptionpb.EncryptedContent` / `MasterKey*` 传递密文与后端配置；
//! 本文件用字段名保持 PascalCase，投影出该包实际消费的字段，供 file/KMS/mem 后端共用。
//! 不负责编解码线格式；未被本包消费的 protobuf 兼容字段仍由 Go 生成类型承载。

use std::collections::HashMap;

/// 已加密内容：密文字节 + 可选元数据（如 IV、密钥 ID），对齐 `encryptionpb.EncryptedContent`。
#[derive(Clone, Debug, Default)]
pub struct EncryptedContent {
    pub Content: Vec<u8>,
    pub Metadata: HashMap<String, Vec<u8>>,
}

/// 文件型主密钥：路径指向本地密钥材料文件，对齐 `encryptionpb.MasterKeyFile`。
#[derive(Clone, Debug, Default)]
pub struct MasterKeyFile {
    pub Path: String,
}

/// KMS 型主密钥：标识云厂商密钥及区域/端点，对齐 `encryptionpb.MasterKeyKms`。
#[derive(Clone, Debug, Default)]
pub struct MasterKeyKms {
    pub KeyId: String,
    pub Region: String,
    pub Endpoint: String,
    pub Vendor: String,
    pub AwsKms: Option<astersql_br_pkg_kms::AwsKmsConfig>,
    pub GcpKms: Option<astersql_br_pkg_kms::GcpKmsConfig>,
}

/// 主密钥后端种类：明文 / 本地文件 / KMS，对应 Go oneof `MasterKey.backend`。
#[derive(Clone, Debug, Default)]
pub enum MasterKeyBackend {
    #[default]
    Unset,
    Plaintext,
    File(MasterKeyFile),
    Kms(MasterKeyKms),
}

/// 主密钥配置入口：仅包装后端选择，对齐 `encryptionpb.MasterKey`。
#[derive(Clone, Debug, Default)]
pub struct MasterKey {
    pub Backend: MasterKeyBackend,
}
