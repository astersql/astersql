// Copyright 2026 AsterSQL.
//! encryption 包与 Go `br/pkg/encryption` 的公开契约对等测试。
//!
//! 覆盖：有效加密方法判定、`NewManager` 入参约束、明文/主密钥两种解密模式的关键错误与空载荷路径。
//! 仅断言 Rust 侧行为与 Go `manager.go` / utils 语义一致，不改加密实现本身。

use crate::{
    CipherInfo, DecryptContent, EncryptionMethod, FileEncryptionInfo, FileEncryptionMode,
    IsEffectiveEncryptionMethod, NewManager,
};

/// 核对公开 API 与 Go Manager 契约：方法判定、构造失败、解密分支与“无加密→None”。
#[test]
fn go_rust_public_contract_matches() {
    // Plaintext 非有效加密；Aes256Ctr 为有效（与 utils.IsEffectiveEncryptionMethod 对齐）。
    assert!(!IsEffectiveEncryptionMethod(EncryptionMethod::Plaintext));
    assert!(IsEffectiveEncryptionMethod(EncryptionMethod::Aes256Ctr));

    // Go：cipherInfo 或 masterKeyConfigs 为 nil 直接报错。
    assert_eq!(
        NewManager(None, None).err().unwrap(),
        "cipherInfo or masterKeyConfigs is nil"
    );
    assert_eq!(
        NewManager(Some(CipherInfo::default()), None).err().unwrap(),
        "cipherInfo or masterKeyConfigs is nil"
    );
    assert_eq!(
        NewManager(None, Some(crate::MasterKeyConfig::default()))
            .err()
            .unwrap(),
        "cipherInfo or masterKeyConfigs is nil"
    );

    let cipher = CipherInfo {
        CipherType: EncryptionMethod::Aes256Ctr,
        CipherKey: vec![0u8; 32],
    };
    let mk = crate::MasterKeyConfig {
        EncryptionType: EncryptionMethod::Plaintext,
        MasterKeys: vec![],
    };
    // cipher 有效时走明文 data-key 管理器；masterKey 配置此时不启用。
    let mut mgr = NewManager(Some(cipher.clone()), Some(mk)).unwrap().unwrap();
    // plaintext path requires cipherInfo set
    // MasterKeyBased 模式需要已加密 data key；空列表应对齐 Go “at least one encrypted data key”。
    let err = mgr
        .Decrypt(
            b"x",
            &FileEncryptionInfo {
                Mode: FileEncryptionMode::MasterKeyBased {
                    DataKeyEncryptedContent: vec![],
                },
                FileIv: vec![0u8; 16],
                EncryptionMethod: EncryptionMethod::Aes256Ctr,
            },
        )
        .unwrap_err();
    assert_eq!(err, "should contain at least one encrypted data key");

    // empty content with plaintext mode
    // PlainTextDataKey + 空密文：解密结果为空切片，验证 utils.Decrypt 空输入路径。
    let out = mgr
        .Decrypt(
            b"",
            &FileEncryptionInfo {
                Mode: FileEncryptionMode::PlainTextDataKey,
                FileIv: vec![0u8; 16],
                EncryptionMethod: EncryptionMethod::Aes256Ctr,
            },
        )
        .unwrap();
    assert!(out.is_empty());
    // Close 与 Go Manager.Close 对齐：释放主密钥后端资源（明文路径可为空操作）。
    mgr.Close();

    // no effective encryption => None
    // cipher 与 masterKey 均非有效加密方法时，Go 返回 (nil, nil)，Rust 为 Ok(None)。
    // Unknown 视为无效加密类型，避免误建 master-key 后端。
    let none = NewManager(
        Some(CipherInfo {
            CipherType: EncryptionMethod::Plaintext,
            CipherKey: vec![],
        }),
        Some(crate::MasterKeyConfig {
            EncryptionType: EncryptionMethod::Unknown,
            MasterKeys: vec![],
        }),
    )
    .unwrap();
    assert!(none.is_none());
}

/// 使用 NIST AES-128-CTR 向量验证 Go `utils.Decrypt` 的真实算法路径和错误边界。
#[test]
fn plaintext_data_key_decrypts_real_ctr_vector() {
    let cipher = CipherInfo {
        CipherType: EncryptionMethod::Aes128Ctr,
        CipherKey: vec![
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ],
    };
    let iv = [
        0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe,
        0xff,
    ];
    let ciphertext = [
        0x87, 0x4d, 0x61, 0x91, 0xb6, 0x20, 0xe3, 0x26, 0x1b, 0xef, 0x68, 0x64, 0x99, 0x0d, 0xb6,
        0xce,
    ];
    let plaintext = [
        0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17,
        0x2a,
    ];

    let manager = NewManager(Some(cipher), Some(crate::MasterKeyConfig::default()))
        .unwrap()
        .unwrap();
    let out = manager
        .Decrypt(
            &ciphertext,
            &FileEncryptionInfo {
                Mode: FileEncryptionMode::PlainTextDataKey,
                FileIv: iv.to_vec(),
                EncryptionMethod: EncryptionMethod::Aes128Ctr,
            },
        )
        .unwrap();
    assert_eq!(out, plaintext);

    assert_eq!(
        DecryptContent(
            b"unchanged",
            &CipherInfo {
                CipherType: EncryptionMethod::Plaintext,
                CipherKey: vec![],
            },
            &[],
        )
        .unwrap(),
        b"unchanged"
    );
    assert_eq!(
        DecryptContent(
            b"not-empty",
            &CipherInfo {
                CipherType: EncryptionMethod::Unknown,
                CipherKey: vec![],
            },
            &[],
        )
        .unwrap_err(),
        "cipher type invalid Unknown"
    );
}

/// Go 允许非 nil 空主密钥切片构造，首次解密时再报告后端为空。
#[test]
fn empty_master_key_slice_fails_at_decrypt_like_go() {
    let manager = NewManager(
        Some(CipherInfo::default()),
        Some(crate::MasterKeyConfig {
            EncryptionType: EncryptionMethod::Aes256Ctr,
            MasterKeys: vec![],
        }),
    )
    .unwrap()
    .unwrap();
    let err = manager
        .Decrypt(
            b"ciphertext",
            &FileEncryptionInfo {
                Mode: FileEncryptionMode::MasterKeyBased {
                    DataKeyEncryptedContent: vec![Default::default()],
                },
                FileIv: vec![0u8; 16],
                EncryptionMethod: EncryptionMethod::Aes256Ctr,
            },
        )
        .unwrap_err();
    assert_eq!(
        err,
        "failed to decrypt data key using master key: internal error: should always contain at least one backend"
    );
}

/// protobuf oneof 未设置时，Go 会进入 default 分支并报告不支持的 mode 类型。
#[test]
fn unset_file_encryption_mode_matches_go_error() {
    let manager = NewManager(
        Some(CipherInfo {
            CipherType: EncryptionMethod::Aes256Ctr,
            CipherKey: vec![0u8; 32],
        }),
        Some(crate::MasterKeyConfig::default()),
    )
    .unwrap()
    .unwrap();

    let info = FileEncryptionInfo::default();
    assert!(matches!(info.Mode, FileEncryptionMode::Unset));
    let err = manager.Decrypt(b"ciphertext", &info).unwrap_err();
    assert_eq!(
        err,
        "internal error: unsupported encryption mode type <nil>"
    );
}
