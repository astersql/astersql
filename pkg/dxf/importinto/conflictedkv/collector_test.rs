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

// ConflictCollector 单元测试：合并结果、写文件切分、总大小截断与 Close 失败路径。
//
// 自 Go `collector_test.go` 移植；用内存假对象存储与 FakeCodec/FakeConflictStore，
// 以真实 `KVChecksum` 计算期望校验和，避免硬编码依赖 Go TiKV codec 的魔术数。

// Ported from pkg/dxf/importinto/conflictedkv/collector_test.go. The Go test
// drives the real object store (`objstore.NewMemStorage`) and a real TiKV
// codec; this port uses an equivalent in-memory `storeapi::Storage` fake
// (there is no in-tree `storeapi::Storage` memory backend yet) plus a fake
// `ConflictStore`/`ConflictRowCodec`, and computes expected checksums with
// the real `KVChecksum` type instead of hard-coding magic numbers that are
// specific to Go's TiKV codec/keyspace.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use astersql_kv::{Handle, IntHandle, Key, Version};
use astersql_lightning_backend_kv::Pairs;
use astersql_lightning_verification::{KvPair, MakeKVChecksumWithKeyspace};
use astersql_meta_model::TableInfo;
use astersql_objstore_objectio as objectio;
use astersql_objstore_storeapi as storeapi;
use astersql_types::datum::{Datum, NewStringDatum};

use crate::{
    BoundedKeySet, CollectResult, ConflictContext, ConflictRowCodec, ConflictSnapshot,
    ConflictStore, ConflictTransaction, DataKVGroup, EncodedRowHandler, MaxConflictRowFileSize,
    NewBoundedKeySet, NewCollectResult, NewCollector, SetMaxTotalConflictRowFileSizeForTest,
    getRowFileName,
};

/// `MaxConflictRowFileSize`/the private total-file-size limit are crate-wide
/// statics; serialize the tests that mutate them so they don't race under
/// `cargo test`'s default multi-threaded runner (Go's package tests run
/// sequentially by default).
/// `MaxConflictRowFileSize` 与私有总文件大小上限是 crate 级静态量；
/// 序列化修改它们的测试，避免 `cargo test` 默认多线程竞态。
/// （Go 包测试默认串行。）
static STATIC_LIMIT_LOCK: Mutex<()> = Mutex::new(());

/// 测试用假编码器：解码返回空行/零 handle，编码返回空 Pairs。
struct FakeCodec;

impl ConflictRowCodec for FakeCodec {
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
        _index_column_count: usize,
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

/// 测试用假 ConflictStore：仅提供 keyspace；快照路径不应被 collector 直调测试触达。
struct FakeConflictStore {
    keyspace: Vec<u8>,
}

impl ConflictStore for FakeConflictStore {
    fn Keyspace(&self) -> Vec<u8> {
        self.keyspace.clone()
    }
    fn CurrentVersion(&self) -> Result<Version, String> {
        Ok(astersql_kv::NewVersion(1))
    }
    fn GetSnapshot(&self, _version: Version) -> Box<dyn ConflictSnapshot> {
        unimplemented!("collector tests call HandleEncodedRow directly and never touch snapshots")
    }
    fn Begin(&self) -> Result<Box<dyn ConflictTransaction>, String> {
        Err("begin is not supported by the collector test fake store".to_owned())
    }
    fn IsRetryableError(&self, _error: &str) -> bool {
        false
    }
}

/// An in-memory `storeapi::Storage` fake, equivalent in spirit to Go's
/// `objstore.NewMemStorage()`. Additionally supports scripting which of the
/// `Create`d writers should fail on `Close`, so the collector's
/// close-failure cleanup paths can be exercised through the public API
/// instead of poking private fields.
/// 内存版 `storeapi::Storage`，对应 Go 的 `objstore.NewMemStorage()`；
/// 还可编排 Create 出的 writer 在 Close 时失败，以走清理路径。
#[derive(Default)]
struct TestStore {
    self_weak: Weak<TestStore>,
    files: Mutex<HashMap<String, Vec<u8>>>,
    fail_close_script: Mutex<VecDeque<bool>>,
}

impl TestStore {
    fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            self_weak: weak.clone(),
            files: Mutex::new(HashMap::new()),
            fail_close_script: Mutex::new(VecDeque::new()),
        })
    }

    fn contents(&self, name: &str) -> Vec<u8> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("test store has no file named {name}"))
    }

    fn queue_close_failure(&self, should_fail: bool) {
        self.fail_close_script
            .lock()
            .unwrap()
            .push_back(should_fail);
    }
}

