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

// MySQL `mysql_native_password` 认证插件的密码摘要与 scramble 校验。
//
// 对照 `mysql_native_password.go`：实现双重 SHA-1 编码/解码，以及握手阶段
// 客户端令牌（scramble）与服务端存储的 stage2 摘要之间的校验。

// 本文件对照 pkg/parser/auth/mysql_native_password.go，实现本地摘要计算与握手令牌校验。

use sha1::{Digest, Sha1};

/// CheckScrambledPassword 校验客户端发来的 mysql_native_password scramble。
///
/// Go 对应协议：服务端发送随机盐，客户端返回
/// `SHA1(password) XOR SHA1(salt + SHA1(SHA1(password)))`；服务端据此恢复 stage1 并复算 stage2。
pub fn CheckScrambledPassword(salt: &[u8], hpwd: &[u8], auth: &[u8]) -> bool {
    // 先计算 SHA1(public_seed || stage2Hash)。Go 的 hash.Write 理论上不报错，
    // 但仍通过 terror.Log 记录错误；保留该错误处理形状。
    let mut crypt = Sha1::new();
    crypt.update(salt);
    crypt.update(hpwd);
    let mut hash = crypt.finalize().to_vec();

    // 长度不一致时不能逐字节异或，原 Go 实现立即拒绝认证。
    if auth.len() != hash.len() {
        return false;
    }
    for (hash_byte, auth_byte) in hash.iter_mut().zip(auth) {
        *hash_byte ^= *auth_byte;
    }

    // 异或恢复 stage1Hash，再次 SHA-1 后应与存储的 stage2Hash 完全相等。
    hpwd == Sha1Hash(&hash)
}

/// Sha1Hash 对应 Go 的 SHA-1 工具函数。
/// SHA-1 在这里是 MySQL 旧认证协议的兼容要求，不表示推荐用于新密码方案。
pub fn Sha1Hash(bytes: &[u8]) -> Vec<u8> {
    Sha1::digest(bytes).to_vec()
}

/// EncodePassword 把字符串明文编码成 `*` 加大写十六进制的双重 SHA-1。
pub fn EncodePassword(password: &str) -> String {
    if password.is_empty() {
        return String::new();
    }
    let hash1 = Sha1Hash(password.as_bytes());
    let hash2 = Sha1Hash(&hash1);

    format!("*{}", hex::encode_upper(hash2))
}

/// EncodePasswordBytes 与 EncodePassword 相同，但保持 Go `[]byte` 输入，避免任何 UTF-8 转换。
pub fn EncodePasswordBytes(password: &[u8]) -> String {
    if password.is_empty() {
        return String::new();
    }
    let hash1 = Sha1Hash(password);
    let hash2 = Sha1Hash(&hash1);

    format!("*{}", hex::encode_upper(hash2))
}

/// DecodePassword 解码带 `*` 前缀的十六进制密码摘要。
pub fn DecodePassword(password: &str) -> Result<Vec<u8>, hex::FromHexError> {
    // Go 的字符串下标按字节计算；从原始字节跳过首字节，避免多字节 UTF-8
    // 前缀在 Rust 字符边界检查处 panic。
    hex::decode(&password.as_bytes()[1..])
}
