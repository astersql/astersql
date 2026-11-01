// Copyright 2026 AsterSQL.

use super::backfilling_import_cloud::{
    CloudImportBackend, CloudImportExecutor, ExternalEngineConfig, ImportError, IndexInfo,
    IngestCollector, get_index_info_and_id,
};
use crate::backfilling_import_cloud::BackendImportError;

#[derive(Default)]
struct BackendStub {
    worker_concurrency: usize,
    resource_updates: Vec<(usize, u64)>,
}

impl CloudImportBackend for BackendStub {
    fn external_storage(&self) -> std::sync::Arc<dyn astersql_ingestor_globalsort::Storage> {
        std::sync::Arc::new(astersql_ingestor_globalsort::MemoryStorage::default())
    }
    fn alloc_new_ts(&mut self) -> Result<u64, String> {
        Ok(99)
    }
    fn ingest_external_engine(
        &mut self,
        _: std::sync::Arc<astersql_ingestor_globalsort::engine::ExternalEngineAdapter>,
    ) -> Result<(), BackendImportError> {
        Ok(())
    }

    fn update_engine_resource(&mut self, concurrency: usize, memory: u64) -> Result<(), String> {
        self.resource_updates.push((concurrency, memory));
        Ok(())
    }

    fn set_worker_concurrency(&mut self, concurrency: usize) {
        self.worker_concurrency = concurrency;
    }

    fn worker_concurrency(&self) -> usize {
        self.worker_concurrency
    }

    fn update_write_speed_limit(&mut self, _bytes_per_second: usize) {}

    fn close(&mut self) {}
}

fn index(id: i64) -> IndexInfo {
    IndexInfo {
        id,
        name: format!("idx_{id}"),
        unique: false,
    }
}

#[test]
fn unknown_single_element_id_preserves_go_zero_value_result() {
    let indexes = [index(7)];
    assert_eq!(get_index_info_and_id(&[8], &indexes), Ok((None, 0)));
}

#[test]
fn zero_concurrency_is_forwarded_without_rust_only_clamping() {
    let mut executor = CloudImportExecutor::new(
        1,
        "t".to_owned(),
        vec![index(7)],
        "s3://bucket".to_owned(),
        BackendStub::default(),
    );

    executor.init(0);
    assert_eq!(executor.backend.worker_concurrency, 0);

    executor.engine_running = true;
    executor.backend.worker_concurrency = 1;
    assert_eq!(executor.resource_modified(0, 64), Ok(()));
    assert_eq!(executor.backend.resource_updates, vec![(0, 64)]);
    assert_eq!(executor.backend.worker_concurrency, 0);
}

#[test]
fn processed_bytes_uses_the_same_signed_to_unsigned_conversion_as_go() {
    let mut collector = IngestCollector::default();
    collector.processed(-1);
    assert_eq!(collector.cluster_write_bytes, u64::MAX);
}

#[test]
fn resource_change_without_running_engine_still_requests_retry() {
    let mut executor = CloudImportExecutor::new(
        1,
        "t".to_owned(),
        vec![index(7)],
        "s3://bucket".to_owned(),
        BackendStub::default(),
    );
    assert_eq!(
        executor.resource_modified(2, 64),
        Err(ImportError::EngineNotStarted)
    );
}

#[test]
fn port_373679dd4d_cloud_subtask_owns_engine_and_allocates_only_legacy_ts() {
    use crate::backfilling_dist_executor::BackfillSubTaskMeta;
    use astersql_ingestor_engineapi::{Context, Engine as _};
    use astersql_ingestor_globalsort as global;
    use std::sync::Arc;
    struct Backend {
        store: Arc<global::MemoryStorage>,
        last: Option<Arc<global::engine::ExternalEngineAdapter>>,
        allocations: usize,
        allocation_error: bool,
        import_error: Option<BackendImportError>,
        stub: BackendStub,
    }
    impl CloudImportBackend for Backend {
        fn external_storage(&self) -> Arc<dyn global::Storage> {
            self.store.clone()
        }
        fn alloc_new_ts(&mut self) -> Result<u64, String> {
            self.allocations += 1;
            if self.allocation_error {
                return Err("alloc TS failed".into());
            }
            Ok(99)
        }
        fn ingest_external_engine(
            &mut self,
            engine: Arc<global::engine::ExternalEngineAdapter>,
        ) -> Result<(), BackendImportError> {
            assert_eq!(engine.KVStatistics(), (0, 0));
            assert_eq!(engine.GetKeyRange().unwrap(), (vec![], vec![]));
            assert_eq!(engine.GetRegionSplitKeys().unwrap(), vec![vec![1], vec![2]]);
            assert_eq!(engine.ResourceHandle().WorkerConcurrency(), 2);
            let (send, receive) = std::sync::mpsc::sync_channel(2);
            engine
                .LoadIngestData(&Context::background(), &send)
                .unwrap();
            let data = receive.recv().unwrap();
            assert!(data.Data.GetTS() == 42 || data.Data.GetTS() == 99);
            data.Data.IncRef();
            data.Data.DecRef();
            self.last = Some(engine);
            self.import_error.clone().map_or(Ok(()), Err)
        }
        fn update_engine_resource(&mut self, c: usize, m: u64) -> Result<(), String> {
            self.stub.update_engine_resource(c, m)
        }
        fn set_worker_concurrency(&mut self, c: usize) {
            self.stub.set_worker_concurrency(c)
        }
        fn worker_concurrency(&self) -> usize {
            self.stub.worker_concurrency()
        }
        fn update_write_speed_limit(&mut self, b: usize) {
            self.stub.update_write_speed_limit(b)
        }
        fn close(&mut self) {
            self.stub.close()
        }
    }
    for (timestamp, failure) in [
        (42, None),
        (0, None),
        (42, Some(BackendImportError::Other("ingest failed".into()))),
        (0, Some(BackendImportError::DuplicateKey)),
    ] {
        let mut executor = CloudImportExecutor::new(
            1,
            "t".into(),
            vec![index(7)],
            "memory://".into(),
            Backend {
                store: Default::default(),
                last: None,
                allocations: 0,
                allocation_error: false,
                import_error: failure.clone(),
                stub: Default::default(),
            },
        );
        executor.init(2);
        let meta = BackfillSubTaskMeta {
            element_ids: vec![7],
            ts: timestamp,
            range_split_keys: vec![vec![1], vec![2]],
            ..Default::default()
        };
        let result = executor.run_subtask(&meta, 1024);
        match failure {
            None => assert_eq!(result, Ok(())),
            Some(BackendImportError::Other(e)) => assert_eq!(result, Err(ImportError::Backend(e))),
            Some(BackendImportError::DuplicateKey) => assert_eq!(
                result,
                Err(ImportError::DuplicateKey {
                    index_name: Some("idx_7".into())
                })
            ),
        }
        assert_eq!(executor.backend.allocations, usize::from(timestamp == 0));
        assert!(!executor.engine_running);
        let engine = executor.backend.last.take().unwrap();
        let (out, _) = std::sync::mpsc::sync_channel(1);
        assert!(
            engine
                .LoadIngestData(&Context::background(), &out)
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
        engine.CloseShared().unwrap();
        executor.backend.allocation_error = true;
        let old = BackfillSubTaskMeta { ts: 0, ..meta };
        assert_eq!(
            executor.run_subtask(&old, 1024),
            Err(ImportError::Backend("alloc TS failed".into()))
        );
        assert!(executor.backend.last.is_none());
    }
}
