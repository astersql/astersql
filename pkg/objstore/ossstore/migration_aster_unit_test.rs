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

// ossstore 迁移对齐综合单元测试。
//
// 对照 Go 行为验证：权限探测清理、对象请求前缀映射、分片写入选项与顺序、
// 凭证快照刷新、重试/日志/endpoint 辅助，以及 `prepare_backend` 清凭证语义。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use crate::*;

/// 可切换失败模式并记录各 API 请求的测试替身。
#[derive(Default)]
struct MockApi {
    get_mode: AtomicUsize,
    put_mode: AtomicUsize,
    delete_mode: AtomicUsize,
    delete_calls: AtomicUsize,
    get_requests: Mutex<Vec<GetObjectInput>>,
    delete_requests: Mutex<Vec<DeleteObjectsInput>>,
    list_requests: Mutex<Vec<ListObjectsV2Input>>,
    copy_requests: Mutex<Vec<CopyObjectInput>>,
    create_requests: Mutex<Vec<CreateMultipartUploadInput>>,
    upload_requests: Mutex<Vec<UploadPartInput>>,
    complete_requests: Mutex<Vec<CompleteMultipartUploadInput>>,
}

impl API for MockApi {
    fn is_bucket_exist(&self, _: &storeapi::Context, _: &str) -> Result<bool> {
        Ok(true)
    }

    fn get_object(&self, _: &storeapi::Context, input: &GetObjectInput) -> Result<GetObjectOutput> {
        self.get_requests.lock().unwrap().push(input.clone());
        // 1=NoSuchKey；2=通用错误；默认返回带 Range 元数据的内容。
        match self.get_mode.load(Ordering::SeqCst) {
            1 => Err(api_error("NoSuchKey", "missing")),
            2 => Err(anyhow!("mock get error")),
            _ => Ok(GetObjectOutput::from_bytes(
                b"0123456789".to_vec(),
                Some("bytes 0-9/100".to_owned()),
            )),
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
        if self.delete_mode.load(Ordering::SeqCst) == 1 {
            Err(anyhow!("mock delete error"))
        } else {
            Ok(())
        }
    }

    fn delete_objects(&self, _: &storeapi::Context, input: &DeleteObjectsInput) -> Result<()> {
        self.delete_requests.lock().unwrap().push(input.clone());
        Ok(())
    }

    fn head_object(&self, _: &storeapi::Context, input: &HeadObjectInput) -> Result<()> {
        // 用键后缀约定模拟 missing / broken / 存在。
        if input.key.ends_with("missing") {
            Err(api_error("NoSuchKey", "missing"))
        } else if input.key.ends_with("broken") {
            Err(anyhow!("some error"))
        } else {
            Ok(())
        }
    }

    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsV2Input,
    ) -> Result<ListObjectsV2Output> {
        self.list_requests.lock().unwrap().push(input.clone());
        Ok(ListObjectsV2Output {
            next_continuation_token: Some("abcdefg".to_owned()),
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
            ],
        })
    }

    fn copy_object(&self, _: &storeapi::Context, input: &CopyObjectInput) -> Result<()> {
        self.copy_requests.lock().unwrap().push(input.clone());
        Ok(())
    }

    fn initiate_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CreateMultipartUploadInput,
    ) -> Result<CreateMultipartUploadOutput> {
        self.create_requests.lock().unwrap().push(input.clone());
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
        Ok(UploadPartOutput {
            etag: format!("etag-{}", input.part_number),
        })
    }

    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
    ) -> Result<()> {
        self.complete_requests.lock().unwrap().push(input.clone());
        Ok(())
    }
}

/// 构造带 KMS/IA 选项的测试 Client。
fn new_client(api: Arc<MockApi>) -> Client {
    let options = s3like::backuppb::S3 {
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        Sse: "KMS".to_owned(),
        SseKmsKeyId: "kms-key".to_owned(),
        StorageClass: "IA".to_owned(),
        ..Default::default()
    };
    Client::new(api, storeapi::NewBucketPrefix("bucket", "prefix/"), options)
}

