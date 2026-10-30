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

// S3Client 权限探测与对象操作单元测试。
//
// 依赖 `main_test` 中的 MockS3/Suite，覆盖 Get/Delete/Exists/List/Copy
// 及 Content-MD5 兼容选项等行为。

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use s3store::*;

#[path = "main_test.rs"]
mod support;
use support::{CreateS3Suite, MockS3};

/// 串行化依赖 HEAD 指标计数器的测试，避免并行干扰。
pub(crate) static HEAD_OBJECT_METRIC_TEST_LOCK: Mutex<()> = Mutex::new(());
/// 串行化依赖 LIST 指标计数器的测试，避免并行干扰。
pub(crate) static LIST_OBJECTS_METRIC_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 构造空 Body 的 GetObject 成功响应。
fn empty_get() -> GetObjectOutput {
    GetObjectOutput {
        body: Box::new(MemoryBody::new(Vec::new())),
        content_length: Some(0),
        content_range: None,
    }
}

/// 构造可切换 s3_compatible（Content-MD5）的测试 Client。
fn client_with_compat(mock: Arc<MockS3>, compatible: bool) -> S3Client {
    let options = s3store::backuppb::S3 {
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        ..Default::default()
    };
    S3Client::new(
        mock,
        storeapi::NewBucketPrefix("bucket", "prefix/"),
        options,
        compatible,
    )
}

