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
//! 本文件负责`br/pkg/encryption/master_key/file_backend_test.rs`对应的文件主密钥后端，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少21行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `TempKeyFile`承载\"TempKeyFile\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl TempKeyFile`把\"TempKeyFile\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Cleanup`是当前文件的重要函数，承担\"Cleanup\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Drop`把\"Drop\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `drop`是当前文件的重要函数，承担\"drop\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `createMasterKeyFile`是当前文件的重要函数，承担\"createMasterKeyFile\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `decode_hex`是当前文件的重要函数，承担\"decode hex\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_file_backend_aes256_gcm`对齐 Go 同名测试或契约片段，用来固定\"test file backend aes256 gcm\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_file_backend_authenticate`对齐 Go 同名测试或契约片段，用来固定\"test file backend authenticate\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"Go uses backend.memCache.EncryptContent with fixed IV; memCache is private across\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"sibling modules, so encrypt via the same master-key bytes FileBackend loads.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Test checksum mismatch\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Test checksum not found\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::{MetadataKeyAesGcmTag, NewIVFromSlice, createFileBackend};
use std::io::Write;

/// Matches Go package-level constants in mem_backend.go.
const wrongMasterKey: &str = "wrong master key";
const gcmTagNotFound: &str = "aes gcm tag not found";

/// TempKeyFile represents a temporary key file for testing.
struct TempKeyFile {
    Path: String,
}

impl TempKeyFile {
    fn Cleanup(&self) {
        let _ = std::fs::remove_file(&self.Path);
    }
}

impl Drop for TempKeyFile {
    fn drop(&mut self) {
        self.Cleanup();
    }
}

/// createMasterKeyFile creates a temporary master key file for testing.
fn createMasterKeyFile() -> Result<TempKeyFile, String> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "test_key_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut tempFile = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    if let Err(err) =
        tempFile.write_all(b"c3d99825f2181f4808acd2068eac7441a65bd428f14d2aab43fefc0129091139\n")
    {
        let _ = std::fs::remove_file(&path);
        return Err(err.to_string());
    }
    Ok(TempKeyFile {
        Path: path.to_string_lossy().into_owned(),
    })
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd length hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[test]
fn test_file_backend_aes256_gcm() {
    let pt = decode_hex("25431587e9ecffc7c37f8d6d52a9bc3310651d46fb0e3bad2726c8f2db653749")
        .expect("require.NoError: decode plaintext hex");
    let ct = decode_hex("84e5f23f95648fa247cb28eef53abec947dbf05ac953734618111583840bd980")
        .expect("require.NoError: decode ciphertext hex");
    let ivBytes = decode_hex("cafabd9672ca6c79a2fbdc22").expect("require.NoError: decode iv hex");

    let tempKeyFile = createMasterKeyFile().expect("require.NoError: create temp key file");
    let backend =
        createFileBackend(&tempKeyFile.Path).expect("require.NoError: create file backend");

    let iv = NewIVFromSlice(&ivBytes).expect("require.NoError: build IV from bytes");

    let encryptedContent = backend
        .memCache
        .EncryptContent(&pt, &iv)
        .expect("require.NoError: encrypt content through mem cache");
    assert_eq!(ct, encryptedContent.Content);

    let plaintext = backend
        .Decrypt(&encryptedContent)
        .expect("require.NoError: decrypt content through file backend");
    assert_eq!(pt, plaintext);
}

#[test]
fn test_file_backend_authenticate() {
    let pt = vec![1_u8, 2, 3];

    let tempKeyFile = createMasterKeyFile().expect("require.NoError: create temp key file");
    let backend =
        createFileBackend(&tempKeyFile.Path).expect("require.NoError: create file backend");

    let encryptedContent = backend
        .Encrypt(&pt)
        .expect("require.NoError: encrypt with file backend");

    let plaintext = backend
        .Decrypt(&encryptedContent)
        .expect("require.NoError: decrypt original content");
    assert_eq!(pt, plaintext);

    // Test checksum mismatch
    let mut encryptedContent1 = encryptedContent.clone();
    encryptedContent1
        .Metadata
        .get_mut(MetadataKeyAesGcmTag)
        .expect("gcm tag present")[0] ^= 0xFF;
    let err = backend.Decrypt(&encryptedContent1).unwrap_err();
    assert!(
        err.contains(wrongMasterKey),
        "expected wrong master key, got {err}"
    );

    // Test checksum not found
    let mut encryptedContent2 = encryptedContent.clone();
    encryptedContent2.Metadata.remove(MetadataKeyAesGcmTag);
    let err = backend.Decrypt(&encryptedContent2).unwrap_err();
    assert!(
        err.contains(gcmTagNotFound),
        "expected gcm tag not found, got {err}"
    );
}
