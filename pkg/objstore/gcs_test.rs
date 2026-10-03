// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// GCS 存储单元测试：读写、枚举、分片上传、重试判定与访问统计。

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use objstore::azblob::StorageOptions;
use objstore::gcs::{
    AccessRecorder, GCS_CLIENT_COUNT, GCSBackendOptions, GCSConfig, GCSStorage, new_gcs_storage,
    should_retry,
};
use objstore::objectio::Context;
use objstore::storeapi::{ReaderOption, Storage, WalkOption, WriterOption};
use rand::RngCore;

/// 用给定桶、前缀与底层 `ObjectStore` 构造测试用 [`GCSStorage`]。
fn gcs_with_store(
    bucket: &str,
    prefix: &str,
    store: Arc<dyn object_store::ObjectStore>,
    recorder: Option<Arc<AccessRecorder>>,
) -> GCSStorage {
    GCSStorage::with_store(
        GCSConfig {
            bucket: bucket.into(),
            prefix: prefix.into(),
            storage_class: "NEARLINE".into(),
            predefined_acl: "private".into(),
            ..Default::default()
        },
        store,
        recorder,
    )
    .unwrap()
}

/// 默认内存后端 + 固定桶/前缀。
fn prepare_gcs_store(recorder: Option<Arc<AccessRecorder>>) -> GCSStorage {
    gcs_with_store("testbucket", "a/b/", Arc::new(InMemory::new()), recorder)
}

/// 覆盖写读删、WalkDir 前缀组合、大批量枚举与 Seek/读边界。
#[test]
fn test_gcs() {
    let ctx = Context::default();
    let object_store: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
    let storage = gcs_with_store("testbucket", "a/b/", object_store.clone(), None);
    storage.WriteFile(&ctx, "key", b"data").unwrap();
    storage.WriteFile(&ctx, "key1", b"data1").unwrap();
    let key2 = b"data22223346757222222222289722222";
    storage.WriteFile(&ctx, "key2", key2).unwrap();
    assert_eq!(storage.ReadFile(&ctx, "key").unwrap(), b"data");
    assert!(storage.FileExists(&ctx, "key").unwrap());
    assert!(!storage.FileExists(&ctx, "key_not_exist").unwrap());
    storage.WriteFile(&ctx, "key_delete", b"data").unwrap();
    storage.DeleteFile(&ctx, "key_delete").unwrap();
    storage.DeleteFile(&ctx, "key_delete").unwrap();

    // 不同 prefix / SubDir 切分应仍能枚举到同一组对象且总大小一致。
    for (prefix, sub_dir) in [("a/b/", ""), ("a/", "b/"), ("a/b", ""), ("a", "b/")] {
        let view = gcs_with_store("testbucket", prefix, object_store.clone(), None);
        let option = (!sub_dir.is_empty()).then(|| WalkOption {
            SubDir: sub_dir.into(),
            ..Default::default()
        });
        let mut total = 0;
        view.WalkDir(&ctx, option.as_ref(), &mut |name, size| {
            total += size;
            let _ = view.Open(&ctx, name, None)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(total, 42);
    }

    for index in 0..1000 {
        storage
            .WriteFile(&ctx, &format!("f{index}"), b"data")
            .unwrap();
    }
    let mut total = 0;
    let mut files = HashSet::new();
    storage
        .WalkDir(&ctx, None, &mut |name, size| {
            total += size;
            files.insert(name.to_owned());
            Ok(())
        })
        .unwrap();
    assert_eq!(total, 4042);
    for name in ["key", "key1", "key2"] {
        assert!(files.contains(name));
    }
    for index in 0..1000 {
        assert!(files.contains(&format!("f{index}")));
    }

    let mut reader = storage.Open(&ctx, "key2", None).unwrap();
    expect_read(&mut reader, 10, b"data222233");
    expect_read(&mut reader, 40, b"46757222222222289722222");
    assert_eq!(reader.seek(SeekFrom::Start(3)).unwrap(), 3);
    expect_read(&mut reader, 5, b"a2222");
    assert_eq!(reader.seek(SeekFrom::Current(3)).unwrap(), 11);
    expect_read(&mut reader, 5, b"67572");
    assert_eq!(reader.seek(SeekFrom::End(-7)).unwrap(), 26);
    expect_read(&mut reader, 5, b"97222");
    assert_eq!(reader.seek(SeekFrom::Start(100)).unwrap(), 100);
    assert_eq!(reader.read(&mut [0; 5]).unwrap(), 0);
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), key2.len() as u64);
    assert!(reader.seek(SeekFrom::End(-10000)).is_err());
    reader.close().unwrap();
    assert_eq!(storage.URI(), "gcs://testbucket/a/b/");
}

