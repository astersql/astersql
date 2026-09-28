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

use crate::mvcc::{Mutation, MutationOp};
use crate::util::{
    exceed_end_key, fingerprint64, keys_to_hash_values, mutations_to_hash_values, safe_copy,
    sort_and_dedup_hash_values, user_keys_to_hash_values,
};

/// 锁指纹必须与 Go `farm.Fingerprint64` 完全相同，而非仅在 Rust 内部自洽。
#[test]
fn fingerprint_matches_go_farmhash() {
    let vectors = [
        (Vec::new(), 0x9ae1_6a3b_2f90_404f),
        ((0_u8..1).collect(), 0xbe60_56ed_f5e9_4b54),
        ((0_u8..11).collect(), 0xf321_2b3c_1d80_3add),
        ((0_u8..17).collect(), 0xbbb6_a6f8_f20d_1f1c),
        ((0_u8..33).collect(), 0xe875_6ec1_cb75_524e),
        ((0_u8..65).collect(), 0xc6a3_282c_3e79_3dbe),
        ((0_u8..129).collect(), 0xce8b_a374_1121_083e),
    ];
    for (input, expected) in vectors {
        assert_eq!(expected, fingerprint64(&input), "length {}", input.len());
    }
}

/// 空 end_key 保持区间开放；等于或大于 end_key 视为越界。
#[test]
fn end_key_comparison_keeps_unbounded_ranges_open() {
    let cases: &[(&[u8], &[u8], bool)] = &[
        (b"abc", b"", false),
        (b"abc", b"abc", true),
        (b"bcd", b"abc", true),
        (b"abc", b"bcd", false),
        (b"", b"abc", false),
        (b"", b"", false),
    ];
    for &(current, end_key, expected) in cases {
        assert_eq!(expected, exceed_end_key(current, end_key));
    }
}

/// 键与 Mutation 路径应产生相同的排序去重哈希集合。
#[test]
fn hashes_are_sorted_deduplicated_for_keys_and_mutations() {
    let cases = [
        (vec![], vec![]),
        (vec![1], vec![1]),
        (vec![1, 2, 3, 4, 5], vec![1, 2, 3, 4, 5]),
        (vec![5, 3, 1, 4, 2], vec![1, 2, 3, 4, 5]),
        (
            vec![3, 1, 4, 1, 5, 9, 2, 6, 5, 3],
            vec![1, 2, 3, 4, 5, 6, 9],
        ),
        (vec![7, 7, 7, 7], vec![7]),
        (vec![3, 3], vec![3]),
        (vec![3, 1], vec![1, 3]),
    ];
    for (input, expected) in cases {
        assert_eq!(expected, sort_and_dedup_hash_values(input));
    }
    let keys = vec![b"same".to_vec(), b"other".to_vec(), b"same".to_vec()];
    let from_keys = keys_to_hash_values(&keys);
    let mutations = keys
        .into_iter()
        .map(|key| Mutation {
            op: MutationOp::Put,
            key,
            value: Vec::new(),
            is_pessimistic_lock: false,
        })
        .collect::<Vec<_>>();
    assert_eq!(from_keys, mutations_to_hash_values(&mutations));
    assert_eq!(
        from_keys,
        user_keys_to_hash_values(
            &mutations
                .iter()
                .map(|m| m.key.as_slice())
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(2, from_keys.len());
}

/// 修改原缓冲区不得影响 safe_copy 返回的副本。
#[test]
fn safe_copy_has_independent_storage() {
    for input in [
        vec![],
        vec![65],
        b"hello world".to_vec(),
        vec![0, 1, 2, 255, 254],
    ] {
        let mut original = input.clone();
        let copied = safe_copy(&original);
        assert_eq!(input, copied);
        if !original.is_empty() {
            original[0] = original[0].wrapping_add(1);
            assert_eq!(input, copied);
            assert_ne!(original, copied);
        }
    }
}
