// Copyright 2026 AsterSQL.

use crate::reader::CancellationToken;
use crate::testutil::{mockOneMultiFileStat, testReadAndCompare};
use crate::{Error, MemoryStorage};

#[test]
fn test_read_and_compare_rejects_empty_expected_kvs() {
    let error = testReadAndCompare(
        &CancellationToken::default(),
        &[],
        &MemoryStorage::default(),
        &["/test/data".to_owned()],
        &["/test/stat".to_owned()],
        Vec::new(),
        1024,
    )
    .expect_err("Go helper requires at least one expected KV");

    assert_eq!(
        Error::InvalidArgument("expected KVs must not be empty".into()),
        error
    );
}

#[test]
fn test_mock_one_multi_file_stat_pairs_files_by_index() {
    let data = vec!["a.data".to_owned(), "b.data".to_owned()];
    let stat = vec!["a.stat".to_owned(), "b.stat".to_owned()];

    let groups = mockOneMultiFileStat(&data, &stat).expect("matching file counts should work");

    assert_eq!(1, groups.len());
    assert_eq!(2, groups[0].filenames.len());
    assert_eq!("a.data", groups[0].filenames[0].data_file);
    assert_eq!("a.stat", groups[0].filenames[0].stat_file);
    assert!(groups[0].filenames[0].properties.is_empty());
    assert_eq!("b.data", groups[0].filenames[1].data_file);
    assert_eq!("b.stat", groups[0].filenames[1].stat_file);
}

#[test]
fn test_mock_one_multi_file_stat_rejects_mismatched_counts() {
    let error = mockOneMultiFileStat(&["a.data".to_owned()], &[])
        .expect_err("mismatched file counts must not be silently truncated");

    assert_eq!(
        Error::InvalidArgument("data and stat file counts differ".into()),
        error
    );
}
