// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `caching_sha2` 散列校验与生成的单元测试，对照 Go `caching_sha2_test.go`。
//
// 覆盖正确/错误口令、过短哈希、摘要类型不兼容、迭代次数非法，
// 以及带随机盐的新哈希往返校验与盐字节约束。

use parser_auth::parser::auth::caching_sha2::{CheckHashingPassword, NewHashPassword};
use parser_auth::parser::mysql::r#const::AuthCachingSha2Password;

// foobarPwdSHA2Hash 对应 Go 包级变量；hex 解码错误在 Go 中被空白标识符忽略。
/// 预置的 `foobar` 口令对应 caching_sha2 认证字符串（与 Go 测试向量一致）。
static FOOBAR_PWD_SHA2_HASH: &[u8; 70] = &hex_literal::hex!(
    "24412430303524031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537"
);

// test_check_sha_password_good 对应 Go 的 TestCheckShaPasswordGood。
// 使用正确明文密码，期望 CheckHashingPassword 返回 true 且无错误。
/// 正确明文应匹配预置哈希。
#[test]
fn test_check_sha_password_good() {
    let pwd = "foobar";
    let matched = CheckHashingPassword(FOOBAR_PWD_SHA2_HASH, pwd, AuthCachingSha2Password).unwrap();
    assert!(matched);
}

// test_check_sha_password_bad 对应 Go 的 TestCheckShaPasswordBad。
// 哈希格式合法但明文不匹配，期望无错误且匹配结果为 false。
/// 明文错误时返回 Ok(false)，而非解析错误。
#[test]
fn test_check_sha_password_bad() {
    let pwd = "not_foobar";
    let pwhash = hex::decode(
        "24412430303524031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537",
    ).unwrap();
    let matched = CheckHashingPassword(&pwhash, pwd, AuthCachingSha2Password).unwrap();
    assert!(!matched);
}

// test_check_sha_password_short 对应 Go 的 TestCheckShaPasswordShort。
// 输入过短哈希，保留解析错误路径断言。
/// 过短输入应走解析失败路径。
#[test]
fn test_check_sha_password_short() {
    let pwd = "not_foobar";
    let pwhash = hex::decode("aaaaaaaa").unwrap();
    assert!(CheckHashingPassword(&pwhash, pwd, AuthCachingSha2Password).is_err());
}

// test_check_sha_password_digest_type_incompatible 对应 Go 的 TestCheckShaPasswordDigestTypeIncompatible。
// 修改 digest type 字节，期望 hashing password 解析阶段报错。
/// 摘要类型非 `A` 时解析报错。
#[test]
fn test_check_sha_password_digest_type_incompatible() {
    let pwd = "not_foobar";
    let pwhash = hex::decode(
        "24422430303524031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537",
    ).unwrap();
    assert!(CheckHashingPassword(&pwhash, pwd, AuthCachingSha2Password).is_err());
}

// test_check_sha_password_iterations_invalid 对应 Go 的 TestCheckShaPasswordIterationsInvalid。
// 修改迭代数字段，期望返回错误而不是误判密码。
/// 非法迭代次数字段应返回错误。
#[test]
fn test_check_sha_password_iterations_invalid() {
    let pwd = "not_foobar";
    let pwhash = hex::decode(
        "24412430304724031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537",
    ).unwrap();
    assert!(CheckHashingPassword(&pwhash, pwd, AuthCachingSha2Password).is_err());
}

// The output from NewHashPassword is not stable as the hash is based on the generated salt.
// This is why CheckHashingPassword is used here.
// test_new_sha2_password 对应 Go 的 TestNewSha2Password。
// 新哈希包含随机 salt，因此用 CheckHashingPassword 回验，并检查 ASCII/NUL 与分隔符结构。
/// 新生成哈希可回验，且盐字节满足 ASCII/非 NUL/`$` 分段约束。
#[test]
fn test_new_sha2_password() {
    let pwd = "testpwd";
    let pwhash = NewHashPassword(pwd, AuthCachingSha2Password);
    let matched = CheckHashingPassword(pwhash.as_bytes(), pwd, AuthCachingSha2Password).unwrap();
    assert!(matched);

    for byte in pwhash.bytes() {
        assert!(byte < 128u8);
        assert_ne!(byte, 0u8); // NUL
    }
    assert_eq!(pwhash.bytes().filter(|byte| *byte == b'$').count(), 3);
}

// benchmark_sha_password 对应 Go 的 BenchmarkShaPassword。
// Rust 保留循环形状，表达每轮重复校验同一 foobar 哈希的基准意图。
/// 基准体：重复校验同一 foobar 哈希（保留 Go 基准意图）。
#[test]
fn benchmark_sha_password_body() {
    let matched =
        CheckHashingPassword(FOOBAR_PWD_SHA2_HASH, "foobar", AuthCachingSha2Password).unwrap();
    assert!(matched);
}