/// 权限探测：NoSuchKey 可接受；Put 失败时仍尝试 Delete 清理。
#[test]
fn permission_cleanup_and_no_such_key_match_go() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_client(api.clone());

    client.CheckBucketExistence(&ctx).unwrap();
    api.get_mode.store(1, Ordering::SeqCst);
    client.CheckGetObject(&ctx).unwrap();

    api.put_mode.store(1, Ordering::SeqCst);
    api.delete_mode.store(1, Ordering::SeqCst);
    assert!(
        client
            .CheckPutAndDeleteObject(&ctx)
            .unwrap_err()
            .to_string()
            .contains("mock put error")
    );
    assert_eq!(api.delete_calls.load(Ordering::SeqCst), 1);

    api.put_mode.store(0, Ordering::SeqCst);
    assert!(
        client
            .CheckPutAndDeleteObject(&ctx)
            .unwrap_err()
            .to_string()
            .contains("mock delete error")
    );
    assert_eq!(api.delete_calls.load(Ordering::SeqCst), 2);
}

/// Get/Delete/Exists/List/Copy 的键前缀与 Range/start_after 映射对齐 Go。
#[test]
fn object_request_mapping_matches_go() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_client(api.clone());

    let response = client.GetObject(&ctx, "object", 0, 10).unwrap();
    assert!(!response.IsFullRange);
    assert_eq!(response.ContentLength, Some(10));
    assert_eq!(response.ContentRange.as_deref(), Some("bytes 0-9/100"));
    assert_eq!(api.get_requests.lock().unwrap()[0].key, "prefix/object");
    assert_eq!(
        api.get_requests.lock().unwrap()[0].range.as_deref(),
        Some("bytes=0-9")
    );

    client
        .DeleteObjects(&ctx, &["sub/object1".into(), "object2".into()])
        .unwrap();
    assert_eq!(
        api.delete_requests.lock().unwrap()[0].keys,
        vec!["prefix/sub/object1", "prefix/object2"]
    );

    assert!(!client.IsObjectExists(&ctx, "missing").unwrap());
    assert!(client.IsObjectExists(&ctx, "object").unwrap());
    assert!(
        client
            .IsObjectExists(&ctx, "broken")
            .unwrap_err()
            .to_string()
            .contains("some error")
    );

    let listed = client
        .ListObjects(&ctx, "target", "after", None, 100)
        .unwrap();
    assert!(listed.IsTruncated);
    assert_eq!(listed.NextContinuationToken.as_deref(), Some("abcdefg"));
    let list_request = &api.list_requests.lock().unwrap()[0];
    assert_eq!(list_request.prefix, "prefix/target");
    assert_eq!(list_request.start_after.as_deref(), Some("prefix/after"));
    assert_eq!(list_request.max_keys, 100);

    client
        .CopyObject(
            &ctx,
            &s3like::CopyInput {
                FromLoc: storeapi::NewBucketPrefix("source-bucket", "source-prefix"),
                FromKey: "source-object".to_owned(),
                ToKey: "dir/dest-object".to_owned(),
            },
        )
        .unwrap();
    let copy = &api.copy_requests.lock().unwrap()[0];
    assert_eq!(copy.source_bucket, "source-bucket");
    assert_eq!(copy.source_key, "source-prefix/source-object");
    assert_eq!(copy.key, "prefix/dir/dest-object");
}

#[test]
fn list_max_keys_uses_go_int32_conversion() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_client(api.clone());

    client
        .ListObjects(&ctx, "", "", None, i32::MAX as isize + 1)
        .unwrap();
    assert_eq!(api.list_requests.lock().unwrap()[0].max_keys, i32::MIN);
}

