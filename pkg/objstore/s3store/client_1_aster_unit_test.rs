// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Aster 迁移对齐：S3Client 请求映射、权限清理、分片与重试策略单元测试。
//
// 使用内存 MockS3 验证 Put/Get/List/Copy/Multipart 等与 Go 侧行为一致。

use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::*;
use anyhow::{Result, anyhow};
use aws_sdk_s3::primitives::ByteStream;

#[derive(Debug)]
struct SevenAttemptRetryer;

impl storeapi::Retryer for SevenAttemptRetryer {
    fn retry_config(&self) -> aws_sdk_s3::config::retry::RetryConfig {
        aws_sdk_s3::config::retry::RetryConfig::standard()
            .with_max_attempts(7)
            .with_initial_backoff(Duration::from_millis(250))
            .with_max_backoff(Duration::from_secs(5))
    }
}

#[derive(Debug)]
struct NeverHttpClient;

#[derive(Debug)]
struct NeverHttpConnector;

impl storeapi::aws_smithy_runtime_api::client::http::HttpConnector for NeverHttpConnector {
    fn call(
        &self,
        _: storeapi::aws_smithy_runtime_api::client::orchestrator::HttpRequest,
    ) -> storeapi::aws_smithy_runtime_api::client::http::HttpConnectorFuture {
        storeapi::aws_smithy_runtime_api::client::http::HttpConnectorFuture::new(async {
            std::future::pending().await
        })
    }
}

impl storeapi::aws_smithy_runtime_api::client::http::HttpClient for NeverHttpClient {
    fn http_connector(
        &self,
        _: &storeapi::aws_smithy_runtime_api::client::http::HttpConnectorSettings,
        _: &storeapi::aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
    ) -> storeapi::aws_smithy_runtime_api::client::http::SharedHttpConnector {
        storeapi::aws_smithy_runtime_api::client::http::SharedHttpConnector::new(NeverHttpConnector)
    }
}

#[derive(Default)]
/// 记录 Mock 收到的各类 API 调用输入。
struct Calls {
    puts: Vec<PutObjectInput>,
    deletes: Vec<DeleteObjectInput>,
    delete_batches: Vec<DeleteObjectsInput>,
    gets: Vec<GetObjectInput>,
    lists: Vec<ListObjectsV2Input>,
    copies: Vec<CopyObjectInput>,
    uploads: Vec<UploadPartInput>,
    completes: Vec<CompleteMultipartUploadInput>,
}

#[derive(Default)]
/// 可注入错误的轻量 S3API Mock（相对 main_test 更精简）。
struct MockS3 {
    calls: Mutex<Calls>,
    put_error: Mutex<Option<anyhow::Error>>,
    delete_error: Mutex<Option<anyhow::Error>>,
    upload_error_on_part: Mutex<Option<i32>>,
    head_error: Mutex<Option<anyhow::Error>>,
    object_lock: Mutex<bool>,
}

