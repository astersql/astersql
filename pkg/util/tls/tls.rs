// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TLS 协议版本与密码套件的 MySQL/OpenSSL 兼容命名。
//
// 提供 `VersionName` / `CipherSuiteName`、支持套件集合 `SupportCipher`，
// 以及进程级 `RequireSecureTransport` 开关（对应 Go atomic.Bool）。

#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;

// These values are defined by the TLS protocol and match Go's crypto/tls constants.
/// TLS 1.2 协议版本号（与 Go crypto/tls 一致）。
const VERSION_TLS12: u16 = 0x0303;
/// TLS 1.3 协议版本号。
const VERSION_TLS13: u16 = 0x0304;

// These cipher suite values match Go's crypto/tls constants.
// 下列常量与 Go crypto/tls cipher suite 数值一一对应。
const TLS_RSA_WITH_RC4_128_SHA: u16 = 0x0005;
const TLS_RSA_WITH_3DES_EDE_CBC_SHA: u16 = 0x000a;
const TLS_RSA_WITH_AES_128_CBC_SHA: u16 = 0x002f;
const TLS_RSA_WITH_AES_256_CBC_SHA: u16 = 0x0035;
const TLS_RSA_WITH_AES_128_CBC_SHA256: u16 = 0x003c;
const TLS_RSA_WITH_AES_128_GCM_SHA256: u16 = 0x009c;
const TLS_RSA_WITH_AES_256_GCM_SHA384: u16 = 0x009d;
const TLS_ECDHE_ECDSA_WITH_RC4_128_SHA: u16 = 0xc007;
const TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA: u16 = 0xc009;
const TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA: u16 = 0xc00a;
const TLS_ECDHE_RSA_WITH_RC4_128_SHA: u16 = 0xc011;
const TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA: u16 = 0xc012;
const TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA: u16 = 0xc013;
const TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA: u16 = 0xc014;
const TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256: u16 = 0xc023;
const TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256: u16 = 0xc027;
const TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: u16 = 0xc02f;
const TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256: u16 = 0xc02b;
const TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384: u16 = 0xc030;
const TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384: u16 = 0xc02c;
const TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305: u16 = 0xcca8;
const TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305: u16 = 0xcca9;
const TLS_AES_128_GCM_SHA256: u16 = 0x1301;
const TLS_AES_256_GCM_SHA384: u16 = 0x1302;
const TLS_CHACHA20_POLY1305_SHA256: u16 = 0x1303;

// RequireSecureTransport Process global variables
// RequireSecureTransport 对应 Go 的 atomic.NewBool(false) 全局变量。
// 原 Go 值可被不同 goroutine 并发读写；用 AtomicBool 保留这个跨线程可见的开关形状。
/// 是否强制安全传输的全局开关（Go `atomic.NewBool(false)`）。
pub static RequireSecureTransport: AtomicBool = AtomicBool::new(false);

// Taken from https://github.com/openssl/openssl/blob/c784a838e0947fcca761ee62def7d077dc06d37f/include/openssl/ssl.h#L141 .
// Update: remove tlsv1.0 and v1.1 support
// versionString 保留 Go map[uint16]string 的查表语义，只列出 TiDB 当前暴露的 TLS 1.2/1.3 名称。
/// TiDB 对外暴露的 TLS 1.2/1.3 版本名查表。
static versionString: LazyLock<HashMap<u16, &'static str>> =
    LazyLock::new(|| HashMap::from([(VERSION_TLS12, "TLSv1.2"), (VERSION_TLS13, "TLSv1.3")]));

