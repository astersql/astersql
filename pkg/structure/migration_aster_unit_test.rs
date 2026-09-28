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

// structure 包的 Aster 迁移单元测试与内存后端。
//
// 提供基于 BTreeMap 的内存 KV（支持正/反向迭代），以及 String / List / Hash
// 与键编码行为的对照 Go 语义测试。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{ErrInvalidListIndex, ErrWriteOnSnapshot, HashPair, NewStructure, TxStructure, kv};

/// 线程安全的内存 KV 存储，供单测作为 reader/writer。
#[derive(Clone, Default)]
pub(super) struct MemoryStore {
    /// 有序键值表；经 Mutex 保护并发访问。
    data: Arc<Mutex<BTreeMap<Vec<u8>, Vec<u8>>>>,
}

/// 内存迭代器：预先物化范围内的键值对并按位置前进。
struct MemoryIterator {
    /// 已收集的 (key, value) 序列。
    entries: Vec<(kv::Key, Vec<u8>)>,
    /// 当前下标。
    position: usize,
}

impl kv::Iterator for MemoryIterator {
    fn Valid(&self) -> bool {
        self.position < self.entries.len()
    }

    fn Key(&self) -> kv::Key {
        self.entries[self.position].0.clone()
    }

    fn Value(&self) -> Vec<u8> {
        self.entries[self.position].1.clone()
    }

    fn Next(&mut self) -> Result<(), super::errors::SharedError> {
        self.position += 1;
        Ok(())
    }

    fn Close(&mut self) {}
}

impl kv::Getter for MemoryStore {
    fn Get(
        &self,
        _: &kv::Context,
        key: kv::Key,
        _: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, super::errors::SharedError> {
        self.data
            .lock()
            .unwrap()
            .get(&key.0)
            .cloned()
            .map(|value| kv::ValueEntry {
                Value: value,
                CommitTs: 0,
            })
            .ok_or_else(|| kv::ErrNotExist.FastGenByArgs(&[]))
    }
}

impl kv::Retriever for MemoryStore {
    /// 正向扫描：`[key, upper_bound)`。
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, super::errors::SharedError> {
        let entries = self
            .data
            .lock()
            .unwrap()
            .iter()
            .filter(|(candidate, _)| {
                candidate.as_slice() >= key.0.as_slice()
                    && upper_bound
                        .as_ref()
                        .is_none_or(|upper| candidate.as_slice() < upper.0.as_slice())
            })
            .map(|(key, value)| (kv::Key(key.clone()), value.clone()))
            .collect();
        Ok(Box::new(MemoryIterator {
            entries,
            position: 0,
        }))
    }

    /// 反向扫描：收集后 reverse，起点为 `< key` 且 `>= lower_bound`。
    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, super::errors::SharedError> {
        let mut entries: Vec<_> = self
            .data
            .lock()
            .unwrap()
            .iter()
            .filter(|(candidate, _)| {
                key.as_ref()
                    .is_none_or(|upper| candidate.as_slice() < upper.0.as_slice())
                    && lower_bound
                        .as_ref()
                        .is_none_or(|lower| candidate.as_slice() >= lower.0.as_slice())
            })
            .map(|(key, value)| (kv::Key(key.clone()), value.clone()))
            .collect();
        entries.reverse();
        Ok(Box::new(MemoryIterator {
            entries,
            position: 0,
        }))
    }
}

impl kv::Mutator for MemoryStore {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), super::errors::SharedError> {
        if value.is_empty() {
            return Err(kv::ErrCannotSetNilValue.FastGenByArgs(&[]));
        }
        self.data.lock().unwrap().insert(key.0, value);
        Ok(())
    }

    fn Delete(&mut self, key: kv::Key) -> Result<(), super::errors::SharedError> {
        self.data.lock().unwrap().remove(&key.0);
        Ok(())
    }
}

impl kv::RetrieverMutator for MemoryStore {}

/// 构造可写 TxStructure 与其共享的 MemoryStore；`prefix` 为结构前缀。
pub(super) fn writable(prefix: &[u8]) -> (TxStructure, MemoryStore) {
    let store = MemoryStore::default();
    let tx = NewStructure(
        Box::new(store.clone()),
        Some(Box::new(store.clone())),
        prefix.to_vec(),
    );
    (tx, store)
}

