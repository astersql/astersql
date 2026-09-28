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

// 向量化加密/压缩内置函数的 aster 单元测试。
//
// 按行验证 AES 模式与 IV 规则、摘要与 PASSWORD、SQL crypt、
// RANDOM_BYTES 边界、COMPRESS 警告语义与密码强度策略。

use crate::expression_encryption_vec::{
    AesMode, ByteColumn, EvalContext, IntColumn, PasswordPolicy, Warning, aes_decrypt_vec,
    aes_encrypt_vec, compress_vec, md5_vec, password_vec, random_bytes_vec, sha1_vec, sha2_vec,
    sm3_vec, sql_decode_vec, sql_encode_vec, uncompress_vec, uncompressed_length_vec,
    validate_password_strength_vec,
};

/// 测试辅助：把 Option 字节切片列转为 ByteColumn。
fn bytes(values: &[Option<&[u8]>]) -> ByteColumn {
    values
        .iter()
        .map(|value| value.map(<[u8]>::to_vec))
        .collect()
}

/// 测试辅助：把十六进制摘要字节解释为 UTF-8 文本。
fn text(value: &Option<Vec<u8>>) -> Option<&str> {
    value
        .as_deref()
        .map(|bytes| std::str::from_utf8(bytes).unwrap())
}

#[test]
/// AES 向量：模式往返、NULL 行、IV 规则与坏密文。
fn aes_vectors_cover_modes_nulls_iv_rules_and_row_failures() {
    let plain = bytes(&[Some(b"pingcap"), None]);
    let keys = bytes(&[Some(b"1234567890123456"), Some(b"ignored")]);
    let extra_iv = bytes(&[Some(b"ignored"), Some(b"ignored")]);
    let mut ctx = EvalContext::default();

    let ecb = AesMode::parse("aes-128-ecb").unwrap();
    let encrypted = aes_encrypt_vec(&mut ctx, &plain, &keys, Some(&extra_iv), ecb).unwrap();
    assert_eq!(
        hex::encode_upper(encrypted[0].as_ref().unwrap()),
        "697BFE9B3F8C2F289DD82C88C7BC95C4"
    );
    assert_eq!(encrypted[1], None);
    assert_eq!(ctx.warnings(), &[Warning::IvIgnored]);
    assert_eq!(
        aes_decrypt_vec(&mut EvalContext::default(), &encrypted, &keys, None, ecb,).unwrap(),
        plain
    );

    for (name, expected) in [
        ("aes-128-cbc", "2ECA0077C5EA5768A0485AA522774792"),
        ("aes-128-ofb", "0515A36BBF3DE0"),
        ("aes-128-cfb", "0515A36BBF3DE0"),
    ] {
        let mode = AesMode::parse(name).unwrap();
        let one_plain = bytes(&[Some(b"pingcap")]);
        let one_key = bytes(&[Some(b"1234567890123456")]);
        let iv = bytes(&[Some(b"1234567890123456extra")]);
        let ciphertext = aes_encrypt_vec(
            &mut EvalContext::default(),
            &one_plain,
            &one_key,
            Some(&iv),
            mode,
        )
        .unwrap();
        assert_eq!(hex::encode_upper(ciphertext[0].as_ref().unwrap()), expected);
        assert_eq!(
            aes_decrypt_vec(
                &mut EvalContext::default(),
                &ciphertext,
                &one_key,
                Some(&iv),
                mode,
            )
            .unwrap(),
            one_plain
        );
    }

    let short_iv = bytes(&[Some(b"too short")]);
    let err = aes_encrypt_vec(
        &mut EvalContext::default(),
        &bytes(&[Some(b"pingcap")]),
        &bytes(&[Some(b"key")]),
        Some(&short_iv),
        AesMode::parse("aes-128-cbc").unwrap(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("at least 16 bytes"));

    let invalid_ciphertext = bytes(&[Some(b"not a block")]);
    assert_eq!(
        aes_decrypt_vec(
            &mut EvalContext::default(),
            &invalid_ciphertext,
            &bytes(&[Some(b"key")]),
            None,
            ecb,
        )
        .unwrap(),
        bytes(&[None])
    );
}

#[test]
/// 摘要与 PASSWORD 按行对照 MySQL/Go 结果。
fn digest_vectors_and_password_match_mysql_results_per_row() {
    let values = bytes(&[Some(b"abc"), Some(b""), None]);
    assert_eq!(
        text(&md5_vec(&values)[0]),
        Some("900150983cd24fb0d6963f7d28e17f72")
    );
    assert_eq!(
        text(&sha1_vec(&values)[0]),
        Some("a9993e364706816aba3e25717850c26c9cd0d89d")
    );
    assert_eq!(
        text(&sm3_vec(&values)[0]),
        Some("66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0")
    );
    assert_eq!(md5_vec(&values)[2], None);

    let lengths: IntColumn = vec![Some(0), Some(224), Some(123)];
    let sha2 = sha2_vec(
        &bytes(&[Some(b"pingcap"), Some(b"pingcap"), Some(b"pingcap")]),
        &lengths,
    )
    .unwrap();
    assert_eq!(
        text(&sha2[0]),
        Some("2871823be240f8ecd1d72f24c99eaa2e58af18b4b8ba99a4fc2823ba5c43930a")
    );
    assert_eq!(
        text(&sha2[1]),
        Some("cd036dc9bec69e758401379c522454ea24a6327b48724b449b40c6b7")
    );
    assert_eq!(sha2[2], None);

    let mut ctx = EvalContext::default();
    let passwords = password_vec(&mut ctx, &bytes(&[None, Some(b""), Some(b"abc")]));
    assert_eq!(text(&passwords[0]), Some(""));
    assert_eq!(text(&passwords[1]), Some(""));
    assert_eq!(
        text(&passwords[2]),
        Some("*0D3CED9BEC10A777AEC23CCC353A8C08A633045E")
    );
    assert_eq!(ctx.warnings(), &[Warning::PasswordDeprecated]);
}

#[test]
/// 历史 SQL DECODE/ENCODE 可逆且匹配 Go 向量。
fn legacy_sql_crypt_is_reversible_and_matches_go_vector() {
    let origin = bytes(&[Some(b"pingcap"), Some(b""), None]);
    let password = bytes(&[Some(b"1234567890123456"), Some(b""), Some(b"password")]);
    // MySQL 历史命名反直觉：DECODE 把明文变成乱码，ENCODE 再还原。
    // MySQL's historical names are counter-intuitive: DECODE maps plain bytes to the scrambled
    // representation, while ENCODE reverses that mapping.
    let encoded = sql_decode_vec(&origin, &password).unwrap();
    assert_eq!(
        hex::encode_upper(encoded[0].as_ref().unwrap()),
        "2C35B5A4ADF391"
    );
    assert_eq!(encoded[1], Some(Vec::new()));
    assert_eq!(encoded[2], None);
    assert_eq!(sql_encode_vec(&encoded, &password).unwrap(), origin);
}

#[test]
/// RANDOM_BYTES 长度边界与 NULL 传播。
fn random_bytes_enforces_mysql_bounds_and_null_propagation() {
    let output = random_bytes_vec(&vec![Some(1), Some(32), None]).unwrap();
    assert_eq!(output[0].as_ref().unwrap().len(), 1);
    assert_eq!(output[1].as_ref().unwrap().len(), 32);
    assert_eq!(output[2], None);
    for length in [0, -1, 1025] {
        assert!(random_bytes_vec(&vec![Some(length)]).is_err());
    }
}

#[test]
/// COMPRESS 往返、头布局与损坏警告语义。
fn compression_round_trip_matches_go_header_and_warning_semantics() {
    let input = bytes(&[Some(b"hello world"), Some(b""), None]);
    let compressed = compress_vec(&input);
    assert_eq!(
        hex::encode_upper(compressed[0].as_ref().unwrap()),
        "0B000000789CCA48CDC9C95728CF2FCA4901040000FFFF1A0B045D"
    );
    assert_eq!(compressed[1], Some(Vec::new()));
    assert_eq!(compressed[2], None);

    let mut ctx = EvalContext::default();
    assert_eq!(uncompress_vec(&mut ctx, &compressed), input);
    assert!(ctx.warnings().is_empty());

    let malformed = bytes(&[Some(b"1"), Some(b"12345")]);
    let mut ctx = EvalContext::default();
    assert_eq!(uncompress_vec(&mut ctx, &malformed), bytes(&[None, None]));
    assert_eq!(ctx.warnings(), &[Warning::ZlibData, Warning::ZlibData]);

    let mut ctx = EvalContext::default();
    assert_eq!(
        uncompressed_length_vec(&mut ctx, &bytes(&[Some(b""), Some(b"1"), None])),
        vec![Some(0), Some(0), None]
    );
    assert_eq!(ctx.warnings(), &[Warning::ZlibData]);
}

#[test]
/// 密码强度：禁用策略、NULL 与各档分数。
fn password_strength_preserves_disabled_null_and_policy_scores() {
    let input = bytes(&[
        None,
        Some(b"123"),
        Some(b"testuser123"),
        Some(b"12345"),
        Some(b"12345678"),
        Some(b"!Abc12345678"),
        Some(b"!Abc87654321"),
    ]);
    let policy = PasswordPolicy {
        username: Some("testuser".to_owned()),
        dictionary: vec!["1234".to_owned()],
        ..PasswordPolicy::default()
    };
    assert_eq!(
        validate_password_strength_vec(&input, false, &policy),
        vec![None, Some(0), Some(0), Some(0), Some(0), Some(0), Some(0)]
    );
    assert_eq!(
        validate_password_strength_vec(&input, true, &policy),
        vec![
            None,
            Some(25),
            Some(0),
            Some(25),
            Some(50),
            Some(75),
            Some(100)
        ]
    );
}
