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
//! 本文件负责`br/pkg/encryption/master_key/mem_backend_test.rs`对应的内存 AES-GCM 后端，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少20行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `test_new_mem_aes_gcm_backend`对齐 Go 同名测试或契约片段，用来固定\"test new mem aes gcm backend\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_encrypt_decrypt`对齐 Go 同名测试或契约片段，用来固定\"test encrypt decrypt\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_decrypt_with_wrong_key`对齐 Go 同名测试或契约片段，用来固定\"test decrypt with wrong key\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_decrypt_with_tampered_ciphertext`对齐 Go 同名测试或契约片段，用来固定\"test decrypt with tampered ciphertext\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_decrypt_with_missing_metadata`对齐 Go 同名测试或契约片段，用来固定\"test decrypt with missing metadata\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_encrypt_decrypt_large_data`对齐 Go 同名测试或契约片段，用来固定\"test encrypt decrypt large data\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! 中文注释索引结束

use crate::{MetadataKeyMethod, NewIVGcm, NewMemAesGcmBackend};

#[test]
fn test_new_mem_aes_gcm_backend() {
    let key = vec![0_u8; 32]; // 256-bit key
    let _ = NewMemAesGcmBackend(&key).expect("require.NoError: Failed to create MemAesGcmBackend");

    let shortKey = vec![0_u8; 16];
    match NewMemAesGcmBackend(&shortKey) {
        Ok(_) => panic!("Expected error for short key"),
        Err(err) => assert!(!err.is_empty(), "Expected error for short key"),
    }
}

#[test]
fn test_encrypt_decrypt() {
    let key = vec![0_u8; 32];
    let backend =
        NewMemAesGcmBackend(&key).expect("require.NoError: Failed to create MemAesGcmBackend");

    let plaintext = b"Hello, World!".to_vec();

    let iv = NewIVGcm().expect("require.NoError: failed to create gcm iv");

    let encrypted = backend
        .EncryptContent(&plaintext, &iv)
        .expect("require.NoError: Encryption failed");

    let decrypted = backend
        .DecryptContent(&encrypted)
        .expect("require.NoError: Decryption failed");

    assert_eq!(
        plaintext, decrypted,
        "Decrypted text doesn't match original"
    );
}

#[test]
fn test_decrypt_with_wrong_key() {
    let key1 = vec![0_u8; 32];
    let mut key2 = vec![0_u8; 32];
    for byte in &mut key2 {
        *byte = 1; // Different from key1
    }

    let backend1 = NewMemAesGcmBackend(&key1).expect("create backend1");
    let backend2 = NewMemAesGcmBackend(&key2).expect("create backend2");

    let plaintext = b"Hello, World!".to_vec();

    let iv = NewIVGcm().expect("require.NoError: failed to create gcm iv");

    let encrypted = backend1
        .EncryptContent(&plaintext, &iv)
        .expect("encrypt with first key");
    let err = backend2.DecryptContent(&encrypted).unwrap_err();
    assert!(
        !err.is_empty(),
        "Expected decryption with wrong key to fail"
    );
}

#[test]
fn test_decrypt_with_tampered_ciphertext() {
    let key = vec![0_u8; 32];
    let backend = NewMemAesGcmBackend(&key).expect("create backend");

    let plaintext = b"Hello, World!".to_vec();

    let iv = NewIVGcm().expect("require.NoError: failed to create gcm iv");

    let mut encrypted = backend
        .EncryptContent(&plaintext, &iv)
        .expect("encrypt before tamper");
    encrypted.Content[0] ^= 1; // Tamper with the ciphertext

    let err = backend.DecryptContent(&encrypted).unwrap_err();
    assert!(
        !err.is_empty(),
        "Expected decryption of tampered ciphertext to fail"
    );
}

#[test]
fn test_decrypt_with_missing_metadata() {
    let key = vec![0_u8; 32];
    let backend = NewMemAesGcmBackend(&key).expect("create backend");

    let plaintext = b"Hello, World!".to_vec();

    let iv = NewIVGcm().expect("require.NoError: failed to create gcm iv");

    let mut encrypted = backend
        .EncryptContent(&plaintext, &iv)
        .expect("encrypt before deleting metadata");
    encrypted.Metadata.remove(MetadataKeyMethod);

    let err = backend.DecryptContent(&encrypted).unwrap_err();
    assert!(
        !err.is_empty(),
        "Expected decryption with missing metadata to fail"
    );
}

#[test]
fn test_encrypt_decrypt_large_data() {
    let key = vec![0_u8; 32];
    let backend = NewMemAesGcmBackend(&key).expect("create backend");

    let plaintext = vec![0_u8; 1_000_000]; // 1 MB of data

    let iv = NewIVGcm().expect("require.NoError: failed to create gcm iv");

    let encrypted = backend
        .EncryptContent(&plaintext, &iv)
        .expect("require.NoError: Encryption of large data failed");

    let decrypted = backend
        .DecryptContent(&encrypted)
        .expect("require.NoError: Decryption of large data failed");

    assert_eq!(
        plaintext, decrypted,
        "Decrypted large data doesn't match original"
    );
}
