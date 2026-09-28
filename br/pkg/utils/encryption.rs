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

//! Backup content decryption helpers ported from `br/pkg/utils/encryption.go`.
//! 备份内容解密与“有效加密方法”判定；与 Go 错误包装语义对齐。
//! IV 由调用方提供，本模块不负责密钥派生。

use crate::kvproto::brpb;
use crate::kvproto::encryptionpb::EncryptionMethod;
use astersql_br_pkg_errors::ErrInvalidArgument;
use astersql_errors::{Annotate, SharedError};
use astersql_util_encrypt::AESDecryptWithCTR;

/// Decrypts backup content according to the cipher info.
/// 空内容或无 cipher 时原样返回；Plaintext 不解密。
/// AES-*-CTR 走 AESDecryptWithCTR；其它类型报 ErrInvalidArgument。
pub fn Decrypt(
    content: Vec<u8>,
    cipher: Option<&brpb::CipherInfo>,
    iv: &[u8],
) -> Result<Vec<u8>, SharedError> {
    if content.is_empty() || cipher.is_none() {
        return Ok(content);
    }

    let cipher = cipher.unwrap();
    match cipher.get_cipher_type() {
        EncryptionMethod::Plaintext => Ok(content),
        EncryptionMethod::Aes128Ctr | EncryptionMethod::Aes192Ctr | EncryptionMethod::Aes256Ctr => {
            AESDecryptWithCTR(&content, cipher.get_cipher_key(), iv).map_err(SharedError::new)
        }
        // 未知/未支持类型：Annotate 保留 Go 风格错误链。
        other => Err(Annotate(
            Some(SharedError::new((*ErrInvalidArgument).clone())),
            format!("cipher type invalid {other:?}"),
        )
        .expect("annotate invalid cipher")),
    }
}

/// Returns whether the encryption method is considered effective (not unknown/plaintext).
/// Unknown/Plaintext 视为未启用加密；其余方法视为有效。
pub fn IsEffectiveEncryptionMethod(method: EncryptionMethod) -> bool {
    method != EncryptionMethod::Unknown && method != EncryptionMethod::Plaintext
}
