// Copyright 2020 PingCAP, Inc.
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

// `s3store` 集成级单元测试。
//
// 通过内存中的 `MockS3`（实现 `S3API`）驱动 `s3like::Storage`，覆盖后端选项解析、
// 读写/删除/存在性、分片上传、Range/Seek 与读侧重试、WalkDir 分页列举、
// 远程锁、对象锁与重试策略等路径，对齐 Go `s3_test.go`。
#![allow(non_snake_case)]

// 测试目标 crate 由 `[[test]]` 按包名自动链接；此处别名为代码库常用的短名 `s3store`。
// The crate under test is auto-linked by its package name for `[[test]]` targets;
// alias it to the short name used across the porting codebase.
extern crate astersql_objstore_s3store as s3store;

use std::any::Any;
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use objstore::locking::{ErrLocked, LockMetaInput, TryLockRemote};
use objstore::parse::{
    BackendOptions, ParseBackend, S3BackendOptions as ParseS3BackendOptions, StorageBackend,
};
use prefetch::reader::ReadCloser;
use s3like::{ParseRangeInfo, RangeInfo};
use s3store::*;

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

#[derive(Debug)]
struct BucketRegionHttpClient(String);

#[derive(Debug)]
struct BucketRegionHttpConnector(String);

impl storeapi::aws_smithy_runtime_api::client::http::HttpConnector for BucketRegionHttpConnector {
    fn call(
        &self,
        _: storeapi::aws_smithy_runtime_api::client::orchestrator::HttpRequest,
    ) -> storeapi::aws_smithy_runtime_api::client::http::HttpConnectorFuture {
        let body = format!(
            "<LocationConstraint xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{}</LocationConstraint>",
            self.0
        );
        let mut response =
            storeapi::aws_smithy_runtime_api::client::orchestrator::HttpResponse::new(
                200.try_into().unwrap(),
                aws_sdk_s3::primitives::SdkBody::from(body),
            );
        response
            .headers_mut()
            .insert("x-amz-bucket-region", self.0.clone());
        storeapi::aws_smithy_runtime_api::client::http::HttpConnectorFuture::ready(Ok(response))
    }
}

impl storeapi::aws_smithy_runtime_api::client::http::HttpClient for BucketRegionHttpClient {
    fn http_connector(
        &self,
        _: &storeapi::aws_smithy_runtime_api::client::http::HttpConnectorSettings,
        _: &storeapi::aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
    ) -> storeapi::aws_smithy_runtime_api::client::http::SharedHttpConnector {
        storeapi::aws_smithy_runtime_api::client::http::SharedHttpConnector::new(
            BucketRegionHttpConnector(self.0.clone()),
        )
    }
}

#[derive(Debug)]
struct BucketRegionRedirectHttpClient;

#[derive(Debug)]
struct BucketRegionRedirectHttpConnector;

struct OperationOnlyRetryer(Arc<AtomicUsize>);

#[derive(Debug)]
struct OperationOnlyClassifier(Arc<AtomicUsize>);

impl storeapi::Retryer for OperationOnlyRetryer {
    fn retry_config(&self) -> aws_sdk_s3::config::retry::RetryConfig {
        aws_sdk_s3::config::retry::RetryConfig::standard().with_max_attempts(1)
    }

    fn retry_classifier(
        &self,
    ) -> Option<storeapi::aws_smithy_runtime_api::client::retries::classifiers::SharedRetryClassifier>
    {
        Some(
            storeapi::aws_smithy_runtime_api::client::retries::classifiers::SharedRetryClassifier::new(
                OperationOnlyClassifier(self.0.clone()),
            ),
        )
    }
}

impl storeapi::aws_smithy_runtime_api::client::retries::classifiers::ClassifyRetry
    for OperationOnlyClassifier
{
    fn classify_retry(
        &self,
        _: &storeapi::aws_smithy_runtime_api::client::interceptors::context::InterceptorContext,
    ) -> storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryAction {
        self.0.fetch_add(1, Ordering::SeqCst);
        storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryAction::NoActionIndicated
    }

    fn name(&self) -> &'static str {
        "operation-only retry classifier"
    }
}

#[derive(Clone)]
struct BucketRegionWarningWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BucketRegionWarningWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl storeapi::aws_smithy_runtime_api::client::http::HttpConnector
    for BucketRegionRedirectHttpConnector
{
    fn call(
        &self,
        _: storeapi::aws_smithy_runtime_api::client::orchestrator::HttpRequest,
    ) -> storeapi::aws_smithy_runtime_api::client::http::HttpConnectorFuture {
        let mut response =
            storeapi::aws_smithy_runtime_api::client::orchestrator::HttpResponse::new(
                301.try_into().unwrap(),
                aws_sdk_s3::primitives::SdkBody::empty(),
            );
        response
            .headers_mut()
            .insert("x-amz-bucket-region", "us-west-2");
        storeapi::aws_smithy_runtime_api::client::http::HttpConnectorFuture::ready(Ok(response))
    }
}

impl storeapi::aws_smithy_runtime_api::client::http::HttpClient for BucketRegionRedirectHttpClient {
    fn http_connector(
        &self,
        _: &storeapi::aws_smithy_runtime_api::client::http::HttpConnectorSettings,
        _: &storeapi::aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
    ) -> storeapi::aws_smithy_runtime_api::client::http::SharedHttpConnector {
        storeapi::aws_smithy_runtime_api::client::http::SharedHttpConnector::new(
            BucketRegionRedirectHttpConnector,
        )
    }
}

/// 记录 MockS3 收到的各类 API 调用入参，供断言键名、Range、ACL 等。
#[derive(Default)]
struct Calls {
    gets: Vec<GetObjectInput>,
    puts: Vec<PutObjectInput>,
    deletes: Vec<DeleteObjectInput>,
    lists: Vec<ListObjectsV2Input>,
    creates: Vec<CreateMultipartUploadInput>,
}

/// GetObject 响应体的行为模式：正常、慢读、失败计数、限量、总失败、交替失败。
#[derive(Clone)]
enum BodyMode {
    Normal,
    Slow,
    FailCount(Arc<AtomicI32>),
    Limited(usize),
    AlwaysFail,
    Alternating(Arc<AtomicUsize>),
}

impl Default for BodyMode {
    fn default() -> Self {
        Self::Normal
    }
}

/// 单次 GetObject 的预设结果：指定 body 模式、普通错误、API 错误或 Content-Range。
#[derive(Clone)]
enum GetPlan {
    Body(BodyMode),
    Error(String),
    ApiError(&'static str, &'static str),
    Range(Option<String>),
}

/// 单次 HeadObject 的预设：复制状态字符串、普通错误或 API 错误。
enum HeadPlan {
    Status(String),
    Error(String),
    ApiError(&'static str, &'static str),
}

