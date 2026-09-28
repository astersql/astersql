// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// AES 加解密工具：PKCS7 填充与多种分组模式（ECB/CBC/OFB/CTR/CFB）。
//
// 对应 Go `pkg/util/encrypt/aes.go`，供 SQL 函数 `AES_ENCRYPT`/`AES_DECRYPT` 等使用。
// 密钥长度支持 16/24/32 字节（AES-128/192/256）；需 IV 的模式要求 16 字节 IV。
// `DeriveKeyMySQL` 按 MySQL 算法从口令派生定长密钥。

use aes::{Aes128, Aes192, Aes256};
use cipher::block_padding::Pkcs7;
use cipher::{
    AsyncStreamCipher, BlockDecrypt, BlockDecryptMut, BlockEncrypt, BlockEncryptMut, KeyInit,
    KeyIvInit, StreamCipher, generic_array::GenericArray,
};
use std::fmt;

/// 加密/解密过程中的错误（无效密钥、IV、填充或数据损坏等）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptError(String);

impl EncryptError {
    /// 由错误消息构造 `EncryptError`。
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for EncryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EncryptError {}

/// 校验 IV 长度必须为 AES 块大小（16 字节）。
fn check_iv(iv: &[u8]) -> Result<(), EncryptError> {
    if iv.len() == 16 {
        Ok(())
    } else {
        Err(EncryptError::new("invalid IV length"))
    }
}

/// 校验密钥长度为 16/24/32 字节之一。
fn check_key(key: &[u8]) -> Result<(), EncryptError> {
    match key.len() {
        16 | 24 | 32 => Ok(()),
        n => Err(EncryptError::new(format!(
            "crypto/aes: invalid key size {n}"
        ))),
    }
}

/// 使用 PKCS7 对数据按 `blockSize` 补齐。
// PKCS7Pad pads data using PKCS7.
pub fn PKCS7Pad(data: &[u8], blockSize: usize) -> Result<Vec<u8>, EncryptError> {
    if blockSize == 0 || blockSize > u8::MAX as usize {
        return Err(EncryptError::new("Invalid padding size"));
    }
    let padLen = blockSize - data.len() % blockSize;
    let mut out = data.to_vec();
    out.resize(out.len() + padLen, padLen as u8);
    Ok(out)
}

/// 去除 PKCS7 填充并校验填充合法性。
// PKCS7Unpad unpads data using PKCS7.
pub fn PKCS7Unpad(data: &[u8], blockSize: usize) -> Result<Vec<u8>, EncryptError> {
    if blockSize == 0 || data.is_empty() || data.len() % blockSize != 0 {
        return Err(EncryptError::new("Invalid padding size"));
    }
    let pad = data[data.len() - 1];
    let padLen = pad as usize;
    if padLen == 0 || padLen > blockSize {
        return Err(EncryptError::new("Invalid padding size"));
    }
    if data[data.len() - padLen..data.len() - 1]
        .iter()
        .any(|v| *v != pad)
    {
        return Err(EncryptError::new("Invalid padding"));
    }
    Ok(data[..data.len() - padLen].to_vec())
}

/// ECB 模式下按 16 字节块加/解密（不处理填充；调用方负责对齐）。
fn ecb_crypt(data: &[u8], key: &[u8], encrypt: bool) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    if data.len() % 16 != 0 {
        return Err(EncryptError::new("Corrupted data"));
    }
    let mut out = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {{
            let cipher =
                <$cipher>::new_from_slice(key).map_err(|_| EncryptError::new("invalid key"))?;
            for chunk in out.chunks_exact_mut(16) {
                let block = GenericArray::from_mut_slice(chunk);
                if encrypt {
                    cipher.encrypt_block(block);
                } else {
                    cipher.decrypt_block(block);
                }
            }
        }};
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => unreachable!(),
    }
    Ok(out)
}

/// AES-ECB 加密：先 PKCS7 填充再按块加密。
pub fn AESEncryptWithECB(str_: &[u8], key: &[u8]) -> Result<Vec<u8>, EncryptError> {
    ecb_crypt(&PKCS7Pad(str_, 16)?, key, true)
}