#[test]
/// 覆盖 AccessBuckets/List/Get/PutAndDelete 权限探测的成功与失败路径。
fn test_client_permission() {
    let _list_guard = LIST_OBJECTS_METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();

    suite.MockS3.push_head_bucket(Ok(()));
    suite
        .MockS3
        .push_head_bucket(Err(anyhow!("mock head bucket error")));
    s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::AccessBuckets]).unwrap();
    assert!(
        s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::AccessBuckets])
            .unwrap_err()
            .to_string()
            .contains("mock head bucket error")
    );

    suite.MockS3.push_list(Ok(ListObjectsV2Output::default()));
    suite.MockS3.push_list(Err(anyhow!("mock list error")));
    s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::ListObjects]).unwrap();
    assert!(
        s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::ListObjects])
            .unwrap_err()
            .to_string()
            .contains("mock list error")
    );

    suite.MockS3.push_get(Ok(empty_get()));
    suite
        .MockS3
        .push_get(Err(api_error("NoSuchKey", "missing")));
    suite.MockS3.push_get(Err(anyhow!("mock get error")));
    s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::GetObject]).unwrap();
    s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::GetObject]).unwrap();
    assert!(
        s3like::CheckPermissions(&ctx, &suite.Client, &[storeapi::Permission::GetObject])
            .unwrap_err()
            .to_string()
            .contains("mock get error")
    );

    for result in [
        Ok(()),
        Err(anyhow!("mock put error")),
        Ok(()),
        Ok(()),
        Err(anyhow!("mock put error")),
    ] {
        suite.MockS3.push_put(result);
    }
    for result in [
        Ok(()),
        Ok(()),
        Err(anyhow!("mock del error")),
        Err(api_error("AccessDenied", "AccessDenied")),
        Err(anyhow!("mock del error")),
    ] {
        suite.MockS3.push_delete(result);
    }
    s3like::CheckPermissions(
        &ctx,
        &suite.Client,
        &[storeapi::Permission::PutAndDeleteObject],
    )
    .unwrap();
    for expected in [
        "mock put error",
        "mock del error",
        "AccessDenied",
        "mock put error",
    ] {
        let error = s3like::CheckPermissions(
            &ctx,
            &suite.Client,
            &[storeapi::Permission::PutAndDeleteObject],
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
    }

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(calls.head_buckets.len(), 2);
    assert_eq!(calls.lists.len(), 2);
    assert_eq!(calls.gets.len(), 3);
    assert_eq!(calls.puts.len(), 5);
    assert_eq!(calls.deletes.len(), 5);
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
/// Go 仅对 NoSuchKey 清理错误抑制告警，Put 成功时仍返回该错误。
fn permission_cleanup_returns_no_such_key_after_successful_put() {
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();
    suite.MockS3.push_put(Ok(()));
    suite
        .MockS3
        .push_delete(Err(api_error("NoSuchKey", "cleanup target missing")));

    let error = suite.Client.CheckPutAndDeleteObject(&ctx).unwrap_err();
    assert!(error.to_string().contains("NoSuchKey"), "{error:#}");

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(calls.puts.len(), 1);
    assert_eq!(calls.deletes.len(), 1);
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
/// 校验 GetObject 的 Range 映射、响应字段与错误传播。
fn test_client_get_object() {
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();
    suite.MockS3.push_get(Ok(GetObjectOutput {
        body: Box::new(MemoryBody::new(Vec::new())),
        content_length: Some(10),
        content_range: Some("bytes 0-9/100".to_owned()),
    }));
    suite.MockS3.push_get(Err(anyhow!("mock get error")));

    let response = suite.Client.GetObject(&ctx, "object", 0, 10).unwrap();
    assert!(!response.IsFullRange);
    assert_eq!(response.ContentLength, Some(10));
    assert_eq!(response.ContentRange.as_deref(), Some("bytes 0-9/100"));
    let error = match suite.Client.GetObject(&ctx, "object", 0, 10) {
        Ok(_) => panic!("expected mock get error"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("mock get error"));

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(calls.gets.len(), 2);
    assert_eq!(calls.gets[0].0.bucket, "bucket");
    assert_eq!(calls.gets[0].0.key, "prefix/object");
    assert_eq!(calls.gets[0].0.range.as_deref(), Some("bytes=0-9"));
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
/// 校验批量删除的键前缀拼接与错误传播。
fn test_client_delete_objects() {
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();
    suite.MockS3.push_delete_batch(Ok(()));
    suite
        .MockS3
        .push_delete_batch(Err(anyhow!("mock delete error")));

    suite
        .Client
        .DeleteObjects(
            &ctx,
            &[
                "sub/object1".to_owned(),
                "object2".to_owned(),
                "sub/sub2/object3".to_owned(),
            ],
        )
        .unwrap();
    let error = suite
        .Client
        .DeleteObjects(&ctx, &["sub/object1".to_owned(), "object2".to_owned()])
        .unwrap_err();
    assert!(error.to_string().contains("mock delete error"));

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(
        calls.delete_batches[0].0.keys,
        [
            "prefix/sub/object1",
            "prefix/object2",
            "prefix/sub/sub2/object3"
        ]
    );
    assert_eq!(calls.delete_batches[1].0.keys.len(), 2);
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
/// 校验 Exists 对 NotFound 等错误码的处理及指标递增。
fn test_client_is_object_exists() {
    let _guard = HEAD_OBJECT_METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();
    let counter = s3like::S3_API_CALL_COUNTER
        .with_label_values(&[s3like::BACKEND_S3, s3like::API_CALL_HEAD_OBJECTS]);
    let before = counter.get();

    for code in ["NotFound", "NoSuchKey", "NoSuchBucket"] {
        suite.MockS3.push_head(Err(api_error(code, "missing")));
        assert!(!suite.Client.IsObjectExists(&ctx, "object").unwrap());
    }
    suite.MockS3.push_head(Err(anyhow!("some error")));
    let error = suite.Client.IsObjectExists(&ctx, "object").unwrap_err();
    assert!(error.to_string().contains("some error"));
    suite.MockS3.push_head(Ok(HeadObjectOutput::default()));
    assert!(suite.Client.IsObjectExists(&ctx, "object").unwrap());
    assert_eq!(counter.get(), before + 5.0);

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(calls.heads.len(), 5);
    assert!(calls.heads.iter().all(|call| call.0.key == "prefix/object"));
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
/// 校验 ListObjects 分页字段、前缀与指标记录。
fn test_client_list_objects() {
    let _guard = LIST_OBJECTS_METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();
    let counter = s3like::S3_API_CALL_COUNTER
        .with_label_values(&[s3like::BACKEND_S3, s3like::API_CALL_LIST_OBJECTS]);
    let before = counter.get();
    suite.MockS3.push_list(Ok(ListObjectsV2Output {
        is_truncated: true,
        contents: vec![
            ListedObject {
                key: "prefix/target/object1".to_owned(),
                size: 10,
            },
            ListedObject {
                key: "prefix/target/sub/".to_owned(),
                size: 0,
            },
            ListedObject {
                key: "prefix/target/sub/object2".to_owned(),
                size: 20,
            },
        ],
        next_continuation_token: Some("prefix/target/sub/object2".to_owned()),
    }));
    suite.MockS3.push_list(Err(anyhow!("mock list error")));

    let response = suite
        .Client
        .ListObjects(&ctx, "target", "", None, 100)
        .unwrap();
    assert!(response.IsTruncated);
    assert_eq!(
        response.NextContinuationToken.as_deref(),
        Some("prefix/target/sub/object2")
    );
    assert_eq!(
        response.Objects,
        [
            s3like::Object {
                Key: "prefix/target/object1".to_owned(),
                Size: 10
            },
            s3like::Object {
                Key: "prefix/target/sub/".to_owned(),
                Size: 0
            },
            s3like::Object {
                Key: "prefix/target/sub/object2".to_owned(),
                Size: 20
            },
        ]
    );
    let error = match suite.Client.ListObjects(&ctx, "target", "", None, 100) {
        Ok(_) => panic!("expected mock list error"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("mock list error"));
    // 多包并行测试时全局计数器可能被其它用例增加，故用 >= 而非精确相等；
    // 下方 Mock 调用日志才是权威调用次数。
    // Exact equality is racy when cargo tests many packages in one process that
    // share S3_API_CALL_COUNTER; the mock call log below is the authoritative count.
    assert!(
        counter.get() >= before + 2.0,
        "ListObjects should record at least two API calls"
    );

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(calls.lists.len(), 2);
    assert_eq!(calls.lists[0].0.prefix, "prefix/target");
    assert_eq!(calls.lists[0].0.max_keys, 100);
    assert_eq!(calls.lists[0].0.continuation_token, None);
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
/// 兼容模式下 Put/UploadPart 应带 Content-MD5，Delete 不带。
fn test_content_md5_option_for_s3_compatible() {
    let ctx = storeapi::Context::default();
    for compatible in [false, true] {
        let mock = Arc::new(MockS3::default());
        let client = client_with_compat(mock.clone(), compatible);

        client.PutObject(&ctx, "object", b"data").unwrap();
        client.CheckPutAndDeleteObject(&ctx).unwrap();

        let mut writer = client.MultipartWriter(&ctx, "object").unwrap();
        assert_eq!(writer.write(&ctx, b"part").unwrap(), 4);

        let uploader = client.MultipartUploader("object", 5 * 1024 * 1024, 2);
        uploader
            .Upload(&ctx, &mut Cursor::new(b"part".to_vec()))
            .unwrap();

        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.puts.len(), 3);
        assert_eq!(calls.puts[0].0.bucket, "bucket");
        assert_eq!(calls.puts[0].0.key, "prefix/object");
        assert_eq!(calls.puts[0].0.body, b"data");
        assert_eq!(calls.puts[0].1.content_md5, compatible);
        assert!(calls.puts[1].0.key.starts_with("prefix/perm-check/"));
        assert_eq!(calls.puts[1].1.content_md5, compatible);
        assert_eq!(calls.deletes[0].0.key, calls.puts[1].0.key);
        assert!(!calls.deletes[0].1.content_md5);
        assert_eq!(calls.creates[0].0.key, "prefix/object");
        assert_eq!(calls.uploads[0].0.key, "prefix/object");
        assert_eq!(calls.uploads[0].1.content_md5, compatible);
        assert_eq!(calls.puts[2].1.content_md5, compatible);
        drop(calls);
        mock.assert_drained();
    }
}

#[test]
/// 校验 CopySource 路径拼接与错误传播。
fn test_client_copy_object() {
    let suite = CreateS3Suite();
    let ctx = storeapi::Context::default();
    suite.MockS3.push_copy(Ok(()));
    suite.MockS3.push_copy(Err(anyhow!("mock copy error")));

    suite
        .Client
        .CopyObject(
            &ctx,
            &s3like::CopyInput {
                FromLoc: storeapi::NewBucketPrefix("source-bucket", "source-prefix"),
                FromKey: "/source-object".to_owned(),
                ToKey: "dir/dest-object".to_owned(),
            },
        )
        .unwrap();
    let error = suite
        .Client
        .CopyObject(
            &ctx,
            &s3like::CopyInput {
                FromLoc: storeapi::NewBucketPrefix("source-bucket", "source-prefix"),
                FromKey: "source-object".to_owned(),
                ToKey: "dir/dest-object".to_owned(),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("mock copy error"));

    let calls = suite.MockS3.calls.lock().unwrap();
    assert_eq!(calls.copies.len(), 2);
    assert_eq!(
        calls.copies[0].0.copy_source,
        "source-bucket/source-prefix/source-object"
    );
    assert_eq!(calls.copies[0].0.key, "prefix/dir/dest-object");
    drop(calls);
    suite.MockS3.assert_drained();
}

#[test]
fn multipart_writer_rejects_part_10001_before_network() {
    let mock = Arc::new(MockS3::default());
    let client = client_with_compat(mock.clone(), false);
    let ctx = storeapi::Context::default();
    let mut writer = client.MultipartWriter(&ctx, "object").unwrap();
    for _ in 0..storeapi::MaxUploadParts {
        assert_eq!(writer.write(&ctx, b"x").unwrap(), 1);
    }
    let error = writer.write(&ctx, b"overflow").unwrap_err();
    assert!(
        error
            .get_ref()
            .unwrap()
            .is::<storeapi::ExceedMaxUploadParts>()
    );
    assert_eq!(mock.calls.lock().unwrap().uploads.len(), 10000);
}
#[test]
fn multipart_uploader_normalizes_sdk_part_limit_error() {
    let mock = Arc::new(MockS3::default());
    mock.push_upload(Err(anyhow!("exceeded MaxUploadParts")));
    let client = client_with_compat(mock.clone(), false);
    let ctx = storeapi::Context::default();
    let error = client
        .MultipartUploader("object", 1, 1)
        .Upload(&ctx, &mut Cursor::new(b"ab".to_vec()))
        .unwrap_err();
    assert!(error.is::<storeapi::ExceedMaxUploadParts>());
    assert_eq!(mock.calls.lock().unwrap().uploads.len(), 1);
}
#[test]
fn multipart_uploader_enforces_object_part_limit() {
    let mock = Arc::new(MockS3::default());
    let client = client_with_compat(mock.clone(), false);
    let error = client
        .MultipartUploader("object", 1, 1)
        .Upload(
            &storeapi::Context::default(),
            &mut Cursor::new(vec![b'x'; 10001]),
        )
        .unwrap_err();
    assert!(error.is::<storeapi::ExceedMaxUploadParts>());
    assert!(mock.calls.lock().unwrap().uploads.is_empty());
}

#[test]
fn multipart_uploader_normalizes_part_limit_at_all_sdk_stages() {
    for stage in ["put", "create", "complete"] {
        let mock = Arc::new(MockS3::default());
        let error = anyhow!("SDK MaxUploadParts limit");
        match stage {
            "put" => mock.push_put(Err(error)),
            "create" => mock.push_create(Err(error)),
            _ => mock.push_complete(Err(error)),
        }
        let client = client_with_compat(mock.clone(), false);
        let data = if stage == "put" {
            b"a".to_vec()
        } else {
            b"ab".to_vec()
        };
        let error = client
            .MultipartUploader("object", 1, 1)
            .Upload(&storeapi::Context::default(), &mut Cursor::new(data))
            .unwrap_err();
        assert!(
            error.is::<storeapi::ExceedMaxUploadParts>(),
            "{stage}: {error}"
        );
        mock.assert_drained();
    }
}

#[test]
fn port_cb395494e6_encryption_context_requests_and_upload_paths() {
    for context in ["", "eyJ0ZW5hbnQiOiJ0ZW5hbnQtYSJ9"] {
        let mock = Arc::new(MockS3::default());
        let client = S3Client::new(
            mock.clone(),
            storeapi::NewBucketPrefix("bucket", "prefix/"),
            backuppb::S3 {
                Bucket: "bucket".into(),
                Sse: "aws:kms".into(),
                SseKmsKeyId: "key-id".into(),
                SseKmsEncryptionContext: context.into(),
                Acl: "private".into(),
                StorageClass: "STANDARD_IA".into(),
                ..Default::default()
            },
            false,
        );
        let ctx = storeapi::Context::default();
        client.PutObject(&ctx, "object", b"data").unwrap();
        client.CheckPutAndDeleteObject(&ctx).unwrap();
        client.MultipartWriter(&ctx, "object").unwrap();
        client
            .CopyObject(
                &ctx,
                &s3like::CopyInput {
                    FromLoc: storeapi::NewBucketPrefix("source", "source-prefix/"),
                    FromKey: "object".into(),
                    ToKey: "copy".into(),
                },
            )
            .unwrap();
        for data in [vec![1; 2], vec![1; 5]] {
            client
                .MultipartUploader("upload", 3, 2)
                .Upload(&ctx, &mut Cursor::new(data))
                .unwrap();
        }
        let calls = mock.calls.lock().unwrap();
        let expected = (!context.is_empty()).then_some(context);
        assert_eq!(calls.puts.len(), 3);
        for (input, _) in &calls.puts {
            assert_eq!(input.sse_kms_encryption_context.as_deref(), expected);
            assert_eq!(input.server_side_encryption.as_deref(), Some("aws:kms"));
            assert_eq!(input.sse_kms_key_id.as_deref(), Some("key-id"));
        }
        assert_eq!(calls.puts[0].0.acl.as_deref(), Some("private"));
        assert_eq!(
            calls.puts[0].0.storage_class.as_deref(),
            Some("STANDARD_IA")
        );
        assert!(calls.puts[1].0.acl.is_none());
        assert_eq!(calls.creates.len(), 2);
        for (input, _) in &calls.creates {
            assert_eq!(input.sse_kms_encryption_context.as_deref(), expected);
            assert_eq!(input.server_side_encryption.as_deref(), Some("aws:kms"));
            assert_eq!(input.sse_kms_key_id.as_deref(), Some("key-id"));
        }
        assert_eq!(calls.creates[0].0.acl.as_deref(), Some("private"));
        assert_eq!(
            calls.creates[0].0.storage_class.as_deref(),
            Some("STANDARD_IA")
        );
        let copy = &calls.copies[0].0;
        assert_eq!(copy.sse_kms_encryption_context.as_deref(), expected);
        assert_eq!(copy.server_side_encryption.as_deref(), Some("aws:kms"));
        assert_eq!(copy.sse_kms_key_id.as_deref(), Some("key-id"));
        assert_eq!(copy.copy_source, "source/source-prefix/object");
        assert_eq!(copy.bucket, "bucket");
        assert_eq!(calls.uploads.len(), 2);
        assert_eq!(calls.completes.len(), 1);
    }
}

#[test]
fn port_cb395494e6_sdk_headers_for_copy_single_and_multipart_upload() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    for context in ["", "eyJ0ZW5hbnQiOiJ0ZW5hbnQtYSJ9"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut captured = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(20);
            while captured.len() < 6 && Instant::now() < deadline {
                let (stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let lower = request.to_ascii_lowercase();
                if lower.contains("expect: 100-continue") {
                    reader
                        .get_mut()
                        .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                        .unwrap();
                }
                if lower.contains("transfer-encoding: chunked") {
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let size =
                            usize::from_str_radix(line.trim().split(';').next().unwrap(), 16)
                                .unwrap();
                        let mut body = vec![0; size + 2];
                        reader.read_exact(&mut body).unwrap();
                        if size == 0 {
                            break;
                        }
                    }
                } else if let Some(length) = lower.lines().find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|x| x.trim().parse::<usize>().unwrap())
                }) {
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                }
                let line = request.lines().next().unwrap();
                let body = if line.contains("?uploads") {
                    "<InitiateMultipartUploadResult><Bucket>bucket</Bucket><Key>prefix/object</Key><UploadId>upload-id</UploadId></InitiateMultipartUploadResult>"
                } else if line.starts_with("POST ") {
                    "<CompleteMultipartUploadResult><ETag>etag</ETag></CompleteMultipartUploadResult>"
                } else if lower.contains("x-amz-copy-source:") {
                    "<CopyObjectResult><ETag>etag</ETag></CopyObjectResult>"
                } else {
                    ""
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"etag\"\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
                captured.push(request);
            }
            assert_eq!(captured.len(), 6);
            captured
        });
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let sdk = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version_latest()
                .region(aws_types::region::Region::new("us-east-1"))
                .credentials_provider(aws_credential_types::Credentials::new(
                    "test", "test", None, None, "test",
                ))
                .endpoint_url(endpoint)
                .force_path_style(true)
                .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
                .timeout_config(
                    aws_sdk_s3::config::timeout::TimeoutConfig::builder()
                        .operation_timeout(Duration::from_secs(5))
                        .build(),
                )
                .build(),
        );
        let api = Arc::new(AwsS3Api::new(sdk, runtime, None));
        let client = S3Client::new(
            api,
            storeapi::NewBucketPrefix("bucket", "prefix/"),
            backuppb::S3 {
                Sse: "aws:kms".into(),
                SseKmsKeyId: "destination-key".into(),
                SseKmsEncryptionContext: context.into(),
                ..Default::default()
            },
            false,
        );
        let ctx = storeapi::Context::default();
        let result: anyhow::Result<()> = (|| {
            client.CopyObject(
                &ctx,
                &s3like::CopyInput {
                    FromLoc: storeapi::NewBucketPrefix("source", "source-prefix/"),
                    FromKey: "object".into(),
                    ToKey: "copy".into(),
                },
            )?;
            client
                .MultipartUploader("object", 5 * 1024 * 1024, 2)
                .Upload(&ctx, &mut Cursor::new(vec![1; 32]))?;
            client
                .MultipartUploader("object", 5 * 1024 * 1024, 2)
                .Upload(&ctx, &mut Cursor::new(vec![1; 5 * 1024 * 1024 + 1]))?;
            Ok(())
        })();
        let captured = server.join().unwrap();
        result.unwrap();
        let mut counts = std::collections::BTreeMap::new();
        for request in captured {
            let line = request.lines().next().unwrap();
            let operation = if line.contains("?uploads") {
                "create"
            } else if line.contains("partNumber=") {
                "part"
            } else if line.starts_with("POST ") {
                "complete"
            } else if request.to_ascii_lowercase().contains("x-amz-copy-source:") {
                "copy"
            } else {
                "put"
            };
            *counts.entry(operation).or_insert(0) += 1;
            let header = |name: &str| {
                request
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then(|| value.trim()))
            };
            let writes = matches!(operation, "put" | "copy" | "create");
            assert_eq!(
                header("x-amz-server-side-encryption-context"),
                (writes && !context.is_empty()).then_some(context)
            );
            assert_eq!(
                header("x-amz-server-side-encryption"),
                writes.then_some("aws:kms")
            );
            assert_eq!(
                header("x-amz-server-side-encryption-aws-kms-key-id"),
                writes.then_some("destination-key")
            );
        }
        assert_eq!(
            counts,
            std::collections::BTreeMap::from([
                ("copy", 1),
                ("put", 1),
                ("create", 1),
                ("part", 2),
                ("complete", 1)
            ])
        );
    }
}
