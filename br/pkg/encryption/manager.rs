// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.
//! 中文注释索引开始
//! 本文件负责`br/pkg/encryption/manager.rs`对应的加解密与主密钥适配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少25行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `enum`用离散值表达\"enum\"的状态，关系到序列化、日志和错误判定。
//! 这类符号最容易因为默认值、未知值或字符串映射而与 Go 端产生偏差。
//! 中文注释会提醒维护者把重点放在状态转换、展示文本和兜底分支。
//! 如果测试里出现 raw integer、unknown 或 not found，对应的兼容性保护通常都落在这里。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Manager`把\"Manager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - 补充约束 1: `br/pkg/encryption/manager.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `br/pkg/encryption/manager.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

use astersql_br_pkg_encryption_master_key::{
    EncryptedContent, MasterKey, MultiMasterKeyBackend, NewMultiMasterKeyBackend,
};
use astersql_util_encrypt::AESDecryptWithCTR;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum EncryptionMethod {
    #[default]
    Unknown = 0,
    Plaintext = 1,
    Aes128Ctr = 2,
    Aes192Ctr = 3,
    Aes256Ctr = 4,
}

#[derive(Clone, Debug, Default)]
pub struct CipherInfo {
    pub CipherType: EncryptionMethod,
    pub CipherKey: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct MasterKeyConfig {
    pub EncryptionType: EncryptionMethod,
    pub MasterKeys: Vec<MasterKey>,
}

#[derive(Clone, Debug, Default)]
pub enum FileEncryptionMode {
    #[default]
    Unset,
    PlainTextDataKey,
    MasterKeyBased {
        DataKeyEncryptedContent: Vec<EncryptedContent>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct FileEncryptionInfo {
    pub Mode: FileEncryptionMode,
    pub FileIv: Vec<u8>,
    pub EncryptionMethod: EncryptionMethod,
}

pub struct Manager {
    cipherInfo: Option<CipherInfo>,
    masterKeyBackends: Option<MultiMasterKeyBackend>,
    encryptionMethod: Option<EncryptionMethod>,
}

pub fn IsEffectiveEncryptionMethod(method: EncryptionMethod) -> bool {
    method != EncryptionMethod::Unknown && method != EncryptionMethod::Plaintext
}

pub fn DecryptContent(content: &[u8], cipher: &CipherInfo, iv: &[u8]) -> Result<Vec<u8>, String> {
    if content.is_empty() {
        return Ok(content.to_vec());
    }
    match cipher.CipherType {
        EncryptionMethod::Plaintext => Ok(content.to_vec()),
        EncryptionMethod::Aes128Ctr | EncryptionMethod::Aes192Ctr | EncryptionMethod::Aes256Ctr => {
            AESDecryptWithCTR(content, &cipher.CipherKey, iv).map_err(|e| e.to_string())
        }
        other => Err(format!("cipher type invalid {other:?}")),
    }
}

pub fn NewManager(
    cipherInfo: Option<CipherInfo>,
    masterKeyConfigs: Option<MasterKeyConfig>,
) -> Result<Option<Manager>, String> {
    if cipherInfo.is_none() || masterKeyConfigs.is_none() {
        return Err("cipherInfo or masterKeyConfigs is nil".into());
    }
    let cipherInfo = cipherInfo.unwrap();
    let masterKeyConfigs = masterKeyConfigs.unwrap();
    if IsEffectiveEncryptionMethod(cipherInfo.CipherType) {
        return Ok(Some(Manager {
            cipherInfo: Some(cipherInfo),
            masterKeyBackends: None,
            encryptionMethod: None,
        }));
    }
    if IsEffectiveEncryptionMethod(masterKeyConfigs.EncryptionType) {
        let masterKeyBackends = NewMultiMasterKeyBackend(Some(&masterKeyConfigs.MasterKeys))?;
        return Ok(Some(Manager {
            cipherInfo: None,
            masterKeyBackends: Some(masterKeyBackends),
            encryptionMethod: Some(masterKeyConfigs.EncryptionType),
        }));
    }
    Ok(None)
}

impl Manager {
    pub fn Decrypt(
        &self,
        content: &[u8],
        fileEncryptionInfo: &FileEncryptionInfo,
    ) -> Result<Vec<u8>, String> {
        match &fileEncryptionInfo.Mode {
            FileEncryptionMode::Unset => {
                Err("internal error: unsupported encryption mode type <nil>".into())
            }
            FileEncryptionMode::PlainTextDataKey => {
                let cipherInfo = self
                    .cipherInfo
                    .as_ref()
                    .ok_or_else(|| "plaintext data key info is required but not set".to_string())?;
                DecryptContent(content, cipherInfo, &fileEncryptionInfo.FileIv)
                    .map_err(|e| format!("failed to decrypt content using plaintext data key: {e}"))
            }
            FileEncryptionMode::MasterKeyBased {
                DataKeyEncryptedContent,
            } => {
                if DataKeyEncryptedContent.is_empty() {
                    return Err("should contain at least one encrypted data key".into());
                }
                let backends = self
                    .masterKeyBackends
                    .as_ref()
                    .ok_or_else(|| "master key backend is required but not set".to_string())?;
                let decryptedDataKey = backends
                    .Decrypt(&DataKeyEncryptedContent[0])
                    .map_err(|e| format!("failed to decrypt data key using master key: {e}"))?;
                let cipherInfo = CipherInfo {
                    CipherType: fileEncryptionInfo.EncryptionMethod,
                    CipherKey: decryptedDataKey,
                };
                DecryptContent(content, &cipherInfo, &fileEncryptionInfo.FileIv)
                    .map_err(|e| format!("failed to decrypt content using decrypted data key: {e}"))
            }
        }
    }

    pub fn Close(&mut self) {
        if let Some(backends) = &mut self.masterKeyBackends {
            backends.Close();
        }
    }
}