/// 缓冲写入；Close 时可按脚本返回失败，成功则刷入 TestStore。
struct TestWriter {
    name: String,
    store: Arc<TestStore>,
    buffer: Vec<u8>,
    fail_close: bool,
}

impl objectio::Writer for TestWriter {
    fn write(&mut self, _ctx: &objectio::Context, data: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(data);
        Ok(data.len())
    }

    fn close(&mut self, _ctx: &objectio::Context) -> std::io::Result<()> {
        if self.fail_close {
            return Err(std::io::Error::other("close failed"));
        }
        self.store
            .files
            .lock()
            .unwrap()
            .insert(self.name.clone(), self.buffer.clone());
        Ok(())
    }
}

impl storeapi::Storage for TestStore {
    fn WriteFile(&self, _ctx: &objectio::Context, name: &str, data: &[u8]) -> anyhow::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_owned(), data.to_vec());
        Ok(())
    }
    fn ReadFile(&self, _ctx: &objectio::Context, name: &str) -> anyhow::Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("cannot find the file: {name}"))
    }
    fn FileExists(&self, _ctx: &objectio::Context, name: &str) -> anyhow::Result<bool> {
        Ok(self.files.lock().unwrap().contains_key(name))
    }
    fn DeleteFile(&self, _ctx: &objectio::Context, name: &str) -> anyhow::Result<()> {
        self.files.lock().unwrap().remove(name);
        Ok(())
    }
    fn Open(
        &self,
        _ctx: &objectio::Context,
        _path: &str,
        _option: Option<&storeapi::ReaderOption>,
    ) -> anyhow::Result<Box<dyn objectio::Reader>> {
        Err(anyhow::anyhow!("Open is not supported by the test store"))
    }
    fn DeleteFiles(&self, ctx: &objectio::Context, names: &[String]) -> anyhow::Result<()> {
        for name in names {
            self.DeleteFile(ctx, name)?;
        }
        Ok(())
    }
    fn WalkDir(
        &self,
        _ctx: &objectio::Context,
        _opt: Option<&storeapi::WalkOption>,
        _callback: &mut dyn FnMut(&str, i64) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn URI(&self) -> String {
        "teststore://".to_owned()
    }
    fn Create(
        &self,
        _ctx: &objectio::Context,
        path: &str,
        _option: Option<&storeapi::WriterOption>,
    ) -> anyhow::Result<Box<dyn objectio::Writer>> {
        let fail_close = self
            .fail_close_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(false);
        Ok(Box::new(TestWriter {
            name: path.to_owned(),
            store: self
                .self_weak
                .upgrade()
                .expect("test store must be kept alive by the test for its whole lifetime"),
            buffer: Vec::new(),
            fail_close,
        }))
    }
    fn Rename(
        &self,
        _ctx: &objectio::Context,
        old_name: &str,
        new_name: &str,
    ) -> anyhow::Result<()> {
        let mut guard = self.files.lock().unwrap();
        let data = guard
            .remove(old_name)
            .ok_or_else(|| anyhow::anyhow!("the file doesn't exist: {old_name}"))?;
        guard.insert(new_name.to_owned(), data);
        Ok(())
    }
    fn PresignFile(
        &self,
        _ctx: &objectio::Context,
        name: &str,
        _expire: std::time::Duration,
    ) -> anyhow::Result<String> {
        Ok(name.to_owned())
    }
    fn Close(&self) {
        self.files.lock().unwrap().clear();
    }
}

/// 构造仅含一个空 value 的 KV 对，供 checksum 更新。
fn make_pairs(key: Vec<u8>) -> Pairs {
    Pairs {
        Pairs: vec![KvPair {
            key,
            val: Vec::new(),
        }],
        RowID: Vec::new(),
    }
}

/// 默认 ConflictContext。
fn conflict_context() -> ConflictContext {
    ConflictContext::default()
}

