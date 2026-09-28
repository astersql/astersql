// Copyright 2026 AsterSQL.

use super::*;
use astersql_lightning_mydump::{Compression, SourceFileMeta};
use astersql_objstore_storeapi::{Context, Storage};
use std::io::Write;
use std::sync::Arc;

struct ParquetBoundary;
impl ImportSizeEstimator for ParquetBoundary {
    fn EstimateRealSize(
        &self,
        _: &Context,
        _: &SourceFileMeta,
        _: &dyn Storage,
    ) -> Result<i64, String> {
        unreachable!()
    }
    fn ParquetExpansionRatio(
        &self,
        _: &Context,
        _: &str,
        _: i64,
        _: &dyn Storage,
    ) -> Result<f64, String> {
        Ok(2.0)
    }
}

#[test]
fn compressed_file_estimator_samples_real_gzip_twice_and_observes_cancel() {
    let store = astersql_objstore::azblob::MemoryStorage::default();
    let raw = vec![b'x'; 10_000];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw).unwrap();
    let gzip = encoder.finish().unwrap();
    let context = Context::default();
    store.WriteFile(&context, "data.csv.gz", &gzip).unwrap();
    let file = SourceFileMeta {
        path: "data.csv.gz".into(),
        compression: Compression::Gz,
        file_size: gzip.len() as i64,
        ..Default::default()
    };
    let estimator = HostImportSizeEstimator {
        ParquetEstimator: Arc::new(ParquetBoundary),
    };
    assert_eq!(
        estimator.EstimateRealSize(&context, &file, &store).unwrap(),
        10_000
    );
    assert_eq!(
        estimator
            .ParquetExpansionRatio(&context, "p.parquet", 1, &store)
            .unwrap(),
        2.0
    );
    context.cancel();
    assert!(estimator.EstimateRealSize(&context, &file, &store).is_err());
}
