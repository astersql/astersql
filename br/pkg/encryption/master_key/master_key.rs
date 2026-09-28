// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.
// Copyright 2026 AsterSQL.
//! 中文注释索引开始
//! 本文件负责`br/pkg/encryption/master_key/master_key.rs`对应的主密钥后端装配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少12行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `Decrypt`是当前文件的重要函数，承担\"Decrypt\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `enum`用离散值表达\"enum\"的状态，关系到序列化、日志和错误判定。
//! 这类符号最容易因为默认值、未知值或字符串映射而与 Go 端产生偏差。
//! 中文注释会提醒维护者把重点放在状态转换、展示文本和兜底分支。
//! 如果测试里出现 raw integer、unknown 或 not found，对应的兼容性保护通常都落在这里。
//! - `impl Backend`把\"Backend\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! 中文注释索引结束

use crate::file_backend::{FileBackend, createFileBackend};
use crate::kms_backend::{KmsBackend, NewKmsBackend};
use crate::pb::{EncryptedContent, MasterKey, MasterKeyBackend, MasterKeyKms};
use astersql_br_pkg_kms::{MasterKeyKms as KmsConfig, NewAwsKms, NewGcpKms, Provider};

pub const StorageVendorNameAWS: &str = "aws";
pub const StorageVendorNameAzure: &str = "azure";
pub const StorageVendorNameGCP: &str = "gcp";

pub trait Backend {
    fn Decrypt(&self, ciphertext: &EncryptedContent) -> Result<Vec<u8>, String>;
    fn Close(&mut self);
}

pub enum AnyBackend {
    File(FileBackend),
    Kms(KmsBackend),
}

impl Backend for AnyBackend {
    fn Decrypt(&self, ciphertext: &EncryptedContent) -> Result<Vec<u8>, String> {
        match self {
            AnyBackend::File(f) => f.Decrypt(ciphertext),
            AnyBackend::Kms(k) => k.Decrypt(ciphertext),
        }
    }
    fn Close(&mut self) {
        match self {
            AnyBackend::File(f) => f.Close(),
            AnyBackend::Kms(k) => k.Close(),
        }
    }
}

pub fn CreateBackend(config: Option<&MasterKey>) -> Result<AnyBackend, String> {
    let Some(config) = config else {
        return Err("master key config is nil".into());
    };
    match &config.Backend {
        MasterKeyBackend::Unset => Err("unknown master key backend type".into()),
        MasterKeyBackend::Plaintext => Err("should not create plaintext master key".into()),
        MasterKeyBackend::File(file) => Ok(AnyBackend::File(
            createFileBackend(&file.Path).map_err(|e| format!("master key config is nil: {e}"))?,
        )),
        MasterKeyBackend::Kms(kms) => createCloudBackend(kms),
    }
}

pub fn createCloudBackend(config: &MasterKeyKms) -> Result<AnyBackend, String> {
    let provider_config = KmsConfig {
        KeyId: config.KeyId.clone(),
        Region: config.Region.clone(),
        Endpoint: config.Endpoint.clone(),
        AwsKms: config.AwsKms.clone(),
        GcpKms: config.GcpKms.clone(),
    };
    match config.Vendor.as_str() {
        StorageVendorNameAWS => CreateKmsBackendWithProvider(Box::new(
            NewAwsKms(&provider_config).map_err(|e| format!("new AWS KMS: {e}"))?,
        )),
        StorageVendorNameAzure => Err("not implemented Azure KMS".into()),
        StorageVendorNameGCP => CreateKmsBackendWithProvider(Box::new(
            NewGcpKms(&provider_config).map_err(|e| format!("new GCP KMS: {e}"))?,
        )),
        other => Err(format!("vendor not found: {other}")),
    }
}

pub fn CreateKmsBackendWithProvider(
    provider: Box<dyn Provider + Send>,
) -> Result<AnyBackend, String> {
    Ok(AnyBackend::Kms(NewKmsBackend(provider)?))
}
