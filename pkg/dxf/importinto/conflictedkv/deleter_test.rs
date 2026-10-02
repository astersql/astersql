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

// Deleter 单元测试：data / 唯一索引 KV 组经生产删除管线删尽冲突键。
//
// 自 Go `deleter_test.go` 移植；无 mock TiDB store 时，用 FakeCodec/ConflictStore
// 驱动真实 Deleter/Handler/快照与 delete-worker，并断言重试/批量/冲刷路径。

// Ported from pkg/dxf/importinto/conflictedkv/deleter_test.go. Go's test
// stands up a real mock TiDB store/table via `testkit`, inserts rows,
// deletes a subset of their raw KVs to simulate a conflicted/inconsistent
// table, then runs `Deleter` and checks the table is repaired. This crate
// has no such mock-store dependency wired for `conflictedkv`, so this port
// drives the real `Deleter`/`DataKVHandler`/`IndexKVHandler`/
// `LazyRefreshedSnapshot` dispatch and delete-worker logic (the actual unit
// under test) against a small fake `ConflictRowCodec`/`ConflictStore`/
// `ConflictSnapshot`/`ConflictTransaction` that models a cluster containing
// a known set of "conflicted" rows, and asserts every one of them is
// deleted exactly through the production retry/batch/flush pipeline. Each
// conflicting key is sent twice on the channel, exactly like Go's test, to
// exercise the same duplicate-delivery path.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use astersql_kv::tikvstore::ValueEntry;
use astersql_kv::{Handle, IntHandle, Key, Version};
use astersql_lightning_backend_kv::Pairs;
use astersql_lightning_verification::KvPair;
use astersql_meta_model::{IndexColumn, IndexInfo, TableInfo};
use astersql_types::datum::{Datum, NewIntDatum};

use crate::{
    BufferedKeyCountLimit, ConflictContext, ConflictKVPair, ConflictRowCodec, ConflictSnapshot,
    ConflictStore, ConflictTransaction, DataKVGroup, NewDeleter, TrafficRecorder,
};

/// `BufferedKeyCountLimit` is a crate-wide static; serialize tests that
/// mutate it (Go's package tests run sequentially by default).
/// `BufferedKeyCountLimit` 为 crate 级静态量；序列化修改它的测试。
static BUFFERED_KEY_COUNT_LOCK: Mutex<()> = Mutex::new(());

/// 默认 ConflictContext。
fn conflict_context() -> ConflictContext {
    ConflictContext::default()
}

/// 测试用行键：`row:1:{id}`。
fn row_key(id: i64) -> Vec<u8> {
    format!("row:1:{id}").into_bytes()
}

/// Decodes handles from `row:<id>` (data kv) or `idx:<id>` (index kv) keys,
/// and always re-encodes down to the single row key, so a conflicted index
/// entry and its owning row resolve to the same deletable kv.
/// 从 `row:` / `idx:` 键解码 handle，编码时统一落到同一行键以便删除。
struct FakeCodec;

impl ConflictRowCodec for FakeCodec {
    fn StripKeyspacePrefix(&self, key: &Key) -> Result<Key, String> {
        Ok(key.clone())
    }

    fn DecodeRowKey(&self, key: &Key) -> Result<Box<dyn Handle>, String> {
        let text = String::from_utf8(key.0.clone()).map_err(|error| error.to_string())?;
        let id: i64 = text
            .strip_prefix("row:")
            .ok_or_else(|| format!("not a row key: {text}"))?
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())?;
        Ok(Box::new(IntHandle(id)))
    }

    fn DecodeRow(&self, handle: &dyn Handle, _value: &[u8]) -> Result<Vec<Datum>, String> {
        Ok(vec![NewIntDatum(handle.IntValue())])
    }

    fn DecodeTableID(&self, _key: &Key) -> i64 {
        1
    }

    fn DecodeIndexHandle(
        &self,
        key: &Key,
        _value: &[u8],
        _index_column_count: usize,
    ) -> Result<Box<dyn Handle>, String> {
        let text = String::from_utf8(key.0.clone()).map_err(|error| error.to_string())?;
        let id: i64 = text
            .strip_prefix("idx:")
            .ok_or_else(|| format!("not an index key: {text}"))?
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())?;
        Ok(Box::new(IntHandle(id)))
    }

    fn EncodeRowKey(&self, table_id: i64, handle: &dyn Handle) -> Key {
        Key(format!("row:{}:{}", table_id, handle.IntValue()).into_bytes())
    }

    fn EncodeRow(
        &mut self,
        handle: &dyn Handle,
        _row: &[Datum],
        _auto_row_id: i64,
    ) -> Result<Pairs, String> {
        Ok(Pairs {
            Pairs: vec![KvPair {
                key: self.EncodeRowKey(1, handle).0,
                val: Vec::new(),
            }],
            RowID: Vec::new(),
        })
    }

    fn Close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