/// Mock：记录调用并按注入状态返回成功或错误。
impl S3API for MockS3 {
    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsV2Input,
        _: RequestOptions,
    ) -> Result<ListObjectsV2Output> {
        self.calls.lock().unwrap().lists.push(input.clone());
        Ok(ListObjectsV2Output {
            next_continuation_token: Some("next".to_owned()),
            is_truncated: true,
            contents: vec![ListedObject {
                key: "prefix/target/object".to_owned(),
                size: 11,
            }],
        })
    }

    fn get_object(
        &self,
        _: &storeapi::Context,
        input: &GetObjectInput,
        _: RequestOptions,
    ) -> Result<GetObjectOutput> {
        self.calls.lock().unwrap().gets.push(input.clone());
        Ok(GetObjectOutput {
            body: Box::new(MemoryBody::new(b"payload".to_vec())),
            content_length: Some(7),
            content_range: Some("bytes 0-6/20".to_owned()),
        })
    }

    fn put_object(
        &self,
        _: &storeapi::Context,
        input: &PutObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.calls.lock().unwrap().puts.push(input.clone());
        if let Some(err) = self.put_error.lock().unwrap().take() {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn delete_object(
        &self,
        _: &storeapi::Context,
        input: &DeleteObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.calls.lock().unwrap().deletes.push(input.clone());
        if let Some(err) = self.delete_error.lock().unwrap().take() {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn delete_objects(
        &self,
        _: &storeapi::Context,
        input: &DeleteObjectsInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .delete_batches
            .push(input.clone());
        Ok(())
    }

    fn head_object(
        &self,
        _: &storeapi::Context,
        _: &HeadObjectInput,
        _: RequestOptions,
    ) -> Result<HeadObjectOutput> {
        if let Some(err) = self.head_error.lock().unwrap().take() {
            Err(err)
        } else {
            Ok(HeadObjectOutput {
                replication_status: "COMPLETED".to_owned(),
            })
        }
    }

    fn copy_object(
        &self,
        _: &storeapi::Context,
        input: &CopyObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.calls.lock().unwrap().copies.push(input.clone());
        Ok(())
    }

    fn create_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CreateMultipartUploadInput,
        _: RequestOptions,
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
        _: RequestOptions,
    ) -> Result<UploadPartOutput> {
        self.calls.lock().unwrap().uploads.push(input.clone());
        if *self.upload_error_on_part.lock().unwrap() == Some(input.part_number) {
            return Err(anyhow!("part failed"));
        }
        Ok(UploadPartOutput {
            e_tag: Some(format!("etag-{}", input.part_number)),
        })
    }

    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.calls.lock().unwrap().completes.push(input.clone());
        Ok(())
    }

    fn get_object_lock_configuration(
        &self,
        _: &storeapi::Context,
        _: &GetObjectLockConfigurationInput,
        _: RequestOptions,
    ) -> Result<bool> {
        Ok(*self.object_lock.lock().unwrap())
    }
}

/// 构造带固定桶/前缀与 ACL/SSE 配置的测试 Client。
fn client(api: Arc<MockS3>, compatible: bool) -> S3Client {
    let options = backuppb::S3 {
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        Acl: "private".to_owned(),
        Sse: "aws:kms".to_owned(),
        SseKmsKeyId: "key-id".to_owned(),
        StorageClass: "STANDARD_IA".to_owned(),
        ..Default::default()
    };
    S3Client::new(
        api,
        storeapi::NewBucketPrefix("bucket", "prefix/"),
        options,
        compatible,
    )
}

#[test]
/// 校验 Put/Get Range/批量删/List/Copy 的键与 Range 头映射。
fn put_range_batch_and_copy_inputs_match_go() {
    let api = Arc::new(MockS3::default());
    let cli = client(api.clone(), true);
    let ctx = storeapi::Context::default();

    cli.PutObject(&ctx, "dir/file", b"abc").unwrap();
    let response = cli.GetObject(&ctx, "object", 0, 7).unwrap();
    cli.DeleteObjects(&ctx, &["a".to_owned(), "sub/b".to_owned()])
        .unwrap();
    let listed = cli
        .ListObjects(&ctx, "target", "after", Some("token"), 100)
        .unwrap();
    cli.CopyObject(
        &ctx,
        &s3like::CopyInput {
            FromLoc: storeapi::NewBucketPrefix("source", "from/"),
            FromKey: "file".to_owned(),
            ToKey: "copied".to_owned(),
        },
    )
    .unwrap();

    assert!(!response.IsFullRange);
    assert_eq!(response.ContentLength, Some(7));
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.puts[0].key, "prefix/dir/file");
    assert_eq!(calls.puts[0].body, b"abc");
    assert_eq!(calls.puts[0].acl.as_deref(), Some("private"));
    assert_eq!(calls.gets[0].range.as_deref(), Some("bytes=0-6"));
    assert_eq!(calls.delete_batches[0].keys, ["prefix/a", "prefix/sub/b"]);
    assert_eq!(calls.lists[0].prefix, "prefix/target");
    assert_eq!(calls.lists[0].start_after.as_deref(), Some("prefix/after"));
    assert_eq!(listed.Objects[0].Size, 11);
    assert_eq!(calls.copies[0].copy_source, "source/from/file");
    assert_eq!(calls.copies[0].key, "prefix/copied");
}

#[test]
/// Go 的 path.Join 会清理 CopySource 点段，int 到 int32 则保留强制转换语义。
fn copy_source_and_large_list_limit_match_go_conversions() {
    let api = Arc::new(MockS3::default());
    let cli = client(api.clone(), false);
    let ctx = storeapi::Context::default();

    cli.CopyObject(
        &ctx,
        &s3like::CopyInput {
            FromLoc: storeapi::NewBucketPrefix("source", "from/dir"),
            FromKey: "../file".to_owned(),
            ToKey: "copied".to_owned(),
        },
    )
    .unwrap();
    cli.ListObjects(&ctx, "", "", None, (i32::MAX as isize).saturating_add(1))
        .unwrap();

    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.copies[0].copy_source, "source/from/file");
    assert_eq!(calls.lists[0].max_keys, i32::MIN);
}

