// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// RawHandler 单测：覆盖单键/批量读写、范围扫描与按区间删除。
//
// Raw KV 与 MVCC（多版本并发控制）路径隔离，此处只验证内存 BTree 语义。

use crate::raw_handler::{KvPair, RawHandler};

/// 串联 put/get/delete、batch、scan、delete_range，校验端到端行为。
#[test]
fn test_raw_handler() {
    let h = RawHandler::new();
    let keys: Vec<Vec<u8>> = (0..10).map(|i| format!("key{i}").into_bytes()).collect();
    let vals: Vec<Vec<u8>> = (0..10).map(|i| format!("val{i}").into_bytes()).collect();

    // 单键写入后可读，删除后不再存在。
    h.raw_put(keys[0].clone(), vals[0].clone());
    let get = h.raw_get(&keys[0]);
    assert!(!get.not_found);
    assert_eq!(get.value, vals[0]);
    h.raw_delete(&keys[0]);

    // 批量写入再批量读，内容应完全一致。
    let batch_pairs = vec![
        KvPair {
            key: keys[1].clone(),
            value: vals[1].clone(),
        },
        KvPair {
            key: keys[3].clone(),
            value: vals[3].clone(),
        },
        KvPair {
            key: keys[5].clone(),
            value: vals[5].clone(),
        },
    ];
    h.raw_batch_put(&batch_pairs);
    let batch_get = h.raw_batch_get(&[keys[1].clone(), keys[3].clone(), keys[5].clone()]);
    assert_eq!(batch_pairs, batch_get);
    h.raw_batch_delete(&[keys[1].clone(), keys[3].clone(), keys[5].clone()]);

    // 正向扫描：从 keys[0] 起最多取 2 条，应命中 keys[6]/keys[7]。
    let scan_pairs = vec![
        KvPair {
            key: keys[6].clone(),
            value: vals[6].clone(),
        },
        KvPair {
            key: keys[7].clone(),
            value: vals[7].clone(),
        },
        KvPair {
            key: keys[8].clone(),
            value: vals[8].clone(),
        },
    ];
    h.raw_batch_put(&scan_pairs);

    let scan = h.raw_scan(&keys[0], &keys[9], 2, false);
    assert_eq!(scan.len(), 2);
    assert_eq!(scan, scan_pairs[..2]);

    // 半开区间 [start, end) 删除后，同范围扫描应为空。
    h.raw_delete_range(&keys[0], &keys[9]);
    let scan = h.raw_scan(&keys[0], &keys[9], 2, false);
    assert!(scan.is_empty());
}

/// Go's lockstore `SeekForPrev` returns no iterator for an empty target;
/// reverse RawScan must not reinterpret an empty start as an unbounded scan.
#[test]
fn test_raw_handler_reverse_empty_start() {
    let h = RawHandler::new();
    h.raw_put(b"a".to_vec(), b"va".to_vec());
    h.raw_put(b"b".to_vec(), b"vb".to_vec());

    assert!(h.raw_scan(&[], &[], 10, true).is_empty());
}

/// Go's iterator-based implementation treats inverted bounds as an empty
/// range. `BTreeMap::range` must not be allowed to turn those requests into a
/// panic, including RawDeleteRange's empty end key.
#[test]
fn test_raw_handler_inverted_ranges_are_empty() {
    let h = RawHandler::new();
    h.raw_put(b"b".to_vec(), b"vb".to_vec());

    assert!(h.raw_scan(b"z", b"a", 10, false).is_empty());
    assert!(h.raw_scan(b"a", b"z", 10, true).is_empty());

    h.raw_delete_range(b"a", b"");
    assert_eq!(h.raw_get(b"b").value, b"vb");
}
