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
