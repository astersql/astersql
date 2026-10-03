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

// `DataKVHandler` / `IndexKVHandler` / `LazyRefreshedSnapshot` 的单元测试。
//
// 用假的 `ConflictRowCodec`/`ConflictStore`/`ConflictSnapshot` 复现 Go testkit
// 夹具形状：数据 KV 重复行、索引 KV 部分命中与已处理句柄过滤。
// Handle 为行唯一标识；快照（Snapshot）是某版本下只读视图。

// 自 Go 同名测试移植。Go 侧依赖 mock TiDB store/testkit 生成行与索引 KV；
// 本 crate 无该依赖，因此对真实 Handler 调度逻辑配以假编解码与存储。
// Ported from pkg/dxf/importinto/conflictedkv/handler_test.go. Go's test
// stands up a real mock TiDB store/table via `testkit` to generate row and
// index KV fixtures. This crate has no such mock-store dependency wired for
// `conflictedkv`, so this port drives the real `DataKVHandler`/
// `IndexKVHandler`/`LazyRefreshedSnapshot` dispatch logic (the actual unit
// under test) against a small fake `ConflictRowCodec`/`ConflictStore`/
// `ConflictSnapshot` that reproduces the exact same fixture shape Go's test
// builds: 10 duplicate rows for the data-kv case, and 16 index entries where
// only handles 1..=5 have a matching data row and handles {1, 3} are
// pre-marked as already handled, for the index-kv case.

use std::collections::HashMap;
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
    BufferedHandleLimit, ConflictContext, ConflictKVPair, ConflictRowCodec, ConflictSnapshot,
    ConflictStore, ConflictTransaction, DataKVGroup, EncodedRowHandler, Handler, KVHandler,
    NewBaseHandler, NewBoundedKeySet, NewDataKVHandler, NewIndexKVHandler, NewKeyFilter,
    NewLazyRefreshedSnapshot, TrafficRecorder,
};

/// `BufferedHandleLimit` 为 crate 级静态量；串行化会修改它的测试
/// （Go 包测试默认串行）。
/// `BufferedHandleLimit` is a crate-wide static; serialize tests that mutate
/// it (Go's package tests run sequentially by default).
static BUFFERED_HANDLE_LIMIT_LOCK: Mutex<()> = Mutex::new(());

/// 构造默认冲突处理上下文。
fn conflict_context() -> ConflictContext {
    ConflictContext::default()
}

/// 假编解码器：句柄编为 `row:<id>`（数据 KV）或 `idx:<id>`（索引 KV）；
/// `auto_row_id != 0` 时多产出一条 KV，模拟非聚簇主键附加 `_tidb_rowid`。
/// Encodes/decodes handles as `row:<id>` (data kv) or `idx:<id>` (index kv)
/// keys, and reports kv-pair counts that mirror clustered vs non-clustered
/// primary keys: an extra kv is produced whenever `auto_row_id != 0`, i.e.
/// whenever the target table does not have a clustered index, exactly like
/// the real codec appending an extra `_tidb_rowid` kv.
/// 测试用冲突行编解码器。
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
        let id = handle.IntValue();
        Ok(vec![NewIntDatum(id), NewIntDatum(id), NewIntDatum(id)])
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
        _handle: &dyn Handle,
        row: &[Datum],
        auto_row_id: i64,
    ) -> Result<Pairs, String> {
        let extra = usize::from(auto_row_id != 0);
        Ok(Pairs {
            Pairs: (0..row.len() + extra).map(|_| KvPair::default()).collect(),
            RowID: Vec::new(),
        })
    }

    fn Close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
/// 记录集群读写字节的假流量计数器。
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

/// 仅对 `existing_row_keys` 中的句柄返回数据的假快照，对应 Go 夹具表中真实插入的行。
/// A `ConflictSnapshot` that only "has" data for the handles listed in
/// `existing_row_keys`, mirroring rows that were actually inserted in Go's
/// fixture table.
/// 测试用冲突快照。
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

/// 测试用冲突存储：提供快照；Begin 故意不支持。
struct FakeConflictStore {
    existing: HashMap<Vec<u8>, ValueEntry>,
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
        Err("begin is not supported by the handler test fake store".to_owned())
    }
    fn IsRetryableError(&self, _error: &str) -> bool {
        false
    }
}

/// 记录每次 `HandleEncodedRow` 调用，对应 Go 闭包统计行/KV 数与已处理句柄。
/// Records every `HandleEncodedRow` call, mirroring Go's closures that count
/// rows/kv-pairs and record which handles were actually processed.
#[derive(Default)]
/// 记录型行处理器。
struct RecordingRowHandler {
    row_count: i64,
    kv_pair_count: i64,
    handled_handles: Vec<String>,
}

