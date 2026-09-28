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

// auth 包迁移对齐综合测试：身份显示/还原与三类密码插件向量。
//
// 一次性覆盖 UserIdentity/RoleIdentity、mysql_native_password、SM3，
// 以及 caching_sha2 / tidb_sm3 的校验、错误路径与随机盐往返。

use parser_auth::parser::auth::auth::{
    RoleIdentity, UserIdentity, optional_identity_string, optional_login_string,
};
use parser_auth::parser::auth::caching_sha2::{CheckHashingPassword, NewHashPassword};
use parser_auth::parser::auth::mysql_native_password::{
    CheckScrambledPassword, DecodePassword, EncodePassword, EncodePasswordBytes, Sha1Hash,
};
use parser_auth::parser::auth::tidb_sm3::{NewSM3, Sm3Hash};
use parser_auth::parser::format::{DefaultRestoreFlags, NewRestoreCtx};

/// caching_sha2_password 插件名常量。
const CACHING_SHA2: &str = "caching_sha2_password";
/// tidb_sm3_password 插件名常量。
const TIDB_SM3: &str = "tidb_sm3_password";

/// 用户/角色身份的显示字符串与 Restore 结果应与 Go 规则一致。
#[test]
fn identities_match_go_display_and_restore_rules() {
    let user = UserIdentity {
        username: "login`name".into(),
        hostname: "host".into(),
        current_user: false,
        auth_username: "matched".into(),
        auth_hostname: "%".into(),
        auth_plugin: CACHING_SHA2.into(),
    };
    // AuthIdentity 优先于登录身份；None 对应 Go nil 得到空串。
    assert_eq!(user.identity_string(), "matched@%");
    assert_eq!(user.login_string(), "login`name@host");
    assert_eq!(optional_identity_string(None), "");
    assert_eq!(optional_login_string(None), "");

    // Restore 需转义标识符中的反引号。
    let mut output = Vec::new();
    user.restore(&mut NewRestoreCtx(DefaultRestoreFlags, &mut output))
        .unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "`login``name`@`host`");

    // 空主机名：SQL 还原不输出 @，但 role_string 仍保留 `@`` 形态。
    let role = RoleIdentity {
        username: "reader".into(),
        hostname: String::new(),
    };
    let mut output = Vec::new();
    role.restore(&mut NewRestoreCtx(DefaultRestoreFlags, &mut output))
        .unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "`reader`");
    assert_eq!(role.role_string(), "`reader`@``");
}

/// `CURRENT_USER` 还原为关键字本身，不附带 @host。
#[test]
fn current_user_restore_matches_go() {
    let user = UserIdentity {
        username: String::new(),
        hostname: String::new(),
        current_user: true,
        auth_username: String::new(),
        auth_hostname: String::new(),
        auth_plugin: String::new(),
    };
    let mut output = Vec::new();
    user.restore(&mut NewRestoreCtx(DefaultRestoreFlags, &mut output))
        .unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "CURRENT_USER");
}

/// mysql_native_password 编码/解码与 scramble 校验向量对齐 Go。
#[test]
fn mysql_native_password_vectors_match_go() {
    assert_eq!(
        EncodePassword("123"),
        "*23AE809DDACAF96AF0FD78ED04B6A265E05AA257"
    );
    assert_eq!(EncodePasswordBytes(b"123"), EncodePassword("123"));
    assert_eq!(EncodePassword(""), "");
    assert_eq!(
        DecodePassword(&EncodePassword("123")).unwrap(),
        Sha1Hash(&Sha1Hash(b"123"))
    );
    // Go 按字节跳过首字符；多字节 UTF-8 前缀不会触发切片 panic，而是返回 hex 解码错误。
    assert!(DecodePassword("é23").is_err());

    // 握手盐与客户端认证响应的固定测试向量。
    let salt = [
        85, 92, 45, 22, 58, 79, 107, 6, 122, 125, 58, 80, 12, 90, 103, 32, 90, 10, 74, 82,
    ];
    let auth = [
        24, 180, 183, 225, 166, 6, 81, 102, 70, 248, 199, 143, 91, 204, 169, 9, 161, 171, 203, 33,
    ];
    let hpwd = DecodePassword(&EncodePassword("abc")).unwrap();
    assert!(CheckScrambledPassword(&salt, &hpwd, &auth));
    assert!(!CheckScrambledPassword(&salt, &hpwd, b"xxyyzz"));
}

/// SM3 标准向量与流式 Write/Sum 结果应与一次性摘要一致。
#[test]
fn sm3_vectors_and_streaming_match_go() {
    assert_eq!(
        hex::encode(Sm3Hash(b"abc")),
        "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
    );
    assert_eq!(
        hex::encode(Sm3Hash(
            b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd"
        )),
        "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732"
    );
    let mut streaming = NewSM3();
    streaming.Write(b"a").unwrap();
    streaming.Write(b"bc").unwrap();
    assert_eq!(streaming.Sum(&[]), Sm3Hash(b"abc"));
}

/// caching_sha2 / tidb_sm3 已知向量与格式错误路径对齐 Go。
#[test]
fn hashing_password_vectors_and_errors_match_go() {
    let sha2 = hex::decode("24412430303524031A69251C34295C4B35167C7F1E5A7B63091349503974624D34504B5A424679354856336868686F52485A736E4A733368786E427575516C73446469496537").unwrap();
    let sm3 = hex::decode("24412430303524031a69251c34295c4b35167c7f1e5a7b63091349536c72627066426a635061762e556e6c63533159414d7762317261324a5a3047756b4244664177434e3043").unwrap();
    assert!(CheckHashingPassword(&sha2, "foobar", CACHING_SHA2).unwrap());
    assert!(!CheckHashingPassword(&sha2, "not_foobar", CACHING_SHA2).unwrap());
    assert!(CheckHashingPassword(&sm3, "foobar", TIDB_SM3).unwrap());
    assert!(CheckHashingPassword(b"aaaaaaaa", "x", CACHING_SHA2).is_err());
    assert!(CheckHashingPassword(b"$B$005$abcdefghijklmnopqrsthash", "x", CACHING_SHA2).is_err());
    assert!(CheckHashingPassword(b"$A$00G$abcdefghijklmnopqrsthash", "x", CACHING_SHA2).is_err());
}

/// 新哈希可往返校验；盐为 7 位 ASCII 且不含 NUL；未知插件返回空串。
#[test]
fn generated_hashes_round_trip_and_use_safe_salts() {
    for plugin in [CACHING_SHA2, TIDB_SM3] {
        let encoded = NewHashPassword("testpwd", plugin);
        assert!(CheckHashingPassword(encoded.as_bytes(), "testpwd", plugin).unwrap());
        // 盐与摘要编码均限制为非 NUL 的 7 位字节；`$` 恰好三段分隔符共 3 个。
        assert!(encoded.bytes().all(|byte| byte < 128 && byte != 0));
        assert_eq!(encoded.bytes().filter(|byte| *byte == b'$').count(), 3);
    }
    assert_eq!(NewHashPassword("testpwd", "unknown"), "");
}