/// 记录集群读/写字节，供断言删除路径产生流量。
struct FakeTrafficRecorder {
    read_bytes: AtomicU64,
    write_bytes: AtomicU64,
}

impl TrafficRecorder for FakeTrafficRecorder {
    fn IncClusterReadBytes(&self, bytes: u64) {
        self.read_bytes.fetch_add(bytes, Ordering::AcqRel);
    }
    fn IncClusterWriteBytes(&self, bytes: u64) {
        self.write_bytes.fetch_add(bytes, Ordering::AcqRel);
    }
}

/// 仅返回 `existing` 中仍存在的键。
struct FakeSnapshot {
    existing: HashMap<Vec<u8>, ValueEntry>,
}

impl ConflictSnapshot for FakeSnapshot {
    fn BatchGet(
        &self,
        _context: &ConflictContext,
        keys: &[Key],
    ) -> Result<HashMap<Vec<u8>, ValueEntry>, String> {
        let mut result = HashMap::new();
        for key in keys {
            if let Some(entry) = self.existing.get(&key.0) {
                result.insert(key.0.clone(), entry.clone());
            }
        }
        Ok(result)
    }
}

/// Commit 时把 pending 删除键并入共享列表。
struct FakeTransaction {
    deleted: Arc<Mutex<Vec<Key>>>,
    pending: Vec<Key>,
    commit_error: Option<String>,
}

impl ConflictTransaction for FakeTransaction {
    fn Delete(&mut self, key: &Key) -> Result<(), String> {
        self.pending.push(key.clone());
        Ok(())
    }
    fn Commit(self: Box<Self>, _context: &ConflictContext) -> Result<(), String> {
        if let Some(error) = self.commit_error {
            return Err(error);
        }
        self.deleted.lock().unwrap().extend(self.pending);
        Ok(())
    }
    fn Rollback(self: Box<Self>) -> Result<(), String> {
        Ok(())
    }
}

/// 持有初始存在键集合与已删除键列表。
struct FakeConflictStore {
    existing: HashMap<Vec<u8>, ValueEntry>,
    deleted: Arc<Mutex<Vec<Key>>>,
    commit_error: Option<String>,
}

impl ConflictStore for FakeConflictStore {
    fn Keyspace(&self) -> Vec<u8> {
        Vec::new()
    }
    fn CurrentVersion(&self) -> Result<Version, String> {
        Ok(astersql_kv::NewVersion(1))
    }
    fn GetSnapshot(&self, _version: Version) -> Box<dyn ConflictSnapshot> {
        Box::new(FakeSnapshot {
            existing: self.existing.clone(),
        })
    }
    fn Begin(&self) -> Result<Box<dyn ConflictTransaction>, String> {
        Ok(Box::new(FakeTransaction {
            deleted: self.deleted.clone(),
            pending: Vec::new(),
            commit_error: self.commit_error.clone(),
        }))
    }
    fn IsRetryableError(&self, _error: &str) -> bool {
        false
    }
}

/// 构造带可选索引元信息的测试表。
fn make_table(indices: Vec<IndexInfo>) -> Arc<TableInfo> {
    Arc::new(TableInfo {
        PKIsHandle: true,
        Indices: indices,
        ..Default::default()
    })
}

