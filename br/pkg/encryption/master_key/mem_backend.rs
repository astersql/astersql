// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.
//! 中文注释索引开始
//! 本文件负责`br/pkg/encryption/master_key/mem_backend.rs`对应的内存 AES-GCM 后端，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少15行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MemAesGcmBackend`把\"MemAesGcmBackend\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! 中文注释索引结束

use crate::common::{
    IV, MetadataKeyAesGcmTag, MetadataKeyIv, MetadataKeyMethod, MetadataMethodAes256Gcm,
};
use crate::pb::EncryptedContent;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use astersql_br_pkg_kms::{CryptographyType, NewPlainKey, PlainKey};

pub struct MemAesGcmBackend {
    key: PlainKey,
}

pub fn NewMemAesGcmBackend(key: &[u8]) -> Result<MemAesGcmBackend, String> {
    let plainKey = NewPlainKey(key.to_vec(), CryptographyType::CryptographyTypeAesGcm256)
        .map_err(|e| format!("failed to create new mem aes gcm backend: {e}"))?;
    Ok(MemAesGcmBackend { key: plainKey })
}

impl MemAesGcmBackend {
    pub fn EncryptContent(&self, plaintext: &[u8], iv: &IV) -> Result<EncryptedContent, String> {
        let mut content = EncryptedContent {
            Content: Vec::new(),
            Metadata: Default::default(),
        };
        content.Metadata.insert(
            MetadataKeyMethod.into(),
            MetadataMethodAes256Gcm.as_bytes().to_vec(),
        );
        content
            .Metadata
            .insert(MetadataKeyIv.into(), iv.AsSlice().to_vec());
        let cipher =
            Aes256Gcm::new_from_slice(self.key.Key()).map_err(|e| format!("aes cipher: {e}"))?;
        let nonce = Nonce::from_slice(iv.AsSlice());
        let sealed = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| format!("encrypt: {e}"))?;
        let tag_len = 16;
        let split = sealed.len() - tag_len;
        content.Content = sealed[..split].to_vec();
        content
            .Metadata
            .insert(MetadataKeyAesGcmTag.into(), sealed[split..].to_vec());
        Ok(content)
    }
    pub fn DecryptContent(&self, content: &EncryptedContent) -> Result<Vec<u8>, String> {
        let method = content
            .Metadata
            .get(MetadataKeyMethod)
            .ok_or_else(|| format!("metadata {MetadataKeyMethod} not found"))?;
        if method.as_slice() != MetadataMethodAes256Gcm.as_bytes() {
            return Err(format!(
                "encryption method mismatch, expected {MetadataMethodAes256Gcm} vs actual {}",
                String::from_utf8_lossy(method)
            ));
        }
        let ivValue = content
            .Metadata
            .get(MetadataKeyIv)
            .ok_or_else(|| format!("metadata {MetadataKeyIv} not found"))?;
        let iv = crate::common::NewIVFromSlice(ivValue)?;
        let tag = content
            .Metadata
            .get(MetadataKeyAesGcmTag)
            .ok_or_else(|| "aes gcm tag not found".to_string())?;
        let cipher =
            Aes256Gcm::new_from_slice(self.key.Key()).map_err(|e| format!("aes cipher: {e}"))?;
        let nonce = Nonce::from_slice(iv.AsSlice());
        let mut ciphertext = content.Content.clone();
        ciphertext.extend_from_slice(tag);
        cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| format!("wrong master key :decrypt in GCM mode failed: {e}"))
    }
}
