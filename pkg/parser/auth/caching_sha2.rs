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

// `caching_sha2_password` / `tidb_sm3_password` 散列生成与校验。
//
// 对照 `caching_sha2.go`，按 SHA-crypt 规范混合口令与盐，输出 `$A$轮数$盐+摘要`
// 形态的认证字符串。摘要算法可选 SHA-256 或 SM3（国密哈希）。

// 本文件对照 pkg/parser/auth/caching_sha2.go，实现 caching_sha2_password 与
// tidb_sm3_password 的散列生成和校验。
// Resources:
// - https://dev.mysql.com/doc/refman/8.0/en/caching-sha2-pluggable-authentication.html
// - https://dev.mysql.com/doc/dev/mysql-server/latest/page_caching_sha2_authentication_exchanges.html
// - https://dev.mysql.com/doc/dev/mysql-server/latest/namespacesha2__password.html
// - https://www.akkadia.org/drepper/SHA-crypt.txt
// - https://dev.mysql.com/worklog/task/?id=9591
// 密文以 `$` 分隔为摘要类型 `A`、缩小后的迭代次数、以及 salt+hash 三段。

/// MIXCHARS 对应 Go 常量：SHA-256/SM3 每轮混合使用的摘要字节数。
use super::tidb_sm3::Sm3Hash;
use crate::parser::mysql::r#const::{AuthCachingSha2Password, AuthTiDBSM3Password};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const MIXCHARS: usize = 32;
/// SALT_LENGTH 对应 Go 常量：认证字符串中盐的固定长度。
pub const SALT_LENGTH: usize = 20;
/// ITERATION_MULTIPLIER 对应 Go 常量：认证字符串中的轮数单位。
pub const ITERATION_MULTIPLIER: usize = 1000;

/// b64From24bit 对应 Go 的自定义 crypt-base64 编码。
/// 输入固定取三个字节，并按低六位优先的顺序向输出追加 n 个字符。
fn b64From24bit(b: &[u8], mut n: usize, buf: &mut Vec<u8>) {
    let b64t = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut w = ((b[0] as i64) << 16) | ((b[1] as i64) << 8) | b[2] as i64;
    while n > 0 {
        n -= 1;
        buf.push(b64t[(w & 0x3f) as usize]);
        w >>= 6;
    }
}

/// Sha256Hash 对应 Go 的 SHA-256 工具函数，返回固定 32 字节摘要的动态切片形状。
pub fn Sha256Hash(input: &[u8]) -> Vec<u8> {
    Sha256::digest(input).to_vec()
}

/// hashCrypt 对应 Go 的 SHA-crypt 主流程。
/// `hash` 必须像 SHA-256 一样返回 32 字节；调用方负责选择 SHA-256 或 SM3。
fn hashCrypt(
    plaintext: &str,
    salt: &[u8],
    iterations: usize,
    hash: fn(&[u8]) -> Vec<u8>,
) -> String {
    // 以下编号沿用 SHA-crypt 规范，方便逐段核对 Go 源码。
    // 1、2、3：A 缓冲区先拼入口令与盐。
    let mut buf_a = Vec::with_capacity(4096);
    buf_a.extend_from_slice(plaintext.as_bytes());
    buf_a.extend_from_slice(salt);

    // 4～8：B 摘要使用 password+salt+password，随后释放临时缓冲区。
    let mut buf_b = Vec::with_capacity(4096);
    buf_b.extend_from_slice(plaintext.as_bytes());
    buf_b.extend_from_slice(salt);
    buf_b.extend_from_slice(plaintext.as_bytes());
    let sum_b = hash(&buf_b);

    // 9、10：按口令长度循环混入完整摘要，最后写入不足 32 字节的尾段。
    let mut i = plaintext.len();
    while i > MIXCHARS {
        buf_a.extend_from_slice(&sum_b[..MIXCHARS]);
        i -= MIXCHARS;
    }
    buf_a.extend_from_slice(&sum_b[..i]);

    // 11：根据口令长度各二进制位的奇偶选择口令或 B 摘要。
    i = plaintext.len();
    while i > 0 {
        if i % 2 == 0 {
            buf_a.extend_from_slice(plaintext.as_bytes());
        } else {
            buf_a.extend_from_slice(&sum_b);
        }
        i >>= 1;
    }

    // 12：得到第一轮 A 摘要。
    let mut sum_a = hash(&buf_a);

    // 13～15：口令每个字节对应一次完整口令写入，形成 DP 摘要。
    let mut buf_dp = Vec::with_capacity(plaintext.len() * plaintext.len());
    for _ in plaintext.as_bytes() {
        buf_dp.extend_from_slice(plaintext.as_bytes());
    }
    let sum_dp = hash(&buf_dp);

    // 16：把 DP 摘要重复/截断到与口令相同的字节长度。
    let mut p = Vec::with_capacity(MIXCHARS);
    i = plaintext.len();
    while i > 0 {
        let take = i.min(MIXCHARS);
        p.extend_from_slice(&sum_dp[..take]);
        i = i.saturating_sub(MIXCHARS);
    }

    // 17～19：写入 16+sumA[0] 份盐，生成 DS 摘要。
    let mut buf_ds = Vec::new();
    for _ in 0..(16 + sum_a[0] as usize) {
        buf_ds.extend_from_slice(salt);
    }
    let sum_ds = hash(&buf_ds);

    // 20：把 DS 摘要重复/截断到盐的长度。
    let mut s = Vec::with_capacity(MIXCHARS);
    i = salt.len();
    while i > 0 {
        let take = i.min(MIXCHARS);
        s.extend_from_slice(&sum_ds[..take]);
        i = i.saturating_sub(MIXCHARS);
    }

    // 21：每轮依据轮号的奇偶以及 3、7 的整除关系选择混合材料。
    let mut sum_c = Vec::new();
    for round in 0..iterations {
        let mut buf_c = Vec::new();
        if round & 1 != 0 {
            buf_c.extend_from_slice(&p);
        } else {
            buf_c.extend_from_slice(&sum_a);
        }
        if round % 3 != 0 {
            buf_c.extend_from_slice(&s);
        }
        if round % 7 != 0 {
            buf_c.extend_from_slice(&p);
        }
        if round & 1 != 0 {
            buf_c.extend_from_slice(&sum_a);
        } else {
            buf_c.extend_from_slice(&p);
        }
        sum_c = hash(&buf_c);
        sum_a = sum_c.clone();
    }

    // 22：输出 `$A$轮数$盐`，再按 MySQL 指定的摘要字节置换编码。
    let mut buf = Vec::with_capacity(100);
    buf.extend_from_slice(b"$A$");
    buf.extend_from_slice(format!("{:03X}", iterations / ITERATION_MULTIPLIER).as_bytes());
    buf.push(b'$');
    buf.extend_from_slice(salt);
    for triple in [
        [sum_c[0], sum_c[10], sum_c[20]],
        [sum_c[21], sum_c[1], sum_c[11]],
        [sum_c[12], sum_c[22], sum_c[2]],
        [sum_c[3], sum_c[13], sum_c[23]],
        [sum_c[24], sum_c[4], sum_c[14]],
        [sum_c[15], sum_c[25], sum_c[5]],
        [sum_c[6], sum_c[16], sum_c[26]],
        [sum_c[27], sum_c[7], sum_c[17]],
        [sum_c[18], sum_c[28], sum_c[8]],
        [sum_c[9], sum_c[19], sum_c[29]],
    ] {
        b64From24bit(&triple, 4, &mut buf);
    }
    b64From24bit(&[0, sum_c[31], sum_c[30]], 3, &mut buf);

    // Go 的 bytes.Buffer.String 不校验 UTF-8；这里沿用其字节到字符串的直接转换意图。
    String::from_utf8_lossy(&buf).into_owned()
}

