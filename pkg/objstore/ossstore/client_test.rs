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

// OSS Client 单元测试：用可控 MockApi 验证权限探测、对象 CRUD、列表与拷贝。
//
// 各 `*_mode` 原子开关模拟成功/失败/`NoSuchKey`/`AccessDenied` 等路径，
// 并断言 Client 是否正确拼接桶前缀、Range 与错误透传。

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use task_ossstore::*;

/// 可按 mode 切换各 API 行为的测试替身，并记录请求与调用次数。
#[derive(Default)]
struct MockApi {
    bucket_mode: AtomicUsize,
    get_mode: AtomicUsize,
    put_mode: AtomicUsize,
    delete_mode: AtomicUsize,
    delete_objects_mode: AtomicUsize,
    head_mode: AtomicUsize,
    list_mode: AtomicUsize,
    copy_mode: AtomicUsize,
    delete_calls: AtomicUsize,
    get_requests: Mutex<Vec<GetObjectInput>>,
    delete_requests: Mutex<Vec<DeleteObjectsInput>>,
    list_requests: Mutex<Vec<ListObjectsV2Input>>,
    copy_requests: Mutex<Vec<CopyObjectInput>>,
    presign_mode: AtomicUsize,
    presign_requests: Mutex<Vec<(GetObjectInput, Duration)>>,
    upload_mode: AtomicUsize,
    complete_mode: AtomicUsize,
    upload_requests: Mutex<Vec<UploadPartInput>>,
    complete_requests: Mutex<Vec<CompleteMultipartUploadInput>>,
    abort_requests: Mutex<Vec<AbortMultipartUploadInput>>,
    /// CheckGetObject 成功路径会关闭 body；用于断言 close 被调用。
    body_closed: Arc<AtomicBool>,
}

/// 空读 body，close 时置位 `closed`。
struct TrackingBody {
    closed: Arc<AtomicBool>,
}

/// 模拟分片会话创建后读取源数据失败。
struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("mock read error"))
    }
}

impl Read for TrackingBody {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

impl prefetch::reader::ReadCloser for TrackingBody {
    fn close(&mut self) -> io::Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl API for MockApi {
    fn is_bucket_exist(&self, _: &storeapi::Context, _: &str) -> Result<bool> {
        match self.bucket_mode.load(Ordering::SeqCst) {
            1 => Err(anyhow!("mock head bucket error")),
            _ => Ok(true),
        }
    }

    fn get_object(&self, _: &storeapi::Context, input: &GetObjectInput) -> Result<GetObjectOutput> {
        self.get_requests.lock().unwrap().push(input.clone());
        // 1=TrackingBody；2=NoSuchKey；3=通用错误；4=带 Range 的字节内容。
        match self.get_mode.load(Ordering::SeqCst) {
            1 => Ok(GetObjectOutput {
                body: Box::new(TrackingBody {
                    closed: self.body_closed.clone(),
                }),
                content_length: Some(0),
                content_range: None,
            }),
            2 => Err(api_error("NoSuchKey", "missing")),
            3 => Err(anyhow!("mock get error")),
            4 => Ok(GetObjectOutput::from_bytes(
                b"0123456789".to_vec(),
                Some("bytes 0-9/100".to_owned()),
            )),
            _ => Ok(GetObjectOutput::from_bytes(Vec::new(), None)),
        }
    }

    fn put_object(&self, _: &storeapi::Context, _: &PutObjectInput) -> Result<()> {
        if self.put_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock put error"))
        } else {
            Ok(())
        }
    }

    fn delete_object(&self, _: &storeapi::Context, _: &DeleteObjectInput) -> Result<()> {
        self.delete_calls.fetch_add(1, Ordering::SeqCst);
        match self.delete_mode.load(Ordering::SeqCst) {
            1 => Err(anyhow!("mock del error")),
            2 => Err(api_error("AccessDenied", "AccessDenied")),
            _ => Ok(()),
        }
    }

    fn delete_objects(&self, _: &storeapi::Context, input: &DeleteObjectsInput) -> Result<()> {
        self.delete_requests.lock().unwrap().push(input.clone());
        if self.delete_objects_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock delete error"))
        } else {
            Ok(())
        }
    }

