// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Aster 补充的 objstore 综合单元测试，对齐 Go 行为。
//
// 覆盖 Azure 选项/认证、内存对象读写与范围读、GCS 分片写、批处理 JSON/commit、
// 压缩包装，以及 flags 与 HDFS 命令解析。

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use object_store::memory::InMemory;
use serial_test::serial;
use azblob_test_support::*;

#[test]
#[serial]
/// 校验加密密钥派生、progress/URL 解析与 SharedKey/SAS 选择。
fn azure_options_urls_progress_and_auth_match_go() {
    let mut options = AzblobBackendOptions {
        encryption_key: "customer-key".to_owned(),
        ..Default::default()
    };
    let mut backend = AzureBlobStorageConfig::default();
    // 客户密钥应 Base64 编码，并带 SHA-256 摘要字段。
    options.apply(&mut backend).unwrap();
    let key = backend.encryption_key.as_ref().unwrap();
    assert_eq!(key.encryption_key, "Y3VzdG9tZXIta2V5");
    assert_eq!(
        key.encryption_key_sha256,
        "doGE+d1FuA2y3bvgVArZ6qRr6RySPwPGW/thG3fDpAU="
    );

    assert_eq!(progress("12/99").unwrap(), (12, 99));
    assert!(progress("12").is_err());
    assert_eq!(
        url_of_object_by_endpoint("https://example.invalid/root", "bucket", "a/b").unwrap(),
        "https://example.invalid/root/bucket/a/b"
    );

    backend.bucket = "bucket".to_owned();
    backend.account_name = "acct".to_owned();
    backend.shared_key = "secret".to_owned();
    let selected = select_azure_client(&mut backend, &StorageOptions::default()).unwrap();
    assert_eq!(selected.auth, AzureAuth::SharedKey);
    assert_eq!(selected.service_url, "https://acct.blob.core.windows.net");

    backend.access_sig = "?sv=1".to_owned();
    let selected = select_azure_client(&mut backend, &StorageOptions::default()).unwrap();
    assert_eq!(selected.auth, AzureAuth::Sas);
    assert_eq!(
        selected.service_url,
        "https://acct.blob.core.windows.net/?sv=1"
    );
}

#[test]
/// 用 InMemory 后端验证 Azure/GCS 的读写、范围 Open、WalkDir 与 URI。
fn azure_and_gcs_object_operations_ranges_and_walk_match_go() {
    let ctx = objectio::Context::default();
    let store = Arc::new(InMemory::new());
    let azure = AzureBlobStorage::with_store(
        AzureBlobStorageConfig {
            bucket: "test".to_owned(),
            prefix: "a/b/".to_owned(),
            ..Default::default()
        },
        store.clone(),
        "account",
        "https://account.blob.core.windows.net",
    )
    .unwrap();

    storeapi::Storage::WriteFile(&azure, &ctx, "key", b"0123456789").unwrap();
    assert_eq!(
        storeapi::Storage::ReadFile(&azure, &ctx, "key").unwrap(),
        b"0123456789"
    );
    assert!(storeapi::Storage::FileExists(&azure, &ctx, "key").unwrap());

    // 半开区间 [2,7) 应读出 "23456"。
    let option = storeapi::ReaderOption {
        StartOffset: Some(2),
        EndOffset: Some(7),
        ..Default::default()
    };
    let mut reader = storeapi::Storage::Open(&azure, &ctx, "key", Some(&option)).unwrap();
    let mut body = String::new();
    reader.read_to_string(&mut body).unwrap();
    assert_eq!(body, "23456");
    assert_eq!(reader.seek(SeekFrom::End(-3)).unwrap(), 7);
    let mut tail = String::new();
    reader.read_to_string(&mut tail).unwrap();
    assert_eq!(tail, ""); // the configured half-open end remains 7
    assert_eq!(reader.file_size().unwrap(), 10);

    let mut walked = Vec::new();
    storeapi::Storage::WalkDir(&azure, &ctx, None, &mut |name, size| {
        walked.push((name.to_owned(), size));
        Ok(())
    })
    .unwrap();
    assert_eq!(walked, vec![("key".to_owned(), 10)]);
    assert_eq!(storeapi::Storage::URI(&azure), "azure://test/a/b/");

    let gcs = GCSStorage::with_store(
        GCSConfig {
            bucket: "test-gcs".to_owned(),
            prefix: "prefix".to_owned(),
            ..Default::default()
        },
        store,
        None,
    )
    .unwrap();
    storeapi::Storage::WriteFile(&gcs, &ctx, "g", b"google").unwrap();
    assert_eq!(
        storeapi::Storage::ReadFile(&gcs, &ctx, "g").unwrap(),
        b"google"
    );
    assert_eq!(storeapi::Storage::URI(&gcs), "gcs://test-gcs/prefix");
}