#[test]
/// Put 失败时仍执行 Delete 清理，且返回 Put 错误。
fn permission_cleanup_runs_and_preserves_put_error() {
    let api = Arc::new(MockS3::default());
    // 同时注入 Put/Delete 错误，断言返回值保留 Put 错误且两边都调用。
    *api.put_error.lock().unwrap() = Some(anyhow!("put denied"));
    *api.delete_error.lock().unwrap() = Some(anyhow!("delete denied"));
    let err = client(api.clone(), false)
        .CheckPutAndDeleteObject(&storeapi::Context::default())
        .unwrap_err();
    assert!(err.to_string().contains("put denied"));
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.puts.len(), 1);
    assert_eq!(calls.deletes.len(), 1);
}

#[test]
/// NotFound 等错误码视为不存在；AccessDenied 仍为错误。
fn standard_not_found_codes_are_absent_not_errors() {
    let _guard = crate::client_test::HEAD_OBJECT_METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let api = Arc::new(MockS3::default());
    let cli = client(api.clone(), false);
    // 标准「不存在」错误码应映射为 false，而非 Err。
    for code in ["NotFound", "NoSuchBucket", "NoSuchKey"] {
        *api.head_error.lock().unwrap() = Some(api_error(code, "missing"));
        assert!(
            !cli.IsObjectExists(&storeapi::Context::default(), "missing")
                .unwrap()
        );
    }
    *api.head_error.lock().unwrap() = Some(api_error("AccessDenied", "denied"));
    assert!(
        cli.IsObjectExists(&storeapi::Context::default(), "missing")
            .is_err()
    );
}

