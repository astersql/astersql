// Copyright 2026 AsterSQL.

use crate::backfilling_dist_executor::{BackfillSubTaskMeta, ExternalMetaStorage, MetaError};
use crate::backfilling_merge_sort::{
    MergeBackendError, MergeSortBackend, MergeSortError, MergeSortExecutor,
};
use crate::backfilling_read_index::SortedKvMeta;

#[derive(Default)]
struct BackendStub {
    merge_concurrency: Option<usize>,
    worker_pool_size: usize,
    tuned: Vec<(usize, bool)>,
}

impl MergeSortBackend for BackendStub {
    fn merge_overlapping_files(
        &mut self,
        _files: &[String],
        _part_size: u64,
        _output_prefix: &str,
        concurrency: usize,
    ) -> Result<Vec<SortedKvMeta>, MergeBackendError> {
        self.merge_concurrency = Some(concurrency);
        Ok(vec![SortedKvMeta {
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
            file_count: 1,
            total_kv_size: 10,
        }])
    }

    fn tune_worker_pool_size(&mut self, concurrency: usize, wait: bool) {
        self.tuned.push((concurrency, wait));
        self.worker_pool_size = concurrency;
    }

    fn worker_pool_size(&self) -> usize {
        self.worker_pool_size
    }
}

struct FailingStorage;

impl ExternalMetaStorage for FailingStorage {
    fn read(&self, _path: &str) -> Result<Vec<u8>, String> {
        unreachable!("merge-sort completion only writes metadata")
    }

    fn write(&mut self, _path: &str, _value: &[u8]) -> Result<(), String> {
        Err("write failed".to_owned())
    }
}

fn executor() -> MergeSortExecutor<BackendStub> {
    let mut executor = MergeSortExecutor::new(
        7,
        11,
        Vec::new(),
        "s3://bucket".to_owned(),
        BackendStub::default(),
    );
    executor.init();
    executor
}

#[test]
fn zero_concurrency_is_forwarded_like_go() {
    let mut executor = executor();
    let mut meta = BackfillSubTaskMeta::default();

    executor
        .run_subtask(13, &mut meta, 0, 0, None)
        .expect("merge succeeds");
    assert_eq!(executor.backend.merge_concurrency, Some(0));

    executor.running = true;
    executor.backend.worker_pool_size = 1;
    executor
        .resource_modified(0)
        .expect("running merge accepts resource update");
    assert_eq!(executor.backend.tuned, vec![(0, true)]);
}

#[test]
fn failed_external_meta_write_drops_finished_subtask_cache_like_go() {
    let mut executor = executor();
    let mut meta = BackfillSubTaskMeta::default();
    let mut storage = FailingStorage;

    assert_eq!(
        executor.run_subtask(13, &mut meta, 0, 1, Some(&mut storage)),
        Err(MergeSortError::ExternalMeta(MetaError::External(
            "write failed".to_owned()
        )))
    );
    assert_eq!(executor.subtask_sorted_kv_meta, None);
}