#[test]
/// 校验 GCS 分片 Writer 最小块约束、写出内容与 should_retry 分类。
fn gcs_multipart_and_retry_match_go() {
    let ctx = objectio::Context::default();
    let store = Arc::new(InMemory::new());
    assert!(GCSWriter::new(ctx.clone(), store.clone(), "multi", 1, 2).is_err());

    let mut writer =
        GCSWriter::new(ctx, store.clone(), "multi", GCS_MINIMUM_CHUNK_SIZE, 2).unwrap();
    objectio::Writer::write(&mut writer, &objectio::Context::default(), b"hello").unwrap();
    objectio::Writer::write(&mut writer, &objectio::Context::default(), b" world").unwrap();
    objectio::Writer::close(&mut writer, &objectio::Context::default()).unwrap();

    let gcs = GCSStorage::with_store(
        GCSConfig {
            bucket: "bucket".to_owned(),
            ..Default::default()
        },
        store,
        None,
    )
    .unwrap();
    assert_eq!(
        storeapi::Storage::ReadFile(&gcs, &objectio::Context::default(), "multi").unwrap(),
        b"hello world"
    );

    assert!(should_retry(&std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "broken pipe"
    )));
    assert!(should_retry(&std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "eof"
    )));
    assert!(!should_retry(&std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "bad request"
    )));
}

#[test]
/// 覆盖 Effect JSON、Batched commit 延迟可见性与 Gzip 透明压缩。
fn batched_json_commit_and_compression_match_go() {
    let effects = vec![
        Effect::Put(EffPut {
            file: "example.txt".to_owned(),
            content: b"Hello, world".to_vec(),
        }),
        Effect::DeleteFiles(EffDeleteFiles {
            files: vec!["old".to_owned(), "tmp".to_owned()],
        }),
        Effect::DeleteFile(EffDeleteFile("obsolete".to_owned())),
        Effect::Rename(EffRename {
            from: "old_name".to_owned(),
            to: "new_name".to_owned(),
        }),
    ];
    let json = json_effects(&effects).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(value[0]["type"], "objstore.EffPut");
    assert_eq!(value[0]["effect"]["content"], "SGVsbG8sIHdvcmxk");

    let ctx = objectio::Context::default();
    let inner = MemoryStorage::default();
    let batched = Batched::new(inner.clone());
    storeapi::Storage::WriteFile(&batched, &ctx, "compressed", b"hello world").unwrap();
    // commit 前底层不可见；commit 后内容落盘。
    assert!(!storeapi::Storage::FileExists(&inner, &ctx, "compressed").unwrap());
    batched.commit(&ctx).unwrap();
    assert_eq!(
        storeapi::Storage::ReadFile(&inner, &ctx, "compressed").unwrap(),
        b"hello world"
    );

    let compressed = WithCompression::new(
        inner.clone(),
        objectio::CompressType::Gzip,
        Default::default(),
    );
    storeapi::Storage::WriteFile(&compressed, &ctx, "gzip", b"hello,world!").unwrap();
    assert_ne!(
        storeapi::Storage::ReadFile(&inner, &ctx, "gzip").unwrap(),
        b"hello,world!"
    );
    assert_eq!(
        storeapi::Storage::ReadFile(&compressed, &ctx, "gzip").unwrap(),
        b"hello,world!"
    );
}

#[test]
#[serial]
/// 校验 BackendOptions 标志解析、隐藏标志与 HDFS 可执行路径拼装。
fn flags_and_hdfs_command_match_go() {
    let mut flags = FlagSet::default();
    define_flags(&mut flags);
    flags.set("gcs.endpoint", "http://gcs.invalid").unwrap();
    flags.set("azblob.account-name", "account").unwrap();
    let mut options = BackendOptions::default();
    options.parse_from_flags(&flags).unwrap();
    assert_eq!(options.gcs.endpoint, "http://gcs.invalid");
    assert_eq!(options.azblob.account_name, "account");
    hidden_flags_for_stream(&mut flags).unwrap();
    assert!(flags.is_hidden("gcs.endpoint"));
    assert!(flags.is_hidden("azblob.account-key"));

    unsafe { std::env::remove_var("HADOOP_HOME") };
    assert!(
        get_hdfs_bin()
            .unwrap_err()
            .to_string()
            .contains("HADOOP_HOME")
    );
    unsafe { std::env::set_var("HADOOP_HOME", "/opt/hadoop") };
    assert_eq!(
        get_hdfs_bin().unwrap(),
        std::path::PathBuf::from("/opt/hadoop/bin/hdfs")
    );
    let spec = dfs_command_spec(&["-ls", "hdfs://bucket/key"]).unwrap();
    assert_eq!(
        spec.program,
        std::path::PathBuf::from("/opt/hadoop/bin/hdfs")
    );
    assert_eq!(spec.args, vec!["dfs", "-ls", "hdfs://bucket/key"]);
    unsafe { std::env::remove_var("HADOOP_HOME") };
}
