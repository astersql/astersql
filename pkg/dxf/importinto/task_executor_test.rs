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

// Import Into 任务执行器的可执行 Go 对照回归。

#[test]
fn zero_writer_budget_keeps_go_default_block_size() {
    assert_eq!(
        crate::task_executor::getAdjustedBlockSize(0, 16 * 1024 * 1024),
        16 * 1024 * 1024
    );
}

#[test]
fn duplicate_key_error_is_normalized_to_go_executor_code() {
    let duplicate = astersql_lightning_common::ErrFoundDuplicateKeys(&[0x80, 0x81], &[0x01]);
    let error = crate::task_executor::normalizeSubtaskErr(anyhow::Error::new(duplicate));
    assert!(error.to_string().contains("[executor:8167]"));
    assert!(!error.to_string().contains("found duplicate key"));
    let wrapped = astersql_lightning_common::CommonError::new("wrapped", "outer").wrap(
        astersql_lightning_common::CommonError::new("wrapped", "middle").wrap(
            astersql_lightning_common::ErrFoundDuplicateKeys(&[0x80], &[0x01]),
        ),
    );
    let wrapped = crate::task_executor::normalizeSubtaskErr(anyhow::Error::new(wrapped));
    assert!(wrapped.to_string().contains("[executor:8167]"));
    let go_error = astersql_errors::Normalize(
        "found duplicate key '%s', value '%s'",
        &[astersql_errors::RFCCodeText(
            "Lightning:Restore:ErrFoundDuplicateKey",
        )],
    )
    .FastGenByArgs(&["\\x80\\x81".into(), "\\x01".into()]);
    let traced = astersql_errors::Trace(Some(go_error)).unwrap();
    let normalized = crate::task_executor::normalizeSubtaskErr(anyhow::Error::new(traced));
    assert!(normalized.to_string().contains("[executor:8167]"));
    let sort_duplicate = astersql_ingestor_globalsort::Error::DuplicateKey {
        key: vec![0x80],
        value: vec![0x01],
    };
    let normalized = crate::task_executor::normalizeSubtaskErr(anyhow::Error::new(sort_duplicate));
    assert!(normalized.to_string().contains("[executor:8167]"));
    let other = crate::task_executor::normalizeSubtaskErr(anyhow::anyhow!("other error"));
    assert_eq!(other.to_string(), "other error");
}

