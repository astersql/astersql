// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Common key wrappers matching `br/pkg/kms/common.go`.
//
// 提供加密/明文密钥的类型包装与长度约束，对齐 Go `br/pkg/kms/common.go`。
// 空密文密钥与 AES-GCM-256 长度不匹配会在构造期拒绝，避免下游误用。

/// EncryptedKey is used to mark data as an encrypted key.
// 新类型区分“密文密钥字节”，防止与明文密钥在类型层混淆。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EncryptedKey(pub Vec<u8>);

// 拒绝空切片：Go NewEncryptedKey 对空输入同样报错。
pub fn NewEncryptedKey(key: Vec<u8>) -> Result<EncryptedKey, String> {
    if key.is_empty() {
        return Err("encrypted key cannot be empty".into());
    }
    Ok(EncryptedKey(key))
}

impl EncryptedKey {
    // 字节级相等比较，供备份元数据中密钥指纹校验。
    pub fn Equal(&self, other: &EncryptedKey) -> bool {
        self.0 == other.0
    }
}

/// CryptographyType represents different cryptography methods.
// 整数值与 Go/加密协议约定一致：0=明文标记，1=AES-256-GCM。
// 保持为开放整数新类型，以便像 Go `type CryptographyType int` 一样表示未知值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CryptographyType(pub i32);

impl CryptographyType {
    pub const CryptographyTypePlain: Self = Self(0);
    pub const CryptographyTypeAesGcm256: Self = Self(1);

    // Plain 不限制长度；AesGcm256 要求恰好 32 字节密钥材料。
    pub fn TargetKeySize(self) -> usize {
        match self.0 {
            1 => 32,
            _ => 0,
        }
    }
}

/// PlainKey is used to mark a byte slice as a plaintext key.
// 携带算法标签与密钥字节；标签决定 NewPlainKey 的长度校验策略。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainKey {
    pub tag: CryptographyType,
    pub key: Vec<u8>,
}

// limitation>0 时强制精确长度；Plain 的 TargetKeySize=0 跳过长度检查。
pub fn NewPlainKey(key: Vec<u8>, t: CryptographyType) -> Result<PlainKey, String> {
    let limitation = t.TargetKeySize();
    if limitation > 0 && key.len() != limitation {
        return Err(format!(
            "encryption method and key length mismatch, expect {limitation} got {}",
            key.len()
        ));
    }
    Ok(PlainKey { key, tag: t })
}

impl PlainKey {
    // 返回构造时绑定的算法标签，供加密管线选择实现。
    pub fn KeyTag(&self) -> CryptographyType {
        self.tag
    }

    pub fn Key(&self) -> &[u8] {
        &self.key
    }
}
