// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// encode-and-sort 算子的可执行 Go 行为回归：错误取消、并发首错、
// writer 内存、keyspace 编码、对象存储与关闭资源。

use crate::{getWriterMemorySizeLimit, subtaskPrefix};
use astersql_dxf_framework_proto::{NewAllocatable, StepResource};
use astersql_executor_importer::Plan;
use astersql_lightning_backend::{BackendError, ChunkFlushStatus, EngineWriter};
use astersql_lightning_backend_encode::{Context, Rows};
use std::sync::{Arc, Mutex};

struct CloseProbe {
    calls: Arc<Mutex<Vec<(String, bool)>>>,
    name: String,
    fail: bool,
}

impl EngineWriter for CloseProbe {
    fn AppendRows(&mut self, _: &Context, _: &[String], _: &dyn Rows) -> Result<(), BackendError> {
        Ok(())
    }
    fn IsSynced(&self) -> bool {
        true
    }
    fn Close(&mut self, ctx: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        self.calls
            .lock()
            .unwrap()
            .push((self.name.clone(), ctx.cancelled));
        if self.fail {
            Err(BackendError::new(format!("{} close failed", self.name)))
        } else {
            Ok(None)
        }
    }
}

#[test]
fn worker_close_stops_after_data_error_and_uses_live_context() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut worker = crate::chunkWorker {
        ctx: Context::cancelled(),
        dataWriter: Some(Arc::new(Mutex::new(Box::new(CloseProbe {
            calls: calls.clone(),
            name: "data".into(),
            fail: true,
        }) as Box<dyn EngineWriter>))),
        indexWriter: Some(Arc::new(Mutex::new(Box::new(CloseProbe {
            calls: calls.clone(),
            name: "index".into(),
            fail: false,
        }) as Box<dyn EngineWriter>))),
        collector: None,
    };
    assert_eq!(worker.Close().unwrap_err().to_string(), "data close failed");
    assert_eq!(*calls.lock().unwrap(), vec![("data".into(), false)]);
}

#[test]
fn worker_close_reports_index_error_after_data_success() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut worker = crate::chunkWorker {
        ctx: Context::default(),
        dataWriter: Some(Arc::new(Mutex::new(Box::new(CloseProbe {
            calls: calls.clone(),
            name: "data".into(),
            fail: false,
        }) as Box<dyn EngineWriter>))),
        indexWriter: Some(Arc::new(Mutex::new(Box::new(CloseProbe {
            calls: calls.clone(),
            name: "index".into(),
            fail: true,
        }) as Box<dyn EngineWriter>))),
        collector: None,
    };
    assert_eq!(
        worker.Close().unwrap_err().to_string(),
        "index close failed"
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![("data".into(), false), ("index".into(), false)]
    );
}

#[test]
fn chunk_writer_proxy_keeps_writer_open_across_chunks() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut worker = crate::chunkWorker {
        ctx: Context::default(),
        dataWriter: Some(Arc::new(Mutex::new(Box::new(CloseProbe {
            calls: calls.clone(),
            name: "data".into(),
            fail: false,
        }) as Box<dyn EngineWriter>))),
        indexWriter: None,
        collector: None,
    };
    for _ in 0..2 {
        let (mut data, _) = worker.chunkWriters();
        data.as_mut().unwrap().Close(&Context::default()).unwrap();
    }
    assert!(calls.lock().unwrap().is_empty());
    worker.Close().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec![("data".into(), false)]);
}

#[test]
fn operator_wire_inputs_are_send() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<Plan>();
    assert_send::<astersql_executor_importer::Chunk>();
    assert_send::<crate::TaskMeta>();
    assert_send::<astersql_lightning_backend::OpenedEngine>();
    assert_sync::<astersql_lightning_backend::OpenedEngine>();
}