/// 读指定长度并断言内容。
fn expect_read(reader: &mut Box<dyn objstore::objectio::Reader>, len: usize, expected: &[u8]) {
    let mut bytes = vec![0; len];
    let count = reader.read(&mut bytes).unwrap();
    assert_eq!(&bytes[..count], expected);
}

/// 校验凭据文件 apply、`send_credentials` 缺文件报错，以及 `reset` 保持 client_count。
#[test]
fn test_new_gcs_storage() {
    let temp = tempfile::tempdir().unwrap();
    let credentials = temp.path().join("credentials.json");
    std::fs::write(&credentials, r#"{"type":"service_account"}"#).unwrap();
    let options = GCSBackendOptions {
        endpoint: "http://gcs.invalid".into(),
        storage_class: "NEARLINE".into(),
        predefined_acl: "private".into(),
        credentials_file: credentials.to_string_lossy().into_owned(),
    };
    let mut config = GCSConfig {
        bucket: "testbucket".into(),
        prefix: "a/b/".into(),
        ..Default::default()
    };
    options.apply(&mut config).unwrap();
    assert_eq!(config.credentials_blob, r#"{"type":"service_account"}"#);
    assert_eq!(config.storage_class, "NEARLINE");
    assert!(
        new_gcs_storage(
            GCSConfig {
                bucket: "testbucket".into(),
                ..Default::default()
            },
            &StorageOptions {
                send_credentials: true,
                ..Default::default()
            }
        )
        .is_err()
    );

    let mut storage = gcs_with_store("testbucket", "a/b", Arc::new(InMemory::new()), None);
    assert_eq!(storage.client_count(), GCS_CLIENT_COUNT);
    storage.WriteFile(&Context::default(), "x", b"x").unwrap();
    let mut names: Vec<String> = Vec::new();
    storage
        .WalkDir(&Context::default(), None, &mut |name, _| {
            names.push(name.into());
            Ok(())
        })
        .unwrap();
    assert_eq!(names, vec!["x"]);
    storage.reset().unwrap();
    assert_eq!(storage.client_count(), GCS_CLIENT_COUNT);
}

/// 范围读：StartOffset/EndOffset 应只返回区间内字节。
#[test]
fn test_read_range() {
    let ctx = Context::default();
    let storage = prepare_gcs_store(None);
    storage.WriteFile(&ctx, "key", b"0123456789").unwrap();
    let mut reader = storage
        .Open(
            &ctx,
            "key",
            Some(&ReaderOption {
                StartOffset: Some(2),
                EndOffset: Some(5),
                ..Default::default()
            }),
        )
        .unwrap();
    let mut content = [0; 10];
    let count = reader.read(&mut content).unwrap();
    assert_eq!(&content[..count], b"234");
}

/// 高并发 Create 走 multipart 路径，读回应与随机数据一致。
#[test]
fn test_multi_part_upload() {
    const SIZE: usize = 100 * 1024 * 1024;
    let object_store: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
    let storage = gcs_with_store("testbucket", "multipart/", object_store, None);
    let ctx = Context::default();
    let mut data = vec![0_u8; SIZE];
    rand::thread_rng().fill_bytes(&mut data);
    let mut writer = storage
        .Create(
            &ctx,
            "TestMultiPartUpload",
            Some(&WriterOption {
                Concurrency: 10,
                ..Default::default()
            }),
        )
        .unwrap();
    writer.write(&ctx, &data).unwrap();
    writer.close(&ctx).unwrap();
    assert_eq!(storage.ReadFile(&ctx, "TestMultiPartUpload").unwrap(), data);
}

/// 本地文件系统后端上并行读写大量小/大文件（性能冒烟）。
#[test]
fn test_speed_read_many_files() {
    let temp = tempfile::tempdir().unwrap();
    let object_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(LocalFileSystem::new_with_prefix(temp.path()).unwrap());
    let storage = Arc::new(
        GCSStorage::with_store(
            GCSConfig {
                bucket: "testbucket".into(),
                prefix: "speed/".into(),
                ..Default::default()
            },
            object_store,
            None,
        )
        .unwrap(),
    );
    let small_names = (0..1000)
        .map(|i| format!("TestSpeedReadManySmallFiles/{i}"))
        .collect::<Vec<_>>();
    run_parallel(
        storage.clone(),
        small_names.clone(),
        Arc::new(vec![0; 1024]),
        true,
    );
    for size in [10, 100, 1000] {
        run_parallel(
            storage.clone(),
            small_names[..size].to_vec(),
            Arc::new(Vec::new()),
            false,
        );
    }

    let large_names = (0..30)
        .map(|i| format!("TestSpeedReadManyLargeFiles/{i}"))
        .collect::<Vec<_>>();
    run_parallel(
        storage.clone(),
        large_names.clone(),
        Arc::new(vec![0; 100 * 1024 * 1024]),
        true,
    );
    run_parallel(storage, large_names, Arc::new(Vec::new()), false);
}

/// 4 线程争抢名称队列，并行写或读。
fn run_parallel(storage: Arc<GCSStorage>, names: Vec<String>, data: Arc<Vec<u8>>, write: bool) {
    let names = Arc::new(names);
    let next = Arc::new(AtomicUsize::new(0));
    thread::scope(|scope| {
        for _ in 0..4 {
            let storage = storage.clone();
            let names = names.clone();
            let data = data.clone();
            let next = next.clone();
            scope.spawn(move || {
                let ctx = Context::default();
                loop {
                    let index = next.fetch_add(1, Ordering::AcqRel);
                    let Some(name) = names.get(index) else { break };
                    if write {
                        storage.WriteFile(&ctx, name, &data).unwrap();
                    } else {
                        assert!(!storage.ReadFile(&ctx, name).unwrap().is_empty());
                    }
                }
            });
        }
    });
}

/// `should_retry`：可重试消息与 UnexpectedEof 为真，InvalidInput 为假。
#[test]
fn test_gcs_should_retry() {
    for message in [
        "http2: server sent GOAWAY and closed the connection",
        "http2: client connection lost",
        "status code 401",
    ] {
        assert!(should_retry(&std::io::Error::other(message)));
    }
    assert!(should_retry(&std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "EOF"
    )));
    assert!(!should_retry(&std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "bad request"
    )));
}

