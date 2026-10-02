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

// 加密与压缩标量内置函数的单元测试。
//
// 覆盖 AES 模式往返与 IV 校验、摘要/PASSWORD/SQL crypt、
// COMPRESS 往返与警告，以及密码强度策略路径。

use crate::expression_encryption::builtin_encryption::*;

#[test]
/// AES 多模式加解密往返，以及 IV 忽略/过短/不支持模式与坏密文。
fn aes_modes_round_trip_and_validate_iv_and_ciphertext() {
    let iv = b"1234567890123456extra";
    for bits in [128, 192, 256] {
        for suffix in ["ecb", "cbc", "ofb", "cfb"] {
            let mode = format!("aes-{bits}-{suffix}");
            let supplied_iv = (suffix != "ecb").then_some(iv.as_slice());
            let encrypted = aes_encrypt(b"pingcap", b"key", &mode, supplied_iv).unwrap();
            let ciphertext = encrypted.value.as_deref().unwrap();
            assert!(!ciphertext.is_empty());
            assert_eq!(
                aes_decrypt(ciphertext, b"key", &mode, supplied_iv)
                    .unwrap()
                    .value
                    .as_deref(),
                Some(b"pingcap".as_slice())
            );
        }
    }

    // ECB 不需要 IV：传入 IV 应产生 IvIgnored 警告。
    let ignored = aes_encrypt(b"pingcap", b"key", "aes-128-ecb", Some(iv)).unwrap();
    assert_eq!(ignored.warnings, vec![EncryptionWarning::IvIgnored]);
    assert!(aes_encrypt(b"x", b"k", "aes-128-cbc", Some(b"short")).is_err());
    assert!(aes_encrypt(b"x", b"k", "aes-128-ctr", None).is_err());
    assert_eq!(
        aes_decrypt(b"not a block", b"key", "aes-128-ecb", None)
            .unwrap()
            .value,
        None
    );
}

#[test]
/// Go 表驱动用例的代表性精确密文，覆盖全部模式与三种密钥长度。
fn aes_ciphertext_vectors_match_go_for_every_supported_mode_and_key_size() {
    let iv = b"1234567890123456";
    for (mode, expected) in [
        ("aes-128-ecb", "697BFE9B3F8C2F289DD82C88C7BC95C4"),
        ("aes-192-ecb", "9B139FD002E6496EA2D5C73A2265E661"),
        ("aes-256-ecb", "F80DCDEDDBE5663BDB68F74AEDDB8EE3"),
        ("aes-128-cbc", "2ECA0077C5EA5768A0485AA522774792"),
        ("aes-192-cbc", "516391DB38E908ECA93AAB22870EC787"),
        ("aes-256-cbc", "5D0E22C1E77523AEF5C3E10B65653C8F"),
        ("aes-128-ofb", "0515A36BBF3DE0"),
        ("aes-192-ofb", "FE09DCCF14D458"),
        ("aes-256-ofb", "2E70FCAC0C0834"),
        ("aes-128-cfb", "0515A36BBF3DE0"),
        ("aes-192-cfb", "FE09DCCF14D458"),
        ("aes-256-cfb", "2E70FCAC0C0834"),
    ] {
        let supplied_iv = (!mode.ends_with("ecb")).then_some(iv.as_slice());
        let encrypted = aes_encrypt(b"pingcap", b"1234567890123456", mode, supplied_iv)
            .unwrap()
            .value
            .unwrap();
        assert_eq!(hex::encode_upper(&encrypted), expected, "{mode}");
    }
}