    fn head_object(&self, _: &storeapi::Context, _: &HeadObjectInput) -> Result<()> {
        match self.head_mode.load(Ordering::SeqCst) {
            1 => Err(api_error("NoSuchKey", "missing")),
            2 => Err(anyhow!("some error")),
            _ => Ok(()),
        }
    }

    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsV2Input,
    ) -> Result<ListObjectsV2Output> {
        self.list_requests.lock().unwrap().push(input.clone());
        if self.list_mode.load(Ordering::SeqCst) == 1 {
            return Err(anyhow!("mock list error"));
        }
        // 返回含普通对象与「目录」占位键的截断列表。
        Ok(ListObjectsV2Output {
            is_truncated: true,
            next_continuation_token: Some("abcdefg".to_owned()),
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
        })
    }

    fn copy_object(&self, _: &storeapi::Context, input: &CopyObjectInput) -> Result<()> {
        self.copy_requests.lock().unwrap().push(input.clone());
        if self.copy_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock copy error"))
        } else {
            Ok(())
        }
    }

    fn presign_get_object(
        &self,
        _: &storeapi::Context,
        input: &GetObjectInput,
        expire: Duration,
    ) -> Result<String> {
        self.presign_requests
            .lock()
            .unwrap()
            .push((input.clone(), expire));
        if self.presign_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock presign error"))
        } else {
            Ok("https://bucket.example.com/prefix/object?signature=test".to_owned())
        }
    }

    fn initiate_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CreateMultipartUploadInput,
    ) -> Result<CreateMultipartUploadOutput> {
        Ok(CreateMultipartUploadOutput {
            bucket: input.bucket.clone(),
            key: input.key.clone(),
            upload_id: "upload-1".to_owned(),
        })
    }

    fn upload_part(
        &self,
        _: &storeapi::Context,
        input: &UploadPartInput,
    ) -> Result<UploadPartOutput> {
        self.upload_requests.lock().unwrap().push(input.clone());
        if self.upload_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock upload error"))
        } else {
            Ok(UploadPartOutput {
                etag: format!("etag-{}", input.part_number),
            })
        }
    }

    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
    ) -> Result<()> {
        self.complete_requests.lock().unwrap().push(input.clone());
        if self.complete_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock complete error"))
        } else {
            Ok(())
        }
    }

    fn abort_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &AbortMultipartUploadInput,
    ) -> Result<()> {
        self.abort_requests.lock().unwrap().push(input.clone());
        Ok(())
    }
}

/// 固定 bucket=`bucket`、prefix=`prefix/` 的测试 Client。
fn new_test_client(api: Arc<MockApi>) -> Client {
    Client::new(
        api,
        storeapi::NewBucketPrefix("bucket", "prefix/"),
        s3like::backuppb::S3 {
            Bucket: "bucket".to_owned(),
            Prefix: "prefix/".to_owned(),
            ..Default::default()
        },
    )
}

/// 断言 Result 为错误且消息包含 `needle`。
fn assert_error_contains<T>(result: Result<T>, needle: &str) {
    match result {
        Ok(_) => panic!("expected error containing {needle:?}"),
        Err(error) => assert!(error.to_string().contains(needle)),
    }
}

#[test]
fn presign_object_forwards_prefixed_get_request_and_expiration() {
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());
    let ctx = storeapi::Context::default();

    let url = client
        .PresignObject(&ctx, "object", Duration::from_secs(3600))
        .unwrap();

    assert_eq!(
        url,
        "https://bucket.example.com/prefix/object?signature=test"
    );
    assert_eq!(
        *api.presign_requests.lock().unwrap(),
        vec![(
            GetObjectInput {
                bucket: "bucket".to_owned(),
                key: "prefix/object".to_owned(),
                range: None,
            },
            Duration::from_secs(3600),
        )]
    );

    api.presign_mode.store(1, Ordering::SeqCst);
    assert_error_contains(
        client.PresignObject(&ctx, "object", Duration::from_secs(3600)),
        "mock presign error",
    );
}