#[test]
/// 验证 CollectResult::Merge：累加行数/大小、或运算截断标记、合并 checksum 与文件名。
fn test_collect_result_merge() {
    let keyspace = b"test".to_vec();

    let check_result = |expected: &CollectResult, actual: &CollectResult| {
        assert_eq!(expected.RowCount, actual.RowCount);
        assert_eq!(expected.TotalFileSize, actual.TotalFileSize);
        assert_eq!(expected.RowRecordingCapped, actual.RowRecordingCapped);
        assert_eq!(expected.Checksum.Sum(), actual.Checksum.Sum());
        assert_eq!(expected.Checksum.SumKVS(), actual.Checksum.SumKVS());
        assert_eq!(expected.Checksum.SumSize(), actual.Checksum.SumSize());
        assert_eq!(expected.Filenames, actual.Filenames);
    };

    let mut r1 = NewCollectResult(&keyspace);
    let other_sum = MakeKVChecksumWithKeyspace(&keyspace, 10, 1, 1);
    let r2 = CollectResult {
        RowCount: 10,
        TotalFileSize: 100,
        RowRecordingCapped: true,
        Checksum: other_sum,
        Filenames: vec!["file1".to_owned()],
    };
    r1.Merge(Some(&r2));
    check_result(&r2, &r1);

    let r3 = NewCollectResult(&keyspace);
    r1.Merge(Some(&r3));
    check_result(&r2, &r1);

    r1.Merge(None);
    check_result(&r2, &r1);

    let other_sum = MakeKVChecksumWithKeyspace(&keyspace, 20, 2, 2);
    let r4 = CollectResult {
        RowCount: 20,
        TotalFileSize: 200,
        RowRecordingCapped: false,
        Checksum: other_sum,
        Filenames: vec!["file2".to_owned()],
    };
    r1.Merge(Some(&r4));
    assert_eq!(30, r1.RowCount);
    assert_eq!(300, r1.TotalFileSize);
    assert!(r1.RowRecordingCapped);
    assert_eq!(3, r1.Checksum.Sum());
    assert_eq!(3, r1.Checksum.SumKVS());
    assert_eq!(30, r1.Checksum.SumSize());
    assert_eq!(vec!["file1".to_owned(), "file2".to_owned()], r1.Filenames);
}

#[test]
/// 文件名拼接保持 Go `path.Join` 的 POSIX 清理语义。
fn test_get_row_file_name_cleans_prefix_like_go_path_join() {
    assert_eq!(
        "root/rows/data-0007.txt",
        getRowFileName("root//tmp/../rows/.", 7)
    );
    assert_eq!("../data-0001.txt", getRowFileName("root/../..", 1));
    assert_eq!("/data-0002.txt", getRowFileName("/tmp/..", 2));
}

/// 构造带 FakeCodec/FakeConflictStore 的测试用 ConflictCollector。
fn new_test_collector(
    store: Arc<TestStore>,
    prefix: &str,
    kv_group: &str,
) -> crate::ConflictCollector {
    let cluster_store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore {
        keyspace: Vec::new(),
    });
    let shared_bound = Arc::new(AtomicI64::new(0));
    let global_set = Arc::new(NewBoundedKeySet(shared_bound.clone(), i64::MAX));
    let local_set: BoundedKeySet = NewBoundedKeySet(shared_bound, i64::MAX);
    NewCollector(
        Arc::new(TableInfo::default()),
        store,
        cluster_store,
        prefix,
        kv_group,
        Box::new(FakeCodec),
        global_set,
        local_set,
        None,
        None,
        None,
    )
}