/// 向通道发送冲突键（每键两次），跑 Deleter，断言全部冲突 id 被删除。
fn do_test_deleter(kv_group: &str, conflicted_ids: &[i64], is_data_kv: bool) {
    let _guard = BUFFERED_KEY_COUNT_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let previous_limit = BufferedKeyCountLimit.load(Ordering::Acquire);
    // Force the buffered-key flush path to run multiple times instead of
    // once at `Close`, matching Go's `BufferedKeyCountLimit = 2` override.
    // 强制缓冲键冲刷多次执行，而非仅在 Close 时一次。
    BufferedKeyCountLimit.store(2, Ordering::Release);

    let existing: HashMap<Vec<u8>, ValueEntry> = conflicted_ids
        .iter()
        .map(|&id| {
            (
                row_key(id),
                ValueEntry {
                    Value: vec![1],
                    CommitTs: 0,
                },
            )
        })
        .collect();
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore {
        existing,
        deleted: deleted.clone(),
        commit_error: None,
    });
    let traffic_recorder = Arc::new(FakeTrafficRecorder::default());
    let target_table = if is_data_kv {
        make_table(Vec::new())
    } else {
        make_table(vec![IndexInfo {
            ID: 2,
            Columns: vec![IndexColumn::default()],
            ..Default::default()
        }])
    };
    let mut deleter = NewDeleter(
        target_table,
        store,
        kv_group,
        Box::new(FakeCodec),
        None,
        Some(traffic_recorder.clone()),
    );

    let (sender, receiver) = mpsc::channel::<ConflictKVPair>();
    for &id in conflicted_ids {
        let key = if is_data_kv {
            format!("row:{id}")
        } else {
            format!("idx:{id}")
        };
        // Every conflicting key is sent twice, exercising the same
        // duplicate-delivery path Go's test drives.
        // 每个冲突键发送两次，覆盖重复投递路径。
        for _ in 0..2 {
            sender
                .send(ConflictKVPair {
                    Key: Key(key.clone().into_bytes()),
                    Value: Vec::new(),
                })
                .unwrap();
        }
    }
    drop(sender);

    let context = conflict_context();
    deleter
        .Run(&context, &receiver)
        .expect("Run should succeed");

    BufferedKeyCountLimit.store(previous_limit, Ordering::Release);

    assert!(traffic_recorder.read_bytes.load(Ordering::Acquire) > 0);
    assert!(traffic_recorder.write_bytes.load(Ordering::Acquire) > 0);

    let deleted_ids: HashSet<i64> = deleted
        .lock()
        .unwrap()
        .iter()
        .map(|key| {
            let text = String::from_utf8(key.0.clone()).unwrap();
            text.rsplit(':').next().unwrap().parse::<i64>().unwrap()
        })
        .collect();
    let expected: HashSet<i64> = conflicted_ids.iter().copied().collect();
    assert_eq!(expected, deleted_ids);
}

#[test]
/// data KV 组冲突删除。
fn test_deleter_data_kv_conflicts() {
    do_test_deleter(DataKVGroup, &[1, 2, 3, 4, 5, 6, 7], true);
}

#[test]
/// 唯一索引 KV 组冲突删除。
fn test_deleter_index_kv_conflicts() {
    let kv_group = astersql_ingestor_globalsort::kvgroup::IndexID2KVGroup(2);
    do_test_deleter(&kv_group, &[1, 2, 3, 4, 5], false);
}

#[test]
fn go_commit_3268b6550f_propagates_commit_error_without_deleting_key() {
    let key = row_key(1);
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore {
        existing: HashMap::from([(
            key.clone(),
            ValueEntry {
                Value: b"still-present".to_vec(),
                CommitTs: 0,
            },
        )]),
        deleted: deleted.clone(),
        commit_error: Some("injected commit error".to_owned()),
    });
    let mut deleter = NewDeleter(
        make_table(Vec::new()),
        store,
        DataKVGroup,
        Box::new(FakeCodec),
        None,
        None,
    );
    let (sender, receiver) = mpsc::channel();
    sender
        .send(ConflictKVPair {
            Key: Key(b"row:1".to_vec()),
            Value: Vec::new(),
        })
        .unwrap();
    drop(sender);

    let error = deleter.Run(&conflict_context(), &receiver).unwrap_err();
    assert_eq!(error, "injected commit error");
    assert!(deleted.lock().unwrap().is_empty());
}