#[test]
fn global_sort_writer_persists_files_to_object_store() {
    use astersql_ingestor_simplesst::writer::WriterBuilder;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_objstore_storeapi::{Context as StoreContext, Storage, StorageRef};
    let store: StorageRef = Arc::new(NewMemStorage::default());
    let sink = Arc::new(crate::ObjectStoreWriterSink::new(
        store.clone(),
        Context::default(),
    ));
    let mut writer = WriterBuilder::new().build_with_sink(sink, "10/20", "data/worker");
    writer.write_row(b"row", b"value").unwrap();
    let summary = writer.close().unwrap();
    let paths = &summary.MultipleFilesStats[0].Filenames[0];
    assert!(
        store
            .FileExists(&StoreContext::default(), &paths[0])
            .unwrap()
    );
    assert!(
        store
            .FileExists(&StoreContext::default(), &paths[1])
            .unwrap()
    );
}

#[test]
fn global_data_engine_writer_encodes_backend_rows_to_object_files() {
    use astersql_ingestor_simplesst::writer::WriterBuilder;
    use astersql_lightning_backend_kv::MakeRowsFromKvPairs;
    use astersql_lightning_verification::KvPair;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_objstore_storeapi::{Context as StoreContext, Storage, StorageRef};
    let store: StorageRef = Arc::new(NewMemStorage::default());
    let sink = Arc::new(crate::ObjectStoreWriterSink::new(
        store.clone(),
        Context::default(),
    ));
    let writer = WriterBuilder::new().build_with_sink(sink, "10/20", "data/worker");
    let mut engine = crate::GlobalDataEngineWriter::new(writer);
    let rows = MakeRowsFromKvPairs(vec![KvPair {
        key: b"k".to_vec(),
        val: b"v".to_vec(),
    }]);
    engine
        .AppendRows(&Context::default(), &[], rows.as_ref())
        .unwrap();
    engine.Close(&Context::default()).unwrap();
    let summary = engine.summary().unwrap();
    let paths = &summary.MultipleFilesStats[0].Filenames[0];
    assert!(
        store
            .FileExists(&StoreContext::default(), &paths[0])
            .unwrap()
    );
    assert!(
        store
            .FileExists(&StoreContext::default(), &paths[1])
            .unwrap()
    );
}

#[test]
fn global_index_route_writer_persists_per_index_files() {
    use astersql_executor_importer::{NewIndexRouteWriter, WriterFactory};
    use astersql_ingestor_simplesst::writer::WriterBuilder;
    use astersql_lightning_backend_kv::GroupedPairs;
    use astersql_lightning_verification::KvPair;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_objstore_storeapi::{Context as StoreContext, Storage, StorageRef};
    use std::collections::BTreeMap;
    let store: StorageRef = Arc::new(NewMemStorage::default());
    let sink = Arc::new(crate::ObjectStoreWriterSink::new(
        store.clone(),
        Context::default(),
    ));
    let factory: WriterFactory = Arc::new(move |index_id| {
        let writer_id = format!("index/{index_id}/worker");
        let writer = WriterBuilder::new().build_with_sink(sink.clone(), "10/20", &writer_id);
        Ok(Box::new(crate::GlobalIndexSstWriter::new(writer)))
    });
    let mut route = NewIndexRouteWriter(Default::default(), factory);
    let rows = GroupedPairs(BTreeMap::from([
        (
            7_i64,
            vec![KvPair {
                key: b"i7".to_vec(),
                val: b"v7".to_vec(),
            }],
        ),
        (
            8_i64,
            vec![KvPair {
                key: b"i8".to_vec(),
                val: b"v8".to_vec(),
            }],
        ),
    ]));
    route.AppendRows(&Context::default(), &[], &rows).unwrap();
    route.Close(&Context::default()).unwrap();
    let context = StoreContext::default();
    let mut found = Vec::new();
    store
        .WalkDir(&context, None, &mut |name, _| {
            found.push(name.to_owned());
            Ok(())
        })
        .unwrap();
    assert!(found.iter().any(|name| name.contains("index/7/worker")));
    assert!(found.iter().any(|name| name.contains("index/8/worker")));
}

