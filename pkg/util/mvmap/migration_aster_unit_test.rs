// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// MVMap 迁移补充单元测试。
//
// 相对 Go 原测试，额外覆盖：同 key 多 value 顺序与 Len、Get 对传入 seed
// 切片的反转语义、物理插入顺序迭代、跨 data/entry 分片边界，以及 FNV
// 测试向量。

/// 同 key 多次 Put 保留写入顺序；缺失键为空；Len 计 value 条数。
#[test]
fn multiple_values_preserve_put_order_and_len() {
    let mut map = super::NewMVMap();
    map.Put(b"abc", b"abc1");
    map.Put(b"abc", b"abc2");
    map.Put(b"def", b"def1");
    map.Put(b"def", b"def2");

    // Get 结果应按 Put 的时间顺序（经内部 reverse 后）返回。
    assert_eq!(
        map.Get(b"abc", Vec::new()),
        vec![&b"abc1"[..], &b"abc2"[..]]
    );
    assert_eq!(
        map.Get(b"def", Vec::new()),
        vec![&b"def1"[..], &b"def2"[..]]
    );
    assert!(map.Get(b"missing", Vec::new()).is_empty());
    assert_eq!(map.Len(), 4);
}

/// Get 会把匹配 value 追加到传入 values，再整体 reverse；seed 因此落到末尾。
#[test]
fn get_matches_go_seed_reversal_semantics() {
    let mut map = super::NewMVMap();
    map.Put(b"key", b"first");
    map.Put(b"key", b"second");

    // 传入含 seed 的 values，对齐 Go 对输入切片先 append 再 reverse 的语义。
    let values = map.Get(b"key", vec![b"seed".as_slice()]);
    assert_eq!(values, vec![&b"first"[..], &b"second"[..], &b"seed"[..]]);
}

/// 迭代器按 entryStore 物理插入顺序产出，耗尽后持续返回 (None, None)。
#[test]
fn iterator_follows_physical_insertion_order_and_ends_with_none() {
    let mut map = super::NewMVMap();
    map.Put(b"abc", b"abc1");
    map.Put(b"abc", b"abc2");
    map.Put(b"def", b"def1");

    let mut iterator = map.NewIterator();
    assert_eq!(iterator.Next(), (Some(&b"abc"[..]), Some(&b"abc1"[..])));
    assert_eq!(iterator.Next(), (Some(&b"abc"[..]), Some(&b"abc2"[..])));
    assert_eq!(iterator.Next(), (Some(&b"def"[..]), Some(&b"def1"[..])));
    assert_eq!(iterator.Next(), (None, None));
    // 再次 Next 仍应是哨兵，避免游标回绕。
    assert_eq!(iterator.Next(), (None, None));
}

/// 大 value 与大量同 key 写入会跨 data/entry 分片，读写与迭代计数仍一致。
#[test]
fn data_and_entry_slice_boundaries_remain_readable() {
    let mut map = super::NewMVMap();
    // 64KiB 正好触碰 maxDataSliceLen，迫使后续写入换片。
    let large = vec![7_u8; 64 * 1024];
    map.Put(b"large", &large);
    map.Put(b"after-large", b"value");

    // 超过 maxEntrySliceLen(8192) 条，迫使 entry 分片扩容。
    for index in 0..8_200_u32 {
        map.Put(b"repeated", &index.to_le_bytes());
    }

    assert_eq!(map.Get(b"large", Vec::new()), vec![large.as_slice()]);
    assert_eq!(map.Get(b"after-large", Vec::new()), vec![&b"value"[..]]);
    let repeated = map.Get(b"repeated", Vec::new());
    assert_eq!(repeated.len(), 8_200);
    assert_eq!(
        repeated.first().copied(),
        Some(0_u32.to_le_bytes().as_slice())
    );
    assert_eq!(
        repeated.last().copied(),
        Some(8_199_u32.to_le_bytes().as_slice())
    );
    let mut count = 0;
    let mut iterator = map.NewIterator();
    while iterator.Next() != (None, None) {
        count += 1;
    }
    assert_eq!(count, map.Len());
}

/// FNV-1 64 位哈希与 Go 标准库已知测试向量一致。
#[test]
fn fnv_hash_matches_go_test_vector() {
    let bytes = [0xcb, 0xf2, 0x9c, 0xe4, 0x84, 0x22, 0x23, 0x25];
    assert_eq!(super::fnv::fnv_hash64(&bytes), 0x51_af_63_43_08_c2_12_fc);
}