#[test]
fn import_step_dispatch_uses_bound_runtime_store_for_all_host_stages() {
    use astersql_dxf_framework_proto::{step, subtask::StepResource};
    use astersql_dxf_framework_taskexecutor_execute as execute;
    use std::sync::{Arc, Mutex};

    struct Step;
    impl execute::StepExecFrameworkInfo for Step {
        fn restricted(&self) {}
        fn GetStep(&self) -> step::Step {
            0
        }
        fn GetResource(&self) -> Option<Arc<StepResource>> {
            None
        }
        fn SetResource(&self, _: Arc<StepResource>) {}
        fn GetMeterRecorder(&self) -> Option<Arc<astersql_dxf_framework_metering::Recorder>> {
            None
        }
        fn GetCheckpointUpdateFunc(&self) -> Option<execute::CheckpointUpdateFunc> {
            None
        }
        fn GetCheckpointFunc(&self) -> Option<execute::CheckpointGetFunc> {
            None
        }
    }
    impl execute::StepExecutor for Step {
        fn Init(&mut self, _: execute::Context) -> anyhow::Result<()> {
            Ok(())
        }
        fn RunSubtask(
            &mut self,
            _: execute::Context,
            _: &mut astersql_dxf_framework_proto::subtask::Subtask,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn RealtimeSummary(&mut self) -> Option<&execute::SubtaskSummary> {
            None
        }
        fn ResetSummary(&mut self) {}
        fn Cleanup(&mut self, _: execute::Context) -> anyhow::Result<()> {
            Ok(())
        }
        fn TaskMetaModified(&mut self, _: execute::Context, _: Vec<u8>) -> anyhow::Result<()> {
            Ok(())
        }
        fn ResourceModified(
            &mut self,
            _: execute::Context,
            _: &StepResource,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn SetFrameworkInfo(&mut self, _: execute::FrameworkInfo) {}
    }
    struct Host {
        store: Arc<astersql_store_mockstore_mockstorage::MockStorage>,
        calls: Mutex<Vec<step::Step>>,
    }
    struct Backend;
    impl crate::task_executor::WriteIngestBackend for Backend {
        fn SetCollector(&self, _: Arc<dyn execute::Collector + Send + Sync>) {}
        fn CloseExternalEngine(
            &self,
            _: &crate::task_executor::WriteIngestRequest,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn ImportEngine(&self, _: i64, _: i64, _: i64) -> anyhow::Result<()> {
            Ok(())
        }
        fn GetExternalEngineConflictInfo(
            &self,
            _: i64,
        ) -> astersql_ingestor_engineapi::ConflictInfo {
            Default::default()
        }
        fn CleanupEngine(&self, _: i64) -> anyhow::Result<()> {
            Ok(())
        }
        fn Close(&self) {}
    }
    impl crate::task_executor::ImportStepHost for Host {
        fn TaskStore(&self) -> Arc<dyn astersql_kv::Storage + Send + Sync> {
            self.store.clone()
        }
        fn NewWriteIngestBackend(
            &self,
            _: &astersql_dxf_framework_proto::task::Task,
            _: &crate::TaskMeta,
            store: Arc<dyn astersql_kv::Storage + Send + Sync>,
        ) -> Result<Arc<dyn crate::task_executor::WriteIngestBackend>, astersql_errors::SharedError>
        {
            assert!(Arc::ptr_eq(
                &store,
                &(self.store.clone() as Arc<dyn astersql_kv::Storage + Send + Sync>)
            ));
            assert_eq!(store.GetKeyspace(), "task_ks");
            Ok(Arc::new(Backend))
        }
        fn NewStepExecutor(
            &self,
            stage: step::Step,
            _: &astersql_dxf_framework_proto::task::Task,
            _: &crate::TaskMeta,
            store: Arc<dyn astersql_kv::Storage + Send + Sync>,
        ) -> Result<Box<dyn execute::StepExecutor>, astersql_errors::SharedError> {
            assert!(Arc::ptr_eq(
                &store,
                &(self.store.clone() as Arc<dyn astersql_kv::Storage + Send + Sync>)
            ));
            assert_eq!(store.GetKeyspace(), "task_ks");
            self.calls.lock().unwrap().push(stage);
            Ok(Box::new(Step))
        }
    }
    let kv_store = astersql_store_mockstore_mockstorage::KVStore::New(Arc::new(
        astersql_store_mockstore_mockstorage::MemoryPdClient::default(),
    ));
    let host = Arc::new(Host {
        store: astersql_store_mockstore_mockstorage::NewMockStorage(
            kv_store,
            Some(astersql_store_mockstore_mockstorage::KeyspaceMeta {
                Name: "task_ks".into(),
                Id: 1,
            }),
        )
        .unwrap(),
        calls: Mutex::new(Vec::new()),
    });
    let object_store = Arc::new(astersql_objstore::azblob::MemoryStorage::default());
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: object_store.clone(),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let meta = crate::TaskMeta::default().Marshal().unwrap();
    let stages = [
        step::ImportStepImport,
        step::ImportStepEncodeAndSort,
        step::ImportStepMergeSort,
        step::ImportStepWriteAndIngest,
        step::ImportStepPostProcess,
        step::ImportStepCollectConflicts,
        step::ImportStepConflictResolution,
    ];
    for stage in stages {
        let mut task = crate::task_executor::NodeFrameworkTask(2, 1, meta.clone(), stage);
        task.TaskBase.Keyspace = "another_ks".into();
        assert!(
            crate::task_executor::GetImportStepExecutor(&task, runtime.clone(), host.as_ref())
                .is_ok()
        );
    }
    assert_eq!(
        *host.calls.lock().unwrap(),
        stages
            .into_iter()
            .filter(|stage| *stage != step::ImportStepEncodeAndSort
                && *stage != step::ImportStepMergeSort
                && *stage != step::ImportStepWriteAndIngest)
            .collect::<Vec<_>>()
    );
    let unknown = crate::task_executor::NodeFrameworkTask(2, 1, meta, 0);
    assert!(
        crate::task_executor::GetImportStepExecutor(&unknown, runtime.clone(), host.as_ref())
            .is_err()
    );
    let invalid = crate::task_executor::NodeFrameworkTask(2, 1, Vec::new(), step::ImportStepImport);
    assert!(
        crate::task_executor::GetImportStepExecutor(&invalid, runtime.clone(), host.as_ref())
            .is_err()
    );
    struct Retry;
    impl astersql_dxf_framework_taskexecutor::Extension for Retry {
        fn IsIdempotent(&self, _: &astersql_dxf_framework_taskexecutor::Subtask) -> bool {
            false
        }
        fn GetStepExecutor(
            &self,
            _: &astersql_dxf_framework_taskexecutor::Task,
        ) -> astersql_dxf_framework_taskexecutor::Result<
            Arc<dyn astersql_dxf_framework_taskexecutor::StepExecutor>,
        > {
            Err(astersql_dxf_framework_taskexecutor::ExecutorError(
                "unreachable".into(),
            ))
        }
        fn IsRetryableError(&self, _: &astersql_dxf_framework_taskexecutor::ExecutorError) -> bool {
            false
        }
    }
    let extension = crate::task_executor::ImportTaskExtension {
        Runtime: runtime,
        Host: host,
        NodeResource: astersql_dxf_framework_taskexecutor::NodeResource {
            TotalCPU: 1,
            TotalMem: 4 * 1024 * 1024,
            TotalDisk: 0,
        },
        RetryPolicy: Arc::new(Retry),
    };
    let node_task = astersql_dxf_framework_taskexecutor::Task {
        TaskBase: astersql_dxf_framework_taskexecutor::TaskBase {
            ID: 2,
            Type: "ImportInto".into(),
            Step: step::ImportStepMergeSort,
            RequiredSlots: 1,
            ..Default::default()
        },
        Meta: crate::TaskMeta::default().Marshal().unwrap(),
    };
    let node_step =
        astersql_dxf_framework_taskexecutor::Extension::GetStepExecutor(&extension, &node_task)
            .unwrap();
    assert!(
        astersql_dxf_framework_taskexecutor::Extension::IsIdempotent(
            &extension,
            &astersql_dxf_framework_taskexecutor::Subtask::default()
        )
    );
    let mut node_meta = crate::MergeSortStepMeta::default();
    node_meta.KVGroup = "data".into();
    let mut node_subtask = astersql_dxf_framework_taskexecutor::Subtask {
        SubtaskBase: astersql_dxf_framework_taskexecutor::SubtaskBase {
            ID: 9,
            TaskID: 2,
            Step: step::ImportStepMergeSort,
            ExecID: "node".into(),
            ..Default::default()
        },
        Meta: node_meta.Marshal().unwrap(),
    };
    let context = astersql_dxf_framework_taskexecutor::Context::Background();
    node_step.Init(&context).unwrap();
    node_step.RunSubtask(&context, &mut node_subtask).unwrap();
    let inline: serde_json::Value = serde_json::from_slice(&node_subtask.Meta).unwrap();
    assert_eq!(inline["ExternalPath"], "2/9/meta.json");
    node_step.Cleanup(&context).unwrap();
    use astersql_objstore_storeapi::{Context as ObjectContext, Storage};
    assert!(
        object_store
            .FileExists(&ObjectContext::default(), "2/9/meta.json")
            .unwrap()
    );
}

#[test]
fn merge_sort_step_persists_go_external_meta_even_without_input_files() {
    use astersql_dxf_framework_proto::{step, subtask};
    use astersql_dxf_framework_taskexecutor_execute as execute;
    use astersql_objstore_storeapi::{Context as ObjectContext, Storage};
    use std::sync::Arc;

    let store = Arc::new(astersql_objstore::azblob::MemoryStorage::default());
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: store.clone(),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let task_meta = crate::TaskMeta::default();
    let task = crate::task_executor::NodeFrameworkTask(
        2,
        1,
        task_meta.Marshal().unwrap(),
        step::ImportStepMergeSort,
    );
    let mut executor = crate::task_executor::NewMergeSortStepExecutor(2, task_meta, runtime);
    execute::SetFrameworkInfo(
        Some(&mut executor),
        &task,
        Arc::new(subtask::StepResource {
            CPU: subtask::NewAllocatable(1),
            Mem: subtask::NewAllocatable(4 * 1024 * 1024),
        }),
        None,
        None,
    );
    let mut meta = crate::MergeSortStepMeta::default();
    meta.KVGroup = crate::proto::DATA_KV_GROUP.into();
    let mut subtask = subtask::NewSubtask(
        step::ImportStepMergeSort,
        2,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        meta.Marshal().unwrap(),
        1,
    );
    subtask.SubtaskBase.ID = 3;
    execute::StepExecutor::Init(&mut executor, execute::Context::new()).unwrap();
    execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut subtask)
        .unwrap();
    let inline: serde_json::Value = serde_json::from_slice(&subtask.Meta).unwrap();
    assert_eq!(inline["ExternalPath"], "2/3/meta.json");
    let external = store
        .ReadFile(&ObjectContext::default(), "2/3/meta.json")
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&external).unwrap();
    assert_eq!(value["kv-group"], "data");
    assert_eq!(value["data-files"], serde_json::json!([]));
    let adapter = crate::task_executor::MergeStoreAdapter(store.clone());
    let keys = (0..8).map(|n| vec![n]).collect::<Vec<_>>();
    let values = (0..8).map(|n| vec![n + 10]).collect::<Vec<_>>();
    let (data_files, _) =
        astersql_ingestor_globalsort::MockExternalEngine(&adapter, &keys, &values).unwrap();
    let mut nonempty = crate::MergeSortStepMeta::default();
    nonempty.KVGroup = "data".into();
    nonempty.DataFiles = data_files;
    let mut second = subtask::NewSubtask(
        step::ImportStepMergeSort,
        2,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        nonempty.Marshal().unwrap(),
        1,
    );
    second.SubtaskBase.ID = 4;
    execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut second).unwrap();
    let external = store
        .ReadFile(&ObjectContext::default(), "2/4/meta.json")
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&external).unwrap();
    assert_eq!(value["total-kv-cnt"], 8);
    assert_ne!(value["multiple-files-stats"], serde_json::json!([]));
    let mut invalid = subtask::NewSubtask(
        step::ImportStepMergeSort,
        2,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        br#"{"kv-group":"data","data-files":42}"#.to_vec(),
        1,
    );
    invalid.SubtaskBase.ID = 5;
    assert!(
        execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut invalid)
            .is_err()
    );
    execute::StepExecutor::Cleanup(&mut executor, execute::Context::new()).unwrap();
}