/// 覆盖 String 读写、迭代、自增、清空，以及快照上写操作报错。
#[test]
fn string_round_trip_iteration_and_snapshot_errors_match_go() {
    let (mut tx, store) = writable(&[0]);
    tx.Set(b"a", b"1").unwrap();

    let mut values = Vec::new();
    tx.Iterate(b"a", b"b", |key, value| {
        values.push((key.to_vec(), value.to_vec()));
        Ok(())
    })
    .unwrap();
    assert_eq!(values, vec![(b"a".to_vec(), b"1".to_vec())]);
    assert_eq!(tx.Get(b"a").unwrap(), Some(b"1".to_vec()));
    assert_eq!(tx.Inc(b"a", 1).unwrap(), 2);
    assert_eq!(tx.Get(b"a").unwrap(), Some(b"2".to_vec()));
    assert_eq!(tx.GetInt64(b"a").unwrap(), 2);
    tx.Clear(b"a").unwrap();
    assert_eq!(tx.Get(b"a").unwrap(), None);

    // 无 writer 的快照结构：写路径应返回 ErrWriteOnSnapshot。
    let mut snapshot = NewStructure(Box::new(store), None, vec![1]);
    for error in [
        snapshot.Set(b"a", b"1").unwrap_err(),
        snapshot.Inc(b"a", 1).unwrap_err(),
        snapshot.Clear(b"a").unwrap_err(),
    ] {
        assert!(ErrWriteOnSnapshot.Equal(Some(&error)));
    }
}

/// 覆盖 List 的推入/弹出、下标读写、负下标与清空。
#[test]
fn list_push_pop_index_set_and_clear_match_go() {
    let (mut tx, store) = writable(&[0]);
    tx.LPush(b"a", &[b"3".to_vec(), b"2".to_vec(), b"1".to_vec()])
        .unwrap();
    tx.LPush(b"a", &[b"11".to_vec()]).unwrap();
    assert_eq!(
        tx.LGetAll(b"a").unwrap(),
        Some(vec![
            b"3".to_vec(),
            b"2".to_vec(),
            b"1".to_vec(),
            b"11".to_vec()
        ])
    );
    assert_eq!(tx.LPop(b"a").unwrap(), Some(b"11".to_vec()));
    assert_eq!(tx.LLen(b"a").unwrap(), 3);
    assert_eq!(tx.LIndex(b"a", 1).unwrap(), Some(b"2".to_vec()));
    tx.LSet(b"a", 1, b"4").unwrap();
    assert_eq!(tx.LIndex(b"a", 1).unwrap(), Some(b"4".to_vec()));
    tx.LSet(b"a", 1, b"2").unwrap();
    assert!(ErrInvalidListIndex.Equal(Some(&tx.LSet(b"a", 100, b"x").unwrap_err())));
    assert_eq!(tx.LIndex(b"a", -1).unwrap(), Some(b"3".to_vec()));

    assert_eq!(tx.LPop(b"a").unwrap(), Some(b"1".to_vec()));
    assert_eq!(tx.LLen(b"a").unwrap(), 2);
    tx.RPush(b"a", &[b"4".to_vec()]).unwrap();
    assert_eq!(tx.LLen(b"a").unwrap(), 3);
    assert_eq!(tx.LIndex(b"a", -1).unwrap(), Some(b"4".to_vec()));
    assert_eq!(tx.RPop(b"a").unwrap(), Some(b"4".to_vec()));
    assert_eq!(tx.RPop(b"a").unwrap(), Some(b"3".to_vec()));
    assert_eq!(tx.RPop(b"a").unwrap(), Some(b"2".to_vec()));
    assert_eq!(tx.LLen(b"a").unwrap(), 0);

    tx.LPush(b"a", &[b"1".to_vec()]).unwrap();
    tx.LClear(b"a").unwrap();
    assert_eq!(tx.LLen(b"a").unwrap(), 0);

    let mut snapshot = NewStructure(Box::new(store), None, vec![1]);
    assert!(ErrWriteOnSnapshot.Equal(Some(&snapshot.LPush(b"a", &[b"1".to_vec()]).unwrap_err())));
    assert!(ErrWriteOnSnapshot.Equal(Some(&snapshot.RPop(b"a").unwrap_err())));
    assert!(ErrWriteOnSnapshot.Equal(Some(&snapshot.LSet(b"a", 1, b"2").unwrap_err())));
    assert!(ErrWriteOnSnapshot.Equal(Some(&snapshot.LClear(b"a").unwrap_err())));
}