#[test]
fn aliyun_presign_uses_public_endpoint_and_signs_temporary_credentials() {
    let api = Arc::new(
        AliyunOssApi::new(
            Arc::new(StaticCredentialsProvider::new(
                "access-key-id".to_owned(),
                "access-key-secret".to_owned(),
                "recognizable-security-token".to_owned(),
            )),
            "https://oss-cn-hangzhou.aliyuncs.com".to_owned(),
            "cn-hangzhou".to_owned(),
            None,
        )
        .unwrap(),
    );
    let client = Client::new(
        api,
        storeapi::NewBucketPrefix("bucket", "prefix/"),
        s3like::backuppb::S3::default(),
    );

    let signed = client
        .PresignObject(
            &storeapi::Context::default(),
            "object",
            Duration::from_secs(3600),
        )
        .unwrap();
    assert!(signed.starts_with("https://bucket.oss-cn-hangzhou.aliyuncs.com/prefix/object?"));
    assert!(signed.contains("x-oss-security-token=recognizable-security-token"));
    assert!(signed.contains("x-oss-signature="));
}

/// 覆盖 CheckBucket/List/Get/PutAndDelete 各权限探测的成功与失败分支。
#[test]
fn test_client_permission() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());

    client.CheckBucketExistence(&ctx).unwrap();
    api.bucket_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.CheckBucketExistence(&ctx), "mock head bucket error");
    api.bucket_mode.store(0, Ordering::SeqCst);

    client.CheckListObjects(&ctx).unwrap();
    api.list_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.CheckListObjects(&ctx), "mock list error");
    api.list_mode.store(0, Ordering::SeqCst);

    client.CheckGetObject(&ctx).unwrap();
    api.get_mode.store(1, Ordering::SeqCst);
    client.CheckGetObject(&ctx).unwrap();
    assert!(api.body_closed.load(Ordering::SeqCst));
    api.get_mode.store(2, Ordering::SeqCst);
    // NoSuchKey 在权限探测中视为可接受（对象可不存在）。
    client.CheckGetObject(&ctx).unwrap();
    api.get_mode.store(3, Ordering::SeqCst);
    assert_error_contains(client.CheckGetObject(&ctx), "mock get error");

    api.put_mode.store(0, Ordering::SeqCst);
    api.delete_mode.store(0, Ordering::SeqCst);
    client.CheckPutAndDeleteObject(&ctx).unwrap();

    api.put_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.CheckPutAndDeleteObject(&ctx), "mock put error");
    api.put_mode.store(0, Ordering::SeqCst);
    api.delete_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.CheckPutAndDeleteObject(&ctx), "mock del error");
    api.delete_mode.store(2, Ordering::SeqCst);
    assert_error_contains(client.CheckPutAndDeleteObject(&ctx), "AccessDenied");
    api.put_mode.store(1, Ordering::SeqCst);
    api.delete_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.CheckPutAndDeleteObject(&ctx), "mock put error");
    assert_eq!(api.delete_calls.load(Ordering::SeqCst), 5);
}

/// 验证 GetObject 的 Range 请求映射与错误透传。
#[test]
fn test_client_get_object() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    api.get_mode.store(4, Ordering::SeqCst);
    let client = new_test_client(api.clone());

    let response = client.GetObject(&ctx, "object", 0, 10).unwrap();
    assert!(!response.IsFullRange);
    assert_eq!(response.ContentLength, Some(10));
    assert_eq!(response.ContentRange.as_deref(), Some("bytes 0-9/100"));
    assert_eq!(
        api.get_requests.lock().unwrap()[0],
        GetObjectInput {
            bucket: "bucket".to_owned(),
            key: "prefix/object".to_owned(),
            range: Some("bytes=0-9".to_owned()),
        }
    );

    api.get_mode.store(3, Ordering::SeqCst);
    assert_error_contains(client.GetObject(&ctx, "object", 0, 10), "mock get error");
}

/// 验证批量删除时相对路径被加上桶前缀。
#[test]
fn test_client_delete_objects() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());
    let names = vec![
        "sub/object1".to_owned(),
        "object2".to_owned(),
        "sub/sub2/object3".to_owned(),
    ];

    client.DeleteObjects(&ctx, &names).unwrap();
    assert_eq!(
        api.delete_requests.lock().unwrap()[0].keys,
        vec![
            "prefix/sub/object1",
            "prefix/object2",
            "prefix/sub/sub2/object3"
        ]
    );

    api.delete_objects_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.DeleteObjects(&ctx, &names[..2]), "mock delete error");
}