/// 构造期 Context 取消不应影响后续用新 Context 的操作。
#[test]
fn test_ctx_usage() {
    let construction_context = Context::default();
    let storage = prepare_gcs_store(None);
    construction_context.cancel();
    let operation_context = Context::default();
    assert!(!storage.FileExists(&operation_context, "key").unwrap());
    assert!(
        storage
            .FileExists(&construction_context, "key")
            .unwrap_err()
            .to_string()
            .contains("operation cancelled")
    );
}

/// 批量删除：存在与不存在的文件均可成功，存在者应消失。
#[test]
fn test_delete_files() {
    let ctx = Context::default();
    let storage = prepare_gcs_store(None);
    storage.WriteFile(&ctx, "key", b"0123456789").unwrap();
    storage
        .DeleteFiles(&ctx, &["key".into(), "not-exist-file".into()])
        .unwrap();
    assert!(!storage.FileExists(&ctx, "key").unwrap());
}

/// AccessRecorder：WriteFile / ReadFile / Create / Open 的计数与字节累计。
#[test]
fn test_gcs_access_recording() {
    let ctx = Context::default();
    let recorder = Arc::new(AccessRecorder::default());
    let storage = prepare_gcs_store(Some(recorder.clone()));
    storage.WriteFile(&ctx, "a.txt", b"hello").unwrap();
    assert_eq!(recorder.snapshot(), (0, 1, 0, 5));
    storage.ReadFile(&ctx, "a.txt").unwrap();
    assert_eq!(recorder.snapshot(), (1, 1, 5, 5));
    let mut writer = storage.Create(&ctx, "b.txt", None).unwrap();
    writer.write(&ctx, b" world!").unwrap();
    writer.close(&ctx).unwrap();
    assert_eq!(recorder.snapshot(), (1, 2, 5, 12));
    let mut reader = storage.Open(&ctx, "b.txt", None).unwrap();
    let mut output = [0; 20];
    assert_eq!(reader.read(&mut output).unwrap(), 7);
    assert_eq!(reader.seek(SeekFrom::Start(0)).unwrap(), 0);
    assert_eq!(reader.read(&mut output[..1]).unwrap(), 1);
    reader.close().unwrap();
    // Open performs an attribute request, and each newly opened range reader
    // performs another GET, including the reader recreated after Seek.
    assert_eq!(recorder.snapshot(), (4, 2, 13, 12));

    storage.Rename(&ctx, "a.txt", "renamed.txt").unwrap();
    assert!(!storage.FileExists(&ctx, "a.txt").unwrap());
    assert_eq!(storage.ReadFile(&ctx, "renamed.txt").unwrap(), b"hello");
    // Go implements Rename as ReadFile + WriteFile + DeleteFile, so the
    // corresponding traffic must pass through access recording as well.
    assert_eq!(recorder.snapshot(), (6, 3, 23, 17));
}

