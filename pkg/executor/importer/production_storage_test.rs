// Copyright 2026 AsterSQL.

use super::*;

struct CloudFactory;
impl ImportStorageFactory for CloudFactory {
    fn Open(
        &self,
        _: &astersql_objstore_storeapi::Context,
        uri: &str,
        _: &str,
    ) -> Result<SharedStorage, String> {
        Err(format!("host cloud factory received {uri}"))
    }
}

#[test]
fn server_disk_factory_opens_real_csv_from_parent_directory() {
    let directory = std::env::temp_dir().join(format!(
        "astersql-import-storage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let file = directory.join("data.csv");
    std::fs::write(&file, b"1,2\n").unwrap();
    let context = astersql_objstore_storeapi::Context::default();
    let factory = HostImportStorageFactory {
        CloudFactory: std::sync::Arc::new(CloudFactory),
    };
    let storage = factory
        .Open(&context, file.to_str().unwrap(), "IMPORT INTO data source")
        .unwrap();
    let reader = storage
        .lock()
        .unwrap()
        .Open(&context, "data.csv", None)
        .unwrap();
    assert_eq!(reader.GetFileSize().unwrap(), 4);
    assert_eq!(
        crate::import::storage_path(file.to_str().unwrap()),
        "data.csv"
    );
    assert!(
        factory
            .Open(&context, "s3://bucket/data.csv", "IMPORT INTO data source")
            .err()
            .unwrap()
            .contains("host cloud factory received")
    );
    context.cancel();
    assert!(
        ServerDiskImportStorageFactory
            .Open(&context, file.to_str().unwrap(), "IMPORT INTO data source")
            .is_err()
    );
    drop(reader);
    drop(storage);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn parquet_storage_opens_real_file_with_known_unknown_size_and_cancellation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dumpformat/parquetfile/testfiles/hive_dump.parquet")
        .canonicalize()
        .unwrap();
    let context = astersql_objstore_storeapi::Context::default();
    let storage = ServerDiskImportStorageFactory
        .Open(&context, path.to_str().unwrap(), "IMPORT INTO data source")
        .unwrap();
    let size = std::fs::metadata(&path).unwrap().len() as i64;
    for file_size in [0, size] {
        let file = astersql_lightning_mydump::SourceFileMeta {
            path: "hive_dump.parquet".into(),
            file_size,
            source_type: astersql_lightning_mydump::SourceType::Parquet,
            ..Default::default()
        };
        let mut parser =
            crate::import::open_parquet_file(storage.clone(), &context, &file, "UTC").unwrap();
        assert_eq!(parser.source().whole_file_preloaded(), file_size > 0);
        let count = parser.total_rows();
        assert!(count > 0);
        for _ in 0..count {
            assert!(!parser.read_row().unwrap().is_empty());
        }
        assert_eq!(parser.read_row().unwrap_err().0, "EOF");
    }
    context.cancel();
    let file = astersql_lightning_mydump::SourceFileMeta {
        path: "hive_dump.parquet".into(),
        file_size: size,
        ..Default::default()
    };
    assert!(
        crate::import::open_parquet_file(storage, &context, &file, "UTC")
            .err()
            .unwrap()
            .contains("cancelled")
    );
}
