// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TiDB SM3 摘要与 `tidb_sm3_password` 哈希校验的单元测试。
//
// 对照 `tidb_sm3_test.go`：覆盖固定向量摘要、正确/错误密码校验、
// 过短 hash、不兼容 digest type、非法迭代轮数，以及新密码生成结构。

use parser_auth::parser::auth::caching_sha2::{CheckHashingPassword, NewHashPassword};
use parser_auth::parser::auth::tidb_sm3::{NewSM3, Sm3Hash};
use parser_auth::parser::mysql::r#const::AuthTiDBSM3Password;

/// foobarPwdSM3Hash 对应 Go 包级变量：保存 foobar 的已知 SM3 密码 hash fixture。
// foobarPwdSM3Hash 对应 Go 包级变量：保存 foobar 的已知 SM3 密码 hash fixture。
// hex 解码错误在 Go 中被忽略；沿用“fixture 必须能解码”的测试前提。
fn foobar_pwd_sm3_hash() -> Vec<u8> {
    hex::decode("24412430303524031a69251c34295c4b35167c7f1e5a7b63091349536c72627066426a635061762e556e6c63533159414d7762317261324a5a3047756b4244664177434e3043").unwrap()
}

/// SM3 固定向量用例：明文与期望十六进制摘要。
struct Sm3Case {
    /// 待哈希的输入文本。
    text: &'static str,
    /// 期望的 SM3 摘要十六进制。
    expect_hex: &'static str,
}

/// TestSM3 对应 Go 测试：校验 SM3 对短文本和长文本的固定摘要。
// TestSM3 对应 Go 测试：校验 SM3 对短文本和长文本的固定摘要。
#[test]
fn test_sm3() {
    let test_cases = [
        Sm3Case {
            text: "abc",
            expect_hex: "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0",
        },
        Sm3Case {
            text: "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd",
            expect_hex: "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732",
        },
    ];

    for test_case in test_cases {
        let expect = hex::decode(test_case.expect_hex).unwrap();
        let result = Sm3Hash(test_case.text.as_bytes());
        assert_eq!(expect, result);
    }
}

/// Go 的 `Sum(in)` 会先把 `in` 写入状态，但实际返回值只包含固定长度摘要。
#[test]
fn sum_with_input_returns_only_digest_and_updates_state() {
    let mut hash = NewSM3();
    let result = hash.Sum(b"abc");
    let expected = Sm3Hash(b"abc");

    assert_eq!(32, result.len());
    assert_eq!(expected, result);
    assert_eq!(expected, hash.Sum(&[]));
}

/// TestCheckSM3PasswordGood 对应 Go 测试：正确密码应通过 TiDB SM3 hash 校验。
// TestCheckSM3PasswordGood 对应 Go 测试：正确密码应通过 TiDB SM3 hash 校验。
#[test]
fn test_check_sm3_password_good() {
    let pwd = "foobar";
    let r = CheckHashingPassword(&foobar_pwd_sm3_hash(), pwd, AuthTiDBSM3Password).unwrap();
    assert!(r);
}

/// TestCheckSM3PasswordBad 对应 Go 测试：同一格式的 hash 遇到错误密码应返回 false 且不报错。
// TestCheckSM3PasswordBad 对应 Go 测试：同一格式的 hash 遇到错误密码应返回 false 且不报错。
#[test]
fn test_check_sm3_password_bad() {
    let pwd = "not_foobar";
    let pwhash = hex::decode("24412430303524031a69251c34295c4b35167c7f1e5a7b6309134956387565426743446d3643446176712f6c4b63323667346e48624872776f39512e4342416a693656676f2f").unwrap();
    let r = CheckHashingPassword(&pwhash, pwd, AuthTiDBSM3Password).unwrap();
    assert!(!r);
}

/// TestCheckSM3PasswordShort 对应 Go 测试：过短 hash 属于格式错误，应返回 error。
// TestCheckSM3PasswordShort 对应 Go 测试：过短 hash 属于格式错误，应返回 error。
#[test]
fn test_check_sm3_password_short() {
    let pwd = "not_foobar";
    let pwhash = hex::decode("aaaaaaaa").unwrap();
    assert!(CheckHashingPassword(&pwhash, pwd, AuthTiDBSM3Password).is_err());
}

/// TestCheckSM3PasswordDigestTypeIncompatible 对应 Go 测试：digest type 与 TiDB SM3 不兼容时应报错。
// TestCheckSM3PasswordDigestTypeIncompatible 对应 Go 测试：digest type 与 TiDB SM3 不兼容时应报错。
#[test]
fn test_check_sm3_password_digest_type_incompatible() {
    let pwd = "not_foobar";
    let pwhash = hex::decode("24432430303524031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537").unwrap();
    assert!(CheckHashingPassword(&pwhash, pwd, AuthTiDBSM3Password).is_err());
}

/// TestCheckSM3PasswordIterationsInvalid 对应 Go 测试：迭代轮数编码非法时应报错。
// TestCheckSM3PasswordIterationsInvalid 对应 Go 测试：迭代轮数编码非法时应报错。
#[test]
fn test_check_sm3_password_iterations_invalid() {
    let pwd = "not_foobar";
    let pwhash = hex::decode("24412430304724031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537").unwrap();
    assert!(CheckHashingPassword(&pwhash, pwd, AuthTiDBSM3Password).is_err());
}

/// TestNewSM3Password 对应 Go 测试：新生成的 SM3 密码 hash 可回验，并检查 ASCII/NUL 与分隔符结构。
// TestNewSM3Password 对应 Go 测试：新生成的 SM3 密码 hash 可回验，并检查 ASCII/NUL 与分隔符结构。
#[test]
fn test_new_sm3_password() {
    let pwd = "testpwd";
    let pwhash = NewHashPassword(pwd, AuthTiDBSM3Password);
    let r = CheckHashingPassword(pwhash.as_bytes(), pwd, AuthTiDBSM3Password).unwrap();
    assert!(r);

    for byte in pwhash.bytes() {
        // Go 遍历字符串字节，约束生成内容是非 NUL 的 ASCII 字节。
        assert!(byte < 128);
        assert_ne!(byte, 0); // NUL
    }
    assert_eq!(pwhash.bytes().filter(|byte| *byte == b'$').count(), 3);
}

/// BenchmarkSM3Password 对应 Go benchmark：重复校验固定 foobar SM3 hash。
// BenchmarkSM3Password 对应 Go benchmark：重复校验固定 foobar SM3 hash。
// 这里保留 b.N 循环形状；真实 benchmark runner 尚未迁入 Rust。
#[test]
fn benchmark_sm3_password_body() {
    let matched =
        CheckHashingPassword(&foobar_pwd_sm3_hash(), "foobar", AuthTiDBSM3Password).unwrap();
    assert!(matched);
}
