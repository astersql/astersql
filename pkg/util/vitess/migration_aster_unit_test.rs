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

// vitess 迁移回归：`HashUint64` 与 Go Vitess 向量及确定性。
//
// 用固定 shard key 向量校验 null-key DES 哈希数值，并确认非恒等、可重复。

use super::vitess_hash::HashUint64;

/// 表驱动：若干 shard key 的期望 u64 哈希（与 Go Vitess 向量一致）。
#[test]
fn hash_uint64_matches_go_vitess_vectors() {
    let cases = [
        (30_375_298_039_u64, 0x0312_6566_1e5f_1133_u64),
        (1_123, 0x031b_565d_41bd_f8ca),
        (30_573_721_600, 0x1efd_6439_f205_0ffd),
        (116, 0x1e17_88ff_0fde_093c),
        (u64::MAX, 0x3555_50b2_150e_2451),
    ];

    for (shard_key, expected) in cases {
        assert_eq!(
            HashUint64(shard_key).expect("zero-key DES initialization must succeed"),
            expected,
            "unexpected Vitess hash for shard key {shard_key}",
        );
    }
}

/// 同一输入两次哈希相等，且结果不等于输入本身（非恒等哈希）。
#[test]
fn hash_uint64_is_deterministic_and_not_an_identity_hash() {
    let shard_key = 0x0102_0304_0506_0708_u64;
    let first = HashUint64(shard_key).expect("zero-key DES initialization must succeed");
    let second = HashUint64(shard_key).expect("zero-key DES initialization must succeed");

    assert_eq!(first, second);
    assert_ne!(first, shard_key);
}