/// MultipartWriter：SSE/存储类选项、分片序号与 Complete 的 parts 顺序对齐 Go。
#[test]
fn multipart_writer_matches_go_part_order_and_options() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_client(api.clone());
    let mut writer = client.MultipartWriter(&ctx, "large").unwrap();
    assert_eq!(writer.Write(&ctx, b"first").unwrap(), 5);
    assert_eq!(writer.Write(&ctx, b"second").unwrap(), 6);
    writer.Close(&ctx).unwrap();

    let create = &api.create_requests.lock().unwrap()[0];
    assert_eq!(create.server_side_encryption.as_deref(), Some("KMS"));
    assert_eq!(create.sse_kms_key_id.as_deref(), Some("kms-key"));
    assert_eq!(create.storage_class.as_deref(), Some("IA"));
    let uploads = api.upload_requests.lock().unwrap();
    assert_eq!(
        uploads.iter().map(|p| p.part_number).collect::<Vec<_>>(),
        vec![1, 2]
    );
    let complete = &api.complete_requests.lock().unwrap()[0];
    assert_eq!(
        complete.parts,
        vec![
            CompletedPart {
                etag: "etag-1".to_owned(),
                part_number: 1
            },
            CompletedPart {
                etag: "etag-2".to_owned(),
                part_number: 2
            },
        ]
    );
}

#[test]
fn multipart_writer_allows_write_and_complete_after_close_like_go() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_client(api.clone());
    let mut writer = client.MultipartWriter(&ctx, "repeat").unwrap();

    writer.Write(&ctx, b"one").unwrap();
    writer.Close(&ctx).unwrap();
    writer.Write(&ctx, b"two").unwrap();
    writer.Close(&ctx).unwrap();

    assert_eq!(
        api.upload_requests
            .lock()
            .unwrap()
            .iter()
            .map(|part| part.part_number)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(api.complete_requests.lock().unwrap().len(), 2);
}

#[test]
fn multipart_writer_delegates_cancelled_context_to_api_like_go() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MockApi::default());
    let client = new_client(api.clone());
    let mut writer = client.MultipartWriter(&ctx, "cancelled").unwrap();
    ctx.cancel();

    assert_eq!(writer.Write(&ctx, b"data").unwrap(), 4);
    writer.Close(&ctx).unwrap();
    assert_eq!(api.upload_requests.lock().unwrap().len(), 1);
    assert_eq!(api.complete_requests.lock().unwrap().len(), 1);
}

/// 每次拉取递增序列号的凭证 mock。
struct CountingProvider(AtomicUsize);

impl CredentialsProvider for CountingProvider {
    fn get_credentials(&self) -> Result<ProviderCredentials> {
        let value = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ProviderCredentials {
            access_key_id: value.to_string(),
            access_key_secret: format!("secret-{value}"),
            security_token: format!("token-{value}"),
            provider_name: "mock".to_owned(),
        })
    }
}