/// HeadObject：NoSuchKey→false，其它错误透传，成功→true。
#[test]
fn test_client_is_object_exists() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());

    api.head_mode.store(1, Ordering::SeqCst);
    assert!(!client.IsObjectExists(&ctx, "object").unwrap());
    api.head_mode.store(2, Ordering::SeqCst);
    assert_error_contains(client.IsObjectExists(&ctx, "object"), "some error");
    api.head_mode.store(0, Ordering::SeqCst);
    assert!(client.IsObjectExists(&ctx, "object").unwrap());
}

/// ListObjects 前缀拼接、continuation、截断标志与错误路径。
#[test]
fn test_client_list_objects() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());

    let response = client.ListObjects(&ctx, "target", "", None, 100).unwrap();
    assert!(response.IsTruncated);
    assert_eq!(response.NextContinuationToken.as_deref(), Some("abcdefg"));
    assert_eq!(
        response.Objects,
        vec![
            s3like::Object {
                Key: "prefix/target/object1".to_owned(),
                Size: 10,
            },
            s3like::Object {
                Key: "prefix/target/sub/".to_owned(),
                Size: 0,
            },
            s3like::Object {
                Key: "prefix/target/sub/object2".to_owned(),
                Size: 20,
            },
        ]
    );
    let request = api.list_requests.lock().unwrap()[0].clone();
    assert_eq!(request.prefix, "prefix/target");
    assert_eq!(request.continuation_token, None);
    assert_eq!(request.max_keys, 100);

    api.list_mode.store(1, Ordering::SeqCst);
    assert_error_contains(
        client.ListObjects(&ctx, "target", "", None, 100),
        "mock list error",
    );
}

/// CopyObject：源/目标桶与前缀拼接，以及拷贝失败透传。
#[test]
fn test_client_copy_object() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());
    let input = s3like::CopyInput {
        FromLoc: storeapi::NewBucketPrefix("source-bucket", "source-prefix"),
        FromKey: "source-object".to_owned(),
        ToKey: "dir/dest-object".to_owned(),
    };

    client.CopyObject(&ctx, &input).unwrap();
    assert_eq!(
        api.copy_requests.lock().unwrap()[0],
        CopyObjectInput {
            bucket: "bucket".to_owned(),
            key: "prefix/dir/dest-object".to_owned(),
            source_bucket: "source-bucket".to_owned(),
            source_key: "source-prefix/source-object".to_owned(),
        }
    );

    api.copy_mode.store(1, Ordering::SeqCst);
    assert_error_contains(client.CopyObject(&ctx, &input), "mock copy error");
}

/// Go SDK 对非正上传参数使用默认值，并在 Complete 失败后清理分片会话。
#[test]
fn test_multipart_uploader_defaults_and_abort() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_test_client(api.clone());

    client
        .MultipartUploader("object", -1, 0)
        .Upload(&ctx, &mut io::Cursor::new(vec![b'x'; 5 * 1024 * 1024 + 1]))
        .unwrap();
    assert_eq!(api.upload_requests.lock().unwrap().len(), 1);
    assert_eq!(api.complete_requests.lock().unwrap().len(), 1);

    api.complete_mode.store(1, Ordering::SeqCst);
    assert_error_contains(
        client
            .MultipartUploader("object", 2, 1)
            .Upload(&ctx, &mut io::Cursor::new(b"data")),
        "mock complete error",
    );
    assert_eq!(api.abort_requests.lock().unwrap().len(), 1);

    api.complete_mode.store(0, Ordering::SeqCst);
    api.upload_mode.store(1, Ordering::SeqCst);
    assert_error_contains(
        client
            .MultipartUploader("object", 2, 1)
            .Upload(&ctx, &mut io::Cursor::new(b"data")),
        "mock upload error",
    );
    assert_eq!(api.abort_requests.lock().unwrap().len(), 2);

    api.upload_mode.store(0, Ordering::SeqCst);
    assert_error_contains(
        client
            .MultipartUploader("object", 2, 1)
            .Upload(&ctx, &mut FailingReader),
        "mock read error",
    );
    assert_eq!(api.abort_requests.lock().unwrap().len(), 3);
}