/// CheckHashingPassword 校验 caching_sha2_password 或 tidb_sm3_password 认证字符串。
pub fn CheckHashingPassword(
    pwhash: &[u8],
    password: &str,
    hash_name: &str,
) -> Result<bool, String> {
    let parts: Vec<&[u8]> = pwhash.split(|b| *b == b'$').collect();
    if parts.len() != 4 {
        return Err("failed to decode hash parts".to_owned());
    }
    if parts[1] != b"A" {
        return Err("digest type is incompatible".to_owned());
    }

    // 轮数以十六进制存储；解析失败必须与格式错误区分返回。
    let encoded_rounds =
        std::str::from_utf8(parts[2]).map_err(|_| "failed to decode iterations")?;
    let encoded_iterations = i64::from_str_radix(encoded_rounds, 16)
        .map_err(|_| "failed to decode iterations".to_owned())?
        .wrapping_mul(ITERATION_MULTIPLIER as i64);
    // Go 先在 int64 中做二补码乘法，再转为平台 int；负数 range 执行零轮。
    let iterations = usize::try_from(encoded_iterations).unwrap_or(0);
    // Go 直接取前 20 字节；过短输入会 panic，保留这一前置格式约束。
    let salt = &parts[3][..SALT_LENGTH];

    let new_hash = match hash_name {
        AuthCachingSha2Password => hashCrypt(password, salt, iterations, Sha256Hash),
        AuthTiDBSM3Password => hashCrypt(password, salt, iterations, Sm3Hash),
        _ => String::new(),
    };
    Ok(pwhash == new_hash.as_bytes())
}

/// NewHashPassword 为 caching_sha2_password 或 tidb_sm3_password 生成带随机盐的新认证字符串。
pub fn NewHashPassword(password: &str, hash_name: &str) -> String {
    let mut salt = [0_u8; SALT_LENGTH];
    // Go 忽略 rand.Read 的错误；保留调用形状，不在这里引入新的错误返回。
    rand::rngs::OsRng.fill_bytes(&mut salt);

    // 盐限制为 7 位，避免多字节 UTF-8；`$` 与 NUL 会破坏认证字符串分段，因此逐字节重抽。
    for byte in &mut salt {
        *byte &= !128;
        while *byte == b'$' || *byte == 0 {
            let mut replacement = [0_u8; 1];
            rand::rngs::OsRng.fill_bytes(&mut replacement);
            *byte = replacement[0] & !128;
        }
    }

    match hash_name {
        AuthCachingSha2Password => hashCrypt(password, &salt, 5 * ITERATION_MULTIPLIER, Sha256Hash),
        AuthTiDBSM3Password => hashCrypt(password, &salt, 5 * ITERATION_MULTIPLIER, Sm3Hash),
        _ => String::new(),
    }
}
