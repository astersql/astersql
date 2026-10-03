// Copyright 2026 AsterSQL.

use crate::task_executor::{WriteIngestBackend, WriteIngestRequest};
use crate::write_ingest_backend::*;
use execute::Collector;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Rpc {
    ingests: AtomicUsize,
    writes: AtomicUsize,
    rows: Mutex<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>,
    failure: &'static str,
}
impl RegionImportTransport for Rpc {
    fn Scan(
        &self,
        _: &local::CancellationToken,
        range: &local::KeyRange,
    ) -> local::Result<Vec<local::region_job::LocatedRegion>> {
        Ok(vec![local::region_job::LocatedRegion {
            region: local::job_worker::RegionInfo {
                id: 1,
                leader_store_id: 1,
                peer_store_ids: vec![1],
            },
            key_range: range.clone(),
        }])
    }
    fn Write(
        &self,
        token: &local::CancellationToken,
        job: &RegionJob,
        data: &dyn api::IngestData,
    ) -> local::Result<TikvWriteResult> {
        token.check()?;
        let mut pool = local::local::membuf::NewPool(Vec::new());
        let mut iter = data.NewIter(
            &api::Context::background(),
            &job.key_range.start,
            &job.key_range.end,
            Arc::get_mut(&mut pool).unwrap(),
        );
        let mut bytes = 0;
        let mut count = 0;
        let mut valid = iter.First();
        while valid {
            let key = iter.Key().to_vec();
            let value = iter.Value().to_vec();
            bytes += (key.len() + value.len()) as i64;
            count += 1;
            self.rows.lock().unwrap().insert(key, value);
            valid = iter.Next();
        }
        if let Some(error) = iter.Error() {
            return Err(local::Error::InvalidData(error.to_string()));
        }
        iter.Close()
            .map_err(|e| local::Error::InvalidData(e.to_string()))?;
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(TikvWriteResult {
            total_bytes: bytes,
            count,
            ..Default::default()
        })
    }
    fn Ingest(&self, _: &local::CancellationToken, _: &RegionJob) -> local::Result<()> {
        if self.ingests.fetch_add(1, Ordering::SeqCst) == 0 && !self.failure.is_empty() {
            return Err(local::Error::Retryable(self.failure.into()));
        }
        Ok(())
    }
    fn Close(&self) {}
}

#[test]
fn physical_region_rewrite_increments_progress_without_recounting_logical_import() {
    for (failure, expected_writes) in [("", 1), ("KVIngestFailed", 2), ("ServerIsBusy", 1)] {
        let store = Arc::new(astersql_objstore::azblob::MemoryStorage::default());
        use astersql_objstore_storeapi::Storage;
        let pairs = (1u8..=3)
            .map(|key| global::KvPair {
                key: vec![key],
                value: vec![key + 10],
            })
            .collect::<Vec<_>>();
        store
            .WriteFile(
                &Default::default(),
                "data",
                &pairs
                    .iter()
                    .flat_map(|pair| {
                        astersql_ingestor_simplesst::file::encode_kv(&pair.key, &pair.value)
                            .unwrap()
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        store.WriteFile(&Default::default(), "stat", &[]).unwrap();
        let rpc = Arc::new(Rpc {
            ingests: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            rows: Default::default(),
            failure,
        });
        let backend = GlobalSortWriteIngestBackend::new(store, rpc.clone(), 1);
        let summary = Arc::new(execute::SubtaskSummary::default());
        backend.SetCollector(Arc::new(crate::task_executor::ingestCollector {
            summary: summary.clone(),
            kvGroup: "data".into(),
            meterRec: Default::default(),
        }));
        let request = WriteIngestRequest {
            SubtaskID: 1,
            KVGroup: "data".into(),
            TS: 42,
            DataFiles: vec!["data".into()],
            StatFiles: vec!["stat".into()],
            StartKey: vec![1],
            EndKey: vec![4],
            JobKeys: vec![vec![1], vec![4]],
            SplitKeys: vec![vec![1], vec![4]],
            TotalFileSize: 6,
            TotalKVCount: 3,
            MemCapacity: 8 * 1024 * 1024,
            OnDup: api::OnDuplicateKeyError,
            FilePrefix: "test".into(),
        };
        backend.CloseExternalEngine(&request).unwrap();
        backend.ImportEngine(1, 96 * 1024 * 1024, 960_000).unwrap();
        assert_eq!(rpc.rows.lock().unwrap().len(), 3);
        assert_eq!(rpc.writes.load(Ordering::SeqCst), expected_writes);
        let logical = backend.engines.lock().unwrap()[&1].ImportedStatistics();
        assert_eq!(logical, (6, 3));
        assert_eq!(
            summary.Processed.load(Ordering::SeqCst),
            6 * expected_writes as i64
        );
        assert_eq!(
            summary.RowCnt.load(Ordering::SeqCst),
            3 * expected_writes as i64
        );
        backend.CleanupEngine(1).unwrap();
        assert!(backend.engines.lock().unwrap().is_empty());
        backend.Close();
    }
}