impl EncodedRowHandler for RecordingRowHandler {
    fn HandleEncodedRow(
        &mut self,
        _context: &ConflictContext,
        row_key: &Key,
        _row: &[Datum],
        pairs: &Pairs,
    ) -> Result<(), String> {
        self.row_count += 1;
        self.kv_pair_count += pairs.Pairs.len() as i64;
        self.handled_handles.push(
            String::from_utf8_lossy(&row_key.0)
                .rsplit(':')
                .next()
                .unwrap()
                .to_owned(),
        );
        Ok(())
    }
}

/// 构造最小 TableInfo；`clustered` 对应 `PKIsHandle`（聚簇主键）。
fn make_table(clustered: bool, indices: Vec<IndexInfo>) -> Arc<TableInfo> {
    Arc::new(TableInfo {
        PKIsHandle: clustered,
        Indices: indices,
        ..Default::default()
    })
}

/// 数据 KV Handler：同一冲突行发送 10 次，校验行数与按聚簇与否变化的 KV 对数。
fn do_test_data_kv_handler(clustered: bool, expected_kv_pairs: i64) {
    let target_table = make_table(clustered, Vec::new());
    let progress_collector =
        Arc::new(astersql_dxf_framework_taskexecutor_execute::TestCollector::default());
    let base = NewBaseHandler(
        target_table,
        DataKVGroup,
        Box::new(FakeCodec),
        Some(progress_collector.clone()
            as Arc<dyn astersql_dxf_framework_taskexecutor_execute::Collector>),
    );
    let mut data_handler = NewDataKVHandler(base);
    data_handler.PreRun().expect("PreRun should succeed");

    let (sender, receiver) = mpsc::channel::<ConflictKVPair>();
    // 同一重复行发送 10 次，对齐 Go 夹具中 10 次相同冲突 data-kv 写入。
    // The exact same duplicate row is sent 10 times, matching Go's fixture of
    // 10 identical conflicted data-kv writes for the same handle.
    for _ in 0..10 {
        sender
            .send(ConflictKVPair {
                Key: Key(b"row:100".to_vec()),
                Value: Vec::new(),
            })
            .unwrap();
    }
    drop(sender);

    let context = conflict_context();
    let mut row_handler = RecordingRowHandler::default();
    data_handler
        .Run(&context, &receiver, &mut row_handler)
        .expect("Run should succeed");
    data_handler
        .Close(&context, &mut row_handler)
        .expect("Close should succeed");

    assert_eq!(10, row_handler.row_count);
    assert_eq!(expected_kv_pairs, row_handler.kv_pair_count);
    assert_eq!(10, progress_collector.ProcessedCnt.load(Ordering::SeqCst));
}

#[test]
/// 聚簇主键表：每行 3 个 datum → 期望 30 个 KV 对。
fn test_data_kv_handler_clustered_pk_table() {
    do_test_data_kv_handler(true, 30);
}

#[test]
/// 非聚簇主键表：额外 `_tidb_rowid` KV → 期望 40 个 KV 对。
fn test_data_kv_handler_non_clustered_pk_table() {
    do_test_data_kv_handler(false, 40);
}

