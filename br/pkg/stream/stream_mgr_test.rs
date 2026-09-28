// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::stream_mgr::{
    BuildObserveDataRanges, BuildObserveMetaRange, EncryptedFileInfo, FastUnmarshalMetaData,
    FastUnmarshalMetaDataWithOptions, MetadataHelper, ObserveDataSource, ObserveTableFilter,
};
use crate::stubs::Storage;
use crate::stubs::backuppb::{CompressionType, DataFileGroup, DataFileInfo, MetaVersion, Metadata};
use crate::stubs::errors::Error;
use crate::stubs::model::{CIStr, DBInfo, PartitionDefinition, PartitionInfo, TableInfo};

struct CountingStorage {
    data: Vec<u8>,
    reads: AtomicUsize,
    delay: Duration,
}

impl Storage for CountingStorage {
    fn ReadFile(&self, _path: &str) -> Result<Vec<u8>, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        thread::sleep(self.delay);
        Ok(self.data.clone())
    }
    fn WriteFile(&self, _path: &str, _data: &[u8]) -> Result<(), String> {
        unreachable!()
    }
    fn ListFiles(&self, _sub_dir: &str) -> Result<Vec<(String, i64)>, String> {
        unreachable!()
    }
    fn DeleteFile(&self, _path: &str) -> Result<(), String> {
        unreachable!()
    }
}

#[test]
fn zstd_decodes_arbitrary_valid_frames() {
    let plain = b"a distinct zstd frame that is not the historical fixture";
    let storage = CountingStorage {
        data: zstd::stream::encode_all(&plain[..], 3).unwrap(),
        reads: AtomicUsize::new(0),
        delay: Duration::ZERO,
    };
    let decoded = MetadataHelper::new()
        .ReadFile(
            "arbitrary.zst",
            0,
            0,
            plain.len() as u64,
            CompressionType::ZSTD,
            &storage,
        )
        .unwrap();
    assert_eq!(decoded, plain);
}

#[test]
fn hard_v1_conversion_preserves_resolved_ts_and_length() {
    let raw = br#"{"MetaVersion":"V1","Files":[{"Path":"data.log","MinTs":11,"MaxTs":19,"ResolvedTs":17,"Length":23}]}"#;
    let metadata = MetadataHelper::ParseToMetadataHard(raw).unwrap();
    assert_eq!(metadata.MetaVersion, MetaVersion::V1);
    assert_eq!(metadata.FileGroups[0].MinResolvedTs, 17);
    assert_eq!(metadata.FileGroups[0].Length, 23);
}

