// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `VersionName` 单元测试（对应 Go `TestVersionName`）。
//
// 覆盖 SSL/TLS 标准常量及未知版本号的十六进制回退。

use super::tls::VersionName;

// 与 Go crypto/tls 一致的版本常量，供表驱动用例使用。
const VERSION_SSL30: u16 = 0x0300;
const VERSION_TLS10: u16 = 0x0301;
const VERSION_TLS11: u16 = 0x0302;
const VERSION_TLS12: u16 = 0x0303;
const VERSION_TLS13: u16 = 0x0304;

/// 单个 VersionName 期望用例。
struct VersionNameCase {
    version: u16,
    name: &'static str,
}

// test_version_name 对应 Go 的 TestVersionName。
// Go 覆盖标准 TLS/SSL 常量和未知版本号，未知值应回退到十六进制字符串。
#[test]
/// 表驱动校验 VersionName 输出。
fn test_version_name() {
    let tests = [
        VersionNameCase {
            version: VERSION_SSL30,
            name: "SSLv3",
        },
        VersionNameCase {
            version: VERSION_TLS10,
            name: "TLS 1.0",
        },
        VersionNameCase {
            version: VERSION_TLS11,
            name: "TLS 1.1",
        },
        VersionNameCase {
            version: VERSION_TLS12,
            name: "TLSv1.2",
        },
        VersionNameCase {
            version: VERSION_TLS13,
            name: "TLSv1.3",
        },
        VersionNameCase {
            version: VERSION_TLS13 + 1,
            name: "0x0305",
        },
    ];

    for tc in tests {
        let n = VersionName(tc.version);
        assert_eq!(
            n, tc.name,
            "VersionName({}) expected {}, but got {}",
            tc.version, tc.name, n
        );
    }
}