/// 索引 KV Handler：缓冲限制、已处理过滤、快照命中共同决定哪些句柄进入行处理。
fn do_test_index_kv_handler(clustered: bool, expected_kv_pairs: i64) {
    let _guard = BUFFERED_HANDLE_LIMIT_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let previous_limit = BufferedHandleLimit.load(Ordering::Acquire);
    BufferedHandleLimit.store(2, Ordering::Release);

    let target_index_id = 2_i64;
    let target_table = make_table(
        clustered,
        vec![IndexInfo {
            ID: target_index_id,
            Columns: vec![IndexColumn::default()],
            ..Default::default()
        }],
    );

    // 夹具中仅句柄 1..=5 在集群有对应数据行。
    // Only handles 1..=5 have a matching data row in the fixture cluster.
    let existing: HashMap<Vec<u8>, ValueEntry> = (1..=5_i64)
        .map(|id| {
            (
                format!("row:1:{id}").into_bytes(),
                ValueEntry {
                    Value: vec![0_u8],
                    CommitTs: 0,
                },
            )
        })
        .collect();
    let cluster_store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore { existing });

    let shared_size = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let mut already_processed = NewBoundedKeySet(shared_size, 1 << 20);
    already_processed.Add(&Key(b"row:1:1".to_vec()));
    already_processed.Add(&Key(b"row:1:3".to_vec()));
    let already_processed = Arc::new(already_processed);

    let progress_collector =
        Arc::new(astersql_dxf_framework_taskexecutor_execute::TestCollector::default());
    let base = NewBaseHandler(
        target_table,
        astersql_ingestor_globalsort::kvgroup::IndexID2KVGroup(target_index_id),
        Box::new(FakeCodec),
        Some(progress_collector.clone()
            as Arc<dyn astersql_dxf_framework_taskexecutor_execute::Collector>),
    );
    let traffic_recorder = Arc::new(FakeTrafficRecorder::default());
    let mut index_handler = NewIndexKVHandler(
        base,
        NewLazyRefreshedSnapshot(cluster_store, Some(traffic_recorder.clone())),
        Some(NewKeyFilter(
            already_processed.clone(),
            Arc::new(std::sync::Mutex::new(NewBoundedKeySet(
                Arc::new(std::sync::atomic::AtomicI64::new(0)),
                1 << 20,
            ))),
        )),
    );
    index_handler.PreRun().expect("PreRun should succeed");

    // 仅 1..=5 有数据行；1 与 3 已标记处理过；最终仅 {2,4,5} 进入行处理器。
    // id:  1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
    // Only 1..=5 have data rows; 1 and 3 are pre-marked as already handled;
    // so only handles {2, 4, 5} should ever reach the row handler.
    let (sender, receiver) = mpsc::channel::<ConflictKVPair>();
    for id in 1..=16_i64 {
        sender
            .send(ConflictKVPair {
                Key: Key(format!("idx:{id}").into_bytes()),
                Value: Vec::new(),
            })
            .unwrap();
    }
    drop(sender);

    let context = conflict_context();
    let mut row_handler = RecordingRowHandler::default();
    index_handler
        .Run(&context, &receiver, &mut row_handler)
        .expect("Run should succeed");
    index_handler
        .Close(&context, &mut row_handler)
        .expect("Close should flush any leftover buffered handles");

    // MVI entries for the same rows can arrive in later snapshot batches.
    let (sender, receiver) = mpsc::channel();
    for id in [2, 4, 5] {
        sender
            .send(ConflictKVPair {
                Key: Key(format!("idx:{id}").into_bytes()),
                Value: Vec::new(),
            })
            .unwrap();
    }
    drop(sender);
    index_handler
        .Run(&context, &receiver, &mut row_handler)
        .unwrap();
    index_handler.Close(&context, &mut row_handler).unwrap();

    BufferedHandleLimit.store(previous_limit, Ordering::Release);

    assert!(traffic_recorder.read_bytes.load(Ordering::Acquire) > 0);
    assert_eq!(3, row_handler.row_count);
    assert_eq!(expected_kv_pairs, row_handler.kv_pair_count);
    let mut handled = row_handler.handled_handles.clone();
    handled.sort();
    assert_eq!(
        vec!["2".to_owned(), "4".to_owned(), "5".to_owned()],
        handled
    );
    assert_eq!(19, progress_collector.ProcessedCnt.load(Ordering::SeqCst));

    // 已在过滤集合中的句柄不得进入行处理器。
    // Handles already in the filter set must never reach the row handler.
    assert!(!row_handler.handled_handles.contains(&"1".to_owned()));
    assert!(!row_handler.handled_handles.contains(&"3".to_owned()));
}

#[test]
/// 聚簇主键 + 索引冲突：3 行 × 3 KV = 9。
fn test_index_kv_handler_clustered_pk_table() {
    do_test_index_kv_handler(true, 9);
}

#[test]
/// 非聚簇主键 + 索引冲突：3 行 × 4 KV = 12。
fn test_index_kv_handler_non_clustered_pk_table() {
    do_test_index_kv_handler(false, 12);
}

#[test]
/// PreRun：表上找不到索引 ID 时应失败。
fn test_index_kv_handler_pre_run_rejects_unknown_index() {
    let target_table = make_table(true, Vec::new());
    let base = NewBaseHandler(target_table, "999", Box::new(FakeCodec), None);
    let cluster_store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore {
        existing: HashMap::new(),
    });
    let mut index_handler =
        NewIndexKVHandler(base, NewLazyRefreshedSnapshot(cluster_store, None), None);
    let error = index_handler
        .PreRun()
        .expect_err("PreRun must fail when the index id is not found on the table");
    assert!(error.contains("999"));
}