#[test]
fn marshal_v1_flattens_and_clears_groups_in_place() {
    let mut metadata = Metadata {
        MetaVersion: MetaVersion::V1,
        FileGroups: vec![DataFileGroup {
            DataFilesInfo: vec![DataFileInfo {
                Path: "kept.log".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        Files: vec![DataFileInfo::default(), DataFileInfo::default()],
        ..Default::default()
    };
    MetadataHelper::Marshal(&mut metadata).unwrap();
    assert!(metadata.FileGroups.is_empty());
    assert_eq!(metadata.Files.len(), 1);
    assert_eq!(metadata.Files[0].Path, "kept.log");
}

#[test]
fn encrypted_read_checks_ciphertext_before_requiring_manager() {
    let storage = CountingStorage {
        data: b"ciphertext".to_vec(),
        reads: AtomicUsize::new(0),
        delay: Duration::ZERO,
    };
    let info = EncryptedFileInfo {
        EncryptionInfo: astersql_br_pkg_encryption::FileEncryptionInfo {
            Mode: astersql_br_pkg_encryption::FileEncryptionMode::PlainTextDataKey,
            ..Default::default()
        },
        Checksum: Some(vec![0; 32]),
    };
    let err = MetadataHelper::new()
        .ReadFileWithEncryption(
            "encrypted",
            0,
            0,
            0,
            CompressionType::UNKNOWN,
            &storage,
            Some(&info),
        )
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("checksum mismatch before decryption")
    );

    let info_without_checksum = EncryptedFileInfo {
        Checksum: None,
        ..info
    };
    let err = MetadataHelper::new()
        .ReadFileWithEncryption(
            "encrypted",
            0,
            0,
            0,
            CompressionType::UNKNOWN,
            &storage,
            Some(&info_without_checksum),
        )
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "need to decrypt data but encryption manager not set"
    );
}

#[test]
fn observe_meta_range_matches_tablecodec_meta_prefix() {
    let range = BuildObserveMetaRange();
    assert_eq!(range.StartKey, b"m");
    assert_eq!(range.EndKey, b"n");
}

#[test]
fn repeated_cached_slices_share_one_storage_read() {
    let helper = MetadataHelper::new();
    helper.InitCacheEntry("combined", 2);
    let storage = CountingStorage {
        data: b"abcdefgh".to_vec(),
        reads: AtomicUsize::new(0),
        delay: Duration::ZERO,
    };
    assert_eq!(
        helper
            .ReadFile("combined", 0, 4, 4, CompressionType::UNKNOWN, &storage)
            .unwrap(),
        b"abcd"
    );
    assert_eq!(
        helper
            .ReadFile("combined", 4, 4, 4, CompressionType::UNKNOWN, &storage)
            .unwrap(),
        b"efgh"
    );
    assert_eq!(storage.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_slices_of_one_cache_entry_download_once() {
    let helper = Arc::new(MetadataHelper::new());
    helper.InitCacheEntry("combined", 2);
    let storage = Arc::new(CountingStorage {
        data: b"abcdefgh".to_vec(),
        reads: AtomicUsize::new(0),
        delay: Duration::from_millis(50),
    });
    let handles = [(0, b"abcd".to_vec()), (4, b"efgh".to_vec())]
        .into_iter()
        .map(|(offset, expected)| {
            let helper = helper.clone();
            let storage = storage.clone();
            thread::spawn(move || {
                let actual = helper
                    .ReadFile(
                        "combined",
                        offset,
                        4,
                        4,
                        CompressionType::UNKNOWN,
                        storage.as_ref(),
                    )
                    .unwrap();
                assert_eq!(actual, expected);
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(storage.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn fast_unmarshal_ignores_non_metadata_files() {
    let storage = Arc::new(crate::stubs::MemStorage::new());
    storage.WriteFile("v1/backupmeta/a.meta", b"meta").unwrap();
    storage
        .WriteFile("v1/backupmeta/README", b"not metadata")
        .unwrap();
    let mut visited = Vec::new();
    FastUnmarshalMetaData(storage, 0, u64::MAX, |path, _| {
        visited.push(path);
        Ok(())
    })
    .unwrap();
    assert_eq!(visited, ["v1/backupmeta/a.meta"]);
}

#[test]
fn parallel_fast_unmarshal_honors_skip_condition() {
    let storage = Arc::new(crate::stubs::MemStorage::new());
    for path in [
        "v1/backupmeta/a.meta",
        "v1/backupmeta/skip.meta",
        "v1/backupmeta/b.meta",
    ] {
        storage.WriteFile(path, path.as_bytes()).unwrap();
    }
    let visited = std::sync::Mutex::new(Vec::new());
    FastUnmarshalMetaDataWithOptions(
        storage,
        0,
        u64::MAX,
        2,
        |path| path.ends_with("skip.meta"),
        |path, _| {
            visited.lock().unwrap().push(path);
            Ok(())
        },
    )
    .unwrap();
    let mut visited = visited.into_inner().unwrap();
    visited.sort();
    assert_eq!(visited, ["v1/backupmeta/a.meta", "v1/backupmeta/b.meta"]);
}

struct Catalog;

impl ObserveDataSource for Catalog {
    fn ListDatabases(&self, backup_ts: u64) -> Result<Vec<DBInfo>, Error> {
        assert_eq!(backup_ts, 42);
        Ok(vec![
            DBInfo {
                ID: 1,
                Name: CIStr {
                    O: "app".into(),
                    L: "app".into(),
                },
            },
            DBInfo {
                ID: 2,
                Name: CIStr {
                    O: "INFORMATION_SCHEMA".into(),
                    L: "information_schema".into(),
                },
            },
        ])
    }
    fn ListTables(&self, backup_ts: u64, database_id: i64) -> Result<Vec<TableInfo>, Error> {
        assert_eq!(backup_ts, 42);
        assert_eq!(database_id, 1);
        Ok(vec![
            TableInfo {
                ID: 10,
                Name: CIStr {
                    O: "plain".into(),
                    L: "plain".into(),
                },
                ..Default::default()
            },
            TableInfo {
                ID: 20,
                Name: CIStr {
                    O: "partitioned".into(),
                    L: "partitioned".into(),
                },
                Partition: Some(PartitionInfo {
                    Definitions: vec![
                        PartitionDefinition {
                            ID: 21,
                            ..Default::default()
                        },
                        PartitionDefinition {
                            ID: 22,
                            ..Default::default()
                        },
                    ],
                }),
                ..Default::default()
            },
            TableInfo {
                ID: 30,
                Name: CIStr {
                    O: "excluded".into(),
                    L: "excluded".into(),
                },
                ..Default::default()
            },
        ])
    }
}

struct AppFilter;

impl ObserveTableFilter for AppFilter {
    fn MatchSchema(&self, schema: &str) -> bool {
        schema == "app" || schema == "INFORMATION_SCHEMA"
    }
    fn MatchTable(&self, _schema: &str, table: &str) -> bool {
        table != "excluded"
    }
}

#[test]
fn observe_data_ranges_cover_all_and_filtered_partition_paths() {
    let all = BuildObserveDataRanges(&Catalog, &["*.*".into()], &AppFilter, 999).unwrap();
    assert_eq!(
        (all[0].StartKey.as_slice(), all[0].EndKey.as_slice()),
        (&b"t"[..], &b"u"[..])
    );
    let filtered = BuildObserveDataRanges(&Catalog, &["app.*".into()], &AppFilter, 42).unwrap();
    let expected = [10, 21, 22]
        .into_iter()
        .map(crate::stubs::tablecodec::GenTableRecordPrefix)
        .collect::<Vec<_>>();
    assert_eq!(
        filtered
            .into_iter()
            .map(|range| range.StartKey)
            .collect::<Vec<_>>(),
        expected
    );
}
