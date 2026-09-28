// Copyright 2026 AsterSQL.

use super::production_regions::LocalRegionStorage;
use astersql_lightning_mydump as mydump;

#[test]
fn local_region_storage_streams_large_csv_split() {
    let directory = std::env::temp_dir().join(format!(
        "astersql-regions-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let csv = b"1,2\n3,4\n5,6\n7,8\n";
    std::fs::write(directory.join("data.csv"), csv).unwrap();
    let table = mydump::MDTableMeta {
        db: "db".into(),
        name: "t".into(),
        data_files: vec![mydump::FileInfo {
            file_meta: mydump::FileMeta {
                path: "data.csv".into(),
                file_size: csv.len() as i64,
                real_size: csv.len() as i64,
                source_type: mydump::SourceType::Csv,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut config = mydump::NewDataDivideConfig();
    config.column_count = 2;
    config.strict_format = true;
    config.region_size = 4;
    let store = LocalRegionStorage {
        parent: directory.clone(),
    };
    let regions = mydump::MakeTableRegions(&table, &config, &store).unwrap();
    assert!(regions.len() > 1);
    assert_eq!(regions.first().unwrap().chunk.offset, 0);
    assert_eq!(regions.last().unwrap().chunk.end_offset, csv.len() as i64);
    std::fs::remove_dir_all(directory).unwrap();
}