/// 单次 ListObjectsV2 的预设：返回一页结果或错误。
enum ListPlan {
    Page(ListObjectsV2Output),
    Error(String),
}

/// 每次只吐出一个字节的慢速 body，用于验证流式读取。
struct SlowBody {
    data: Vec<u8>,
    position: usize,
}

impl Read for SlowBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.position == self.data.len() {
            return Ok(0);
        }
        output[0] = self.data[self.position];
        self.position += 1;
        Ok(1)
    }
}

impl ReadCloser for SlowBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 前 N 次 read 返回错误，之后正常读取，用于验证读侧重试。
struct FailCountBody {
    inner: Cursor<Vec<u8>>,
    remaining: Arc<AtomicI32>,
}

impl Read for FailCountBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.remaining.load(Ordering::SeqCst) > 0 {
            self.remaining.fetch_sub(1, Ordering::SeqCst);
            return Err(io::Error::other("mock read error"));
        }
        self.inner.read(output)
    }
}

impl ReadCloser for FailCountBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 累计读取超过 limit 后报错，模拟连接提前断开 / 不完整响应。
struct LimitedBody {
    inner: Cursor<Vec<u8>>,
    read: usize,
    limit: usize,
}

impl Read for LimitedBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(output)?;
        if self.read + count > self.limit {
            return Err(io::Error::other("read exceeded limit"));
        }
        self.read += count;
        Ok(count)
    }
}

impl ReadCloser for LimitedBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 每次 read 都失败的 body。
struct AlwaysFailBody;

impl Read for AlwaysFailBody {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("always fail read"))
    }
}

impl ReadCloser for AlwaysFailBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 偶数次 read 失败、奇数次成功，用于验证重试计数器重置。
struct AlternatingBody {
    inner: Cursor<Vec<u8>>,
    calls: Arc<AtomicUsize>,
}

impl Read for AlternatingBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call % 2 == 0 {
            Err(io::Error::other("mock read error"))
        } else {
            self.inner.read(output)
        }
    }
}

impl ReadCloser for AlternatingBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 可编排的 S3API mock：记录调用，并按队列吐出 get/put/delete/head/list 等计划结果。
struct MockS3 {
    calls: Mutex<Calls>,
    object: Mutex<Vec<u8>>,
    body_mode: Mutex<BodyMode>,
    get_plans: Mutex<VecDeque<GetPlan>>,
    put_errors: Mutex<VecDeque<String>>,
    delete_errors: Mutex<VecDeque<(Option<&'static str>, String)>>,
    head_plans: Mutex<VecDeque<HeadPlan>>,
    list_plans: Mutex<VecDeque<ListPlan>>,
    create_error: Mutex<Option<String>>,
    object_lock: Mutex<VecDeque<bool>>,
}

impl Default for MockS3 {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Calls::default()),
            object: Mutex::new(b"test".to_vec()),
            body_mode: Mutex::new(BodyMode::Normal),
            get_plans: Mutex::new(VecDeque::new()),
            put_errors: Mutex::new(VecDeque::new()),
            delete_errors: Mutex::new(VecDeque::new()),
            head_plans: Mutex::new(VecDeque::new()),
            list_plans: Mutex::new(VecDeque::new()),
            create_error: Mutex::new(None),
            object_lock: Mutex::new(VecDeque::new()),
        }
    }
}

/// 从 `bytes=START-` 形式的 Range 头解析起始偏移；解析失败视为 0。
fn range_start(range: Option<&str>) -> usize {
    range
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.split('-').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// 按 BodyMode 包装响应数据为对应的 ReadCloser。
fn make_body(mode: BodyMode, data: Vec<u8>) -> Box<dyn ReadCloser> {
    match mode {
        BodyMode::Normal => Box::new(MemoryBody::new(data)),
        BodyMode::Slow => Box::new(SlowBody { data, position: 0 }),
        BodyMode::FailCount(remaining) => Box::new(FailCountBody {
            inner: Cursor::new(data),
            remaining,
        }),
        BodyMode::Limited(limit) => Box::new(LimitedBody {
            inner: Cursor::new(data),
            read: 0,
            limit,
        }),
        BodyMode::AlwaysFail => Box::new(AlwaysFailBody),
        BodyMode::Alternating(calls) => Box::new(AlternatingBody {
            inner: Cursor::new(data),
            calls,
        }),
    }
}

// MockS3 对 S3API 的实现：先记录调用，再消费计划队列决定成功/失败与响应内容。
impl S3API for MockS3 {
    fn get_object(
        &self,
        _: &storeapi::Context,
        input: &GetObjectInput,
        _: RequestOptions,
    ) -> Result<GetObjectOutput> {
        // 消费一次 Get 计划：错误则立即返回，否则按 Range 切片对象并组装 body。
        self.calls.lock().unwrap().gets.push(input.clone());
        let plan = self.get_plans.lock().unwrap().pop_front();
        match &plan {
            Some(GetPlan::Error(message)) => return Err(anyhow!(message.clone())),
            Some(GetPlan::ApiError(code, message)) => return Err(api_error(*code, *message)),
            _ => {}
        }

        let source = self.object.lock().unwrap().clone();
        let start = range_start(input.range.as_deref()).min(source.len());
        let data = source[start..].to_vec();
        let mode = match &plan {
            Some(GetPlan::Body(mode)) => mode.clone(),
            _ => self.body_mode.lock().unwrap().clone(),
        };
        let content_range = match &plan {
            Some(GetPlan::Range(value)) => value.clone(),
            _ if input.range.is_some() && !source.is_empty() => Some(format!(
                "bytes {}-{}/{}",
                start,
                source.len() - 1,
                source.len()
            )),
            _ => None,
        };
        Ok(GetObjectOutput {
            body: make_body(mode, data),
            content_length: input.range.is_none().then_some(source.len() as i64),
            content_range,
        })
    }

    fn put_object(
        &self,
        _: &storeapi::Context,
        input: &PutObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.calls.lock().unwrap().puts.push(input.clone());
        if let Some(message) = self.put_errors.lock().unwrap().pop_front() {
            Err(anyhow!(message))
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
        match self.delete_errors.lock().unwrap().pop_front() {
            Some((Some(code), message)) => Err(api_error(code, message)),
            Some((None, message)) => Err(anyhow!(message)),
            None => Ok(()),
        }
    }

    fn head_object(
        &self,
        _: &storeapi::Context,
        _: &HeadObjectInput,
        _: RequestOptions,
    ) -> Result<HeadObjectOutput> {
        match self.head_plans.lock().unwrap().pop_front() {
            Some(HeadPlan::Status(replication_status)) => {
                Ok(HeadObjectOutput { replication_status })
            }
            Some(HeadPlan::Error(message)) => Err(anyhow!(message)),
            Some(HeadPlan::ApiError(code, message)) => Err(api_error(code, message)),
            None => Ok(HeadObjectOutput::default()),
        }
    }

    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsV2Input,
        _: RequestOptions,
    ) -> Result<ListObjectsV2Output> {
        self.calls.lock().unwrap().lists.push(input.clone());
        match self.list_plans.lock().unwrap().pop_front() {
            Some(ListPlan::Page(page)) => Ok(page),
            Some(ListPlan::Error(message)) => Err(anyhow!(message)),
            None => Ok(ListObjectsV2Output::default()),
        }
    }

    fn create_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CreateMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<CreateMultipartUploadOutput> {
        self.calls.lock().unwrap().creates.push(input.clone());
        if let Some(message) = self.create_error.lock().unwrap().take() {
            return Err(anyhow!(message));
        }
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
        Ok(UploadPartOutput {
            e_tag: Some(format!("etag-{}", input.part_number)),
        })
    }

    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &CompleteMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        Ok(())
    }

    fn abort_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &AbortMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        Ok(())
    }

    fn get_object_lock_configuration(
        &self,
        _: &storeapi::Context,
        _: &GetObjectLockConfigurationInput,
        _: RequestOptions,
    ) -> Result<bool> {
        Ok(self
            .object_lock
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(false))
    }
}