#[test]
fn write_ingest_step_orders_backend_calls_and_rewrites_meta_only_for_conflicts() {
    use astersql_dxf_framework_proto::{step, subtask};
    use astersql_dxf_framework_taskexecutor_execute as execute;
    use astersql_objstore_storeapi::{Context as ObjectContext, Storage};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    struct Backend {
        events: Arc<Mutex<Vec<&'static str>>>,
        count: AtomicU64,
    }
    impl crate::task_executor::WriteIngestBackend for Backend {
        fn SetCollector(&self, collector: Arc<dyn execute::Collector + Send + Sync>) {
            self.events.lock().unwrap().push("collector");
            collector.Processed(10, 2);
        }
        fn CloseExternalEngine(
            &self,
            request: &crate::task_executor::WriteIngestRequest,
        ) -> anyhow::Result<()> {
            assert_eq!(request.TS, 42);
            assert_eq!(request.KVGroup, "data");
            assert_eq!(request.JobKeys, vec![vec![1]]);
            assert_eq!(
                request.OnDup,
                astersql_ingestor_engineapi::OnDuplicateKeyError
            );
            self.events.lock().unwrap().push("close-engine");
            Ok(())
        }
        fn ImportEngine(&self, id: i64, size: i64, keys: i64) -> anyhow::Result<()> {
            assert!(id > 0);
            assert_eq!(size, 96 * 1024 * 1024);
            assert_eq!(keys, 960_000);
            self.events.lock().unwrap().push("import");
            Ok(())
        }
        fn GetExternalEngineConflictInfo(
            &self,
            _: i64,
        ) -> astersql_ingestor_engineapi::ConflictInfo {
            self.events.lock().unwrap().push("conflict");
            astersql_ingestor_engineapi::ConflictInfo {
                Count: self.count.load(Ordering::Relaxed),
                Files: vec![],
            }
        }
        fn CleanupEngine(&self, _: i64) -> anyhow::Result<()> {
            self.events.lock().unwrap().push("cleanup");
            Err(anyhow::anyhow!("cleanup warning"))
        }
        fn Close(&self) {
            self.events.lock().unwrap().push("close-backend");
        }
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let backend = Arc::new(Backend {
        events: events.clone(),
        count: AtomicU64::new(0),
    });
    let store = Arc::new(astersql_objstore::azblob::MemoryStorage::default());
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: store.clone(),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let task_meta = crate::TaskMeta::default();
    let task = crate::task_executor::NodeFrameworkTask(
        7,
        1,
        task_meta.Marshal().unwrap(),
        step::ImportStepWriteAndIngest,
    );
    let mut executor =
        crate::task_executor::NewWriteAndIngestStepExecutor(7, task_meta, runtime, backend.clone());
    execute::SetFrameworkInfo(
        Some(&mut executor),
        &task,
        Arc::new(subtask::StepResource {
            CPU: subtask::NewAllocatable(1),
            Mem: subtask::NewAllocatable(8 * 1024 * 1024),
        }),
        None,
        None,
    );
    let mut meta = crate::WriteIngestStepMeta::default();
    meta.KVGroup = "data".into();
    meta.TS = 42;
    meta.RangeSplitKeys = vec![vec![1]];
    let mut wire: serde_json::Value = serde_json::from_slice(&meta.Marshal().unwrap()).unwrap();
    wire["range-job-keys"] = serde_json::Value::Null;
    let bytes = serde_json::to_vec(&wire).unwrap();
    let mut first = subtask::NewSubtask(
        step::ImportStepWriteAndIngest,
        7,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        bytes.clone(),
        1,
    );
    first.SubtaskBase.ID = 1;
    execute::StepExecutor::Init(&mut executor, execute::Context::new()).unwrap();
    execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut first).unwrap();
    assert_eq!(first.Meta, bytes);
    assert!(
        !store
            .FileExists(&ObjectContext::default(), "7/1/meta.json")
            .unwrap()
    );
    backend.count.store(2, Ordering::Relaxed);
    let mut second = subtask::NewSubtask(
        step::ImportStepWriteAndIngest,
        7,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        bytes,
        1,
    );
    second.SubtaskBase.ID = 2;
    execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut second).unwrap();
    let inline: serde_json::Value = serde_json::from_slice(&second.Meta).unwrap();
    assert_eq!(inline["ExternalPath"], "7/2/meta.json");
    assert_eq!(inline["recorded-conflict-kv-count"], 2);
    assert!(
        store
            .FileExists(&ObjectContext::default(), "7/2/meta.json")
            .unwrap()
    );
    let summary = execute::StepExecutor::RealtimeSummary(&mut executor).unwrap();
    assert_eq!(summary.Processed.load(Ordering::Relaxed), 20);
    assert_eq!(summary.RowCnt.load(Ordering::Relaxed), 4);
    let before = events.lock().unwrap().len();
    let mut invalid = subtask::NewSubtask(
        step::ImportStepWriteAndIngest,
        7,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        br#"{"kv-group":"data","range-split-keys":["invalid!"]}"#.to_vec(),
        1,
    );
    invalid.SubtaskBase.ID = 3;
    assert!(
        execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut invalid)
            .is_err()
    );
    assert_eq!(events.lock().unwrap().len(), before);
    execute::StepExecutor::Cleanup(&mut executor, execute::Context::new()).unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "collector",
            "close-engine",
            "import",
            "conflict",
            "cleanup",
            "collector",
            "close-engine",
            "import",
            "conflict",
            "cleanup",
            "close-backend"
        ]
    );
}

