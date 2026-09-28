// Copyright 2026 AsterSQL.
// Copyright 2020-present PingCAP, Inc.
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

// fastrand 迁移对照单元测试：用固定向量校验与 Go 实现一致。
//
// 覆盖 `_wymix`/`wyrand`、有界随机 `Uint32N`/`Uint64N`、`Buf` 字符约束，
// 以及 `Uint32` 非常量源。

use super::{_wymix, Buf, Uint32, Uint32N, Uint64N, wyrand};

/// 校验 `_wymix` 与 `wyrand::Next` 对已知种子输出与 Go 向量一致。
#[test]
fn migration_wymix_and_wyrand_match_go_vectors() {
    assert_eq!(_wymix(0, u64::MAX), 0);
    assert_eq!(_wymix(u64::MAX, u64::MAX), u64::MAX);

    let mut r = wyrand(0);
    assert_eq!(r.Next(), 0x111c_b3a7_8f59_a58e);
    assert_eq!(r.Next(), 0xceab_d938_ff4e_856d);
    assert_eq!(r.Next(), 0x61fb_5131_8f47_d2a4);
}

/// 校验有界随机落在 `[0,n)`，以及 `n==0`/`n==1` 等边界与 Go 契约一致。
#[test]
fn migration_bounded_values_match_go_contract() {
    for _ in 0..4096 {
        assert!(Uint32N(1024) < 1024);
        assert!(Uint64N(1_u64 << 63) < (1_u64 << 63));
        assert_eq!(Uint32N(0), 0);
        assert_eq!(Uint64N(1), 0);
        let _ = Uint64N(0);
    }
}

/// 校验 `Buf` 长度正确，且避免 `\0`/`$`，字节均小于 127。
#[test]
fn migration_buf_has_requested_length_and_avoids_separators() {
    assert!(Buf(0).is_empty());
    let buf = Buf(4096);
    assert_eq!(buf.len(), 4096);
    assert!(buf.iter().all(|&byte| byte != 0 && byte != b'$'));
    assert!(buf.iter().all(|&byte| byte < 127));
}

/// 校验 `Uint32` 不是恒定常量源（连续采样应出现不同值）。
#[test]
fn migration_uint32_source_is_not_constant() {
    let first = Uint32();
    assert!((0..64).any(|_| Uint32() != first));
}
