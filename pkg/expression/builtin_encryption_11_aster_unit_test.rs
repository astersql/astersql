// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 加密、压缩、全文检索与函数参数构造的综合单元测试（aster）。
//
// 对照 Go 向量验证 AES 多模式往返、摘要/PASSWORD/ENCODE、COMPRESS 警告，
// 以及 GROUPING / FTS_MATCH_* / BuildParam 的控制流。

use std::collections::HashSet;

use crate::expression_encryption::builtin_encryption::{
    EncryptionWarning, PasswordPolicy, aes_decrypt, aes_encrypt, compress, md5_hash,
    mysql_password, random_bytes, sha1_hash, sha2_hash, sm3_hash, sql_decode, sql_encode,
    uncompress, uncompressed_length, validate_password_strength,
};
use crate::expression_encryption::builtin_fts::{
    FtsAgainst, FtsSignature, FulltextSearchModifier, MatchArgument, build_match_word,
    build_mysql_match_against, set_mysql_match_against_modifier,
};
use crate::expression_encryption::builtin_func_param::{
    BuildParam, ParamSource, build_int_param, build_string_param,
};
use crate::expression_encryption::builtin_grouping::{GroupingMode, GroupingSig};

#[test]
/// AES 128/192/256 × ECB/CBC/OFB/CFB 往返；IV 忽略警告与坏密文→NULL。
fn encryption_modes_and_null_on_bad_ciphertext_match_go() {
    let iv = b"1234567890123456ignored";
    // 遍历密钥长度与工作模式；ECB 不传有效 IV。
    for bits in [128, 192, 256] {
        for mode in ["ecb", "cbc", "ofb", "cfb"] {
            let mode = format!("aes-{bits}-{mode}");
            let supplied_iv = (mode != format!("aes-{bits}-ecb")).then_some(iv.as_slice());
            let encrypted = aes_encrypt(b"pingcap", b"key", &mode, supplied_iv).unwrap();
            assert!(
                encrypted
                    .value
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
            );
            let decrypted = aes_decrypt(
                encrypted.value.as_deref().unwrap(),
                b"key",
                &mode,
                supplied_iv,
            )
            .unwrap();
            assert_eq!(decrypted.value.as_deref(), Some(b"pingcap".as_slice()));
        }
    }

    // Go 表驱动用例的精确密文，覆盖三种密钥长度和四种工作模式。
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
        let supplied_iv = (!mode.ends_with("ecb")).then_some(&iv[..16]);
        let encrypted = aes_encrypt(b"pingcap", b"1234567890123456", mode, supplied_iv)
            .unwrap()
            .value
            .unwrap();
        assert_eq!(hex::encode_upper(encrypted), expected, "{mode}");
    }

    let ignored = aes_encrypt(b"pingcap", b"key", "AES-128-ECB", Some(iv)).unwrap();
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
/// MD5/SHA/SM3/PASSWORD/DECODE/ENCODE/RANDOM_BYTES 与 Go 向量对齐。
fn hashes_password_sql_crypt_and_random_bytes_match_go() {
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
    assert!(mysql_password("").warnings.is_empty());
    assert_eq!(mysql_password("").value.as_deref(), Some(""));

    let decoded = sql_decode(b"pingcap", b"1234567890123456");
    assert_eq!(hex::encode_upper(&decoded), "2C35B5A4ADF391");
    assert_eq!(sql_encode(&decoded, b"1234567890123456"), b"pingcap");

    assert_eq!(random_bytes(32).unwrap().len(), 32);
    for invalid in [0, -1, 1025] {
        assert!(random_bytes(invalid).is_err());
    }
}

#[test]
/// COMPRESS 头与字节布局、损坏警告，以及密码强度各档分数。
fn mysql_compress_headers_warnings_and_password_scores_match_go() {
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
    let short_length = uncompressed_length(b"1");
    assert_eq!(short_length.value, Some(0));
    assert_eq!(short_length.warnings, vec![EncryptionWarning::ZlibData]);
    assert_eq!(
        hex::encode_upper(compress("你好".as_bytes()).unwrap().value.unwrap()),
        "06000000789C7AB277C1D3A57B01010000FFFF10450489"
    );
    assert_eq!(
        uncompress(&hex::decode("0B000000789CCB48CDC9C95728CF2FCA4901001A0B045D").unwrap())
            .value
            .as_deref(),
        Some(b"hello world".as_slice())
    );
    // 较大输入验证压缩往返。
    let large: Vec<u8> = (0..2048)
        .flat_map(|index| format!("row={index},alpha beta gamma delta;").into_bytes())
        .collect();
    let large_compressed = compress(&large).unwrap().value.unwrap();
    assert_eq!(
        uncompress(&large_compressed).value.as_deref(),
        Some(large.as_slice())
    );

    let corrupt = uncompress(b"1234");
    assert_eq!(corrupt.value, None);
    assert_eq!(corrupt.warnings, vec![EncryptionWarning::ZlibData]);
    // 故意改短长度头，应触发 ZlibBuffer 警告。
    let mut wrong_length = compressed.value.unwrap();
    wrong_length[..4].copy_from_slice(&2_u32.to_le_bytes());
    assert_eq!(
        uncompress(&wrong_length).warnings,
        vec![EncryptionWarning::ZlibBuffer]
    );

    let policy = PasswordPolicy {
        enabled: true,
        check_username: true,
        username: Some("testuser".to_owned()),
        auth_username: None,
        minimum_length: 8,
        mixed_case_count: 1,
        number_count: 1,
        special_char_count: 1,
        dictionary: vec!["1234".to_owned()],
    };
    assert_eq!(
        validate_password_strength(Some("!Abc87654321"), &PasswordPolicy::default()),
        Some(0)
    );
    for (input, score) in [
        ("123", 0),
        ("testuser123", 0),
        ("resutset123", 0),
        ("12345", 25),
        ("12345678", 50),
        ("!Abc12345678", 75),
        ("!Abc87654321", 100),
    ] {
        assert_eq!(
            validate_password_strength(Some(input), &policy),
            Some(score)
        );
    }
    assert_eq!(validate_password_strength(None, &policy), None);
}