/// 测试用默认 S3 配置（固定 region/bucket/prefix 与 ACL/SSE/存储类别）。
fn options() -> backuppb::S3 {
    backuppb::S3 {
        Region: "us-west-2".to_owned(),
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        Acl: "acl".to_owned(),
        Sse: "sse".to_owned(),
        StorageClass: "sc".to_owned(),
        ..Default::default()
    }
}

/// 用默认 options 包装 MockS3 为可测的 Storage。
fn suite(api: Arc<MockS3>) -> s3like::Storage {
    NewS3StorageForTest(api, &options(), None)
}

/// 同上，并挂上访问流量统计（AccessStats）。
fn suite_with_rec(
    api: Arc<MockS3>,
    access: Arc<objectio::recording::AccessStats>,
) -> s3like::Storage {
    NewS3StorageForTest(api, &options(), Some(access))
}

/// 构造一页 ListObjectsV2 输出，含对象列表与是否截断/续传 token。
fn page(keys: &[(&str, i64)], truncated: bool, token: Option<&str>) -> ListObjectsV2Output {
    ListObjectsV2Output {
        next_continuation_token: token.map(str::to_owned),
        is_truncated: truncated,
        contents: keys
            .iter()
            .map(|(key, size)| ListedObject {
                key: (*key).to_owned(),
                size: *size,
            })
            .collect(),
    }
}