/// 覆盖 Hash 的 CRUD、整数自增与反向取末 N 项。
#[test]
fn hash_crud_integer_and_reverse_iteration_match_go() {
    let (mut tx, store) = writable(&[0]);
    let (auto_id_key, auto_id_value) = tx.EncodeHashAutoIDKeyValue(b"a", b"a", 5);
    assert_eq!(auto_id_key, tx.EncodeHashDataKey(b"a", b"a"));
    assert_eq!(auto_id_value, b"5");

    tx.HSet(b"a", b"1", b"1").unwrap();
    tx.HSet(b"a", b"2", b"2").unwrap();
    assert_eq!(tx.HGet(b"a", b"1").unwrap(), Some(b"1".to_vec()));
    assert_eq!(tx.HGet(b"a", b"missing").unwrap(), None);
    assert_eq!(tx.HKeys(b"a").unwrap(), vec![b"1".to_vec(), b"2".to_vec()]);
    assert_eq!(
        tx.HGetAll(b"a").unwrap(),
        vec![
            HashPair {
                Field: b"1".to_vec(),
                Value: b"1".to_vec()
            },
            HashPair {
                Field: b"2".to_vec(),
                Value: b"2".to_vec()
            },
        ]
    );
    let mut iterated = Vec::new();
    tx.HGetIter(b"a", |pair| {
        iterated.push(pair);
        Ok(())
    })
    .unwrap();
    assert_eq!(iterated, tx.HGetAll(b"a").unwrap());
    assert_eq!(
        tx.HGetLastN(b"a", 1).unwrap(),
        vec![HashPair {
            Field: b"2".to_vec(),
            Value: b"2".to_vec()
        }]
    );
    assert_eq!(
        tx.HGetLastN(b"a", 2).unwrap(),
        vec![
            HashPair {
                Field: b"2".to_vec(),
                Value: b"2".to_vec()
            },
            HashPair {
                Field: b"1".to_vec(),
                Value: b"1".to_vec()
            },
        ]
    );
    tx.HDel(b"a", &[b"1".to_vec()]).unwrap();
    assert_eq!(tx.HGet(b"a", b"1").unwrap(), None);
    assert_eq!(tx.HInc(b"a", b"1", 1).unwrap(), 1);
    assert_eq!(tx.HGet(b"a", b"1").unwrap(), Some(b"1".to_vec()));
    tx.HSet(b"a", b"1", b"1").unwrap();
    assert_eq!(tx.HGet(b"a", b"1").unwrap(), Some(b"1".to_vec()));
    assert_eq!(tx.HInc(b"a", b"1", 1).unwrap(), 2);
    assert_eq!(tx.HInc(b"a", b"1", 1).unwrap(), 3);
    assert_eq!(tx.HGetInt64(b"a", b"1").unwrap(), 3);
    tx.HClear(b"a").unwrap();
    assert_eq!(tx.HGetLen(b"a").unwrap(), 0);
    tx.HDel(b"a", &[b"missing".to_vec()]).unwrap();

    // bytes.Equal(nil, nil) causes an absent empty value to be a no-op. Once
    // the field exists, the KV contract rejects an empty replacement.
    tx.HSet(b"a", b"nil_key", b"").unwrap();
    assert_eq!(tx.HGet(b"a", b"nil_key").unwrap(), None);
    tx.HSet(b"a", b"nil_key", b"1").unwrap();
    let error = tx.HSet(b"a", b"nil_key", b"").unwrap_err();
    assert!(kv::ErrCannotSetNilValue.Equal(Some(&error)));
    assert_eq!(tx.HGet(b"a", b"nil_key").unwrap(), Some(b"1".to_vec()));
    tx.HSet(b"a", b"nil_key", b"2").unwrap();
    assert_eq!(tx.HGet(b"a", b"nil_key").unwrap(), Some(b"2".to_vec()));

    let mut snapshot = NewStructure(Box::new(store), None, vec![1]);
    assert!(ErrWriteOnSnapshot.Equal(Some(&snapshot.HInc(b"a", b"1", 1).unwrap_err())));
    assert!(ErrWriteOnSnapshot.Equal(Some(&snapshot.HDel(b"a", &[b"1".to_vec()]).unwrap_err())));
}

/// 覆盖 String/Hash 键编解码，以及按业务 key 边界的哈希迭代。
#[test]
fn key_encoding_and_bounded_hash_iteration_match_go() {
    let (mut tx, _) = writable(&[]);
    let string_key = tx.EncodeStringDataKey(b"key");
    assert_eq!(tx.decodeStringDataKey(string_key).unwrap(), b"key");
    let hash_key = tx.EncodeHashDataKey(b"hash", b"field");
    assert_eq!(
        tx.decodeHashDataKey(hash_key).unwrap(),
        (b"hash".to_vec(), b"field".to_vec())
    );

    tx.Set(&tx.EncodeHashMetaKey(b"aa").0, b"meta").unwrap();
    tx.HSet(b"b", b"", b"value1").unwrap();
    tx.Set(&tx.EncodeHashMetaKey(b"c").0, b"meta").unwrap();
    tx.HSet(b"d", b"", b"value2").unwrap();
    tx.Set(&tx.EncodeHashMetaKey(b"dd").0, b"meta").unwrap();

    let mut rows = Vec::new();
    tx.IterateHashWithBoundedKey(b"a", b"e", |key, field, value| {
        rows.push((key.to_vec(), field.to_vec(), value.to_vec()));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (b"b".to_vec(), Vec::new(), b"value1".to_vec()),
            (b"d".to_vec(), Vec::new(), b"value2".to_vec()),
        ]
    );
}