#[test]
fn encode_sort_step_dispatch_runs_and_persists_external_subtask_meta() {
    use astersql_dxf_framework_proto::{step, subtask, task::Task};
    use astersql_dxf_framework_taskexecutor_execute as execute;
    use astersql_objstore::azblob::MemoryStorage as NewMemStorage;
    use astersql_objstore_storeapi::{
        Context as ObjectContext, ReaderOption, Storage, WalkOption, WriterOption,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingWorker(Arc<AtomicUsize>);
    impl crate::EncodeSortWorker for CountingWorker {
        fn HandleTask(&mut self, task: crate::EncodeSortTask) -> Result<(), String> {
            if task.Chunk.Path != "source.csv" {
                return Err("unexpected chunk".into());
            }
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn Close(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
    struct ClosingStore {
        inner: Arc<NewMemStorage>,
        closes: Arc<AtomicUsize>,
    }
    impl Storage for ClosingStore {
        fn WriteFile(&self, ctx: &ObjectContext, name: &str, data: &[u8]) -> anyhow::Result<()> {
            self.inner.WriteFile(ctx, name, data)
        }
        fn ReadFile(&self, ctx: &ObjectContext, name: &str) -> anyhow::Result<Vec<u8>> {
            self.inner.ReadFile(ctx, name)
        }
        fn FileExists(&self, ctx: &ObjectContext, name: &str) -> anyhow::Result<bool> {
            self.inner.FileExists(ctx, name)
        }
        fn DeleteFile(&self, ctx: &ObjectContext, name: &str) -> anyhow::Result<()> {
            self.inner.DeleteFile(ctx, name)
        }
        fn Open(
            &self,
            ctx: &ObjectContext,
            path: &str,
            opt: Option<&ReaderOption>,
        ) -> anyhow::Result<Box<dyn astersql_objstore::objectio::Reader>> {
            self.inner.Open(ctx, path, opt)
        }
        fn DeleteFiles(&self, ctx: &ObjectContext, names: &[String]) -> anyhow::Result<()> {
            self.inner.DeleteFiles(ctx, names)
        }
        fn WalkDir(
            &self,
            ctx: &ObjectContext,
            opt: Option<&WalkOption>,
            callback: &mut dyn FnMut(&str, i64) -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            self.inner.WalkDir(ctx, opt, callback)
        }
        fn URI(&self) -> String {
            self.inner.URI()
        }
        fn Create(
            &self,
            ctx: &ObjectContext,
            path: &str,
            opt: Option<&WriterOption>,
        ) -> anyhow::Result<Box<dyn astersql_objstore::objectio::Writer>> {
            self.inner.Create(ctx, path, opt)
        }
        fn Rename(&self, ctx: &ObjectContext, old: &str, new: &str) -> anyhow::Result<()> {
            self.inner.Rename(ctx, old, new)
        }
        fn PresignFile(
            &self,
            ctx: &ObjectContext,
            path: &str,
            expire: std::time::Duration,
        ) -> anyhow::Result<String> {
            self.inner.PresignFile(ctx, path, expire)
        }
        fn Close(&self) {
            self.closes.fetch_add(1, Ordering::Relaxed);
        }
    }

    let count = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(NewMemStorage::default());
    let closes = Arc::new(AtomicUsize::new(0));
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: store.clone(),
        ObjectStoreFactory: Some(Arc::new({
            let store = store.clone();
            let closes = closes.clone();
            move || {
                Ok(Arc::new(ClosingStore {
                    inner: store.clone(),
                    closes: closes.clone(),
                }))
            }
        })),
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: Some(Arc::new({
            let count = count.clone();
            move || Ok(Box::new(CountingWorker(count.clone())))
        })),
    });
    let mut task_meta = crate::TaskMeta::default();
    task_meta.Plan.CloudStorageURI = "s3://bucket/import".into();
    let task = crate::task_executor::NodeFrameworkTask(
        11,
        1,
        task_meta.Marshal().unwrap(),
        step::ImportStepEncodeAndSort,
    );
    let mut executor = crate::GetEncodeSortStepExecutor(&task, runtime.clone()).unwrap();
    execute::SetFrameworkInfo(
        Some(executor.as_mut()),
        &task,
        Arc::new(subtask::StepResource {
            CPU: subtask::NewAllocatable(1),
            Mem: subtask::NewAllocatable(4 * 1024 * 1024),
        }),
        None,
        None,
    );
    let mut meta = crate::ImportStepMeta::default();
    meta.Chunks.push(astersql_executor_importer::Chunk {
        Path: "source.csv".into(),
        ..Default::default()
    });
    store
        .WriteFile(
            &ObjectContext::default(),
            "input/meta.json",
            &meta.Marshal().unwrap(),
        )
        .unwrap();
    meta.BaseExternalMeta.ExternalPath = "input/meta.json".into();
    let mut subtask = subtask::NewSubtask(
        step::ImportStepEncodeAndSort,
        11,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        meta.Marshal().unwrap(),
        1,
    );
    executor.Init(execute::Context::new()).unwrap();
    executor
        .RunSubtask(execute::Context::new(), &mut subtask)
        .unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    let inline = crate::ImportStepMeta::Unmarshal(&subtask.Meta).unwrap();
    assert_eq!(inline.BaseExternalMeta.ExternalPath, "11/0/meta.json");
    let bytes = store
        .ReadFile(&ObjectContext::default(), "11/0/meta.json")
        .unwrap();
    let external = crate::ImportStepMeta::Unmarshal(&bytes).unwrap();
    assert_eq!(external.Chunks[0].Path, "source.csv");
    executor.Cleanup(execute::Context::new()).unwrap();
    assert_eq!(closes.load(Ordering::Relaxed), 1);

    struct OtherSteps;
    impl astersql_dxf_framework_taskexecutor::Extension for OtherSteps {
        fn IsIdempotent(&self, _: &astersql_dxf_framework_taskexecutor::Subtask) -> bool {
            false
        }
        fn GetStepExecutor(
            &self,
            _: &astersql_dxf_framework_taskexecutor::Task,
        ) -> astersql_dxf_framework_taskexecutor::Result<
            Arc<dyn astersql_dxf_framework_taskexecutor::StepExecutor>,
        > {
            Ok(Arc::new(
                astersql_dxf_framework_taskexecutor::BaseStepExecutor,
            ))
        }
        fn IsRetryableError(&self, _: &astersql_dxf_framework_taskexecutor::ExecutorError) -> bool {
            false
        }
    }
    let extension = crate::ImportEncodeExtension {
        Runtime: runtime,
        NodeResource: astersql_dxf_framework_taskexecutor::NodeResource {
            TotalCPU: 1,
            TotalMem: 4 * 1024 * 1024,
            TotalDisk: 0,
        },
        OtherSteps: Arc::new(OtherSteps),
    };
    assert!(
        astersql_dxf_framework_taskexecutor::Extension::IsIdempotent(
            &extension,
            &astersql_dxf_framework_taskexecutor::Subtask::default()
        )
    );
    let node_task = astersql_dxf_framework_taskexecutor::Task {
        TaskBase: astersql_dxf_framework_taskexecutor::TaskBase {
            ID: 11,
            Type: "ImportInto".into(),
            Step: step::ImportStepEncodeAndSort as i64,
            RequiredSlots: 1,
            ..Default::default()
        },
        Meta: task.Meta.clone(),
    };
    let node_step =
        astersql_dxf_framework_taskexecutor::Extension::GetStepExecutor(&extension, &node_task)
            .unwrap();
    let mut node_subtask = astersql_dxf_framework_taskexecutor::Subtask {
        SubtaskBase: astersql_dxf_framework_taskexecutor::SubtaskBase {
            ID: 9,
            TaskID: 11,
            Step: step::ImportStepEncodeAndSort as i64,
            ExecID: "node".into(),
            ..Default::default()
        },
        Meta: meta.Marshal().unwrap(),
    };
    let node_context = astersql_dxf_framework_taskexecutor::Context::Background();
    node_step.Init(&node_context).unwrap();
    node_step
        .RunSubtask(&node_context, &mut node_subtask)
        .unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 2);
    let node_inline = crate::ImportStepMeta::Unmarshal(&node_subtask.Meta).unwrap();
    assert_eq!(node_inline.BaseExternalMeta.ExternalPath, "11/9/meta.json");
    let node_external = store
        .ReadFile(&ObjectContext::default(), "11/9/meta.json")
        .unwrap();
    assert_eq!(
        crate::ImportStepMeta::Unmarshal(&node_external)
            .unwrap()
            .Chunks[0]
            .Path,
        "source.csv"
    );
    node_step.Cleanup(&node_context).unwrap();
    assert_eq!(closes.load(Ordering::Relaxed), 2);
}

use crate::{getOnDupForConflictedKV, getOnDupForIndex, getOnDupForKVGroup};
use astersql_executor_importer::{GenKVIndex, OnDupKeyModeCapture, OnDupKeyModeError};
use astersql_ingestor_engineapi::{
    OnDuplicateKeyError, OnDuplicateKeyRecord, OnDuplicateKeyRemove,
};
use std::collections::HashMap;

/// Capture 模式下：data/唯一索引记冲突，非唯一索引 Remove；非法组名报错。
#[test]
fn duplicate_key_policy_distinguishes_data_unique_and_non_unique_groups() {
    assert_eq!(
        getOnDupForConflictedKV(OnDupKeyModeCapture),
        OnDuplicateKeyRecord
    );
    assert_eq!(
        getOnDupForConflictedKV(OnDupKeyModeError),
        OnDuplicateKeyError
    );
    let indices = HashMap::from([
        (
            1,
            GenKVIndex {
                name: "u".to_string(),
                Unique: true,
            },
        ),
        (
            2,
            GenKVIndex {
                name: "i".to_string(),
                Unique: false,
            },
        ),
    ]);
    assert_eq!(
        getOnDupForIndex(&indices, 1, OnDupKeyModeCapture).unwrap(),
        OnDuplicateKeyRecord
    );
    assert_eq!(
        getOnDupForIndex(&indices, 2, OnDupKeyModeCapture).unwrap(),
        OnDuplicateKeyRemove
    );
    assert_eq!(
        getOnDupForKVGroup(&indices, "data", OnDupKeyModeCapture).unwrap(),
        OnDuplicateKeyRecord
    );
    assert!(getOnDupForKVGroup(&indices, "missing", OnDupKeyModeCapture).is_err());
}

#[test]
fn parquet_concurrency_estimate_receives_largest_file_exact_size_and_keeps_fallbacks() {
    use crate::task_executor::parquet_reader_concurrency;
    use astersql_executor_importer::Chunk;
    use astersql_lightning_mydump::SourceType;
    let chunks = [
        Chunk {
            Path: "small.parquet".into(),
            FileSize: 64,
            Type: SourceType::Parquet,
            ..Default::default()
        },
        Chunk {
            Path: "large.parquet".into(),
            FileSize: 256,
            Type: SourceType::Parquet,
            ..Default::default()
        },
    ];
    assert_eq!(
        parquet_reader_concurrency(&chunks, 8, 1000, |path, size| {
            assert_eq!(path, "large.parquet");
            assert_eq!(size, 256);
            Ok(100)
        }),
        3
    );
    assert_eq!(
        parquet_reader_concurrency(&chunks, 8, 1000, |_, _| Err("cannot read source".into())),
        8
    );
    assert_eq!(
        parquet_reader_concurrency(&chunks, 8, 1000, |_, _| Ok(0)),
        8
    );
    assert_eq!(
        parquet_reader_concurrency(&chunks, 8, 10, |_, _| Ok(100)),
        1
    );
    assert_eq!(
        parquet_reader_concurrency(&[], 8, 1000, |_, _| panic!("no parquet")),
        8
    );
    assert_eq!(
        parquet_reader_concurrency(
            &[Chunk {
                Type: SourceType::Csv,
                ..Default::default()
            }],
            8,
            1000,
            |_, _| panic!("no parquet")
        ),
        8
    );
}
