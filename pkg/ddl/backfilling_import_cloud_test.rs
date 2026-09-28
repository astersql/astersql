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
    fn close_external_engine(
        &mut self,
        _config: &ExternalEngineConfig,
        _engine_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    fn has_external_engine(&self, _engine_id: &str) -> bool {
        true
    }

    fn import_engine(&mut self, _engine_id: &str) -> Result<(), BackendImportError> {
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