// Faults replace only the remote multipart boundary; successful requests still
// use the real InMemory upload and its nonempty payload.
#[derive(Debug)]
struct FaultingGCSUpload {
    inner: Box<dyn object_store::MultipartUpload>,
    events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    fail_part: bool,
    fail_complete: bool,
    fail_abort: bool,
}

fn multipart_failure(message: &str) -> object_store::Error {
    object_store::Error::Generic {
        store: "gcs-test",
        source: Box::new(std::io::Error::other(message.to_owned())),
    }
}

impl object_store::MultipartUpload for FaultingGCSUpload {
    fn put_part(&mut self, data: object_store::PutPayload) -> object_store::UploadPart {
        assert!(data.content_length() > 0);
        self.events.lock().unwrap().push("part");
        if self.fail_part {
            Box::pin(async { Err(multipart_failure("stage failed")) })
        } else {
            self.inner.put_part(data)
        }
    }

    fn complete<'a, 'b>(
        &'a mut self,
    ) -> futures::future::BoxFuture<'b, object_store::Result<object_store::PutResult>>
    where
        'a: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            self.events.lock().unwrap().push("complete");
            if self.fail_complete {
                Err(multipart_failure("finalize failed"))
            } else {
                self.inner.complete().await
            }
        })
    }

    fn abort<'a, 'b>(&'a mut self) -> futures::future::BoxFuture<'b, object_store::Result<()>>
    where
        'a: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            self.events.lock().unwrap().push("abort");
            if self.fail_abort {
                Err(multipart_failure("delete failed"))
            } else {
                self.inner.abort().await
            }
        })
    }
}

