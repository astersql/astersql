// Copyright 2026 AsterSQL.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use serial_test::serial;

use crate::hdfs::HDFSStorage;
use crate::{objectio, storeapi};

#[test]
#[serial]
fn write_file_error_includes_combined_output_and_exit_status_like_go() {
    let hadoop_home = tempfile::tempdir().unwrap();
    let bin_dir = hadoop_home.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let hdfs = bin_dir.join("hdfs");
    fs::write(
        &hdfs,
        "#!/bin/sh\nprintf 'stdout detail\\n'\nprintf 'stderr detail\\n' >&2\nexit 7\n",
    )
    .unwrap();
    fs::set_permissions(&hdfs, fs::Permissions::from_mode(0o755)).unwrap();

    unsafe { std::env::set_var("HADOOP_HOME", hadoop_home.path()) };
    unsafe { std::env::remove_var("HADOOP_LINUX_USER") };

    let storage = HDFSStorage::new("hdfs://namenode/backup");
    let error = storeapi::Storage::WriteFile(
        &storage,
        &objectio::Context::default(),
        "object",
        b"payload",
    )
    .unwrap_err()
    .to_string();

    unsafe { std::env::remove_var("HADOOP_HOME") };

    assert!(error.contains("stdout detail"), "{error}");
    assert!(error.contains("stderr detail"), "{error}");
    assert!(error.contains("exit status: 7"), "{error}");
}

#[test]
#[serial]
fn write_and_exists_ignore_cancelled_context_like_go() {
    let hadoop_home = tempfile::tempdir().unwrap();
    let bin_dir = hadoop_home.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let hdfs = bin_dir.join("hdfs");
    fs::write(&hdfs, "#!/bin/sh\ncat >/dev/null\nexit 0\n").unwrap();
    fs::set_permissions(&hdfs, fs::Permissions::from_mode(0o755)).unwrap();

    unsafe { std::env::set_var("HADOOP_HOME", hadoop_home.path()) };
    unsafe { std::env::remove_var("HADOOP_LINUX_USER") };

    let storage = HDFSStorage::new("hdfs://namenode/backup");
    let object_context = objectio::Context::default();
    object_context.cancel();
    storeapi::Storage::WriteFile(&storage, &object_context, "object", b"payload").unwrap();
    assert!(storeapi::Storage::FileExists(&storage, &object_context, "object").unwrap());

    let storage_context = crate::storage::Context::default();
    storage_context.cancel();
    crate::storage::Storage::WriteFile(&storage, &storage_context, "object", b"payload").unwrap();
    assert!(crate::storage::Storage::FileExists(&storage, &storage_context, "object").unwrap());

    unsafe { std::env::remove_var("HADOOP_HOME") };
}