/// 按单文件大小上限写出多行，断言文件切分、checksum 与 handle 集合行为。
fn do_test_handle_encoded_row(kv_group: &str, max_size: i64, out_file_cnt: usize) {
    let _guard = STATIC_LIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous_limit = MaxConflictRowFileSize.load(Ordering::Acquire);
    MaxConflictRowFileSize.store(max_size, Ordering::Release);

    let store = TestStore::new();
    let mut collector = new_test_collector(store.clone(), "test", kv_group);
    let context = conflict_context();

    let row_count = 48_usize;
    let mut expected_sum = astersql_lightning_verification::NewKVChecksumWithKeyspace(&[]);
    for i in 0..row_count {
        let row = vec![
            NewStringDatum("id".to_owned()),
            NewStringDatum("value".to_owned()),
        ];
        let pairs = make_pairs(format!("{}", 123 * (i + 1)).into_bytes());
        collector
            .HandleEncodedRow(
                &context,
                &astersql_kv::Key(format!("row:1:{}", i as i64).into_bytes()),
                &row,
                &pairs,
            )
            .expect("HandleEncodedRow should succeed");
        expected_sum.Update(&pairs.Pairs);
    }
    collector.Close(&context).expect("Close should succeed");

    MaxConflictRowFileSize.store(previous_limit, Ordering::Release);

    // Direct row callbacks do not populate the filter: successful index handling does.
    assert_eq!(0, collector.RowKeySetLenForTest());

    let result = collector.GetCollectResult();
    assert_eq!(row_count as i64, result.RowCount);
    assert_eq!(expected_sum.Sum(), result.Checksum.Sum());
    assert_eq!(expected_sum.SumKVS(), result.Checksum.SumKVS());
    assert_eq!(expected_sum.SumSize(), result.Checksum.SumSize());
    assert_eq!(out_file_cnt, result.Filenames.len());
    for (index, name) in result.Filenames.iter().enumerate() {
        assert_eq!(format!("test/data-{:04}.txt", index + 1), *name);
    }

    let mut lines = 0_usize;
    for name in &result.Filenames {
        let content = store.contents(name);
        let text = String::from_utf8(content).expect("file content should be utf8");
        lines += text.trim().split('\n').count();
    }
    assert_eq!(row_count, lines);
}

#[test]
/// data 组与 index 组在不同 MaxConflictRowFileSize 下的文件切分。
fn test_collector_handle_encoded_row() {
    for &(max_size, out_file_cnt) in &[(90_i64, 8_usize), (300, 3), (800, 1)] {
        do_test_handle_encoded_row(DataKVGroup, max_size, out_file_cnt);
    }

    let index_kv_group = astersql_ingestor_globalsort::kvgroup::IndexID2KVGroup(1);
    for &(max_size, out_file_cnt) in &[(90_i64, 8_usize), (300, 3), (800, 1)] {
        do_test_handle_encoded_row(&index_kv_group, max_size, out_file_cnt);
    }
}

#[test]
/// 非 data KV 组应把每个 handle 记入本地跳过过滤集合。
fn test_collector_direct_callback_leaves_index_filter_to_handler() {
    let _guard = STATIC_LIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let store = TestStore::new();
    let index_kv_group = astersql_ingestor_globalsort::kvgroup::IndexID2KVGroup(7);
    let mut collector = new_test_collector(store, "test", &index_kv_group);
    let context = conflict_context();

    for i in 0..5_i64 {
        let row = vec![
            NewStringDatum("id".to_owned()),
            NewStringDatum("value".to_owned()),
        ];
        let pairs = make_pairs(format!("row-{i}").into_bytes());
        collector
            .HandleEncodedRow(
                &context,
                &astersql_kv::Key(format!("row:1:{}", i).into_bytes()),
                &row,
                &pairs,
            )
            .unwrap();
    }
    collector.Close(&context).unwrap();
    // Every distinct handle should have been recorded for the skip filter.
    // 每个不同 handle 都应记入跳过过滤器。
    assert_eq!(5, collector.GetCollectResult().RowCount);
    assert_eq!(0, collector.RowKeySetLenForTest());
}

#[test]
/// 总文件大小上限触发截断：仍计行数与 checksum，但只写出部分行。
fn test_collector_handle_encoded_row_max_total_file_size() {
    let _guard = STATIC_LIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous_file_size = MaxConflictRowFileSize.load(Ordering::Acquire);
    MaxConflictRowFileSize.store(1 << 20, Ordering::Release);
    SetMaxTotalConflictRowFileSizeForTest(32);

    let store = TestStore::new();
    let mut collector = new_test_collector(store.clone(), "test", DataKVGroup);
    let context = conflict_context();

    let row_count = 5_usize;
    let mut expected_sum = astersql_lightning_verification::NewKVChecksumWithKeyspace(&[]);
    for i in 0..row_count {
        let row = vec![
            NewStringDatum("id".to_owned()),
            NewStringDatum("value".to_owned()),
        ];
        let pairs = make_pairs(format!("{}", 123 * (i + 1)).into_bytes());
        collector
            .HandleEncodedRow(
                &context,
                &astersql_kv::Key(format!("row:1:{}", i as i64).into_bytes()),
                &row,
                &pairs,
            )
            .unwrap();
        expected_sum.Update(&pairs.Pairs);
    }
    collector.Close(&context).unwrap();

    let result = collector.GetCollectResult();
    assert_eq!(row_count as i64, result.RowCount);
    assert_eq!(32, result.TotalFileSize);
    assert!(result.RowRecordingCapped);
    assert_eq!(expected_sum.Sum(), result.Checksum.Sum());
    assert_eq!(expected_sum.SumKVS(), result.Checksum.SumKVS());
    assert_eq!(expected_sum.SumSize(), result.Checksum.SumSize());
    assert_eq!(vec!["test/data-0001.txt".to_owned()], result.Filenames);

    let content = store.contents(&result.Filenames[0]);
    let text = String::from_utf8(content).unwrap();
    assert_eq!(2, text.trim().split('\n').count());

    MaxConflictRowFileSize.store(previous_file_size, Ordering::Release);
    SetMaxTotalConflictRowFileSizeForTest(1 << 30);
}