#[test]
/// 非法 S3BackendOptions（缺密钥、坏 endpoint）应在 ParseBackend 时报出对应错误。
fn test_apply() {
    let cases = [
        (
            ParseS3BackendOptions {
                region: "us-west-2".to_owned(),
                secret_access_key: "cd".to_owned(),
                ..Default::default()
            },
            "access_key not found",
        ),
        (
            ParseS3BackendOptions {
                region: "us-west-2".to_owned(),
                access_key: "ab".to_owned(),
                ..Default::default()
            },
            "secret_access_key not found",
        ),
        (
            ParseS3BackendOptions {
                endpoint: "12345".to_owned(),
                ..Default::default()
            },
            "scheme not found in endpoint",
        ),
        (
            ParseS3BackendOptions {
                endpoint: "http:12345".to_owned(),
                ..Default::default()
            },
            "host not found",
        ),
        (
            ParseS3BackendOptions {
                endpoint: "!http:12345".to_owned(),
                ..Default::default()
            },
            "relative URL without a base",
        ),
    ];
    for (s3, expected) in cases {
        let error = ParseBackend(
            "s3://bucket2/prefix/",
            Some(&BackendOptions {
                s3,
                ..Default::default()
            }),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
    }
}

#[test]
/// 合法选项应正确写入 region/endpoint/force_path_style 以及静态凭证字段。
fn test_apply_update() {
    let cases = [
        (ParseS3BackendOptions::default(), "", "", false),
        (
            ParseS3BackendOptions {
                region: "us-west-2".to_owned(),
                ..Default::default()
            },
            "us-west-2",
            "",
            false,
        ),
        (
            ParseS3BackendOptions {
                endpoint: "https://s3.us-west-2".to_owned(),
                ..Default::default()
            },
            "",
            "https://s3.us-west-2",
            false,
        ),
        (
            ParseS3BackendOptions {
                endpoint: "http://s3.us-west-2".to_owned(),
                ..Default::default()
            },
            "",
            "http://s3.us-west-2",
            false,
        ),
        (
            ParseS3BackendOptions {
                region: "us-west-2".to_owned(),
                provider: "ceph".to_owned(),
                force_path_style: true,
                ..Default::default()
            },
            "us-west-2",
            "",
            true,
        ),
        (
            ParseS3BackendOptions {
                region: "us-west-2".to_owned(),
                provider: "alibaba".to_owned(),
                force_path_style: true,
                ..Default::default()
            },
            "us-west-2",
            "",
            false,
        ),
    ];
    for (s3_options, region, endpoint, force_path_style) in cases {
        let backend = ParseBackend(
            "s3://bucket/prefix/",
            Some(&BackendOptions {
                s3: s3_options,
                ..Default::default()
            }),
        )
        .unwrap();
        let StorageBackend::S3(s3) = backend else {
            panic!("expected S3 backend")
        };
        assert_eq!(s3.bucket, "bucket");
        assert_eq!(s3.prefix, "prefix");
        assert_eq!(s3.region, region);
        assert_eq!(s3.endpoint, endpoint);
        assert_eq!(s3.force_path_style, force_path_style);
    }

    let backend = ParseBackend(
        "s3://bucket/prefix/",
        Some(&BackendOptions {
            s3: ParseS3BackendOptions {
                region: "us-west-2".to_owned(),
                access_key: "ab".to_owned(),
                secret_access_key: "cd".to_owned(),
                session_token: "ef".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        }),
    )
    .unwrap();
    let StorageBackend::S3(s3) = backend else {
        unreachable!()
    };
    assert_eq!(
        (s3.access_key, s3.secret_access_key, s3.session_token),
        ("ab".into(), "cd".into(), "ef".into())
    );
}

#[test]
/// 凭证来源判定与 NewS3StorageForTest 后 options 字段可见性。
fn test_s3_storage() {
    let explicit = backuppb::S3 {
        AccessKey: "ab".to_owned(),
        SecretAccessKey: "cd".to_owned(),
        ..options()
    };
    assert_eq!(credential_source(&explicit), CredentialSource::Static);
    assert_eq!(
        credential_source(&options()),
        CredentialSource::DefaultChain
    );
    assert_eq!(DEFAULT_REGION, "us-east-1");

    let storage = NewS3StorageForTest(Arc::new(MockS3::default()), &explicit, None);
    assert_eq!(storage.GetOptions().Region, "us-west-2");
    assert_eq!(storage.GetOptions().AccessKey, "ab");
}

#[test]
/// URI 应拼出 `s3://bucket/prefix/`。
fn test_s3_uri() {
    let storage = suite(Arc::new(MockS3::default()));
    assert_eq!(storage.URI(), "s3://bucket/prefix/");
}

#[test]
/// Content-Range 解析：合法区间与空/非法字符串的错误信息。
fn test_s3_range() {
    assert_eq!(
        ParseRangeInfo(Some("bytes 0-9/443")).unwrap(),
        RangeInfo {
            Start: 0,
            End: 9,
            Size: 443
        }
    );
    assert!(
        ParseRangeInfo(None)
            .unwrap_err()
            .to_string()
            .contains("ContentRange is empty")
    );
    assert!(
        ParseRangeInfo(Some("bytes "))
            .unwrap_err()
            .to_string()
            .contains("invalid content range")
    );
}

#[test]
/// 成功 WriteFile：校验 Put 入参键前缀、ACL/SSE/存储类别与写流量统计。
fn test_write_no_error() {
    let api = Arc::new(MockS3::default());
    let access = Arc::new(objectio::recording::AccessStats::default());
    let storage = suite_with_rec(api.clone(), access.clone());
    storage
        .WriteFile(&storeapi::Context::default(), "file", b"test")
        .unwrap();
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.puts.len(), 1);
    assert_eq!(calls.puts[0].bucket, "bucket");
    assert_eq!(calls.puts[0].key, "prefix/file");
    assert_eq!(calls.puts[0].body, b"test");
    assert_eq!(calls.puts[0].acl.as_deref(), Some("acl"));
    assert_eq!(calls.puts[0].server_side_encryption.as_deref(), Some("sse"));
    assert_eq!(calls.puts[0].storage_class.as_deref(), Some("sc"));
    assert_eq!(access.traffic.write.load(Ordering::Relaxed), 4);
}

#[test]
/// 分片上传创建失败时，close 应返回原始错误且不抹掉已累计的写流量。
fn test_multi_upload_error_not_overwritten() {
    let api = Arc::new(MockS3::default());
    *api.create_error.lock().unwrap() = Some("mock error".to_owned());
    let access = Arc::new(objectio::recording::AccessStats::default());
    let storage = suite_with_rec(api, access.clone());
    let ctx = storeapi::Context::default();
    let mut writer = storage
        .Create(
            ctx.clone(),
            "file",
            Some(&storeapi::WriterOption {
                Concurrency: 2,
                PartSize: 1024,
            }),
        )
        .unwrap();
    let data = vec![7_u8; 1024 + 6716];
    assert_eq!(writer.write(&ctx, &data).unwrap(), data.len());
    let error = writer.close(&ctx).unwrap_err();
    assert!(error.to_string().contains("mock error"));
    assert_eq!(
        access.traffic.write.load(Ordering::Relaxed),
        data.len() as u64
    );
}

#[test]
/// 成功 ReadFile：键带 prefix，读流量计入 AccessStats。
fn test_read_no_error() {
    let api = Arc::new(MockS3::default());
    let access = Arc::new(objectio::recording::AccessStats::default());
    let storage = suite_with_rec(api.clone(), access.clone());
    assert_eq!(
        storage
            .ReadFile(&storeapi::Context::default(), "file")
            .unwrap(),
        b"test"
    );
    assert_eq!(api.calls.lock().unwrap().gets[0].key, "prefix/file");
    assert_eq!(access.traffic.read.load(Ordering::Relaxed), 4);
}

#[test]
/// Head 成功时 FileExists 为真。
fn test_file_exists_no_error() {
    let api = Arc::new(MockS3::default());
    api.head_plans
        .lock()
        .unwrap()
        .push_back(HeadPlan::Status(String::new()));
    assert!(
        suite(api)
            .FileExists(&storeapi::Context::default(), "file")
            .unwrap()
    );
}

#[test]
/// 复制状态 COMPLETED 时 FileSynced 为真。
fn test_file_synced_no_error() {
    let api = Arc::new(MockS3::default());
    api.head_plans
        .lock()
        .unwrap()
        .push_back(HeadPlan::Status("COMPLETED".to_owned()));
    assert!(
        suite(api)
            .FileSynced(&storeapi::Context::default(), "file")
            .unwrap()
    );
}

#[test]
/// 复制状态 PENDING 时 FileSynced 为假。
fn test_file_synced_pending() {
    let api = Arc::new(MockS3::default());
    api.head_plans
        .lock()
        .unwrap()
        .push_back(HeadPlan::Status("PENDING".to_owned()));
    assert!(
        !suite(api)
            .FileSynced(&storeapi::Context::default(), "file")
            .unwrap()
    );
}

#[test]
/// 空复制状态应报错（状态为空）。
fn test_file_synced_empty_status() {
    let api = Arc::new(MockS3::default());
    api.head_plans
        .lock()
        .unwrap()
        .push_back(HeadPlan::Status(String::new()));
    let error = suite(api)
        .FileSynced(&storeapi::Context::default(), "file")
        .unwrap_err();
    assert!(error.to_string().contains("is empty"));
}

#[test]
/// 成功删除时 Delete 键应带 prefix。
fn test_delete_file_no_error() {
    let api = Arc::new(MockS3::default());
    suite(api.clone())
        .DeleteFile(&storeapi::Context::default(), "file")
        .unwrap();
    assert_eq!(api.calls.lock().unwrap().deletes[0].key, "prefix/file");
}

#[test]
/// NoSuchKey 删除错误应原样传播。
fn test_delete_file_missing() {
    let api = Arc::new(MockS3::default());
    api.delete_errors
        .lock()
        .unwrap()
        .push_back((Some("NoSuchKey"), "no such key".to_owned()));
    assert_eq!(
        suite(api)
            .DeleteFile(&storeapi::Context::default(), "file-missing")
            .unwrap_err()
            .to_string(),
        "NoSuchKey: no such key"
    );
}

#[test]
/// 非 NoSuchKey 的删除错误应包含原始消息。
fn test_delete_file_error() {
    let api = Arc::new(MockS3::default());
    api.delete_errors
        .lock()
        .unwrap()
        .push_back((None, "just some unrelated error".to_owned()));
    assert!(
        suite(api)
            .DeleteFile(&storeapi::Context::default(), "file3")
            .unwrap_err()
            .to_string()
            .contains("just some unrelated error")
    );
}

#[test]
/// Head 返回 NoSuchKey 时 FileExists 为假（非错误）。
fn test_file_exists_missing() {
    let api = Arc::new(MockS3::default());
    api.head_plans
        .lock()
        .unwrap()
        .push_back(HeadPlan::ApiError("NoSuchKey", "no such key"));
    assert!(
        !suite(api)
            .FileExists(&storeapi::Context::default(), "file-missing")
            .unwrap()
    );
}

#[test]
/// Put 失败（如 NoSuchBucket）应向上返回。
fn test_write_error() {
    let api = Arc::new(MockS3::default());
    api.put_errors
        .lock()
        .unwrap()
        .push_back("NoSuchBucket: no such bucket".to_owned());
    assert!(
        suite(api)
            .WriteFile(&storeapi::Context::default(), "file2", b"test")
            .unwrap_err()
            .to_string()
            .contains("NoSuchBucket")
    );
}

#[test]
/// Get 失败时应在错误中附带 bucket/key 上下文。
fn test_read_error() {
    let api = Arc::new(MockS3::default());
    api.get_plans
        .lock()
        .unwrap()
        .push_back(GetPlan::ApiError("NoSuchKey", "no such key"));
    let error = suite(api)
        .ReadFile(&storeapi::Context::default(), "file-missing")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("input.bucket='bucket', input.key='prefix/file-missing'")
    );
}