/// 整快照一致性：SK/Token 与 AK 序号匹配；close 后仍可读最后快照。
#[test]
fn credential_refresher_publishes_whole_snapshots_and_stops() {
    let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
    let refresher = Arc::new(CredentialRefresher::new(provider.clone()));
    assert!(
        refresher
            .get_credentials()
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    refresher.refresh_once().unwrap();
    assert_eq!(refresher.get_credentials().unwrap().access_key_id, "1");
    refresher
        .start_refresh_with_interval(Duration::from_millis(5))
        .unwrap();
    std::thread::sleep(Duration::from_millis(24));
    refresher.close();
    assert!(provider.0.load(Ordering::SeqCst) >= 4);
    let snapshot = refresher.get_credentials().unwrap();
    assert_eq!(
        snapshot.access_key_secret,
        format!("secret-{}", snapshot.access_key_id)
    );
    assert_eq!(
        snapshot.security_token,
        format!("token-{}", snapshot.access_key_id)
    );
}

/// 重试参数、日志级别映射、region/endpoint/内网判定辅助对齐 Go。
#[test]
fn retry_logger_and_store_helpers_match_go() {
    let retryer = OssRetryer::default();
    assert_eq!(retryer.MaxAttempts(), 20);
    let delays = (0..16)
        .map(|_| retryer.RetryDelay(1, &anyhow!("retry")).unwrap())
        .collect::<Vec<_>>();
    assert!(delays.iter().all(|delay| *delay < Duration::from_secs(2)));
    assert!(delays.windows(2).any(|window| window[0] != window[1]));
    assert!(retryer.IsInstanceMetadataError(&anyhow!("GET 100.100.100.200 timed out")));
    assert!(!retryer.IsInstanceMetadataError(&anyhow!("other")));

    assert_eq!(
        get_oss_log_level(log::LevelFilter::Error),
        OssLogLevel::Error
    );
    assert_eq!(get_oss_log_level(log::LevelFilter::Warn), OssLogLevel::Warn);
    assert_eq!(get_oss_log_level(log::LevelFilter::Info), OssLogLevel::Warn);
    assert_eq!(
        get_oss_log_level(log::LevelFilter::Debug),
        OssLogLevel::Debug
    );
    assert_eq!(get_oss_log_level(log::LevelFilter::Trace), OssLogLevel::Off);

    assert_eq!(trim_oss_region_id("oss-cn-hangzhou"), "cn-hangzhou");
    assert_eq!(trim_oss_region_id("cn-hangzhou"), "cn-hangzhou");
    assert!(!can_use_internal_endpoint("", "cn-hangzhou"));
    assert!(!can_use_internal_endpoint("cn-beijing", "cn-hangzhou"));
    assert!(can_use_internal_endpoint("cn-hangzhou", "cn-hangzhou"));
    assert_eq!(
        endpoint_for_region("cn-hangzhou", false),
        "https://oss-cn-hangzhou.aliyuncs.com"
    );
    assert_eq!(
        endpoint_for_region("cn-hangzhou", true),
        "https://oss-cn-hangzhou-internal.aliyuncs.com"
    );
}

/// OSS SDK 标准重试器按结构化状态/服务码与网络错误判断，不解析任意数字文本。
#[test]
fn standard_retry_classification_matches_aliyun_sdk() {
    let retryer = OssRetryer::default();

    for status in [
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::REQUEST_TIMEOUT,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let error = anyhow::Error::new(ali_oss_rs::error::Error::StatusError(status));
        assert!(retryer.IsErrorRetryable(&error), "status {status}");
    }
    let bad_request = anyhow::Error::new(ali_oss_rs::error::Error::StatusError(
        reqwest::StatusCode::BAD_REQUEST,
    ));
    assert!(!retryer.IsErrorRetryable(&bad_request));

    assert!(retryer.IsErrorRetryable(&api_error("RequestTimeTooSkewed", "clock skew")));
    assert!(retryer.IsErrorRetryable(&api_error("BadRequest", "retryable service error")));
    assert!(!retryer.IsErrorRetryable(&api_error("AccessDenied", "permanent")));
    assert!(!retryer.IsErrorRetryable(&anyhow!("operation 503 without structured status")));

    let timed_out = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "socket timed out",
    ));
    let unexpected_eof = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "short response",
    ));
    assert!(retryer.IsErrorRetryable(&timed_out));
    assert!(retryer.IsErrorRetryable(&unexpected_eof));
}

/// prepare_backend preserves the constructor copy; credential mutation follows provider selection.
#[test]
fn prepare_backend_preserves_constructor_copy_and_input() {
    let mut backend = s3like::backuppb::S3 {
        AccessKey: "ak".to_owned(),
        SecretAccessKey: "sk".to_owned(),
        SessionToken: "token".to_owned(),
        ..Default::default()
    };
    let copy = prepare_backend(&mut backend, false).unwrap();
    assert_eq!(copy.AccessKey, "ak");
    assert_eq!(backend.AccessKey, "ak");
    assert_eq!(backend.SecretAccessKey, "sk");
    assert_eq!(backend.SessionToken, "token");
}