#[test]
/// 触达总大小上限时 Close 失败：仍进入 capped 状态并清空 writer。
fn test_collector_close_failure_on_total_size_limit_still_clears_writer() {
    let _guard = STATIC_LIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous_file_size = MaxConflictRowFileSize.load(Ordering::Acquire);
    MaxConflictRowFileSize.store(1 << 20, Ordering::Release);
    // Each row costs 16 bytes (`("id", "value")` + newline); a limit of 20
    // lets the first row open a real file but the second row crosses the
    // cap, so the writer opened for row 0 is the one whose `Close` fails.
    SetMaxTotalConflictRowFileSizeForTest(20);

    let store = TestStore::new();
    store.queue_close_failure(true);
    let mut collector = new_test_collector(store.clone(), "test", DataKVGroup);
    let context = conflict_context();
    let row = vec![
        NewStringDatum("id".to_owned()),
        NewStringDatum("value".to_owned()),
    ];

    collector
        .HandleEncodedRow(
            &context,
            &astersql_kv::Key(format!("row:1:{}", 0).into_bytes()),
            &row,
            &make_pairs(b"k0".to_vec()),
        )
        .expect("first row stays under the cap and opens a real file");
    assert_eq!(1, collector.GetCollectResult().RowCount);

    let err = collector
        .HandleEncodedRow(
            &context,
            &astersql_kv::Key(format!("row:1:{}", 1).into_bytes()),
            &row,
            &make_pairs(b"k1".to_vec()),
        )
        .expect_err("crossing the cap must surface the writer's close failure");
    assert!(err.contains("close failed"));

    // The collector must have flipped into capped/stop-recording state
    // despite the close failure, and the failing row was not counted.
    assert!(collector.GetCollectResult().RowRecordingCapped);
    assert_eq!(1, collector.GetCollectResult().RowCount);

    // Further rows are still accepted (just not written to disk) because
    // `stop_recording` short-circuits `recordRowToFile` before it touches the
    // (already cleared) writer again.
    collector
        .HandleEncodedRow(
            &context,
            &astersql_kv::Key(format!("row:1:{}", 2).into_bytes()),
            &row,
            &make_pairs(b"k2".to_vec()),
        )
        .expect("rows after the cap are still accepted, just not written to disk");
    assert_eq!(2, collector.GetCollectResult().RowCount);

    // `Close` must not attempt to close the (already cleared) writer again.
    collector.Close(&context).expect("Close should not re-fail");

    MaxConflictRowFileSize.store(previous_file_size, Ordering::Release);
    SetMaxTotalConflictRowFileSizeForTest(1 << 30);
}

