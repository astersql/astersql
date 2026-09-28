// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/stream/decode_kv_test.go`.
//! 验证 `EncodeKVEntry`/`EventIterator` 往返与尾部脏字节触发解码错误。
//! HashMap 遍历顺序不定，断言按 key 查期望值，不依赖编码顺序。

use std::collections::HashMap;

use crate::{EncodeKVEntry, Iterator, NewEventIterator};

/// `TestDecodeKVEntry` → `test_decode_kv_entry`：多对 KV 串联编码后应完整还原。
#[test]
fn test_decode_kv_entry() {
    let pairs: HashMap<&str, &str> = HashMap::from([
        ("db", "tidb"),
        ("kv", "tikv"),
        ("company", "PingCAP"),
        ("employee", "Zak"),
    ]);
    let mut buff = Vec::new();
    // 顺序拼接多条长度前缀条目，模拟日志缓冲。
    for (k, v) in &pairs {
        buff.extend_from_slice(&EncodeKVEntry(k.as_bytes(), v.as_bytes()));
    }

    let mut ei = NewEventIterator(buff);
    // Valid→Next 循环：先判定缓冲未耗尽再解码，与 Go 测试写法一致。
    while ei.Valid() {
        ei.Next();
        assert!(ei.GetError().is_none());
        let key = ei.Key();
        let value = ei.Value();
        let v = pairs
            .get(std::str::from_utf8(key).unwrap())
            .expect("key exists");
        assert_eq!(*v, std::str::from_utf8(value).unwrap());
    }
}

/// `TestDecodeKVEntryError` → `test_decode_kv_entry_error`：合法条目后多一字节应报错。
#[test]
fn test_decode_kv_entry_error() {
    let k = b"db";
    let v = b"tidb";
    let mut buff = EncodeKVEntry(k, v);
    // 尾部脏字节使第二次 Next 无法凑齐完整条目。
    buff.push(b'x');

    let mut ei = NewEventIterator(buff);
    ei.Next();
    assert_eq!(ei.Key(), k);
    assert_eq!(ei.Value(), v);
    assert!(ei.Valid());

    // 第二次推进应失败；错误挂在迭代器上，与 Go 行为一致。
    ei.Next();
    assert!(ei.GetError().is_some());
}
