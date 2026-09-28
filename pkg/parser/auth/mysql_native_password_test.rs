// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// `mysql_native_password` 编码、解码与 scramble 校验的单元测试。
//
// 对照 `mysql_native_password_test.go`：覆盖明文双重 SHA-1 字符串形态、
// 星号十六进制解码，以及固定 salt/auth 向量下的握手令牌校验。

use parser_auth::parser::auth::mysql_native_password::{
    CheckScrambledPassword, DecodePassword, EncodePassword, EncodePasswordBytes, Sha1Hash,
};

/// TestEncodePassword 对应 Go 测试：验证明文密码的 MySQL native hash 字符串，以及 []byte 入口与 string 入口一致。
// TestEncodePassword 对应 Go 测试：验证明文密码的 MySQL native hash 字符串，以及 []byte 入口与 string 入口一致。
#[test]
fn test_encode_password() {
    let pwd = "123";

    // Go 原测试使用 require.Equal；这里保留期望常量，便于人工核对编码结果。
    assert_eq!(
        "*23AE809DDACAF96AF0FD78ED04B6A265E05AA257",
        EncodePassword(pwd)
    );
    assert_eq!(EncodePasswordBytes(pwd.as_bytes()), EncodePassword(pwd));
}

/// TestDecodePassword 对应 Go 测试：把星号开头的十六进制密码解回双重 SHA1 摘要。
// TestDecodePassword 对应 Go 测试：把星号开头的十六进制密码解回双重 SHA1 摘要。
#[test]
fn test_decode_password() {
    let x = DecodePassword(&EncodePassword("123")).unwrap();

    // Go 断言 DecodePassword 的结果等于 Sha1Hash(Sha1Hash([]byte("123")))。
    assert_eq!(Sha1Hash(&Sha1Hash(b"123")), x);
}

/// TestCheckScramble 对应 Go 测试：使用固定 salt、hash password 和 auth response 验证 scramble 校验。
// TestCheckScramble 对应 Go 测试：使用固定 salt、hash password 和 auth response 验证 scramble 校验。
#[test]
fn test_check_scramble() {
    let pwd = "abc";
    let salt = vec![
        85, 92, 45, 22, 58, 79, 107, 6, 122, 125, 58, 80, 12, 90, 103, 32, 90, 10, 74, 82,
    ];
    let auth = vec![
        24, 180, 183, 225, 166, 6, 81, 102, 70, 248, 199, 143, 91, 204, 169, 9, 161, 171, 203, 33,
    ];
    let encodepwd = EncodePassword(pwd);
    let hpwd = DecodePassword(&encodepwd).unwrap();

    let res = CheckScrambledPassword(&salt, &hpwd, &auth);
    assert!(res);

    // Go 原注释：Do not panic for invalid input。这里保留“非法 auth 不应 panic，只返回 false”的错误处理语义。
    let res = CheckScrambledPassword(&salt, &hpwd, b"xxyyzz");
    assert!(!res);
}