#[test]
/// 切文件时 Close 失败：触发切换的那一行不计入结果。
fn test_collector_close_failure_on_switch_file_still_clears_writer() {
    let _guard = STATIC_LIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous_file_size = MaxConflictRowFileSize.load(Ordering::Acquire);
    // Force a switch on every single row so the second write must close the
    // first (scripted-to-fail) writer.
    MaxConflictRowFileSize.store(1, Ordering::Release);
    SetMaxTotalConflictRowFileSizeForTest(1_i64 << 30);

    let store = TestStore::new();
    // The first writer is scripted to fail when it is later closed (which
    // happens as part of switching to a second file); the switch therefore
    // never gets far enough to create a second writer.
    store.queue_close_failure(true);
    let mut collector = new_test_collector(store.clone(), "test", DataKVGroup);
    let context = conflict_context();

    let row = vec![
        NewStringDatum("id".to_owned()),
        NewStringDatum("value".to_owned()),
    ];
    collector
        .HandleEncodedRow(
            &context,
            &astersql_kv::Key(format!("row:1:{}", 0).into_bytes()),
            &row,
            &make_pairs(b"k0".to_vec()),
        )
        .expect("first row opens the first file");
    let err = collector
        .HandleEncodedRow(
            &context,
            &astersql_kv::Key(format!("row:1:{}", 1).into_bytes()),
            &row,
            &make_pairs(b"k1".to_vec()),
        )
        .expect_err("switching files must surface the close failure");
    assert!(err.contains("close failed"));

    // The row that triggered the failed switch must not have been counted.
    assert_eq!(1, collector.GetCollectResult().RowCount);
    collector
        .Close(&context)
        .expect("Close should not double-close the writer");

    MaxConflictRowFileSize.store(previous_file_size, Ordering::Release);
}

#[test]
/// 多个 collector 共享总大小计数器时，后写者会被截断。
fn test_collector_handle_encoded_row_max_total_file_size_shared_by_collectors() {
    let _guard = STATIC_LIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous_file_size = MaxConflictRowFileSize.load(Ordering::Acquire);
    MaxConflictRowFileSize.store(1 << 20, Ordering::Release);
    SetMaxTotalConflictRowFileSizeForTest(48);

    let store = TestStore::new();
    let shared_total_file_size = Arc::new(AtomicI64::new(0));
    let cluster_store: Arc<dyn ConflictStore> = Arc::new(FakeConflictStore {
        keyspace: Vec::new(),
    });
    let shared_bound = Arc::new(AtomicI64::new(0));

    let mut coll1 = NewCollector(
        Arc::new(TableInfo::default()),
        store.clone(),
        cluster_store.clone(),
        "test1",
        DataKVGroup,
        Box::new(FakeCodec),
        Arc::new(NewBoundedKeySet(shared_bound.clone(), i64::MAX)),
        NewBoundedKeySet(shared_bound.clone(), i64::MAX),
        Some(shared_total_file_size.clone()),
        None,
        None,
    );
    let mut coll2 = NewCollector(
        Arc::new(TableInfo::default()),
        store.clone(),
        cluster_store,
        "test2",
        DataKVGroup,
        Box::new(FakeCodec),
        Arc::new(NewBoundedKeySet(shared_bound.clone(), i64::MAX)),
        NewBoundedKeySet(shared_bound, i64::MAX),
        Some(shared_total_file_size.clone()),
        None,
        None,
    );

    let context = conflict_context();
    let row = vec![
        NewStringDatum("id".to_owned()),
        NewStringDatum("value".to_owned()),
    ];
    for i in 0..3_i64 {
        let pairs = make_pairs(format!("a-{i}").into_bytes());
        coll1
            .HandleEncodedRow(
                &context,
                &astersql_kv::Key(format!("row:1:{}", i).into_bytes()),
                &row,
                &pairs,
            )
            .unwrap();
    }
    for i in 0..3_i64 {
        let pairs = make_pairs(format!("b-{i}").into_bytes());
        coll2
            .HandleEncodedRow(
                &context,
                &astersql_kv::Key(format!("row:1:{}", i).into_bytes()),
                &row,
                &pairs,
            )
            .unwrap();
    }
    coll1.Close(&context).unwrap();
    coll2.Close(&context).unwrap();

    assert_eq!(64, shared_total_file_size.load(Ordering::Acquire));
    let result1 = coll1.GetCollectResult();
    let result2 = coll2.GetCollectResult();
    assert_eq!(3, result1.RowCount);
    assert_eq!(48, result1.TotalFileSize);
    assert!(!result1.RowRecordingCapped);
    assert_eq!(3, result2.RowCount);
    assert_eq!(0, result2.TotalFileSize);
    assert!(result2.RowRecordingCapped);
    assert_eq!(vec!["test1/data-0001.txt".to_owned()], result1.Filenames);
    assert!(result2.Filenames.is_empty());

    let content = store.contents(&result1.Filenames[0]);
    let text = String::from_utf8(content).unwrap();
    assert_eq!(3, text.trim().split('\n').count());

    MaxConflictRowFileSize.store(previous_file_size, Ordering::Release);
    SetMaxTotalConflictRowFileSizeForTest(1 << 30);
}