#[test]
/// Handle：解码得到 table ID 为 0 时必须拒绝。
fn test_index_kv_handler_rejects_zero_table_id() {
    /// 始终返回 table ID 0 的编解码器。
    struct ZeroTableIDCodec;
    impl ConflictRowCodec for ZeroTableIDCodec {
        fn StripKeyspacePrefix(&self, key: &Key) -> Result<Key, String> {
            Ok(key.clone())
        }
        fn DecodeRowKey(&self, _key: &Key) -> Result<Box<dyn Handle>, String> {
            Ok(Box::new(IntHandle(0)))
        }
        fn DecodeRow(&self, _handle: &dyn Handle, _value: &[u8]) -> Result<Vec<Datum>, String> {
            Ok(Vec::new())
        }
        fn DecodeTableID(&self, _key: &Key) -> i64 {
            0
        }
        fn DecodeIndexHandle(
            &self,
            _key: &Key,
            _value: &[u8],
            _n: usize,
        ) -> Result<Box<dyn Handle>, String> {
            Ok(Box::new(IntHandle(0)))
        }
        fn EncodeRowKey(&self, _table_id: i64, handle: &dyn Handle) -> Key {
            Key(handle.Encoded())
        }
        fn EncodeRow(
            &mut self,
            _handle: &dyn Handle,
            _row: &[Datum],
            _auto_row_id: i64,
        ) -> Result<Pairs, String> {
            Ok(Pairs::default())
        }
        fn Close(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    let target_table = make_table(
        true,
        vec![IndexInfo {
            ID: 2,
            Columns: vec![IndexColumn::default()],
            ..Default::default()
        }],
    );
    let base = NewBaseHandler(target_table, "2", Box::new(ZeroTableIDCodec), None);
    let cluster_store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore {
        existing: HashMap::new(),
    });
    let mut index_handler =
        NewIndexKVHandler(base, NewLazyRefreshedSnapshot(cluster_store, None), None);
    index_handler.PreRun().unwrap();

    let context = conflict_context();
    let mut row_handler = RecordingRowHandler::default();
    let error = index_handler
        .Handle(
            &context,
            ConflictKVPair {
                Key: Key(b"idx:1".to_vec()),
                Value: Vec::new(),
            },
            &mut row_handler,
        )
        .expect_err("a zero table ID must be rejected");
    assert!(error.contains("invalid table ID"));
}

#[test]
fn index_key_is_registered_only_after_successful_row_callback() {
    let _guard = BUFFERED_HANDLE_LIMIT_LOCK.lock().unwrap();
    let previous = BufferedHandleLimit.swap(1, Ordering::AcqRel);
    let table = make_table(
        true,
        vec![IndexInfo {
            ID: 2,
            Columns: vec![IndexColumn::default()],
            ..Default::default()
        }],
    );
    let store = Arc::new(FakeConflictStore {
        existing: HashMap::from([(
            b"row:1:1".to_vec(),
            ValueEntry {
                Value: vec![0],
                CommitTs: 0,
            },
        )]),
    });
    let local = Arc::new(Mutex::new(NewBoundedKeySet(
        Arc::new(std::sync::atomic::AtomicI64::new(0)),
        1024,
    )));
    let filter = NewKeyFilter(
        Arc::new(NewBoundedKeySet(
            Arc::new(std::sync::atomic::AtomicI64::new(0)),
            1024,
        )),
        local.clone(),
    );
    let mut handler = NewIndexKVHandler(
        NewBaseHandler(table, "2", Box::new(FakeCodec), None),
        NewLazyRefreshedSnapshot(store, None),
        Some(filter),
    );
    handler.PreRun().unwrap();
    struct FailOnce(bool);
    impl EncodedRowHandler for FailOnce {
        fn HandleEncodedRow(
            &mut self,
            _: &ConflictContext,
            _: &Key,
            _: &[Datum],
            _: &Pairs,
        ) -> Result<(), String> {
            if !self.0 {
                self.0 = true;
                return Err("handle row".into());
            }
            Ok(())
        }
    }
    let context = ConflictContext::default();
    let mut callback = FailOnce(false);
    let pair = ConflictKVPair {
        Key: Key(b"idx:1".to_vec()),
        Value: Vec::new(),
    };
    assert_eq!(
        handler
            .Handle(&context, pair.clone(), &mut callback)
            .unwrap_err(),
        "handle row"
    );
    assert!(!local.lock().unwrap().Contains(&Key(b"row:1:1".to_vec())));
    handler.Handle(&context, pair, &mut callback).unwrap();
    assert!(local.lock().unwrap().Contains(&Key(b"row:1:1".to_vec())));
    BufferedHandleLimit.store(previous, Ordering::Release);
}