#[test]
/// Head 非 NotFound 错误应向上返回。
fn test_file_exists_error() {
    let api = Arc::new(MockS3::default());
    api.head_plans
        .lock()
        .unwrap()
        .push_back(HeadPlan::Error("just some unrelated error".to_owned()));
    assert!(
        suite(api)
            .FileExists(&storeapi::Context::default(), "file3")
            .unwrap_err()
            .to_string()
            .contains("just some unrelated error")
    );
}

#[test]
/// Open 全量读取纯文本，无 Range，读流量等于内容长度。
fn test_open_as_bufio() {
    let api = Arc::new(MockS3::default());
    *api.object.lock().unwrap() = b"plain text\ncontent".to_vec();
    let access = Arc::new(objectio::recording::AccessStats::default());
    let storage = suite_with_rec(api.clone(), access.clone());
    let mut reader = storage
        .Open(storeapi::Context::default(), "plain-text-file", None)
        .unwrap();
    let mut content = String::new();
    reader.read_to_string(&mut content).unwrap();
    reader.close().unwrap();
    assert_eq!(content, "plain text\ncontent");
    assert_eq!(api.calls.lock().unwrap().gets[0].range, None);
    assert_eq!(access.traffic.read.load(Ordering::Relaxed), 18);
}

#[test]
/// 慢速 body 下仍应完整读出全部字节。
fn test_open_read_slowly() {
    let api = Arc::new(MockS3::default());
    *api.object.lock().unwrap() = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_vec();
    *api.body_mode.lock().unwrap() = BodyMode::Slow;
    let mut reader = suite(api)
        .Open(storeapi::Context::default(), "alphabets", None)
        .unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ");
}

#[test]
/// Seek/再读会触发带 Range 的后续 Get；越过文件尾应钳制到 EOF。
fn test_open_seek() {
    let api = Arc::new(MockS3::default());
    let data: Vec<u8> = (0..1_000_000).map(|index| (index % 251) as u8).collect();
    *api.object.lock().unwrap() = data.clone();
    let mut reader = suite(api.clone())
        .Open(storeapi::Context::default(), "random", None)
        .unwrap();
    let mut sample = [0_u8; 100];
    reader.read_exact(&mut sample).unwrap();
    assert_eq!(&sample, &data[..100]);
    assert_eq!(reader.seek(SeekFrom::Start(2000)).unwrap(), 2000);
    reader.read_exact(&mut sample).unwrap();
    assert_eq!(&sample, &data[2000..2100]);
    assert_eq!(reader.seek(SeekFrom::End(-2000)).unwrap(), 998_000);
    reader.read_exact(&mut sample).unwrap();
    assert_eq!(&sample, &data[998_000..998_100]);
    assert!(reader.seek(SeekFrom::Start(u64::MAX)).is_err());
    assert_eq!(reader.seek(SeekFrom::Current(-8000)).unwrap(), 990_100);
    reader.read_exact(&mut sample).unwrap();
    assert_eq!(&sample, &data[990_100..990_200]);
    for position in [1_000_000, 1_000_001, 2_000_000] {
        assert_eq!(reader.seek(SeekFrom::Start(position)).unwrap(), 1_000_000);
        assert_eq!(reader.read(&mut sample).unwrap(), 0);
    }
    let calls = &api.calls.lock().unwrap().gets;
    assert_eq!(calls[0].range, None);
    assert_eq!(calls[1].range.as_deref(), Some("bytes=998000-"));
    assert_eq!(calls[2].range.as_deref(), Some("bytes=990100-"));
}

/// 准备带起始偏移的读侧重试场景：先成功读 2 字节，可选取消 ctx。
fn prepare_retry_reader(
    cancelled: bool,
) -> (
    Box<dyn objectio::Reader>,
    Arc<AtomicI32>,
    Arc<MockS3>,
    storeapi::Context,
) {
    let api = Arc::new(MockS3::default());
    *api.object.lock().unwrap() = b"0123456789".to_vec();
    let failures = Arc::new(AtomicI32::new(0));
    *api.body_mode.lock().unwrap() = BodyMode::FailCount(failures.clone());
    let ctx = storeapi::Context::default();
    let mut reader = suite(api.clone())
        .Open(
            ctx.clone(),
            "random",
            Some(&storeapi::ReaderOption {
                StartOffset: Some(3),
                ..Default::default()
            }),
        )
        .unwrap();
    let mut first = [0_u8; 2];
    reader.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"34");
    if cancelled {
        ctx.cancel();
    }
    (reader, failures, api, ctx)
}

#[test]
/// 未取消时可重试读；ctx 已取消则不重试并保留首次失败。
fn test_s3_range_reader_retry_read_and_un_retryable_case() {
    let (mut reader, failures, api, _) = prepare_retry_reader(false);
    failures.store(1, Ordering::SeqCst);
    let mut next = [0_u8; 2];
    reader.read_exact(&mut next).unwrap();
    assert_eq!(&next, b"56");
    assert_eq!(api.calls.lock().unwrap().gets.len(), 2);

    let (mut reader, failures, api, _) = prepare_retry_reader(true);
    failures.store(1, Ordering::SeqCst);
    let error = reader.read(&mut next).unwrap_err();
    assert!(error.to_string().contains("mock read error"));
    assert_eq!(api.calls.lock().unwrap().gets.len(), 1);
}