#[test]
/// MultipartWriter：失败 part 不入 Complete，成功 part 保持序号。
fn multipart_only_commits_successful_ordered_parts() {
    let api = Arc::new(MockS3::default());
    // part 2 首次失败后再成功：Complete 只含成功的 [1,2]。
    *api.upload_error_on_part.lock().unwrap() = Some(2);
    let mut writer = client(api.clone(), true)
        .MultipartWriter(&storeapi::Context::default(), "large")
        .unwrap();
    assert_eq!(
        writer.write(&storeapi::Context::default(), b"one").unwrap(),
        3
    );
    assert!(writer.write(&storeapi::Context::default(), b"two").is_err());
    *api.upload_error_on_part.lock().unwrap() = None;
    assert_eq!(
        writer
            .write(&storeapi::Context::default(), b"three")
            .unwrap(),
        5
    );
    writer.close(&storeapi::Context::default()).unwrap();

    let calls = api.calls.lock().unwrap();
    assert_eq!(
        calls
            .uploads
            .iter()
            .map(|p| p.part_number)
            .collect::<Vec<_>>(),
        [1, 2, 2]
    );
    assert_eq!(
        calls.completes[0]
            .parts
            .iter()
            .map(|p| p.part_number)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
/// Go multipartWriter 不引入额外 closed 状态：Close 后仍可续写并再次 Complete。
fn multipart_writer_preserves_go_post_close_behavior() {
    let api = Arc::new(MockS3::default());
    let ctx = storeapi::Context::default();
    let mut writer = client(api.clone(), false)
        .MultipartWriter(&ctx, "large")
        .unwrap();

    writer.write(&ctx, b"one").unwrap();
    writer.close(&ctx).unwrap();
    writer.write(&ctx, b"two").unwrap();
    writer.close(&ctx).unwrap();

    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.completes.len(), 2);
    assert_eq!(calls.completes[0].parts.len(), 1);
    assert_eq!(calls.completes[1].parts.len(), 2);
}

#[test]
/// MultipartUploader：并行上传后 Complete 按 PartNumber 排序。
fn managed_uploader_chunks_and_orders_parallel_parts() {
    let api = Arc::new(MockS3::default());
    let uploader = client(api.clone(), true).MultipartUploader("large", 3, 2);
    uploader
        .Upload(
            &storeapi::Context::default(),
            &mut Cursor::new(b"abcdefgh".to_vec()),
        )
        .unwrap();

    let calls = api.calls.lock().unwrap();
    let mut uploaded = calls
        .uploads
        .iter()
        .map(|part| part.part_number)
        .collect::<Vec<_>>();
    uploaded.sort_unstable();
    assert_eq!(uploaded, [1, 2, 3]);
    assert_eq!(
        calls.completes[0]
            .parts
            .iter()
            .map(|part| part.part_number)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
/// 对象锁探测、凭证来源与重试退避策略对齐 Go。
fn object_lock_credentials_and_retry_policy_match_go() {
    let api = Arc::new(MockS3::default());
    *api.object_lock.lock().unwrap() = true;
    let options = backuppb::S3 {
        Bucket: "bucket".to_owned(),
        ..Default::default()
    };
    assert!(IsObjectLockEnabled(api, &options));

    let static_options = backuppb::S3 {
        AccessKey: "ak".to_owned(),
        SecretAccessKey: "sk".to_owned(),
        SessionToken: "token".to_owned(),
        ..Default::default()
    };
    assert_eq!(credential_source(&static_options), CredentialSource::Static);
    assert_eq!(
        credential_source(&backuppb::S3::default()),
        CredentialSource::DefaultChain
    );
    assert_eq!(
        credential_source(&backuppb::S3 {
            Endpoint: "https://oss-cn.aliyuncs.com".to_owned(),
            ..Default::default()
        }),
        CredentialSource::AliyunMetadata
    );

    let retry = newRetryer();
    assert_eq!(retry.MaxAttempts(), 20);
    let total: Duration = (1..retry.MaxAttempts())
        .map(|attempt| retry.RetryDelay(attempt, &anyhow!("timeout")).unwrap())
        .sum();
    assert!(total > Duration::from_secs(7 * 60));
    assert!(total < Duration::from_secs(9 * 60));
    assert!(!retry.IsErrorRetryable(&anyhow!("169.254.169.254 context deadline exceeded")));
    assert!(retry.IsErrorRetryable(&anyhow!("normal err: context deadline exceeded")));
}

#[test]
/// NewS3Storage 默认注入 TiDB 20 次重试，并尊重调用方提供的 AWS 重试配置。
fn store_options_select_default_and_custom_retry_config() {
    let default_options = storeapi::Options::default();
    let default_retry = retry_config_for_options(&default_options);
    assert_eq!(default_retry.max_attempts(), 20);
    assert_eq!(default_retry.initial_backoff(), Duration::from_secs(1));
    assert_eq!(default_retry.max_backoff(), Duration::from_secs(32));
    assert!(retry_classifier_for_options(&default_options).is_some());

    let custom_options = storeapi::Options {
        S3Retryer: Some(Arc::new(SevenAttemptRetryer)),
        ..Default::default()
    };
    let custom_retry = retry_config_for_options(&custom_options);
    assert_eq!(custom_retry.max_attempts(), 7);
    assert_eq!(custom_retry.initial_backoff(), Duration::from_millis(250));
    assert_eq!(custom_retry.max_backoff(), Duration::from_secs(5));
    assert!(retry_classifier_for_options(&custom_options).is_some());
}

#[test]
/// storeapi 提供的共享 HTTP client 会进入 AWS 配置装载与 S3 service 配置路径。
fn store_options_preserve_injected_http_client() {
    let injected =
        storeapi::aws_smithy_runtime_api::client::http::SharedHttpClient::new(NeverHttpClient);
    let options = storeapi::Options {
        HTTPClient: Some(injected.clone()),
        ..Default::default()
    };
    let _selected = http_client_for_options(&options).expect("injected HTTP client");
}

#[test]
/// MemoryBody 读尽后可 close，且不破坏已读内容语义。
fn memory_body_closes_without_losing_read_semantics() {
    let mut body = MemoryBody::new(b"abc".to_vec());
    let mut out = String::new();
    std::io::Read::read_to_string(&mut body, &mut out).unwrap();
    assert_eq!(out, "abc");
    prefetch::reader::ReadCloser::close(&mut body).unwrap();
    let _ = Cursor::new(Vec::<u8>::new());
}

#[test]
/// AWS SDK Body 保持流式读取，不在 GetObject 返回前一次性物化整个对象。
fn aws_body_adapter_reads_incrementally_and_releases_on_close() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let mut body = AwsBody::new(ByteStream::from_static(b"abcdef"), runtime);
    let mut first = [0_u8; 2];
    assert_eq!(std::io::Read::read(&mut body, &mut first).unwrap(), 2);
    assert_eq!(&first, b"ab");
    prefetch::reader::ReadCloser::close(&mut body).unwrap();
    assert!(std::io::Read::read(&mut body, &mut first).is_err());
}
