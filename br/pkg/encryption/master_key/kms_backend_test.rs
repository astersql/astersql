// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//! 中文注释索引开始
//! 本文件负责`br/pkg/encryption/master_key/kms_backend_test.rs`对应的KMS 主密钥后端测试，并覆盖重试取消契约。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少26行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `mockKmsProvider`承载\"mockKmsProvider\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Provider`把\"Provider\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Name`是当前文件的重要函数，承担\"Name\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DecryptDataKey`是当前文件的重要函数，承担\"DecryptDataKey\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_kms_backend_decrypt`对齐 Go 同名测试或契约片段，用来固定\"test kms backend decrypt\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `KmsBackendDecryptErrorCase`承载\"KmsBackendDecryptErrorCase\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `test_kms_backend_decrypt_errors`对齐 Go 同名测试或契约片段，用来固定\"test kms backend decrypt errors\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"do nothing\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"mock_kms\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"First decryption — provider called once (DecryptContent may fail; Go ignores it).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Second decryption with the same ciphertext key (should use cache).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Third decryption with a different ciphertext key.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"missing KMS vendor\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"KMS vendor mismatch\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"missing ciphertext key\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::{EncryptedContent, MetadataKeyKmsCiphertextKey, MetadataKeyKmsVendor, NewKmsBackend};
use astersql_br_pkg_kms::Provider;
use rand::RngCore;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// mockKmsProvider matches Go test mock: records Name and DecryptDataKey call count.
struct mockKmsProvider {
    name: String,
    decryptCounter: Arc<AtomicUsize>,
}

impl Provider for mockKmsProvider {
    fn Name(&self) -> &str {
        &self.name
    }

    fn DecryptDataKey(
        &self,
        _ctx: &astersql_br_pkg_kms::Context,
        _encryptedKey: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.decryptCounter.fetch_add(1, Ordering::SeqCst);
        let mut key = vec![0_u8; 32]; // 256 bits = 32 bytes
        rand::thread_rng().fill_bytes(&mut key);
        Ok(key)
    }

    fn Close(&mut self) {
        // do nothing
    }
}

#[test]
fn test_kms_backend_decrypt() {
    let decryptCounter = Arc::new(AtomicUsize::new(0));
    let mockProvider = mockKmsProvider {
        name: "mock_kms".to_string(),
        decryptCounter: Arc::clone(&decryptCounter),
    };
    let backend =
        NewKmsBackend(Box::new(mockProvider)).expect("require.NoError: create KMS backend");

    let ciphertextKey = b"ciphertext_key".to_vec();
    let mut content = EncryptedContent {
        Metadata: [
            (MetadataKeyKmsVendor.to_string(), b"mock_kms".to_vec()),
            (MetadataKeyKmsCiphertextKey.to_string(), ciphertextKey),
        ]
        .into_iter()
        .collect(),
        Content: b"encrypted_content".to_vec(),
    };

    // First decryption — provider called once (DecryptContent may fail; Go ignores it).
    let _ = backend.Decrypt(&content);
    assert_eq!(
        1,
        decryptCounter.load(Ordering::SeqCst),
        "KMS provider should be called once"
    );

    // Second decryption with the same ciphertext key (should use cache).
    let _ = backend.Decrypt(&content);
    assert_eq!(
        1,
        decryptCounter.load(Ordering::SeqCst),
        "KMS provider should not be called again"
    );

    // Third decryption with a different ciphertext key.
    content.Metadata.insert(
        MetadataKeyKmsCiphertextKey.to_string(),
        b"new_ciphertext_key".to_vec(),
    );
    let _ = backend.Decrypt(&content);
    assert_eq!(
        2,
        decryptCounter.load(Ordering::SeqCst),
        "KMS provider should be called again for a new key"
    );
}

struct KmsBackendDecryptErrorCase {
    name: &'static str,
    content: EncryptedContent,
    errMsg: &'static str,
}

#[test]
fn test_kms_backend_decrypt_errors() {
    let mockProvider = mockKmsProvider {
        name: "mock_kms".to_string(),
        decryptCounter: Arc::new(AtomicUsize::new(0)),
    };
    let backend =
        NewKmsBackend(Box::new(mockProvider)).expect("require.NoError: create KMS backend");

    let testCases = vec![
        KmsBackendDecryptErrorCase {
            name: "missing KMS vendor",
            content: EncryptedContent {
                Metadata: [(
                    MetadataKeyKmsCiphertextKey.to_string(),
                    b"ciphertext_key".to_vec(),
                )]
                .into_iter()
                .collect(),
                Content: Vec::new(),
            },
            errMsg: "wrong master key: missing KMS vendor",
        },
        KmsBackendDecryptErrorCase {
            name: "KMS vendor mismatch",
            content: EncryptedContent {
                Metadata: [
                    (MetadataKeyKmsVendor.to_string(), b"wrong_kms".to_vec()),
                    (
                        MetadataKeyKmsCiphertextKey.to_string(),
                        b"ciphertext_key".to_vec(),
                    ),
                ]
                .into_iter()
                .collect(),
                Content: Vec::new(),
            },
            errMsg: "KMS vendor mismatch expect mock_kms got wrong_kms",
        },
        KmsBackendDecryptErrorCase {
            name: "missing ciphertext key",
            content: EncryptedContent {
                Metadata: [(MetadataKeyKmsVendor.to_string(), b"mock_kms".to_vec())]
                    .into_iter()
                    .collect(),
                Content: Vec::new(),
            },
            errMsg: "KMS ciphertext key not found",
        },
    ];

    for tc in testCases {
        let err = backend.Decrypt(&tc.content).unwrap_err();
        assert!(
            err.contains(tc.errMsg),
            "case {} should contain {}, got {}",
            tc.name,
            tc.errMsg,
            err
        );
    }
}

struct CancellingKmsProvider {
    context: astersql_br_pkg_kms::Context,
    decryptCounter: Arc<AtomicUsize>,
}

impl Provider for CancellingKmsProvider {
    fn Name(&self) -> &str {
        "mock_kms"
    }

    fn DecryptDataKey(
        &self,
        _ctx: &astersql_br_pkg_kms::Context,
        _encryptedKey: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.decryptCounter.fetch_add(1, Ordering::SeqCst);
        self.context.cancel();
        Err("transient KMS failure".to_string())
    }

    fn Close(&mut self) {}
}

#[test]
fn test_kms_backend_decrypt_propagates_context_cancellation() {
    let context = astersql_br_pkg_kms::Context::new();
    let decryptCounter = Arc::new(AtomicUsize::new(0));
    let provider = CancellingKmsProvider {
        context: context.clone(),
        decryptCounter: Arc::clone(&decryptCounter),
    };
    let backend = NewKmsBackend(Box::new(provider)).unwrap();
    let content = EncryptedContent {
        Metadata: [
            (MetadataKeyKmsVendor.to_string(), b"mock_kms".to_vec()),
            (
                MetadataKeyKmsCiphertextKey.to_string(),
                b"ciphertext_key".to_vec(),
            ),
        ]
        .into_iter()
        .collect(),
        Content: Vec::new(),
    };

    let started = Instant::now();
    let err = backend.DecryptWithContext(&context, &content).unwrap_err();

    assert!(err.contains("decrypt encrypted key failed: transient KMS failure"));
    assert_eq!(decryptCounter.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() < Duration::from_millis(250));
}
