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

// MVMap 与 FNV 哈希的 Go 对齐单元测试。
//
// 对应 Go `TestMVMap` / `TestFNVHash`：校验同 key 多 value、Len、物理迭代
// 顺序，以及 FNV-1 64 位已知测试向量。

#![allow(dead_code)]
#![allow(non_snake_case)]

use super::NewMVMap;

/// 对应 Go 的 TestMVMap，验证同 key 多 value、Len 和物理迭代顺序。
// 对应 Go 的 TestMVMap，验证同 key 多 value、Len 和物理迭代顺序。
#[test]
fn test_mvmap() {
    let mut m = NewMVMap();
    // Go 使用 []byte 字面量插入两组 key；Put 顺序必须保留，因为后续 Get/Iterator 断言依赖这个顺序。
    m.Put(b"abc", b"abc1");
    m.Put(b"abc", b"abc2");
    m.Put(b"def", b"def1");
    m.Put(b"def", b"def2");

    let mut v: Vec<&[u8]> = Vec::new();

    v.clear();
    v = m.Get(b"abc", v);
    assert_eq!("[abc1 abc2]", fmt_bytes_list(&v));

    v.clear();
    v = m.Get(b"def", v);
    assert_eq!("[def1 def2]", fmt_bytes_list(&v));
    assert_eq!(4, m.Len());

    let results = vec!["abc abc1", "abc abc2", "def def1", "def def2"];
    let mut it = m.NewIterator();
    for i in 0..4 {
        let (key, val) = it.Next();
        // Go 的 fmt.Sprintf("%s %s", key, val) 会把字节切片按 UTF-8 字符串展示；这里保留该展示断言。
        assert_eq!(results[i], fmt_key_value(key, val));
    }

    let (key, val) = it.Next();
    // 迭代器耗尽后 Go 返回 (nil, nil)；用 Option::None 表达相同哨兵语义。
    assert!(key.is_none());
    assert!(val.is_none());
}

/// 将 value 列表格式化为 Go `fmt.Sprintf("%s", v)` 风格的 `[a b ...]` 字符串。
// fmt_bytes_list 对应 Go 测试中 fmt.Sprintf("%s", v) 的展示效果，仅用于的断言说明。
fn fmt_bytes_list(values: &[&[u8]]) -> String {
    let parts: Vec<String> = values
        .iter()
        .map(|value| String::from_utf8_lossy(value).to_string())
        .collect();
    format!("[{}]", parts.join(" "))
}

/// 将一对可选 key/value 格式化为 `"key val"`，对齐 Go Sprintf 展示。
// fmt_key_value 保留 fmt.Sprintf("%s %s", key, val) 的 key/value 字节展示语义。
fn fmt_key_value(key: Option<&[u8]>, val: Option<&[u8]>) -> String {
    format!(
        "{} {}",
        String::from_utf8_lossy(key.unwrap_or_default()),
        String::from_utf8_lossy(val.unwrap_or_default())
    )
}

/// 对应 Go 的 TestFNVHash；用标准库已知结果对照 `fnv_hash64`。
// 对应 Go 的 TestFNVHash；Go 标准库 New64 的已知结果用于独立对照。
#[test]
fn test_fnv_hash() {
    let b = vec![0xcb, 0xf2, 0x9c, 0xe4, 0x84, 0x22, 0x23, 0x25];
    let sum1 = super::fnv::fnv_hash64(&b);
    let sum2 = 0x51_af_63_43_08_c2_12_fc;
    assert_eq!(sum1, sum2);
}