#[test]
/// Limited body 触发提前 EOF 后，应按已读偏移发起后续 Range Get。
fn test_s3_reader_with_retry_eof() {
    let api = Arc::new(MockS3::default());
    let data: Vec<u8> = (0..100).map(|value| value as u8).collect();
    *api.object.lock().unwrap() = data.clone();
    *api.body_mode.lock().unwrap() = BodyMode::Limited(30);
    let mut reader = suite(api.clone())
        .Open(storeapi::Context::default(), "random", None)
        .unwrap();
    let mut offset = 0;
    for count in [20, 15, 15, 25, 20, 5] {
        let mut output = vec![0; count];
        reader.read_exact(&mut output).unwrap();
        assert_eq!(output, data[offset..offset + count]);
        offset += count;
    }
    assert_eq!(reader.read(&mut [0_u8; 30]).unwrap(), 0);
    let starts: Vec<_> = api
        .calls
        .lock()
        .unwrap()
        .gets
        .iter()
        .map(|call| call.range.clone())
        .collect();
    assert_eq!(
        starts,
        vec![
            None,
            Some("bytes=20-".to_owned()),
            Some("bytes=50-".to_owned()),
            Some("bytes=75-".to_owned())
        ]
    );
}

#[test]
/// AlwaysFail 耗尽重试后返回原始读错误，并留下多次 Get 调用。
fn test_s3_reader_with_retry_failed() {
    let api = Arc::new(MockS3::default());
    *api.object.lock().unwrap() = vec![0; 100];
    *api.body_mode.lock().unwrap() = BodyMode::AlwaysFail;
    let mut reader = suite(api.clone())
        .Open(storeapi::Context::default(), "random", None)
        .unwrap();
    let error = reader.read(&mut [0_u8; 100]).unwrap_err();
    assert_eq!(error.to_string(), "always fail read");
    assert_eq!(api.calls.lock().unwrap().gets.len(), 4);
}

#[test]
/// 交替失败成功时，成功读会重置重试计数，最终完成整文件读取。
fn test_s3_reader_reset_retry() {
    let api = Arc::new(MockS3::default());
    let data: Vec<u8> = (0..100).map(|value| value as u8).collect();
    *api.object.lock().unwrap() = data.clone();
    let body_calls = Arc::new(AtomicUsize::new(0));
    *api.body_mode.lock().unwrap() = BodyMode::Alternating(body_calls.clone());
    let mut reader = suite(api.clone())
        .Open(storeapi::Context::default(), "random", None)
        .unwrap();
    for index in 0..5 {
        let mut output = [0_u8; 20];
        reader.read_exact(&mut output).unwrap();
        assert_eq!(&output, &data[index * 20..(index + 1) * 20]);
    }
    assert_eq!(body_calls.load(Ordering::SeqCst), 10);
    assert_eq!(api.calls.lock().unwrap().gets.len(), 6);
}