/// AES-ECB 解密：按块解密后去除 PKCS7 填充。
pub fn AESDecryptWithECB(cryptStr: &[u8], key: &[u8]) -> Result<Vec<u8>, EncryptError> {
    PKCS7Unpad(&ecb_crypt(cryptStr, key, false)?, 16)
}

/// 按 MySQL 算法从口令派生定长密钥（对口令字节按块大小循环异或）。
// DeriveKeyMySQL derives the encryption key from a password in MySQL's algorithm.
pub fn DeriveKeyMySQL(key: &[u8], blockSize: usize) -> Vec<u8> {
    let mut derived = vec![0; blockSize];
    if blockSize == 0 {
        return derived;
    }
    for (index, value) in key.iter().enumerate() {
        derived[index % blockSize] ^= value;
    }
    derived
}

/// AES-CBC 加密（PKCS7 填充）；`iv` 须为 16 字节。
pub fn AESEncryptWithCBC(str_: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    check_iv(iv)?;
    macro_rules! encrypt {
        ($cipher:ty) => {
            cbc::Encryptor::<$cipher>::new_from_slices(key, iv)
                .unwrap()
                .encrypt_padded_vec_mut::<Pkcs7>(str_)
        };
    }
    Ok(match key.len() {
        16 => encrypt!(Aes128),
        24 => encrypt!(Aes192),
        32 => encrypt!(Aes256),
        _ => unreachable!(),
    })
}

/// AES-CBC 解密并去除 PKCS7 填充；密文长度须为 16 的倍数。
pub fn AESDecryptWithCBC(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    check_iv(iv)?;
    if data.len() % 16 != 0 {
        return Err(EncryptError::new("Corrupted data"));
    }
    macro_rules! decrypt {
        ($cipher:ty) => {
            cbc::Decryptor::<$cipher>::new_from_slices(key, iv)
                .unwrap()
                .decrypt_padded_vec_mut::<Pkcs7>(data)
                .map_err(|_| EncryptError::new("Invalid padding"))?
        };
    }
    Ok(match key.len() {
        16 => decrypt!(Aes128),
        24 => decrypt!(Aes192),
        32 => decrypt!(Aes256),
        _ => unreachable!(),
    })
}

/// OFB 密钥流变换（加密与解密同构，对数据异或密钥流）。
fn ofb(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    check_iv(iv)?;
    let mut out = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            ofb::Ofb::<$cipher>::new_from_slices(key, iv)
                .unwrap()
                .apply_keystream(&mut out)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => unreachable!(),
    }
    Ok(out)
}

/// AES-OFB 加密。
pub fn AESEncryptWithOFB(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    ofb(data, key, iv)
}
/// AES-OFB 解密（与加密同一密钥流操作）。
pub fn AESDecryptWithOFB(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    ofb(data, key, iv)
}

/// CTR 密钥流变换（大端计数器）；加密与解密同构。
fn ctr(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    check_iv(iv)?;
    let mut out = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            ctr::Ctr128BE::<$cipher>::new_from_slices(key, iv)
                .unwrap()
                .apply_keystream(&mut out)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => unreachable!(),
    }
    Ok(out)
}

/// AES-CTR 加密。
pub fn AESEncryptWithCTR(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    ctr(data, key, iv)
}
/// AES-CTR 解密（与加密同一密钥流操作）。
pub fn AESDecryptWithCTR(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    ctr(data, key, iv)
}

/// AES-CFB 加密。
pub fn AESEncryptWithCFB(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    check_iv(iv)?;
    let mut out = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            cfb_mode::Encryptor::<$cipher>::new_from_slices(key, iv)
                .unwrap()
                .encrypt(&mut out)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => unreachable!(),
    }
    Ok(out)
}

/// AES-CFB 解密。
pub fn AESDecryptWithCFB(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, EncryptError> {
    check_key(key)?;
    check_iv(iv)?;
    let mut out = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            cfb_mode::Decryptor::<$cipher>::new_from_slices(key, iv)
                .unwrap()
                .decrypt(&mut out)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => unreachable!(),
    }
    Ok(out)
}