#[test]
/// 摘要、PASSWORD、DECODE/ENCODE、RANDOM_BYTES 对照 Go 向量。
fn hashes_password_sql_crypt_and_random_bytes_match_go_vectors() {
    assert_eq!(md5_hash(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        sha1_hash(b"abc"),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        sha2_hash(b"abc", 224).as_deref(),
        Some("23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7")
    );
    assert_eq!(sha2_hash(b"abc", 0), sha2_hash(b"abc", 256));
    assert_eq!(sha2_hash(b"abc", 123), None);
    assert_eq!(
        sm3_hash(b"abc"),
        "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
    );

    let password = mysql_password("abc");
    assert_eq!(
        password.value.as_deref(),
        Some("*0D3CED9BEC10A777AEC23CCC353A8C08A633045E")
    );
    assert_eq!(
        password.warnings,
        vec![EncryptionWarning::PasswordDeprecated]
    );
    let empty_password = mysql_password("");
    assert_eq!(empty_password.value.as_deref(), Some(""));
    assert!(empty_password.warnings.is_empty());

    let encrypted = sql_decode(b"pingcap", b"1234567890123456");
    assert_eq!(hex::encode_upper(&encrypted), "2C35B5A4ADF391");
    assert_eq!(sql_encode(&encrypted, b"1234567890123456"), b"pingcap");
    assert_eq!(
        hex::encode_upper(sql_decode(
            "分布式データベース".as_bytes(),
            b"pass1234@#$%%^^&"
        )),
        "80CADC8D328B3026D04FB285F36FED04BBCA0CC685BF78B1E687CE"
    );
    assert_eq!(random_bytes(32).unwrap().len(), 32);
    for invalid in [0, -1, 1025] {
        assert!(random_bytes(invalid).is_err());
    }
}

#[test]
/// COMPRESS/UNCOMPRESS/UNCOMPRESSED_LENGTH 与损坏输入警告。
fn compression_round_trip_empty_and_corrupt_inputs_preserve_warnings() {
    let compressed = compress(b"hello world").unwrap();
    assert_eq!(
        hex::encode_upper(compressed.value.as_deref().unwrap()),
        "0B000000789CCA48CDC9C95728CF2FCA4901040000FFFF1A0B045D"
    );
    assert_eq!(
        uncompress(compressed.value.as_deref().unwrap())
            .value
            .as_deref(),
        Some(b"hello world".as_slice())
    );
    assert_eq!(
        uncompressed_length(compressed.value.as_deref().unwrap()).value,
        Some(11)
    );
    assert_eq!(compress(b"").unwrap().value, Some(Vec::new()));

    let corrupt = uncompress(b"1234");
    assert_eq!(corrupt.value, None);
    assert_eq!(corrupt.warnings, vec![EncryptionWarning::ZlibData]);
    let invalid_length = uncompressed_length(b"1");
    assert_eq!(invalid_length.value, Some(0));
    assert_eq!(invalid_length.warnings, vec![EncryptionWarning::ZlibData]);
}

#[test]
fn go_commit_c6e3cf8399_scalar_uncompress_rejects_output_beyond_declared_length() {
    let mut payload = 32_u32.to_le_bytes().to_vec();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &vec![0; 1 << 20]).unwrap();
    payload.extend_from_slice(&encoder.finish().unwrap());

    let result = uncompress(&payload);
    assert_eq!(result.value, None);
    assert_eq!(result.warnings, vec![EncryptionWarning::ZlibBuffer]);

    let handcrafted = hex::decode("20000000789c73741c05a360148c540000a4780410").unwrap();
    let result = uncompress(&handcrafted);
    assert_eq!(result.value, None);
    assert_eq!(result.warnings, vec![EncryptionWarning::ZlibBuffer]);
}

#[test]
/// 密码强度：策略关闭、NULL、用户名命中、字典词与满分路径。
fn password_strength_covers_null_disabled_username_dictionary_and_full_score() {
    // 策略未启用时任何密码强度均为 0。
    let disabled = PasswordPolicy::default();
    assert_eq!(
        validate_password_strength(Some("!Abc87654321"), &disabled),
        Some(0)
    );

    let policy = PasswordPolicy {
        enabled: true,
        check_username: true,
        username: Some("testuser".into()),
        auth_username: None,
        minimum_length: 8,
        mixed_case_count: 1,
        number_count: 1,
        special_char_count: 1,
        dictionary: vec!["1234".into()],
    };
    assert_eq!(validate_password_strength(None, &policy), None);
    for (input, expected) in [
        ("123", 0),
        ("testuser123", 0),
        ("12345", 25),
        ("12345678", 50),
        ("!Abc12345678", 75),
        ("!Abc87654321", 100),
    ] {
        assert_eq!(
            validate_password_strength(Some(input), &policy),
            Some(expected)
        );
    }
}