// tlsCipherString is mapping cipher suites to MySQL/OpenSSL compatible names
// See `openssl ciphers -stdname -v 'ALL'` for mapping info.
// tlsCipherString 对应 Go 的 map[uint16]string，键保持 crypto/tls cipher suite 数值，值保持 MySQL/OpenSSL 兼容名称。
/// cipher suite 编号 → MySQL/OpenSSL 兼容名称。
static tlsCipherString: LazyLock<HashMap<u16, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        // TLS 1.0 - 1.2 cipher suites, mysql compatible names
        (TLS_RSA_WITH_RC4_128_SHA, "RC4-SHA"),
        (TLS_RSA_WITH_3DES_EDE_CBC_SHA, "DES-CBC3-SHA"),
        (TLS_RSA_WITH_AES_128_CBC_SHA, "AES128-SHA"),
        (TLS_RSA_WITH_AES_256_CBC_SHA, "AES256-SHA"),
        (TLS_RSA_WITH_AES_128_CBC_SHA256, "AES128-SHA256"),
        (TLS_RSA_WITH_AES_128_GCM_SHA256, "AES128-GCM-SHA256"),
        (TLS_RSA_WITH_AES_256_GCM_SHA384, "AES256-GCM-SHA384"),
        (TLS_ECDHE_ECDSA_WITH_RC4_128_SHA, "ECDHE-ECDSA-RC4-SHA"),
        (
            TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA,
            "ECDHE-ECDSA-AES128-SHA",
        ),
        (
            TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA,
            "ECDHE-ECDSA-AES256-SHA",
        ),
        (TLS_ECDHE_RSA_WITH_RC4_128_SHA, "ECDHE-RSA-RC4-SHA"),
        (
            TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA,
            "ECDHE-RSA-DES-CBC3-SHA",
        ),
        (TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA, "ECDHE-RSA-AES128-SHA"),
        (TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA, "ECDHE-RSA-AES256-SHA"),
        (
            TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256,
            "ECDHE-ECDSA-AES128-SHA256",
        ),
        (
            TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256,
            "ECDHE-RSA-AES128-SHA256",
        ),
        (
            TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            "ECDHE-RSA-AES128-GCM-SHA256",
        ),
        (
            TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            "ECDHE-ECDSA-AES128-GCM-SHA256",
        ),
        (
            TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
            "ECDHE-RSA-AES256-GCM-SHA384",
        ),
        (
            TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            "ECDHE-ECDSA-AES256-GCM-SHA384",
        ),
        (
            TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305,
            "ECDHE-RSA-CHACHA20-POLY1305",
        ),
        (
            TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305,
            "ECDHE-ECDSA-CHACHA20-POLY1305",
        ),
        // TLS 1.3 cipher suites, compatible with mysql using '_'.
        (TLS_AES_128_GCM_SHA256, "TLS_AES_128_GCM_SHA256"),
        (TLS_AES_256_GCM_SHA384, "TLS_AES_256_GCM_SHA384"),
        (TLS_CHACHA20_POLY1305_SHA256, "TLS_CHACHA20_POLY1305_SHA256"),
    ])
});

// SupportCipher maintains cipher supported by TiDB.
// SupportCipher 对应 Go init 中由 tlsCipherString 反向填充的 map[string]struct{}。
// LazyLock 延迟执行初始化，保留 Go 包加载时构造集合的语义，同时避免在静态初始化中写循环。
/// TiDB 支持的密码套件名称集合（由 tlsCipherString 反向填充）。
pub static SupportCipher: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    // 对应 Go 的 `for _, value := range tlsCipherString`：只关心 cipher 名称是否存在，空 struct 在 Rust 中迁移成 HashSet。
    tlsCipherString.values().copied().collect()
});

// VersionName is like `tls.VersionName()` from crypto/tls, but tries to match the names in MySQL/OpenSSL
// VersionName 先按 TiDB/MySQL/OpenSSL 兼容命名查找 TLS 版本；未知版本再退回 Go crypto/tls.VersionName 风格。
/// 返回 TLS 版本的 MySQL/OpenSSL 兼容名；未知则回退 Go 风格或十六进制。
pub fn VersionName(version: u16) -> String {
    // 对应 Go 的 `if tlsVersion, tlsVersionKnown := versionString[version]; tlsVersionKnown` 分支。
    // 命中时返回 "TLSv1.2"/"TLSv1.3"，而不是 Go 标准库默认名称。
    if let Some(tlsVersion) = versionString.get(&version) {
        return (*tlsVersion).to_owned();
    }

    // Rust has no equivalent of crypto/tls.VersionName, so keep its complete
    // version-name behavior locally.
    match version {
        0x0300 => "SSLv3".to_owned(),
        0x0301 => "TLS 1.0".to_owned(),
        0x0302 => "TLS 1.1".to_owned(),
        VERSION_TLS12 => "TLS 1.2".to_owned(),
        VERSION_TLS13 => "TLS 1.3".to_owned(),
        _ => format!("0x{version:04X}"),
    }
}

// CipherSuiteName convert tls num to string.
// Taken from https://testssl.sh/openssl-rfc.mapping.html .
// CipherSuiteName 按 Go 的 tlsCipherString map 把 cipher suite 编号转换成 MySQL/OpenSSL 兼容名称。
/// 将 cipher suite 编号转为兼容名称；未知返回空串。
pub fn CipherSuiteName(n: u16) -> String {
    // 对应 Go 的 `s, ok := tlsCipherString[n]` 错误处理分支：未知 cipher suite 返回空字符串。
    if let Some(s) = tlsCipherString.get(&n) {
        return (*s).to_owned();
    }

    String::new()
}
