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

// Import Into 任务执行器 Go testkit 场景的可执行后处理与重试回归。

use crate::Checksum;

#[test]
fn import_step_run_subtask_cleans_real_local_engines_before_retry() {
    use crate::task_executor::{EncodeSortImporterHost, NewEncodeSortStepExecutor};
    use astersql_dxf_framework_proto::{step, subtask};
    use astersql_dxf_framework_taskexecutor_execute as execute;
    use astersql_executor_importer::{LocalEngineCleanup, Plan};
    use astersql_lightning_backend::{
        Backend, ClosedEngine, EngineManager, MakeEngineManager, OpenedEngine,
    };
    use astersql_lightning_backend_encode::Context as EncodeContext;
    use std::sync::{Arc, Mutex};

    struct Host {
        backend: Arc<dyn Backend>,
        manager: EngineManager,
        cleanup: LocalEngineCleanup,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }
    impl EncodeSortImporterHost for Host {
        fn EstimateParquetReaderMemory(&self, _: &str) -> Result<i64, String> {
            Ok(0)
        }
        fn OpenDataEngine(&self, context: &EncodeContext, id: i32) -> Result<OpenedEngine, String> {
            let engine = self
                .manager
                .OpenEngine(context, &Default::default(), "test.t", id)
                .map_err(|error| error.to_string())?;
            self.cleanup.Record(id);
            self.calls.lock().unwrap().push("open data");
            Ok(engine)
        }
        fn OpenIndexEngine(
            &self,
            context: &EncodeContext,
            id: i32,
        ) -> Result<OpenedEngine, String> {
            let engine = self
                .manager
                .OpenEngine(context, &Default::default(), "test.t", id)
                .map_err(|error| error.to_string())?;
            self.cleanup.Record(id);
            self.calls.lock().unwrap().push("open index");
            Ok(engine)
        }
        fn ImportAndCleanup(&self, _: &EncodeContext, _: &ClosedEngine) -> Result<i64, String> {
            panic!("sort failure must occur before engine import")
        }
        fn CleanupAllLocalEngines(&self, context: &EncodeContext) {
            self.cleanup.CleanupAll(context);
            self.calls.lock().unwrap().push("cleanup all");
        }
        fn Close(&mut self) {
            self.backend.Close();
            self.calls.lock().unwrap().push("close");
        }
    }
    struct FailSortChunk;
    impl crate::EncodeSortWorker for FailSortChunk {
        fn HandleTask(&mut self, task: crate::EncodeSortTask) -> Result<(), String> {
            let csv =
                std::fs::read_to_string(&task.Chunk.Path).map_err(|error| error.to_string())?;
            if csv != "1,1\n" {
                return Err("unexpected CSV fixture".into());
            }
            Err("occur an error when sort chunk".into())
        }
        fn Close(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    let csv_dir =
        std::env::temp_dir().join(format!("astersql-import-retry-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&csv_dir).unwrap();
    let csv_path = csv_dir.join("data.csv");
    std::fs::write(&csv_path, b"1,1\n").unwrap();
    let (domain, _session) =
        astersql_session::runtime::CreateAnalyzeSession().expect("concrete import session");
    let backend = astersql_session::runtime::NewImportLocalBackend(domain, 4102)
        .expect("concrete local import backend");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let host = Host {
        manager: MakeEngineManager(backend.clone()),
        cleanup: LocalEngineCleanup::new(backend.clone(), "test.t".into()),
        backend,
        calls: calls.clone(),
    };
    let runtime = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: Arc::new(astersql_objstore::azblob::MemoryStorage::default()),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: Some(Arc::new(|| Ok(Box::new(FailSortChunk)))),
    });
    let task_meta = crate::TaskMeta {
        Plan: Plan {
            ThreadCnt: 1,
            InImportInto: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(task_meta.Plan.IsLocalSort());
    let task = crate::task_executor::NodeFrameworkTask(
        1,
        1,
        task_meta.Marshal().unwrap(),
        step::ImportStepEncodeAndSort,
    );
    let mut executor = NewEncodeSortStepExecutor(1, task_meta, runtime);
    executor.SetImporterHost(Box::new(host));
    execute::SetFrameworkInfo(
        Some(&mut executor),
        &task,
        Arc::new(subtask::StepResource {
            CPU: subtask::NewAllocatable(4),
            Mem: subtask::NewAllocatable(8 << 30),
        }),
        None,
        None,
    );
    execute::StepExecutor::Init(&mut executor, execute::Context::new()).unwrap();
    let mut meta = crate::ImportStepMeta::default();
    meta.ID = 1;
    meta.Chunks.push(astersql_executor_importer::Chunk {
        Path: csv_path.to_string_lossy().into_owned(),
        ..Default::default()
    });
    let mut subtask = subtask::NewSubtask(
        step::ImportStepEncodeAndSort,
        1,
        astersql_dxf_framework_proto::r#type::ImportInto,
        "node".into(),
        1,
        meta.Marshal().unwrap(),
        1,
    );
    for _ in 0..2 {
        let error =
            execute::StepExecutor::RunSubtask(&mut executor, execute::Context::new(), &mut subtask)
                .unwrap_err();
        assert!(error.to_string().contains("occur an error when sort chunk"));
        assert!(!error.to_string().contains("already exists"));
    }
    execute::StepExecutor::Cleanup(&mut executor, execute::Context::new()).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "open data",
            "open index",
            "cleanup all",
            "open data",
            "open index",
            "cleanup all",
            "close",
        ]
    );
    std::fs::remove_dir_all(csv_dir).unwrap();
}

#[test]
fn post_process_required_optional_off_match_go_checksum_levels() {
    use crate::subtask_executor::{
        NewPostProcessStepExecutor, PostProcessChecksumManager, PostProcessHost,
    };
    use astersql_dxf_framework_taskexecutor_execute::Context;
    use astersql_executor_importer::{Plan, PostOpLevel, RemoteChecksum};
    use astersql_meta_autoid::AllocatorType;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Host(Arc<Mutex<Vec<&'static str>>>);
    struct Manager(Arc<Mutex<Vec<&'static str>>>);
    fn remote() -> RemoteChecksum {
        RemoteChecksum {
            Checksum: 7,
            TotalKVs: 2,
            TotalBytes: 13,
            ..Default::default()
        }
    }
    impl PostProcessChecksumManager for Manager {
        fn Checksum(&self, _: &Context) -> Result<RemoteChecksum, String> {
            self.0.lock().unwrap().push("checksum");
            Ok(remote())
        }
        fn Close(&self) {
            self.0.lock().unwrap().push("close");
        }
    }
    impl PostProcessHost for Host {
        fn RebaseAllocatorBases(
            &self,
            _: &Context,
            _: &HashMap<AllocatorType, i64>,
            _: &Plan,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push("rebase");
            Ok(())
        }
        fn RemoteChecksumClassic(&self, _: &Context, _: &Plan) -> Result<RemoteChecksum, String> {
            self.0.lock().unwrap().push("checksum");
            Ok(remote())
        }
        fn NewNextGenChecksumManager(
            &self,
            _: &Context,
            _: i64,
            _: &Plan,
        ) -> Result<Box<dyn PostProcessChecksumManager>, String> {
            Ok(Box::new(Manager(self.0.clone())))
        }
    }

    let host = Arc::new(Host::default());
    let matched = br#"{"Checksum":{"-1":{"Sum":7,"KVs":2,"Size":13}}}"#;
    let mismatched = br#"{"Checksum":{"-1":{"Sum":8,"KVs":2,"Size":13}}}"#;
    for (level, input, should_fail) in [
        (PostOpLevel::Required, matched.as_slice(), false),
        (PostOpLevel::Required, mismatched.as_slice(), true),
        (PostOpLevel::Optional, mismatched.as_slice(), false),
        (PostOpLevel::Off, mismatched.as_slice(), false),
    ] {
        host.0.lock().unwrap().clear();
        let mut plan = Plan::default();
        plan.Checksum = level;
        let result = NewPostProcessStepExecutor(1, plan, host.clone()).RunMeta(input);
        if should_fail {
            assert!(
                result
                    .unwrap_err()
                    .contains("checksum mismatched remote vs local")
            );
        } else {
            result.unwrap();
        }
        let calls = host.0.lock().unwrap().clone();
        assert_eq!(calls.first(), Some(&"rebase"));
        if level == PostOpLevel::Off {
            if astersql_config_kerneltype::IsNextGen() {
                assert_eq!(calls, ["rebase", "close"]);
            } else {
                assert_eq!(calls, ["rebase"]);
            }
        } else {
            assert!(calls.contains(&"checksum"));
        }
    }
}

/// 后处理元数据中的 checksum 线格式应能无损转为运行时 KVChecksum。
#[test]
fn post_process_checksum_wire_values_round_trip() {
    let checksum = Checksum {
        Sum: 0x1234,
        KVs: 9,
        Size: 128,
    };
    let kv = checksum.ToKVChecksum();
    assert_eq!(kv.Sum(), checksum.Sum);
    assert_eq!(kv.SumKVS(), checksum.KVs);
    assert_eq!(kv.SumSize(), checksum.Size);
}
