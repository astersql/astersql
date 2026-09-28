// Copyright 2026 AsterSQL.

use std::cell::RefCell;

use crate::proto::{Checksum, PostProcessStepMeta};
use crate::subtask_executor::runPostProcessWith;

#[test]
fn post_process_step_decodes_go_wire_and_calls_host_in_order() {
    use crate::subtask_executor::{
        NewPostProcessStepExecutor, PostProcessChecksumManager, PostProcessHost,
    };
    use astersql_executor_importer::{Plan, PostOpLevel, RemoteChecksum};
    use astersql_meta_autoid::AllocatorType;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct Manager(Arc<Mutex<Vec<&'static str>>>);
    impl PostProcessChecksumManager for Manager {
        fn Checksum(
            &self,
            _: &astersql_dxf_framework_taskexecutor_execute::Context,
        ) -> Result<RemoteChecksum, String> {
            self.0.lock().unwrap().push("nextgen checksum");
            Ok(RemoteChecksum {
                Checksum: 7,
                TotalKVs: 2,
                TotalBytes: 13,
                ..Default::default()
            })
        }
        fn Close(&self) {
            self.0.lock().unwrap().push("close");
        }
    }
    struct Host(Arc<Mutex<Vec<&'static str>>>);
    impl PostProcessHost for Host {
        fn RebaseAllocatorBases(
            &self,
            _: &astersql_dxf_framework_taskexecutor_execute::Context,
            max_ids: &HashMap<AllocatorType, i64>,
            _: &Plan,
        ) -> Result<(), String> {
            assert_eq!(max_ids.get(&AllocatorType::RowId), Some(&11));
            self.0.lock().unwrap().push("rebase");
            Ok(())
        }
        fn RemoteChecksumClassic(
            &self,
            _: &astersql_dxf_framework_taskexecutor_execute::Context,
            _: &Plan,
        ) -> Result<RemoteChecksum, String> {
            self.0.lock().unwrap().push("classic checksum");
            Ok(RemoteChecksum {
                Checksum: 7,
                TotalKVs: 2,
                TotalBytes: 13,
                ..Default::default()
            })
        }
        fn NewNextGenChecksumManager(
            &self,
            _: &astersql_dxf_framework_taskexecutor_execute::Context,
            task_id: i64,
            _: &Plan,
        ) -> Result<Box<dyn PostProcessChecksumManager>, String> {
            assert_eq!(task_id, 9);
            self.0.lock().unwrap().push("create manager");
            Ok(Box::new(Manager(self.0.clone())))
        }
    }

    let host = Arc::new(Host(Arc::new(Mutex::new(Vec::new()))));
    let mut plan = Plan::default();
    plan.Checksum = PostOpLevel::Required;
    let mut executor = NewPostProcessStepExecutor(9, plan, host.clone());
    let mut subtask = astersql_dxf_framework_proto::subtask::NewSubtask(
        astersql_dxf_framework_proto::ImportStepPostProcess,
        9,
        astersql_dxf_framework_proto::ImportInto,
        String::new(),
        1,
        br#"{"Checksum":{"1":{"Sum":7,"KVs":2,"Size":13}},"DeletedRowsChecksum":{"Sum":0,"KVs":0,"Size":0},"MaxIDs":{"0":11}}"#.to_vec(),
        0,
    );
    astersql_dxf_framework_taskexecutor_execute::StepExecutor::RunSubtask(
        &mut executor,
        astersql_dxf_framework_taskexecutor_execute::Context::default(),
        &mut subtask,
    )
    .unwrap();
    if astersql_config_kerneltype::IsNextGen() {
        assert_eq!(
            *host.0.lock().unwrap(),
            ["rebase", "create manager", "nextgen checksum", "close"]
        );
    } else {
        assert_eq!(*host.0.lock().unwrap(), ["rebase", "classic checksum"]);
    }

    host.0.lock().unwrap().clear();
    executor
        .RunMeta(br#"{"Checksum":{},"MaxIDs":{"0":11},"too-many-conflicts-from-index":true}"#)
        .unwrap();
    assert_eq!(*host.0.lock().unwrap(), ["rebase"]);

    host.0.lock().unwrap().clear();
    let error = executor
        .RunMeta(br#"{"Checksum":{},"MaxIDs":{"0":11}}"#)
        .unwrap_err();
    assert!(error.contains("checksum mismatched remote vs local"));
    if astersql_config_kerneltype::IsNextGen() {
        assert_eq!(
            *host.0.lock().unwrap(),
            ["rebase", "create manager", "nextgen checksum", "close"]
        );
    }
}

#[test]
fn minimal_task_chunk_carries_selected_parquet_location_to_parser_boundary() {
    use astersql_executor_importer::ImportChunk;
    let chunk = astersql_executor_importer::Chunk::default();
    let located = crate::subtask_executor::LocatedImportChunk {
        chunk: &chunk,
        location: "Asia/Shanghai",
    };
    assert_eq!(located.ParquetLocation(), Some("Asia/Shanghai"));
    assert_eq!(located.Key(), chunk.GetKey());
}

#[test]
fn post_process_rebases_before_checksum_and_skips_only_after_rebase() {
    let calls = RefCell::new(Vec::new());
    let mut meta = PostProcessStepMeta::default();
    meta.Checksum.insert(
        1,
        Checksum {
            Sum: 7,
            KVs: 2,
            Size: 13,
        },
    );
    meta.TooManyConflictsFromIndex = true;

    runPostProcessWith(
        &meta,
        |_| {
            calls.borrow_mut().push("rebase");
            Ok(())
        },
        |_| {
            calls.borrow_mut().push("verify");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*calls.borrow(), ["rebase"]);

    meta.TooManyConflictsFromIndex = false;
    runPostProcessWith(
        &meta,
        |_| {
            calls.borrow_mut().push("rebase");
            Ok(())
        },
        |checksum| {
            calls.borrow_mut().push("verify");
            assert_eq!(checksum.Sum(), 7);
            assert_eq!(checksum.SumKVS(), 2);
            assert_eq!(checksum.SumSize(), 13);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*calls.borrow(), ["rebase", "rebase", "verify"]);
}

#[test]
fn post_process_preserves_rebase_and_checksum_errors() {
    let meta = PostProcessStepMeta::default();
    let error = runPostProcessWith(
        &meta,
        |_| Err("rebase failed".into()),
        |_| panic!("verify after rebase failure"),
    )
    .unwrap_err();
    assert_eq!(error, "rebase failed");
    let error = runPostProcessWith(&meta, |_| Ok(()), |_| Err("remote checksum failed".into()))
        .unwrap_err();
    assert_eq!(error, "remote checksum failed");
}
