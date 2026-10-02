// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 向量化加密/压缩内置函数的基础单元测试。
//
// 覆盖 AES 行级 NULL/警告、摘要与 SQL crypt、压缩警告与密码强度。

use crate::expression_encryption_vec::*;

/// 测试辅助：构造 ByteColumn。
fn bytes(values: &[Option<&[u8]>]) -> ByteColumn {
    values
        .iter()
        .map(|value| value.map(<[u8]>::to_vec))
        .collect()
}

/// 测试辅助：摘要结果转 &str。
fn text(value: &Option<Vec<u8>>) -> Option<&str> {
    value
        .as_deref()
        .map(|value| std::str::from_utf8(value).unwrap())
}

#[test]
/// 向量化 AES：保留行序、NULL、IV 警告与短 IV 错误。
fn vectorized_aes_preserves_rows_nulls_warnings_and_errors() {
    let plain = bytes(&[Some(b"pingcap"), None]);
    let keys = bytes(&[Some(b"1234567890123456"), Some(b"ignored")]);
    let iv = bytes(&[Some(b"ignored"), Some(b"ignored")]);
    let mode = AesMode::parse("aes-128-ecb").unwrap();
    let mut context = EvalContext::default();
    let encrypted = aes_encrypt_vec(&mut context, &plain, &keys, Some(&iv), mode).unwrap();
    assert_eq!(
        hex::encode_upper(encrypted[0].as_ref().unwrap()),
        "697BFE9B3F8C2F289DD82C88C7BC95C4"
    );
    assert_eq!(encrypted[1], None);
    assert_eq!(context.warnings(), &[Warning::IvIgnored]);
    assert_eq!(
        aes_decrypt_vec(&mut EvalContext::default(), &encrypted, &keys, None, mode).unwrap(),
        plain
    );

    // CBC 要求至少 16 字节 IV，过短应返回语句错误。
    let short_iv = bytes(&[Some(b"short")]);
    assert!(
        aes_encrypt_vec(
            &mut EvalContext::default(),
            &bytes(&[Some(b"x")]),
            &bytes(&[Some(b"key")]),
            Some(&short_iv),
            AesMode::parse("aes-128-cbc").unwrap(),
        )
        .is_err()
    );
}

#[test]
/// 摘要、RANDOM_BYTES、SQL crypt 按行对齐 Go。
fn vectorized_digests_random_and_sql_crypt_match_go_per_row() {
    let values = bytes(&[Some(b"abc"), Some(b""), None]);
    assert_eq!(
        text(&md5_vec(&values)[0]),
        Some("900150983cd24fb0d6963f7d28e17f72")
    );
    assert_eq!(
        text(&sha1_vec(&values)[0]),
        Some("a9993e364706816aba3e25717850c26c9cd0d89d")
    );
    assert_eq!(md5_vec(&values)[2], None);
    assert_eq!(
        sha2_vec(&values, &vec![Some(256), Some(123), None]).unwrap()[1],
        None
    );

    let password = bytes(&[Some(b"1234567890123456"), Some(b""), Some(b"x")]);
    let encoded = sql_decode_vec(&values, &password).unwrap();
    assert_eq!(sql_encode_vec(&encoded, &password).unwrap(), values);

    let random = random_bytes_vec(&vec![Some(1), Some(32), None]).unwrap();
    assert_eq!(random[0].as_ref().unwrap().len(), 1);
    assert_eq!(random[1].as_ref().unwrap().len(), 32);
    assert_eq!(random[2], None);
    assert!(random_bytes_vec(&vec![Some(0)]).is_err());
}

#[test]
/// 压缩警告路径与密码强度策略。
fn vectorized_compression_and_password_strength_cover_warning_and_policy_paths() {
    let input = bytes(&[Some(b"hello world"), Some(b""), None]);
    let compressed = compress_vec(&input);
    assert_eq!(compressed[1], Some(Vec::new()));
    assert_eq!(compressed[2], None);
    let mut context = EvalContext::default();
    assert_eq!(uncompress_vec(&mut context, &compressed), input);
    assert!(context.warnings().is_empty());

    let mut context = EvalContext::default();
    assert_eq!(
        uncompress_vec(&mut context, &bytes(&[Some(b"1")])),
        bytes(&[None])
    );
    assert_eq!(context.warnings(), &[Warning::ZlibData]);

    let policy = PasswordPolicy {
        username: Some("testuser".into()),
        dictionary: vec!["1234".into()],
        ..PasswordPolicy::default()
    };
    assert_eq!(
        validate_password_strength_vec(
            &bytes(&[None, Some(b"123"), Some(b"12345"), Some(b"!Abc87654321")]),
            true,
            &policy,
        ),
        vec![None, Some(25), Some(25), Some(100)]
    );

    // Go reverses the UTF-8 bytes of a username, not its Unicode scalar values. Therefore a
    // valid UTF-8 password containing the character-reversed form of a multibyte username must
    // not be mistaken for the byte-reversed username.
    let unicode_username_policy = PasswordPolicy {
        username: Some("用户".into()),
        ..PasswordPolicy::default()
    };
    assert_eq!(
        validate_password_strength_vec(
            &bytes(&[Some("!Abc户用1234".as_bytes())]),
            true,
            &unicode_username_policy,
        ),
        vec![Some(100)]
    );
}

#[test]
fn go_commit_c6e3cf8399_vectorized_uncompress_rejects_output_beyond_declared_length() {
    let mut payload = 32_u32.to_le_bytes().to_vec();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &vec![0; 1 << 20]).unwrap();
    payload.extend_from_slice(&encoder.finish().unwrap());

    let mut context = EvalContext::default();
    assert_eq!(
        uncompress_vec(&mut context, &vec![Some(payload)]),
        vec![None]
    );
    assert_eq!(context.warnings(), &[Warning::ZlibBuffer]);
}