#[test]
/// GROUPING 元数据、FTS 签名构建与函数参数 BuildParam 控制流。
fn grouping_fts_and_function_parameters_preserve_go_control_flow() {
    let mut grouping = GroupingSig::new();
    assert!(grouping.eval(1).is_err());
    grouping
        .set_metadata(
            GroupingMode::BitAnd,
            vec![HashSet::from([6]), HashSet::from([1])],
        )
        .unwrap();
    assert_eq!(grouping.eval(1).unwrap(), 2);
    assert_eq!(grouping.eval_many(&[1, 2, 4]).unwrap(), vec![2, 1, 1]);
    let cloned = grouping.clone();
    assert_eq!(cloned.metadata().unwrap().mode, GroupingMode::BitAnd);

    let mut numeric_cmp = GroupingSig::new();
    numeric_cmp
        .set_metadata(GroupingMode::NumericCmp, vec![HashSet::from([2])])
        .unwrap();
    assert_eq!(numeric_cmp.eval_many(&[0, 2, 3]).unwrap(), vec![1, 1, 0]);
    let mut numeric_set = GroupingSig::new();
    numeric_set
        .set_metadata(GroupingMode::NumericSet, vec![HashSet::from([1, 2])])
        .unwrap();
    assert_eq!(numeric_set.eval_many(&[1, 3]).unwrap(), vec![0, 1]);

    // 完整复现 Go TestGrouping 的模式、分组 ID 与期望结果表。
    for (grouping_id, mode, ids, expected) in [
        (1, GroupingMode::BitAnd, &[1][..], 0),
        (1, GroupingMode::BitAnd, &[3][..], 0),
        (1, GroupingMode::BitAnd, &[6][..], 1),
        (2, GroupingMode::BitAnd, &[1][..], 1),
        (2, GroupingMode::BitAnd, &[3][..], 0),
        (2, GroupingMode::BitAnd, &[6][..], 0),
        (4, GroupingMode::BitAnd, &[2][..], 1),
        (4, GroupingMode::BitAnd, &[4][..], 0),
        (4, GroupingMode::BitAnd, &[6][..], 0),
        (0, GroupingMode::NumericCmp, &[0][..], 1),
        (0, GroupingMode::NumericCmp, &[2][..], 1),
        (2, GroupingMode::NumericCmp, &[0][..], 0),
        (2, GroupingMode::NumericCmp, &[1][..], 0),
        (2, GroupingMode::NumericCmp, &[2][..], 1),
        (2, GroupingMode::NumericCmp, &[3][..], 1),
        (1, GroupingMode::NumericSet, &[1, 2][..], 0),
        (1, GroupingMode::NumericSet, &[2][..], 1),
        (2, GroupingMode::NumericSet, &[1, 3][..], 1),
        (2, GroupingMode::NumericSet, &[2, 3][..], 0),
    ] {
        let mut signature = GroupingSig::new();
        signature
            .set_metadata(mode, vec![ids.iter().copied().collect()])
            .unwrap();
        assert_eq!(signature.eval(grouping_id).unwrap(), expected);
    }

    let mut invalid = GroupingSig::new();
    assert!(
        invalid
            .set_metadata(GroupingMode::NumericCmp, vec![HashSet::from([1, 2])])
            .is_err()
    );
    assert!(invalid.eval(1).is_err());

    // 非 starter 部署模式禁止 FTS_MATCH_WORD。
    assert!(
        build_match_word(
            false,
            FtsAgainst::String("word".into()),
            &[MatchArgument::StringColumn]
        )
        .is_err()
    );
    let match_word = build_match_word(
        true,
        FtsAgainst::String("word".into()),
        &[MatchArgument::StringColumn],
    )
    .unwrap();
    assert!(match_word.fts_function_used());
    assert!(match_word.eval_real().is_err());

    let mysql =
        build_mysql_match_against(FtsAgainst::Null, &[MatchArgument::StringColumn]).unwrap();
    assert_eq!(mysql.eval_real().unwrap(), None);
    let mut signature = FtsSignature::Mysql(mysql);
    set_mysql_match_against_modifier(&mut signature, FulltextSearchModifier::Boolean).unwrap();
    assert_eq!(
        signature.mysql_modifier(),
        Some(FulltextSearchModifier::Boolean)
    );
    assert!(
        build_mysql_match_against(
            FtsAgainst::String("x".into()),
            &[MatchArgument::NonStringColumn]
        )
        .is_err()
    );

    let string_param = build_string_param(ParamSource::Constant(Some("abc".to_owned()))).unwrap();
    assert_eq!(string_param.value().unwrap().get(99).unwrap(), "abc");
    assert!(matches!(
        build_string_param(ParamSource::Constant(None)).unwrap(),
        BuildParam::ConstNull
    ));
    let int_param = build_int_param(ParamSource::NotProvided, 7).unwrap();
    assert_eq!(*int_param.value().unwrap().get(3).unwrap(), 7);
    let int_column = build_int_param(ParamSource::Column(vec![3, 5, 8]), 0).unwrap();
    assert_eq!(*int_column.value().unwrap().get(2).unwrap(), 8);
    assert!(int_column.value().unwrap().get(3).is_err());
    assert!(build_string_param(ParamSource::EvalError("boom".into())).is_err());
}