#[test]
fn test_gcs_writer_aborts_on_error() {
    use objstore::gcs_extra::{GCS_MINIMUM_CHUNK_SIZE, GCSWriter};
    use objstore::objectio::Writer;

    for (fail_part, fail_complete, fail_abort, prior_part) in [
        (true, false, false, false),
        (true, false, false, true),
        (false, true, false, false),
        (false, true, true, false),
        (true, false, true, false),
    ] {
        let ctx = Context::default();
        let store = Arc::new(InMemory::new());
        let mut writer = GCSWriter::new(
            ctx.clone(),
            store.clone(),
            "object",
            GCS_MINIMUM_CHUNK_SIZE,
            2,
        )
        .unwrap();
        if prior_part {
            writer.write(&ctx, b"staged data").unwrap();
        }
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        writer.wrap_upload_for_test(|inner| {
            Box::new(FaultingGCSUpload {
                inner,
                events: events.clone(),
                fail_part,
                fail_complete,
                fail_abort,
            })
        });
        let stage = writer.write(&ctx, b"next data");
        assert_eq!(stage.is_err(), fail_part);
        let close_error = writer
            .close(&ctx)
            .expect_err("failed uploads must fail Close");
        let mut source: &(dyn std::error::Error + 'static) = &close_error;
        let mut preserved = false;
        while let Some(next) = source.source() {
            preserved |= next.to_string().contains(if fail_part {
                "stage failed"
            } else {
                "finalize failed"
            });
            source = next;
        }
        assert!(
            preserved,
            "original upload failure must remain in the error chain"
        );
        let error = close_error.to_string();
        if fail_part {
            assert!(error.contains("stage failed"), "{error}");
        } else {
            assert!(
                error.contains("failed to finalize multipart upload"),
                "{error}"
            );
            assert!(error.contains("finalize failed"), "{error}");
        }
        if fail_abort {
            assert!(
                error.contains("failed to cancel multipart upload"),
                "{error}"
            );
            assert!(error.contains("delete failed"), "{error}");
        }
        drop(writer);
        let expected = if fail_part {
            vec!["part", "abort"]
        } else {
            vec!["part", "complete", "abort"]
        };
        assert_eq!(*events.lock().unwrap(), expected);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(
            runtime
                .block_on(object_store::ObjectStoreExt::head(
                    store.as_ref(),
                    &object_store::path::Path::from("object")
                ))
                .is_err()
        );
    }
}

#[test]
fn test_gcs_writer_success_does_not_abort() {
    use objstore::gcs_extra::{GCS_MINIMUM_CHUNK_SIZE, GCSWriter};
    use objstore::objectio::Writer;
    for empty in [false, true] {
        let ctx = Context::default();
        let store = Arc::new(InMemory::new());
        let mut writer = GCSWriter::new(
            ctx.clone(),
            store.clone(),
            "object",
            GCS_MINIMUM_CHUNK_SIZE,
            2,
        )
        .unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        writer.wrap_upload_for_test(|inner| {
            Box::new(FaultingGCSUpload {
                inner,
                events: events.clone(),
                fail_part: false,
                fail_complete: false,
                fail_abort: false,
            })
        });
        if !empty {
            writer.write(&ctx, b"data").unwrap();
        }
        writer.close(&ctx).unwrap();
        drop(writer);
        assert_eq!(
            *events.lock().unwrap(),
            if empty {
                vec![]
            } else {
                vec!["part", "complete"]
            }
        );
        if !empty {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let result = runtime
                .block_on(object_store::ObjectStoreExt::get(
                    store.as_ref(),
                    &object_store::path::Path::from("object"),
                ))
                .unwrap();
            assert_eq!(runtime.block_on(result.bytes()).unwrap().as_ref(), b"data");
        }
    }
}

#[test]
fn test_gcs_writer_cancelled_context_still_aborts() {
    use objstore::gcs_extra::{GCS_MINIMUM_CHUNK_SIZE, GCSWriter};
    use objstore::objectio::Writer;
    let ctx = Context::default();
    let mut writer = GCSWriter::new(
        ctx.clone(),
        Arc::new(InMemory::new()),
        "object",
        GCS_MINIMUM_CHUNK_SIZE,
        2,
    )
    .unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    writer.wrap_upload_for_test(|inner| {
        Box::new(FaultingGCSUpload {
            inner,
            events: events.clone(),
            fail_part: false,
            fail_complete: false,
            fail_abort: false,
        })
    });
    ctx.cancel();
    assert!(writer.write(&ctx, b"data").is_err());
    assert!(
        writer
            .close(&ctx)
            .unwrap_err()
            .to_string()
            .contains("operation cancelled")
    );
    drop(writer);
    assert_eq!(*events.lock().unwrap(), vec!["abort"]);
}
