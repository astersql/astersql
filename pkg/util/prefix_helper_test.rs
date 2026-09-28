// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// prefix_helper 单元测试：内存 MemStore 上验证扫描、按前缀删除与行键过滤。

use std::collections::BTreeMap;

use anyhow::{Error, anyhow};

use crate::prefix_helper::{
    DelKeyWithPrefix, Key, KvIterator, Retriever, RetrieverMutator, RowKeyPrefixFilter,
    ScanMetaWithPrefix,
};

/// 填充键的起始下标。
const START_INDEX: i32 = 0;
/// 填充键数量。
const TEST_COUNT: i32 = 12;
/// 前缀与序号合成时的数量级乘数。
const TEST_POW: i32 = 10;

/// 基于 BTreeMap 的内存 KV，实现 Retriever / RetrieverMutator。
#[derive(Default)]
struct MemStore {
    /// 有序键值表。
    data: BTreeMap<Vec<u8>, Vec<u8>>,
}

/// 将区间快照到数组后顺序迭代的简易迭代器。
struct MemIter {
    /// 键列表快照。
    keys: Vec<Key>,
    /// 值列表快照。
    values: Vec<Vec<u8>>,
    /// 当前下标。
    index: usize,
}

impl KvIterator for MemIter {
    fn valid(&self) -> bool {
        self.index < self.keys.len()
    }

    fn key(&self) -> &Key {
        &self.keys[self.index]
    }

    fn value(&self) -> &[u8] {
        &self.values[self.index]
    }

    fn next(&mut self) -> Result<(), Error> {
        self.index += 1;
        Ok(())
    }
}

impl Retriever for MemStore {
    fn iter(&self, start: &Key, end: &Key) -> Result<Box<dyn KvIterator>, Error> {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        // 半开区间 [start, end)，与 PrefixNext 上界配合。
        for (key, value) in self.data.range(start.0.clone()..end.0.clone()) {
            keys.push(Key(key.clone()));
            values.push(value.clone());
        }
        Ok(Box::new(MemIter {
            keys,
            values,
            index: 0,
        }))
    }
}

impl RetrieverMutator for MemStore {
    fn delete(&mut self, key: Key) -> Result<(), Error> {
        self.data.remove(&key.0);
        Ok(())
    }
}

impl MemStore {
    /// 写入键值。
    fn set(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.data.insert(key, value);
    }

    /// 读取键值；不存在则返回错误。
    fn get(&self, key: &[u8]) -> Result<Vec<u8>, Error> {
        self.data
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow!("not exist"))
    }
}

/// 带数字前缀的填充上下文，模拟事务批量写入。
struct MockContext {
    /// 数字前缀（再乘 TEST_POW 编码进键）。
    prefix: i32,
    /// 内存存储。
    store: MemStore,
}

impl MockContext {
    /// 按 START_INDEX..TEST_COUNT 写入一组「键=值」的十进制编码条目。
    fn fill_txn(&mut self) -> Result<(), Error> {
        for i in START_INDEX..TEST_COUNT {
            let val = encode_int(i + (self.prefix * TEST_POW));
            self.store.set(val.clone(), val);
        }
        Ok(())
    }
}

/// 将整数编码为十进制 ASCII 字节，作为测试键。
fn encode_int(n: i32) -> Vec<u8> {
    format!("{n}").into_bytes()
}

/// 覆盖空前缀删除、扫描 filter 早停、以及密集数字前缀整段删除。
#[test]
fn test_prefix() {
    // Go fillTxn is a no-op when txn is still nil before GetTxn; keep that shape.
    let mut store = MemStore::default();
    DelKeyWithPrefix(&mut store, Key(encode_int(10000000))).unwrap();

    let k = b"key100jfowi878230".to_vec();
    store.set(k.clone(), b"val32dfaskli384757^*&%^".to_vec());
    ScanMetaWithPrefix(&store, Key(k.clone()), |_key, _value| true).unwrap();
    ScanMetaWithPrefix(&store, Key(k.clone()), |_key, _value| false).unwrap();
    DelKeyWithPrefix(&mut store, Key(b"key".to_vec())).unwrap();
    assert!(store.get(&k).is_err());

    // Extra coverage: DelKeyWithPrefix removes every key under a dense numeric prefix.
    let store = MemStore::default();
    let mut ctx = MockContext {
        prefix: 10000000,
        store,
    };
    ctx.fill_txn().unwrap();
    DelKeyWithPrefix(&mut ctx.store, Key(encode_int(ctx.prefix))).unwrap();
    assert!(
        ctx.store
            .data
            .keys()
            .all(|key| !key.starts_with(encode_int(ctx.prefix).as_slice()))
    );
}

/// 验证 RowKeyPrefixFilter：带前缀为 false，不带前缀为 true。
#[test]
fn test_prefix_filter() {
    let mut row_key = b"test@#$%l(le[0]..prefix) 2uio".to_vec();
    // 嵌入内嵌 NUL，模拟真实行键中的分隔字节。
    row_key[8] = 0x00;
    row_key[9] = 0x00;
    let f = RowKeyPrefixFilter(Key(row_key.clone()));
    let mut longer = row_key.clone();
    longer.extend_from_slice(b"akjdf3*(34");
    assert!(!f(&Key(longer)));
    assert!(f(&Key(b"sjfkdlsaf".to_vec())));
}
