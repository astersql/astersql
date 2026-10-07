// Copyright 2026 AsterSQL.

use super::modify_column_cloud_store::CloudStore;
use astersql_ingestor_globalsort::{OnDuplicateKey, Storage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn native_cloud_store_streams_simplesst_objects_through_real_local_files() {
    let directory = Directory::new();
    let store = CloudStore::open(
        directory.path().to_str().unwrap(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let mut builder = astersql_ingestor_simplesst::writer::WriterBuilder::new();
    builder.set_memory_size_limit(75);
    let mut writer = builder.build_with_sink(store.clone(), "read", "1");
    for key in [6_u8, 1, 4, 2, 5, 3] {
        writer.write_row(&[key], &[key; 8]).unwrap();
    }
    let summary = writer.close().unwrap();
    assert_eq!(summary.TotalCnt, 6);
    // simplesst deliberately prepends random partition directories. Consume
    // the actual durable writer summary, as the Go cloud planner does.
    let inputs = summary
        .MultipleFilesStats
        .iter()
        .flat_map(|group| group.Filenames.iter().map(|files| files[0].clone()))
        .collect::<Vec<_>>();
    assert_eq!(inputs.len(), 2);
    let files = summary
        .MultipleFilesStats
        .iter()
        .map(|group| astersql_ingestor_globalsort::MultipleFilesStat {
            filenames: group
                .Filenames
                .iter()
                .map(|files| astersql_ingestor_globalsort::FilePair {
                    data_file: files[0].clone(),
                    stat_file: files[1].clone(),
                    properties: vec![],
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut splitter = astersql_ingestor_globalsort::split::NewRangeSplitter(
        &files,
        store.as_ref(),
        i64::MAX,
        i64::MAX,
        i64::MAX,
        i64::MAX,
        i64::MAX,
        i64::MAX,
    )
    .unwrap();
    let range = splitter.SplitOneRangesGroup().unwrap();
    assert!(range.end_key_of_group.is_empty());
    assert_eq!(range.data_files, inputs);
    splitter.Close().unwrap();
    let output = astersql_ingestor_globalsort::merge::merge_overlapping_files_internal(
        &Default::default(),
        &inputs,
        store.as_ref(),
        "merged",
        "1",
        0,
        None,
        None,
        false,
        OnDuplicateKey::Error,
        1,
        &Mutex::new(Default::default()),
    )
    .unwrap();
    let expected = (1_u8..=6)
        .flat_map(|key| astersql_ingestor_simplesst::file::encode_kv(&[key], &[key; 8]).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(store.read(&output).unwrap(), expected);
    // The native transport seeks to a record boundary; it must not skip the
    // prefix a second time after opening the underlying object range.
    let mut range = store.open_at(&output, 75).unwrap();
    let mut tail = Vec::new();
    std::io::Read::read_to_end(&mut range, &mut tail).unwrap();
    assert_eq!(tail, expected[75..]);
    let stats = astersql_ingestor_simplesst::codec::decode_multi_props(
        &store.read("merged/1.stat").unwrap(),
    )
    .unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].FirstKey, vec![1]);
    assert_eq!(stats[0].LastKey, vec![6]);
    assert_eq!(stats[0].Size, 150);
    assert_eq!(stats[0].Keys, 6);
    assert_eq!(stats[0].Offset, 0);
    let paths = store.list_prefix("").unwrap();
    assert!(paths.contains(&output));
    store.delete_files(&paths).unwrap();
    assert!(store.list_prefix("").unwrap().is_empty());
}

#[test]
fn native_cloud_store_cancellation_reaches_object_upload() {
    let directory = Directory::new();
    let cancelled = Arc::new(AtomicBool::new(false));
    let store = CloudStore::open(directory.path().to_str().unwrap(), cancelled.clone()).unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(store.write("cancelled", vec![1]).is_err());
    assert!(store.open("cancelled").is_err());
    assert!(!directory.path().join("cancelled").exists());
}

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("task6-cloud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
