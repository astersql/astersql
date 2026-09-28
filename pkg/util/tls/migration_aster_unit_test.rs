// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// TLS 迁移一致性单测：版本名、密码套件表与安全传输原子开关。
//
// 对照 Go crypto/tls 常量与 TiDB 覆盖命名，确保机械迁移后查表语义不变。

#[path = "tls.rs"]
mod tls;

#[cfg(test)]
mod tests {
    use super::tls::{CipherSuiteName, RequireSecureTransport, SupportCipher, VersionName};
    use std::sync::atomic::Ordering;

    #[test]
    /// 版本号到 MySQL/OpenSSL 风格名称及未知值十六进制回退。
    fn version_names_match_go_crypto_tls_and_tidb_overrides() {
        let cases = [
            (0x0300, "SSLv3"),
            (0x0301, "TLS 1.0"),
            (0x0302, "TLS 1.1"),
            (0x0303, "TLSv1.2"),
            (0x0304, "TLSv1.3"),
            (0x0305, "0x0305"),
        ];

        for (version, expected) in cases {
            assert_eq!(VersionName(version), expected);
        }
    }

    #[test]
    /// 密码套件 ID→名称与 SupportCipher 集合与 Go 表一致。
    fn cipher_names_and_supported_set_match_go_tables() {
        let cases = [
            (0x0005, "RC4-SHA"),
            (0x000a, "DES-CBC3-SHA"),
            (0x002f, "AES128-SHA"),
            (0x0035, "AES256-SHA"),
            (0x003c, "AES128-SHA256"),
            (0x009c, "AES128-GCM-SHA256"),
            (0x009d, "AES256-GCM-SHA384"),
            (0xc007, "ECDHE-ECDSA-RC4-SHA"),
            (0xc009, "ECDHE-ECDSA-AES128-SHA"),
            (0xc00a, "ECDHE-ECDSA-AES256-SHA"),
            (0xc011, "ECDHE-RSA-RC4-SHA"),
            (0xc012, "ECDHE-RSA-DES-CBC3-SHA"),
            (0xc013, "ECDHE-RSA-AES128-SHA"),
            (0xc014, "ECDHE-RSA-AES256-SHA"),
            (0xc023, "ECDHE-ECDSA-AES128-SHA256"),
            (0xc027, "ECDHE-RSA-AES128-SHA256"),
            (0xc02f, "ECDHE-RSA-AES128-GCM-SHA256"),
            (0xc02b, "ECDHE-ECDSA-AES128-GCM-SHA256"),
            (0xc030, "ECDHE-RSA-AES256-GCM-SHA384"),
            (0xc02c, "ECDHE-ECDSA-AES256-GCM-SHA384"),
            (0xcca8, "ECDHE-RSA-CHACHA20-POLY1305"),
            (0xcca9, "ECDHE-ECDSA-CHACHA20-POLY1305"),
            (0x1301, "TLS_AES_128_GCM_SHA256"),
            (0x1302, "TLS_AES_256_GCM_SHA384"),
            (0x1303, "TLS_CHACHA20_POLY1305_SHA256"),
        ];

        assert_eq!(SupportCipher.len(), cases.len());
        for (id, expected) in cases {
            assert_eq!(CipherSuiteName(id), expected);
            assert!(SupportCipher.contains(expected));
        }
        assert_eq!(CipherSuiteName(0xffff), "");
    }

    #[test]
    /// RequireSecureTransport 的 AtomicBool 读写/swap 语义。
    fn secure_transport_flag_has_go_atomic_bool_semantics() {
        RequireSecureTransport.store(false, Ordering::SeqCst);
        assert!(!RequireSecureTransport.load(Ordering::SeqCst));
        assert!(!RequireSecureTransport.swap(true, Ordering::SeqCst));
        assert!(RequireSecureTransport.load(Ordering::SeqCst));
        RequireSecureTransport.store(false, Ordering::SeqCst);
    }
}