#[test]
fn global_worker_builds_data_and_index_writers_and_merges_summaries() {
    use astersql_executor_importer::{GenKVIndex, OnDupKeyModeCapture};
    use astersql_lightning_backend_kv::{GroupedPairs, MakeRowsFromKvPairs};
    use astersql_lightning_verification::KvPair;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_objstore_storeapi::StorageRef;
    use std::collections::{BTreeMap, HashMap};
    let store: StorageRef = Arc::new(NewMemStorage::default());
    let summaries = Arc::new(crate::GlobalWriterSummaries::default());
    let indices = HashMap::from([(
        7,
        GenKVIndex {
            name: "u".into(),
            Unique: true,
        },
    )]);
    let mut worker = crate::BuildGlobalChunkWorker(
        Context::default(),
        10,
        20,
        "worker",
        store,
        b"ks:".to_vec(),
        indices,
        OnDupKeyModeCapture,
        1024,
        1024,
        512,
        512,
        None,
        summaries.clone(),
    );
    let (mut data, mut index) = worker.chunkWriters();
    let data_rows = MakeRowsFromKvPairs(vec![KvPair {
        key: b"row".to_vec(),
        val: b"value".to_vec(),
    }]);
    data.as_mut()
        .unwrap()
        .AppendRows(&Context::default(), &[], data_rows.as_ref())
        .unwrap();
    let index_rows = GroupedPairs(BTreeMap::from([(
        7,
        vec![KvPair {
            key: b"uk".to_vec(),
            val: b"row".to_vec(),
        }],
    )]));
    index
        .as_mut()
        .unwrap()
        .AppendRows(&Context::default(), &[], &index_rows)
        .unwrap();
    worker.Close().unwrap();
    assert_eq!(summaries.data_file_count(), 1);
    assert_eq!(summaries.index_file_count(), 1);
    assert_eq!(summaries.data_meta().TotalKVCnt, 1);
    assert_eq!(summaries.data_meta().StartKey, b"ks:row".to_vec());
    assert_eq!(summaries.index_meta(7).unwrap().TotalKVCnt, 1);
    assert_eq!(summaries.index_meta(7).unwrap().StartKey, b"ks:uk".to_vec());
}

#[test]
fn cancelled_global_worker_uses_fresh_deadline_to_flush_object_files() {
    use astersql_lightning_backend_kv::MakeRowsFromKvPairs;
    use astersql_lightning_verification::KvPair;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    let summaries = Arc::new(crate::GlobalWriterSummaries::default());
    let mut worker = crate::BuildGlobalChunkWorker(
        Context::cancelled(),
        10,
        20,
        "worker",
        Arc::new(NewMemStorage::default()),
        Vec::new(),
        std::collections::HashMap::new(),
        astersql_executor_importer::OnDupKeyModeCapture,
        1024,
        1024,
        512,
        512,
        None,
        summaries.clone(),
    );
    let (mut data, _) = worker.chunkWriters();
    let rows = MakeRowsFromKvPairs(vec![KvPair {
        key: b"r".to_vec(),
        val: b"v".to_vec(),
    }]);
    data.as_mut()
        .unwrap()
        .AppendRows(&Context::default(), &[], rows.as_ref())
        .unwrap();
    worker.Close().unwrap();
    assert_eq!(summaries.data_file_count(), 1);
}

#[test]
fn worker_checksums_merge_into_subtask_summary() {
    use astersql_lightning_verification::{KvPair, NewKVGroupChecksumWithKeyspace};
    let summaries = crate::GlobalWriterSummaries::default();
    let mut first = NewKVGroupChecksumWithKeyspace(b"ks");
    first.UpdateOneDataKV(&KvPair {
        key: b"d1".to_vec(),
        val: b"v1".to_vec(),
    });
    let mut second = NewKVGroupChecksumWithKeyspace(b"ks");
    second.UpdateOneDataKV(&KvPair {
        key: b"d2".to_vec(),
        val: b"v2".to_vec(),
    });
    summaries.merge_checksum(&first);
    summaries.merge_checksum(&second);
    assert_eq!(summaries.checksum().DataAndIndexSumSize().0, 12);
}

#[test]
fn allocator_watermarks_merge_from_parallel_workers_by_maximum() {
    use astersql_lightning_backend_kv::AllocatorType;
    let summaries = crate::GlobalWriterSummaries::default();
    summaries.merge_allocator_maximums(&std::collections::HashMap::from([
        (AllocatorType::RowIDAllocType, 17),
        (AllocatorType::AutoRandomType, 4),
    ]));
    summaries.merge_allocator_maximums(&std::collections::HashMap::from([
        (AllocatorType::RowIDAllocType, 9),
        (AllocatorType::AutoRandomType, 21),
    ]));
    assert_eq!(
        summaries.allocator_maximums()[&AllocatorType::RowIDAllocType],
        17
    );
    assert_eq!(
        summaries.allocator_maximums()[&AllocatorType::AutoRandomType],
        21
    );
}

