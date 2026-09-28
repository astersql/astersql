// Copyright 2023 PingCAP, Inc.
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

// InternalKey 编解码与排序一致性单测。
//
// 验证 encode/decode 往返保持原值，且编码后字节序与 `compare_internal_key` 一致，
// 从而保证外排结果可按相邻键扫描重复项。

use std::cmp::Ordering;

use super::{InternalKey, compare_internal_key, decode_internal_key, encode_internal_key};

/// 覆盖空键到递增长度键的往返与字典序一致性。
#[test]
fn test_internal_key() {
    let inputs = vec![
        InternalKey::new(vec![], vec![]),
        InternalKey::new(vec![], vec![1, 2, 3, 4]),
        InternalKey::new(vec![0], vec![2, 3, 4, 5]),
        InternalKey::new(vec![0, 1], vec![3, 4, 5, 6]),
        InternalKey::new(vec![0, 1, 2], vec![4, 5, 6, 7]),
        InternalKey::new(vec![0, 1, 2, 3], vec![5, 6, 7, 8]),
        InternalKey::new(vec![0, 1, 2, 3, 4], vec![6, 7, 8, 9]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5], vec![7, 8, 9, 10]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5, 6], vec![8, 9, 10, 11]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5, 6, 7], vec![9, 10, 11, 12]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5, 6, 7, 8], vec![10, 11, 12, 13]),
    ];

    // 先验证每条输入能正确编解码往返。
    let mut encoded = Vec::with_capacity(inputs.len());
    for input in &inputs {
        let mut output = Vec::new();
        encode_internal_key(&mut output, input);
        let mut decoded = InternalKey::default();
        decode_internal_key(&output, &mut decoded).unwrap();
        assert_eq!(&decoded, input);
        encoded.push(output);
    }

    // 再验证任意两两比较：逻辑序与编码后字节序一致。
    for i in 0..inputs.len() {
        for j in (i + 1)..inputs.len() {
            let encoded_order = match encoded[i].cmp(&encoded[j]) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            assert_eq!(
                compare_internal_key(&inputs[i], &inputs[j]),
                encoded_order,
                "the order of encoded keys should be the same as the order of internal keys"
            );
        }
    }
}

#[test]
fn decode_errors_preserve_go_slice_state() {
    let cases = [
        (vec![], vec![9; 12]),
        (vec![1, 2, 3], vec![9; 12]),
        (vec![1, 2, 3, 4, 5, 6, 7, 8, 246], vec![9; 12]),
        (
            vec![1, 2, 3, 4, 5, 6, 7, 8, 255],
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9],
        ),
        (
            vec![1, 2, 3, 4, 5, 6, 7, 8, 254],
            vec![1, 2, 3, 4, 5, 6, 7, 9, 9, 9, 9, 9],
        ),
    ];
    for (encoded, expected) in cases {
        let mut key = InternalKey::new(vec![9; 12], vec![42]);
        assert!(decode_internal_key(&encoded, &mut key).is_err());
        assert_eq!(key.key, expected);
        assert_eq!(key.key_id, vec![42]);
    }
    // An append exceeding capacity detaches Go's temporary decode slice.
    let mut key = InternalKey::new(Vec::with_capacity(4), vec![42]);
    key.key.extend_from_slice(&[9; 4]);
    assert!(decode_internal_key(&[1, 2, 3, 4, 5, 6, 7, 8, 255], &mut key).is_err());
    assert_eq!(key.key, vec![9; 4]);
}

#[test]
fn internal_key_format_append_and_reuse() {
    for (key, id, expected) in [
        (vec![], vec![], ""),
        (vec![0, 171, 255], vec![], "00ABFF"),
        (vec![], vec![0, 255], "@00FF"),
        (vec![171], vec![205], "AB@CD"),
    ] {
        assert_eq!(InternalKey::new(key, id).to_string(), expected);
    }
    let mut decoded = InternalKey::new(vec![99; 32], vec![99; 32]);
    let mut inputs = vec![];
    for len in [0, 1, 7, 8, 9, 15, 16, 17] {
        for id in [vec![], vec![0], vec![0, 255], vec![255]] {
            let key = InternalKey::new(vec![255; len], id);
            let mut encoded = vec![42, 43];
            encode_internal_key(&mut encoded, &key);
            assert_eq!(&encoded[..2], &[42, 43]);
            decode_internal_key(&encoded[2..], &mut decoded).unwrap();
            assert_eq!(decoded, key);
            inputs.push((key, encoded[2..].to_vec()));
        }
    }
    for (a, encoded_a) in &inputs {
        for (b, encoded_b) in &inputs {
            assert_eq!(
                compare_internal_key(a, b),
                match encoded_a.cmp(encoded_b) {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                }
            );
        }
    }
}