#[test]
/// WalkDir 分页列举：子目录、前缀过滤与 ListCount 影响请求 prefix/max_keys。
fn test_walk_dir() {
    let api = Arc::new(MockS3::default());
    let objects = [
        ("prefix/sp/.gitignore", 437),
        ("prefix/sp/01.jpg", 27_499),
        ("prefix/sp/1-f.png", 32_507),
        ("prefix/sp/10-f.png", 549_735),
        ("prefix/sp/10-t.jpg", 44_151),
    ];
    let mut plans = api.list_plans.lock().unwrap();
    plans.push_back(ListPlan::Page(page(
        &objects[0..2],
        true,
        Some(objects[1].0),
    )));
    plans.push_back(ListPlan::Page(page(
        &objects[2..4],
        true,
        Some(objects[3].0),
    )));
    plans.push_back(ListPlan::Page(page(&objects[4..5], false, None)));
    plans.push_back(ListPlan::Page(page(
        &objects[0..4],
        true,
        Some(objects[3].0),
    )));
    plans.push_back(ListPlan::Page(page(&objects[4..5], false, None)));
    plans.push_back(ListPlan::Page(page(&objects[2..5], false, None)));
    drop(plans);
    let storage = suite(api.clone());
    let ctx = storeapi::Context::default();
    let mut seen = Vec::new();
    storage
        .WalkDir(
            &ctx,
            Some(&storeapi::WalkOption {
                SubDir: "sp".into(),
                ListCount: 2,
                ..Default::default()
            }),
            |path, size| {
                seen.push((path.to_owned(), size));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(seen.len(), 5);
    seen.clear();
    storage
        .WalkDir(
            &ctx,
            Some(&storeapi::WalkOption {
                ListCount: 4,
                ..Default::default()
            }),
            |path, size| {
                seen.push((path.to_owned(), size));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(seen.len(), 5);
    seen.clear();
    storage
        .WalkDir(
            &ctx,
            Some(&storeapi::WalkOption {
                SubDir: "sp".into(),
                ObjPrefix: "1".into(),
                ListCount: 3,
                ..Default::default()
            }),
            |path, size| {
                seen.push((path.to_owned(), size));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(seen.len(), 3);
    let calls = &api.calls.lock().unwrap().lists;
    assert_eq!(calls[0].prefix, "prefix/sp/");
    assert_eq!(calls[0].max_keys, 2);
    assert_eq!(calls[5].prefix, "prefix/sp/1");
}

#[test]
/// StartAfter 仅作用于首轮 List；后续页使用 continuation token。
fn test_walk_dir_with_start_after() {
    let api = Arc::new(MockS3::default());
    let objects = [
        ("prefix/sp/test_100", 437),
        ("prefix/sp/test_11", 437),
        ("prefix/sp/test_110", 437),
        ("prefix/sp/test_111", 437),
        ("prefix/sp/test_112", 437),
    ];
    let mut plans = api.list_plans.lock().unwrap();
    plans.push_back(ListPlan::Page(page(
        &objects[0..2],
        true,
        Some(objects[1].0),
    )));
    plans.push_back(ListPlan::Page(page(
        &objects[2..4],
        true,
        Some(objects[3].0),
    )));
    plans.push_back(ListPlan::Page(page(&objects[4..5], false, None)));
    drop(plans);
    let mut seen = Vec::new();
    suite(api.clone())
        .WalkDir(
            &storeapi::Context::default(),
            Some(&storeapi::WalkOption {
                SubDir: "sp".into(),
                ListCount: 2,
                StartAfter: "sp/test_10".into(),
                ..Default::default()
            }),
            |path, _| {
                seen.push(path.to_owned());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(seen.len(), 5);
    let calls = &api.calls.lock().unwrap().lists;
    assert_eq!(calls[0].start_after.as_deref(), Some("prefix/sp/test_10"));
    assert_eq!(calls[1].start_after, None);
}

#[test]
/// 空 Prefix 时根列举与 SubDir 列举的 prefix 拼接行为。
fn test_walk_dir_with_empty_prefix() {
    let api = Arc::new(MockS3::default());
    let mut no_prefix = options();
    no_prefix.Prefix.clear();
    api.list_plans
        .lock()
        .unwrap()
        .push_back(ListPlan::Page(page(
            &[("sp/.gitignore", 437), ("prefix/sp/01.jpg", 27_499)],
            false,
            None,
        )));
    api.list_plans
        .lock()
        .unwrap()
        .push_back(ListPlan::Page(page(&[("sp/.gitignore", 437)], false, None)));
    let storage = NewS3StorageForTest(api.clone(), &no_prefix, None);
    let mut all = Vec::new();
    storage
        .WalkDir(
            &storeapi::Context::default(),
            Some(&storeapi::WalkOption {
                ListCount: 2,
                ..Default::default()
            }),
            |path, _| {
                all.push(path.to_owned());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(all, ["sp/.gitignore", "prefix/sp/01.jpg"]);
    let mut subdir = Vec::new();
    storage
        .WalkDir(
            &storeapi::Context::default(),
            Some(&storeapi::WalkOption {
                SubDir: "sp".into(),
                ListCount: 2,
                ..Default::default()
            }),
            |path, _| {
                subdir.push(path.to_owned());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(subdir, ["sp/.gitignore"]);
    let calls = &api.calls.lock().unwrap().lists;
    assert_eq!(calls[0].prefix, "");
    assert_eq!(calls[1].prefix, "sp/");
}

/// 将 s3like::Storage 适配为 objstore::storage::Storage，供远程锁测试复用。
struct LockStorageAdapter(s3like::Storage);

impl objstore::storage::Storage for LockStorageAdapter {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn DeleteFile(&self, _: &objstore::storage::Context, name: &str) -> Result<()> {
        self.0.DeleteFile(&storeapi::Context::default(), name)
    }
    fn WriteFile(&self, _: &objstore::storage::Context, name: &str, data: &[u8]) -> Result<()> {
        self.0.WriteFile(&storeapi::Context::default(), name, data)
    }
    fn ReadFile(&self, _: &objstore::storage::Context, name: &str) -> Result<Vec<u8>> {
        self.0.ReadFile(&storeapi::Context::default(), name)
    }
    fn FileExists(&self, _: &objstore::storage::Context, name: &str) -> Result<bool> {
        self.0.FileExists(&storeapi::Context::default(), name)
    }
    fn Open(
        &self,
        _: &objstore::storage::Context,
        _: &str,
        _: Option<&objstore::storage::ReaderOption>,
    ) -> Result<Box<dyn objstore::storage::ObjectReader>> {
        Err(anyhow!("unused"))
    }
    fn WalkDir(
        &self,
        _: &objstore::storage::Context,
        option: Option<&objstore::storage::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        let mapped = option.map(|value| storeapi::WalkOption {
            SubDir: value.sub_dir.clone(),
            ObjPrefix: value.obj_prefix.clone(),
            SkipSubDir: value.skip_sub_dir,
            IncludeTombstone: value.include_tombstone,
            StartAfter: value.start_after.clone(),
            ..Default::default()
        });
        self.0.WalkDir(
            &storeapi::Context::default(),
            mapped.as_ref(),
            |path, size| callback(path, size),
        )
    }
    fn URI(&self) -> String {
        self.0.URI()
    }
    fn Create(
        &self,
        _: &objstore::storage::Context,
        _: &str,
        _: Option<&objstore::storage::WriterOption>,
    ) -> Result<Box<dyn objstore::storage::ObjectWriter>> {
        Err(anyhow!("unused"))
    }
    fn Rename(&self, _: &objstore::storage::Context, _: &str, _: &str) -> Result<()> {
        Err(anyhow!("unused"))
    }
    fn PresignFile(&self, _: &objstore::storage::Context, _: &str, _: Duration) -> Result<String> {
        Err(anyhow!("unused"))
    }
    fn Close(&self) {}
}

#[test]
/// 根路径（空 Prefix）下 TryLockRemote 初始检查失败时，list 前缀应为锁名本身。
fn test_try_lock_remote_root_path_prefix() {
    let api = Arc::new(MockS3::default());
    api.list_plans
        .lock()
        .unwrap()
        .push_back(ListPlan::Error("stop".to_owned()));
    api.get_plans
        .lock()
        .unwrap()
        .push_back(GetPlan::Error("no such key".to_owned()));
    let storage: objstore::storage::StorageRef = Arc::new(LockStorageAdapter({
        let mut root = options();
        root.Prefix.clear();
        NewS3StorageForTest(api.clone(), &root, None)
    }));
    let error = TryLockRemote(
        &objstore::storage::Context::background(),
        storage,
        "truncating.lock",
        LockMetaInput {
            hint: "hint".to_owned(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("during initial check"));
    assert!(error.downcast_ref::<ErrLocked>().is_none());
    assert_eq!(api.calls.lock().unwrap().lists[0].prefix, "truncating.lock");
}

#[test]
/// NewS3Storage 按 SendCredentials 保留或清空 AccessKey/Secret/SessionToken。
fn test_send_creds() {
    let backend = || backuppb::S3 {
        AccessKey: "ab".to_owned(),
        SecretAccessKey: "cd".to_owned(),
        SessionToken: "ef".to_owned(),
        // A non-AWS provider trusts the configured region, so this parity test
        // exercises construction without making an external bucket-region call.
        Provider: "ovh".to_owned(),
        ..options()
    };

    let mut sent = backend();
    let storage_options = |send_credentials| storeapi::Options {
        SendCredentials: send_credentials,
        HTTPClient: Some(
            storeapi::aws_smithy_runtime_api::client::http::SharedHttpClient::new(NeverHttpClient),
        ),
        ..Default::default()
    };

    NewS3Storage(
        &storeapi::Context::default(),
        &mut sent,
        &storage_options(true),
    )
    .unwrap();
    assert_eq!(
        (&sent.AccessKey, &sent.SecretAccessKey, &sent.SessionToken),
        (&"ab".to_owned(), &"cd".to_owned(), &"ef".to_owned())
    );

    let mut not_sent = backend();
    NewS3Storage(
        &storeapi::Context::default(),
        &mut not_sent,
        &storage_options(false),
    )
    .unwrap();
    assert_eq!(
        (
            &not_sent.AccessKey,
            &not_sent.SecretAccessKey,
            &not_sent.SessionToken
        ),
        (&String::new(), &String::new(), &String::new())
    );
}

#[test]
/// IsObjectLockEnabled 按队列依次返回 mock 的对象锁开关。
fn test_object_lock() {
    let api = Arc::new(MockS3::default());
    api.object_lock
        .lock()
        .unwrap()
        .extend([false, false, false, false, true]);
    let expected = [false, false, false, false, true];
    for value in expected {
        assert_eq!(IsObjectLockEnabled(api.clone(), &options()), value);
    }
}

#[test]
/// NewS3Storage 使用 AWS 探测到的 region，其他 provider 信任配置值。
fn test_s3_storage_bucket_region() {
    let cases = [
        ("", "aws", DEFAULT_REGION),
        ("sdg", "ovh", "sdg"),
        ("", "ovh", ""),
        ("us-west-2", "aws", "us-west-2"),
    ];
    for (region, provider, expected) in cases {
        let mut backend = backuppb::S3 {
            Region: region.to_owned(),
            Bucket: "bucket".to_owned(),
            Prefix: "prefix".to_owned(),
            Provider: provider.to_owned(),
            AccessKey: "ab".to_owned(),
            SecretAccessKey: "cd".to_owned(),
            Endpoint: "http://s3.test".to_owned(),
            ForcePathStyle: true,
            ..Default::default()
        };
        let storage = NewS3Storage(
            &storeapi::Context::default(),
            &mut backend,
            &storeapi::Options {
                HTTPClient: Some(
                    storeapi::aws_smithy_runtime_api::client::http::SharedHttpClient::new(
                        BucketRegionHttpClient(expected.to_owned()),
                    ),
                ),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(storage.GetOptions().Region, expected);
    }
}

#[test]
fn go_commit_c50aae2b1b_region_probe_uses_301_bucket_region_header() {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let operation_retry_calls = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer({
            let warnings = warnings.clone();
            move || BucketRegionWarningWriter(warnings.clone())
        })
        .finish();
    let mut backend = backuppb::S3 {
        Bucket: "bucket".to_owned(),
        AccessKey: "ab".to_owned(),
        SecretAccessKey: "cd".to_owned(),
        Endpoint: "http://s3.test".to_owned(),
        ForcePathStyle: true,
        ..Default::default()
    };
    let result = tracing::subscriber::with_default(subscriber, || {
        NewS3Storage(
            &storeapi::Context::default(),
            &mut backend,
            &storeapi::Options {
                HTTPClient: Some(
                    storeapi::aws_smithy_runtime_api::client::http::SharedHttpClient::new(
                        BucketRegionRedirectHttpClient,
                    ),
                ),
                S3Retryer: Some(Arc::new(OperationOnlyRetryer(
                    operation_retry_calls.clone(),
                ))),
                ..Default::default()
            },
        )
    });
    let storage = result.unwrap();
    assert_eq!(storage.GetOptions().Region, "us-west-2");
    assert_eq!(operation_retry_calls.load(Ordering::SeqCst), 0);
    let _ = storage.FileExists(&storeapi::Context::default(), "object");
    assert!(operation_retry_calls.load(Ordering::SeqCst) > 0);
    let warnings = String::from_utf8(warnings.lock().unwrap().clone()).unwrap();
    assert!(
        !warnings.contains("failed to request s3, checking whether we can retry"),
        "{warnings}"
    );
}

#[test]
/// 自定义 endpoint + ForcePathStyle 应保留在 GetOptions 中。
fn test_s3_storage_custom_aws_endpoint_with_fips_mode() {
    let configured = backuppb::S3 {
        Region: "us-west-2".into(),
        Provider: "aws".into(),
        Endpoint: "http://127.0.0.1:9000".into(),
        ForcePathStyle: true,
        ..options()
    };
    let storage = NewS3StorageForTest(Arc::new(MockS3::default()), &configured, None);
    assert_eq!(storage.GetOptions().Region, "us-west-2");
    assert_eq!(storage.GetOptions().Endpoint, "http://127.0.0.1:9000");
    assert!(storage.GetOptions().ForcePathStyle);
}

#[test]
/// connection reset 可重试；MaxAttempts 与 RetryDelay(0) 符合常量约定。
fn test_retry_error() {
    let retryer = newRetryer();
    let reset = anyhow!("read tcp 127.0.0.1: read: connection reset by peer");
    assert!(retryer.IsErrorRetryable(&reset));
    assert_eq!(retryer.MaxAttempts(), MAX_ATTEMPTS);
    assert_eq!(
        retryer.RetryDelay(0, &reset).unwrap(),
        Duration::from_secs(1)
    );
}

#[test]
/// 读取成功路径可配合 logger 吞掉无 checksum 的调试日志（回归静默行为）。
fn test_s3_read_file_suppresses_skipped_checksum_validation_log() {
    let api = Arc::new(MockS3::default());
    *api.object.lock().unwrap() = b"payload".to_vec();
    assert_eq!(
        suite(api)
            .ReadFile(&storeapi::Context::default(), "object")
            .unwrap(),
        b"payload"
    );
    newLogger().Logf(Classification::Debug, "Response has no supported checksum");
}

#[test]
/// ReadFile 在 body 失败后继续重试，最终返回非重试类错误。
fn test_s3_read_file_retryable() {
    let api = Arc::new(MockS3::default());
    api.get_plans.lock().unwrap().extend([
        GetPlan::Body(BodyMode::AlwaysFail),
        GetPlan::Body(BodyMode::AlwaysFail),
        GetPlan::Error("just some unrelated error".to_owned()),
    ]);
    let error = suite(api)
        .ReadFile(&storeapi::Context::default(), "file")
        .unwrap_err();
    assert!(error.to_string().contains("just some unrelated error"));
}

#[test]
/// Open 时服务端 Content-Range 与请求区间不一致（或为空）应给出明确错误。
fn test_open_range_mismatch_error_msg() {
    let api = Arc::new(MockS3::default());
    *api.object.lock().unwrap() = vec![0; 20];
    api.get_plans
        .lock()
        .unwrap()
        .push_back(GetPlan::Range(Some("bytes 10-20/20".to_owned())));
    let option = storeapi::ReaderOption {
        StartOffset: Some(10),
        EndOffset: Some(30),
        ..Default::default()
    };
    let error = suite(api.clone())
        .Open(storeapi::Context::default(), "test", Some(&option))
        .err()
        .expect("range mismatch");
    assert!(
        error
            .to_string()
            .contains("expected range: [10,30), got: bytes 10-20/20")
    );
    api.get_plans
        .lock()
        .unwrap()
        .push_back(GetPlan::Range(None));
    let error = suite(api)
        .Open(storeapi::Context::default(), "test", Some(&option))
        .err()
        .expect("empty range");
    assert!(error.to_string().contains("ContentRange is empty"));
}
