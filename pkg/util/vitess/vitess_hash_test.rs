// Copyright 2021 PingCAP, Inc.
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

// Vitess 哈希 Go 对照测试：固定样例与大端大写十六进制输出。
//
// 对齐 `vitess_hash_test.go`：对每个 shard key 断言 `HashUint64` 的 hex 字符串。

// 本文件由 pkg/util/vitess/vitess_hash_test.go 迁移而来，校验固定样例和大端十六进制格式。
//

use super::HashUint64;

// TestVitessHash 对应 Go 的同名测试：逐个输入 shard key，断言 null-key DES hash 的大端 hex 字符串。
/// 表驱动校验 Vitess hash 的大端大写 hex 表示。
#[test]
#[allow(non_snake_case)]
pub fn TestVitessHash() {
    let cases = vec![
        (30375298039_u64, "031265661E5F1133"),
        (1123_u64, "031B565D41BDF8CA"),
        (30573721600_u64, "1EFD6439F2050FFD"),
        (116_u64, "1E1788FF0FDE093C"),
        (u64::MAX, "355550B2150E2451"),
    ];

    for (input, expected_hex) in cases {
        // Go 每个样例都先 require.NoError，再 require.Equal；这里保留错误检查和字符串断言顺序。
        let hashed = HashUint64(input).expect("HashUint64 should not fail in Go test");
        assert_eq!(expected_hex, toHex(hashed));
    }
}

// toHex 对应 Go 辅助函数：把 uint64 按大端写入 8 字节，再编码为大写十六进制。
/// 将 u64 按大端编码为大写十六进制字符串（对齐 Go `binary.BigEndian`）。
#[allow(non_snake_case)]
pub fn toHex(value: u64) -> String {
    // binary.BigEndian.PutUint64 是测试兼容性的关键；不能改成小端或普通 Display。
    let keybytes = value.to_be_bytes();
    hex::encode(keybytes).to_uppercase()
}