#[test]
fn async_encode_operator_cancels_on_worker_error() {
    use astersql_dxf_operator::compose::{DataChannel, NewSimpleDataChannel, WithSource};
    use astersql_dxf_operator::operator::Operator;
    use astersql_resourcemanager_pool_workerpool::{Channel, Context as PoolContext};
    struct ErrorWorker;
    impl crate::EncodeSortWorker for ErrorWorker {
        fn HandleTask(&mut self, _: crate::EncodeSortTask) -> Result<(), String> {
            Err("mock err".into())
        }
        fn Close(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
    let context = PoolContext::background();
    let factory = Arc::new(|| -> Result<Box<dyn crate::EncodeSortWorker>, String> {
        Ok(Box::new(ErrorWorker))
    });
    let mut op = crate::newEncodeAndSortOperator(context.clone(), 1, 3, "", 1, factory);
    let source = NewSimpleDataChannel(Channel::bounded(0));
    op.SetSource(source.clone());
    op.Open().unwrap();
    assert!(source.Channel().send(crate::EncodeSortTask {
        Plan: Plan::default(),
        Chunk: Default::default()
    }));
    for _ in 0..300 {
        if context.OperatorErr().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(context.OperatorErr().unwrap().to_string(), "mock err");
    assert!(context.IsCancelled());
    op.Close().unwrap();
}

#[test]
fn encode_context_observes_pool_cancellation_during_chunk() {
    use astersql_resourcemanager_pool_workerpool::Context as PoolContext;
    let pool = PoolContext::background();
    let encode = Context::with_cancel_check(Arc::new({
        let pool = pool.clone();
        move || pool.IsCancelled()
    }));
    assert!(!encode.is_cancelled());
    pool.Cancel();
    assert!(encode.is_cancelled());
}

#[test]
fn encode_context_expires_writer_close_deadline() {
    let context = Context::with_timeout(std::time::Duration::ZERO);
    assert!(context.is_cancelled());
}

#[test]
fn object_sink_stops_writes_after_context_cancellation() {
    let sink = crate::ObjectStoreWriterSink::new(
        Arc::new(astersql_objstore::azblob::MemoryStorage::default()),
        Context::cancelled(),
    );
    let error = astersql_ingestor_simplesst::writer::WriterSink::write_file(&sink, "x", b"data")
        .unwrap_err();
    assert!(error.contains("cancelled"));
}

#[test]
fn object_sink_propagates_deadline_into_active_storage_write() {
    use astersql_objstore::objectio::{Reader as ObjectReader, Writer as ObjectWriter};
    use astersql_objstore_storeapi::{
        Context as StoreContext, ReaderOption, Storage, WalkOption, WriterOption,
    };
    struct SlowStorage;
    impl Storage for SlowStorage {
        fn DeleteFile(&self, _: &StoreContext, _: &str) -> anyhow::Result<()> {
            unreachable!()
        }
        fn WriteFile(&self, context: &StoreContext, _: &str, _: &[u8]) -> anyhow::Result<()> {
            let stop = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while std::time::Instant::now() < stop {
                if context.is_cancelled() {
                    return context.check().map_err(Into::into);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            anyhow::bail!("deadline was not forwarded")
        }
        fn ReadFile(&self, _: &StoreContext, _: &str) -> anyhow::Result<Vec<u8>> {
            unreachable!()
        }
        fn FileExists(&self, _: &StoreContext, _: &str) -> anyhow::Result<bool> {
            unreachable!()
        }
        fn DeleteFiles(&self, _: &StoreContext, _: &[String]) -> anyhow::Result<()> {
            unreachable!()
        }
        fn Open(
            &self,
            _: &StoreContext,
            _: &str,
            _: Option<&ReaderOption>,
        ) -> anyhow::Result<Box<dyn ObjectReader>> {
            unreachable!()
        }
        fn WalkDir(
            &self,
            _: &StoreContext,
            _: Option<&WalkOption>,
            _: &mut dyn FnMut(&str, i64) -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            unreachable!()
        }
        fn URI(&self) -> String {
            "slow://".into()
        }
        fn Create(
            &self,
            _: &StoreContext,
            _: &str,
            _: Option<&WriterOption>,
        ) -> anyhow::Result<Box<dyn ObjectWriter>> {
            unreachable!()
        }
        fn Rename(&self, _: &StoreContext, _: &str, _: &str) -> anyhow::Result<()> {
            unreachable!()
        }
        fn PresignFile(
            &self,
            _: &StoreContext,
            _: &str,
            _: std::time::Duration,
        ) -> anyhow::Result<String> {
            unreachable!()
        }
        fn Close(&self) {}
    }
    let sink = crate::ObjectStoreWriterSink::new(
        Arc::new(SlowStorage),
        Context::with_timeout(std::time::Duration::from_millis(20)),
    );
    let error = astersql_ingestor_simplesst::writer::WriterSink::write_file(&sink, "x", b"data")
        .unwrap_err();
    assert!(error.contains("operation cancelled"), "{error}");
}

#[test]
fn async_encode_operator_keeps_first_error_when_two_workers_fail() {
    use astersql_dxf_operator::compose::{DataChannel, NewSimpleDataChannel, WithSource};
    use astersql_dxf_operator::operator::Operator;
    use astersql_resourcemanager_pool_workerpool::{Channel, Context as PoolContext};
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct FailingWorker {
        id: usize,
        both_running: Arc<Barrier>,
        context: PoolContext,
        second_failed: Arc<AtomicBool>,
    }
    impl crate::EncodeSortWorker for FailingWorker {
        fn HandleTask(&mut self, _: crate::EncodeSortTask) -> Result<(), String> {
            self.both_running.wait();
            if self.id == 0 {
                return Err("first error".into());
            }
            for _ in 0..300 {
                if self.context.OperatorErr().is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            self.second_failed.store(true, Ordering::SeqCst);
            Err("second error".into())
        }
        fn Close(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
    let context = PoolContext::background();
    let count = Arc::new(AtomicUsize::new(0));
    let both_running = Arc::new(Barrier::new(2));
    let second_failed = Arc::new(AtomicBool::new(false));
    let factory: crate::EncodeSortWorkerFactory = Arc::new({
        let context = context.clone();
        let count = count.clone();
        let both_running = both_running.clone();
        let second_failed = second_failed.clone();
        move || {
            Ok(Box::new(FailingWorker {
                id: count.fetch_add(1, Ordering::SeqCst),
                both_running: both_running.clone(),
                context: context.clone(),
                second_failed: second_failed.clone(),
            }))
        }
    });
    let mut op = crate::newEncodeAndSortOperator(context.clone(), 1, 2, "", 2, factory);
    let source = NewSimpleDataChannel(Channel::bounded(0));
    op.SetSource(source.clone());
    op.Open().unwrap();
    for _ in 0..2 {
        assert!(source.Channel().send(crate::EncodeSortTask {
            Plan: Plan::default(),
            Chunk: Default::default()
        }));
    }
    op.Close().unwrap();
    assert_eq!(context.OperatorErr().unwrap().to_string(), "first error");
    assert!(second_failed.load(Ordering::SeqCst));
}

#[test]
fn configured_worker_fails_before_opening_store_without_table_meta() {
    use astersql_dxf_operator::compose::{NewSimpleDataChannel, WithSource};
    use astersql_dxf_operator::operator::Operator;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_resourcemanager_pool_workerpool::{Channel, Context as PoolContext};
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: Arc::new(NewMemStorage::default()),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let factory = crate::NewConfiguredEncodeSortFactory(
        runtime,
        PoolContext::background(),
        crate::TaskMeta::default(),
        1,
        3,
        1024,
        1024,
        512,
        512,
        Arc::new(crate::GlobalWriterSummaries::default()),
    );
    let context = PoolContext::background();
    let mut op = crate::newEncodeAndSortOperator(context.clone(), 1, 3, "", 1, factory);
    op.SetSource(NewSimpleDataChannel(Channel::bounded(0)));
    let error = op.Open().unwrap_err();
    assert!(error.to_string().contains("table info"));
    op.Close().unwrap();
}

#[test]
fn configured_local_worker_requires_preopened_shared_engines() {
    use astersql_dxf_operator::compose::{NewSimpleDataChannel, WithSource};
    use astersql_dxf_operator::operator::Operator;
    use astersql_meta_model::TableInfo;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_resourcemanager_pool_workerpool::{Channel, Context as PoolContext};
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: Arc::new(NewMemStorage::default()),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let mut meta = crate::TaskMeta::default();
    meta.Plan.TableInfo = Some(Arc::new(TableInfo::default()));
    let factory = crate::NewConfiguredEncodeSortFactory(
        runtime,
        PoolContext::background(),
        meta,
        1,
        3,
        1024,
        1024,
        512,
        512,
        Arc::new(crate::GlobalWriterSummaries::default()),
    );
    let context = PoolContext::background();
    let mut op = crate::newEncodeAndSortOperator(context, 1, 3, "", 1, factory);
    op.SetSource(NewSimpleDataChannel(Channel::bounded(0)));
    assert!(op.Open().unwrap_err().to_string().contains("local engines"));
    op.Close().unwrap();
}

#[test]
fn encode_sort_subtask_runner_propagates_initialization_error() {
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_resourcemanager_pool_workerpool::Context as PoolContext;
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: Arc::new(NewMemStorage::default()),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let mut step = crate::ImportStepMeta::default();
    let error = crate::RunEncodeSortChunks(
        PoolContext::background(),
        crate::TaskMeta::default(),
        &mut step,
        runtime,
        1,
        3,
        1,
        1024,
        1024,
        512,
        512,
    )
    .unwrap_err();
    assert!(error.to_string().contains("table info"));
}

#[test]
/// 对象前缀格式与默认 Plan 下 data/index writer 内存份额应对齐 Go。
fn object_prefix_and_writer_budget_match_go_shares() {
    assert_eq!(subtaskPrefix(10, 20), "10/20");
    let resource = StepResource {
        CPU: NewAllocatable(2),
        Mem: NewAllocatable(2 * 1024 * 1024 * 1024),
    };
    let (data, index) = getWriterMemorySizeLimit(&resource, &Plan::default());
    assert_eq!(data, 512 * 1024 * 1024);
    assert_eq!(index, 512 * 1024 * 1024 / 3);
}

#[test]
fn writer_budget_covers_all_go_index_group_counts() {
    use astersql_meta_model::{IndexInfo, StatePublic, TableInfo};
    let gib = 1024_u64 * 1024 * 1024;
    let resource = StepResource {
        CPU: NewAllocatable(1),
        Mem: NewAllocatable((2 * gib) as i64),
    };
    for (count, unique_count, expected_data, expected_index) in [
        (0, 0, gib, 0),
        (1, 1, 768 * 1024 * 1024, 256 * 1024 * 1024),
        (1, 0, 768 * 1024 * 1024, 256 * 1024 * 1024),
        (2, 0, 644245094, 214748364),
        (4, 2, 460175067, 153391689),
        (5, 3, 402653184, 134217728),
    ] {
        let indices: Vec<IndexInfo> = (0..count)
            .map(|id| IndexInfo {
                ID: id as i64 + 1,
                State: StatePublic,
                Unique: id < unique_count,
                ..Default::default()
            })
            .collect();
        let table = TableInfo {
            Indices: indices,
            ..Default::default()
        };
        assert_eq!(
            astersql_executor_importer::GetNumOfIndexGenKV(&table),
            count
        );
        assert_eq!(
            astersql_executor_importer::GetIndicesGenKV(&table)
                .values()
                .filter(|i| i.Unique)
                .count(),
            unique_count
        );
        let plan = Plan {
            DesiredTableInfo: Some(Arc::new(table)),
            ThreadCnt: 1,
            ..Default::default()
        };
        let (data, index) = getWriterMemorySizeLimit(&resource, &plan);
        assert_eq!(data, expected_data, "index groups: {count}");
        if count > 0 {
            assert_eq!(index, expected_index, "index groups: {count}");
        }
        assert!(data + index * count as u64 <= gib);
    }
}
