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

// OSS 底层 API 抽象：请求/响应类型、`API` trait 与阿里云 `AliyunOssApi` 实现。
//
// 对应 Go ossstore 对阿里云 OSS SDK 的封装；支持对象 CRUD、列举、拷贝以及
// 分片上传（Multipart Upload）。`execute` 结合 `OssRetryer` 做可取消重试。

use std::fmt;
use std::io::{self, Cursor, Read};
use std::sync::Arc;

use ali_oss_rs::blocking::bucket::BucketOperations;
use ali_oss_rs::blocking::multipart::MultipartUploadsOperations;
use ali_oss_rs::blocking::object::ObjectOperations;
use anyhow::{Result, anyhow};

use crate::{CredentialsProvider, OssRetryer};

/// ListObjectsV2 请求：按前缀分页列举对象。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListObjectsV2Input {
    /// 目标桶名。
    pub bucket: String,
    /// 键前缀过滤。
    pub prefix: String,
    /// 单页最大返回数。
    pub max_keys: i32,
    /// 上一页返回的续传令牌。
    pub continuation_token: Option<String>,
    /// 从该键之后开始列举（不含该键）。
    pub start_after: Option<String>,
}

/// 列举结果中的单个对象摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListedObject {
    /// 对象完整键。
    pub key: String,
    /// 对象字节大小。
    pub size: i64,
}

/// ListObjectsV2 响应：对象列表与分页信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListObjectsV2Output {
    /// 下一页续传令牌；无更多页时为 `None`。
    pub next_continuation_token: Option<String>,
    /// 是否还有后续页。
    pub is_truncated: bool,
    /// 本页对象列表。
    pub contents: Vec<ListedObject>,
}

/// GetObject 请求：按键读取对象，可选 Range。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GetObjectInput {
    /// 目标桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// HTTP Range，如 `bytes=0-9`。
    pub range: Option<String>,
}

/// GetObject 响应：可读关闭的 body 与长度/Range 元数据。
pub struct GetObjectOutput {
    /// 对象内容流；调用方负责 `close`。
    pub body: Box<dyn prefetch::reader::ReadCloser>,
    /// Content-Length（若可知）。
    pub content_length: Option<i64>,
    /// Content-Range（部分读取时）。
    pub content_range: Option<String>,
}

impl GetObjectOutput {
    /// 用内存字节构造输出，便于测试与缓冲读取路径。
    pub fn from_bytes(data: Vec<u8>, content_range: Option<String>) -> Self {
        Self {
            content_length: i64::try_from(data.len()).ok(),
            body: Box::new(MemoryBody::new(data)),
            content_range,
        }
    }
}

/// Reconstruct the response `Content-Range` that the Go SDK exposes.
///
/// `ali-oss-rs` 0.2.x discards response headers when downloading to a buffer,
/// so ranged reads combine the requested range, downloaded length, and a HEAD
/// result containing the object's total length.
pub(crate) fn content_range_for_download(
    requested_range: Option<&str>,
    downloaded_len: usize,
    object_len: u64,
) -> Result<Option<String>> {
    let Some(requested_range) = requested_range else {
        return Ok(None);
    };
    let bounds = requested_range
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow!("invalid OSS request range {requested_range:?}"))?;
    let (start, requested_end) = bounds
        .split_once('-')
        .ok_or_else(|| anyhow!("invalid OSS request range {requested_range:?}"))?;
    let start = start
        .parse::<u64>()
        .map_err(|error| anyhow!("invalid OSS range start {start:?}: {error}"))?;
    let expected_end = if requested_end.is_empty() {
        object_len.checked_sub(1)
    } else {
        let requested_end = requested_end
            .parse::<u64>()
            .map_err(|error| anyhow!("invalid OSS range end {requested_end:?}: {error}"))?;
        object_len
            .checked_sub(1)
            .map(|last| requested_end.min(last))
    }
    .ok_or_else(|| anyhow!("OSS returned an empty object for range {requested_range:?}"))?;
    let expected_len = expected_end
        .checked_sub(start)
        .and_then(|span| span.checked_add(1))
        .ok_or_else(|| anyhow!("invalid OSS response range for {requested_range:?}"))?;
    if u64::try_from(downloaded_len).ok() != Some(expected_len) {
        return Err(anyhow!(
            "OSS range {requested_range:?} expected {expected_len} bytes, received {downloaded_len}"
        ));
    }
    Ok(Some(format!("bytes {start}-{expected_end}/{object_len}")))
}

/// PutObject 请求：整对象上传。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PutObjectInput {
    /// 目标桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// 待上传内容。
    pub body: Vec<u8>,
}

/// DeleteObject 请求：删除单个对象。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteObjectInput {
    /// 目标桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
}

/// DeleteMultipleObjects 请求：批量删除。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteObjectsInput {
    /// 目标桶名。
    pub bucket: String,
    /// 待删对象键列表。
    pub keys: Vec<String>,
}

/// HeadObject 请求：仅探测对象是否存在/元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeadObjectInput {
    /// 目标桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
}

/// CopyObject 请求：服务端跨键（可跨桶）拷贝。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopyObjectInput {
    /// 目标桶。
    pub bucket: String,
    /// 目标键。
    pub key: String,
    /// 源桶。
    pub source_bucket: String,
    /// 源键。
    pub source_key: String,
}

/// 发起分片上传的请求（可带 SSE/存储类型选项）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateMultipartUploadInput {
    /// 目标桶。
    pub bucket: String,
    /// 目标键。
    pub key: String,
    /// 服务端加密算法，如 KMS。
    pub server_side_encryption: Option<String>,
    /// KMS 密钥 ID。
    pub sse_kms_key_id: Option<String>,
    /// 存储类型，如 IA（低频访问）。
    pub storage_class: Option<String>,
}

/// 发起分片上传的响应，含 `upload_id`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateMultipartUploadOutput {
    /// 桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// 本次分片会话 ID。
    pub upload_id: String,
}

/// 上传单个分片的请求。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UploadPartInput {
    /// 桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// 分片会话 ID。
    pub upload_id: String,
    /// 分片序号（从 1 起）。
    pub part_number: i32,
    /// 分片内容。
    pub body: Vec<u8>,
}

/// 上传分片响应：ETag 用于最终 Complete。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UploadPartOutput {
    /// 分片 ETag。
    pub etag: String,
}

/// 已完成分片的 ETag 与序号，提交 Complete 时使用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletedPart {
    /// 分片 ETag。
    pub etag: String,
    /// 分片序号。
    pub part_number: i32,
}

/// 完成分片上传的请求。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompleteMultipartUploadInput {
    /// 桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// 分片会话 ID。
    pub upload_id: String,
    /// 按序排列的已上传分片。
    pub parts: Vec<CompletedPart>,
}

/// 中止分片上传的请求。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AbortMultipartUploadInput {
    /// 桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// 分片会话 ID。
    pub upload_id: String,
}

/// 列举进行中分片的请求。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListPartsInput {
    /// 桶名。
    pub bucket: String,
    /// 对象键。
    pub key: String,
    /// 分片会话 ID。
    pub upload_id: String,
}

/// OSS 底层操作 trait；默认实现均返回「未实现」错误，便于 mock 只覆盖所需方法。
pub trait API: Send + Sync {
    /// 判断桶是否存在。
    fn is_bucket_exist(&self, _: &storeapi::Context, _: &str) -> Result<bool> {
        Err(anyhow!("OSS operation IsBucketExist is not implemented"))
    }
    /// 查询桶所在 region/location。
    fn bucket_location(&self, _: &storeapi::Context, _: &str) -> Result<String> {
        Err(anyhow!(
            "OSS operation GetBucketLocation is not implemented"
        ))
    }
    /// Head 对象：存在则 Ok，不存在通常为 NoSuchKey。
    fn head_object(&self, _: &storeapi::Context, _: &HeadObjectInput) -> Result<()> {
        Err(anyhow!("OSS operation HeadObject is not implemented"))
    }
    /// 下载对象（可带 Range）。
    fn get_object(&self, _: &storeapi::Context, _: &GetObjectInput) -> Result<GetObjectOutput> {
        Err(anyhow!("OSS operation GetObject is not implemented"))
    }
    /// 整对象上传。
    fn put_object(&self, _: &storeapi::Context, _: &PutObjectInput) -> Result<()> {
        Err(anyhow!("OSS operation PutObject is not implemented"))
    }
    /// 服务端拷贝对象。
    fn copy_object(&self, _: &storeapi::Context, _: &CopyObjectInput) -> Result<()> {
        Err(anyhow!("OSS operation CopyObject is not implemented"))
    }
    /// 删除单个对象。
    fn delete_object(&self, _: &storeapi::Context, _: &DeleteObjectInput) -> Result<()> {
        Err(anyhow!("OSS operation DeleteObject is not implemented"))
    }
    /// 批量删除对象。
    fn delete_objects(&self, _: &storeapi::Context, _: &DeleteObjectsInput) -> Result<()> {
        Err(anyhow!(
            "OSS operation DeleteMultipleObjects is not implemented"
        ))
    }
    /// 按前缀分页列举对象。
    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        _: &ListObjectsV2Input,
    ) -> Result<ListObjectsV2Output> {
        Err(anyhow!("OSS operation ListObjectsV2 is not implemented"))
    }
    /// 发起分片上传，返回 upload_id。
    fn initiate_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &CreateMultipartUploadInput,
    ) -> Result<CreateMultipartUploadOutput> {
        Err(anyhow!(
            "OSS operation InitiateMultipartUpload is not implemented"
        ))
    }
    /// 上传一个分片。
    fn upload_part(&self, _: &storeapi::Context, _: &UploadPartInput) -> Result<UploadPartOutput> {
        Err(anyhow!("OSS operation UploadPart is not implemented"))
    }
    /// 提交全部分片以合成最终对象。
    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &CompleteMultipartUploadInput,
    ) -> Result<()> {
        Err(anyhow!(
            "OSS operation CompleteMultipartUpload is not implemented"
        ))
    }
    /// 中止分片上传并清理已传分片。
    fn abort_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &AbortMultipartUploadInput,
    ) -> Result<()> {
        Err(anyhow!(
            "OSS operation AbortMultipartUpload is not implemented"
        ))
    }
    /// 列举某次分片上传已上传的分片。
    fn list_parts(&self, _: &storeapi::Context, _: &ListPartsInput) -> Result<Vec<CompletedPart>> {
        Err(anyhow!("OSS operation ListParts is not implemented"))
    }
}

/// 携带 OSS 错误码与消息的服务端错误，可从 `anyhow` 链中提取。
#[derive(Debug)]
pub(crate) struct OssServiceError {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl fmt::Display for OssServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for OssServiceError {}

/// 构造带 `code`/`message` 的 `OssServiceError` 并装箱为 `anyhow::Error`。
pub fn api_error(code: impl Into<String>, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(OssServiceError {
        code: code.into(),
        message: message.into(),
    })
}

/// 从错误链中提取 `OssServiceError.code`（若有）。
pub fn error_code(error: &anyhow::Error) -> Option<&str> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<OssServiceError>()
            .map(|e| e.code.as_str())
    })
}

/// 将 ali-oss-rs SDK 错误映射为带码的 `api_error`。
fn sdk_error(operation: &str, error: ali_oss_rs::error::Error) -> anyhow::Error {
    match error {
        ali_oss_rs::error::Error::ApiError(response) => api_error(response.code, response.message),
        // 保留 SDK/reqwest/io 的结构化错误链，供标准重试器按状态码与网络错误类型判断。
        other => anyhow::Error::new(other).context(operation.to_owned()),
    }
}

/// 内存中的可读 body，实现 `ReadCloser`；关闭后再读返回 BrokenPipe。
pub struct MemoryBody {
    inner: Cursor<Vec<u8>>,
    closed: bool,
}

impl MemoryBody {
    /// 用给定字节缓冲构造。
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(data),
            closed: false,
        }
    }
}

impl Read for MemoryBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "body is closed"));
        }
        self.inner.read(buffer)
    }
}

impl prefetch::reader::ReadCloser for MemoryBody {
    fn close(&mut self) -> io::Result<()> {
        self.closed = true;
        Ok(())
    }
}

/// 基于 ali-oss-rs 的阿里云 OSS `API` 实现，含凭证、重试与访问统计。
pub struct AliyunOssApi {
    /// 动态凭证提供者（可含刷新器）。
    credentials: Arc<dyn CredentialsProvider>,
    /// OSS endpoint，如 `https://oss-cn-hangzhou.aliyuncs.com`。
    endpoint: String,
    /// 区域 ID，如 `cn-hangzhou`。
    region: String,
    /// 共享 HTTP 客户端。
    http_client: reqwest::blocking::Client,
    /// 可选的对象访问统计记录器。
    access_rec: Option<Arc<objectio::recording::AccessStats>>,
}

impl AliyunOssApi {
    /// 构造 API 实现；内部创建 blocking reqwest 客户端。
    pub fn new(
        credentials: Arc<dyn CredentialsProvider>,
        endpoint: String,
        region: String,
        access_rec: Option<Arc<objectio::recording::AccessStats>>,
    ) -> Result<Self> {
        let http_client = reqwest::blocking::Client::builder().build()?;
        Ok(Self {
            credentials,
            endpoint,
            region,
            http_client,
            access_rec,
        })
    }

    /// 用当前凭证构建一次 SDK Client（含可选 STS token）。
    fn client(&self) -> Result<ali_oss_rs::blocking::Client> {
        let credential = self.credentials.get_credentials()?;
        let mut builder = ali_oss_rs::blocking::ClientBuilder::new(
            credential.access_key_id,
            credential.access_key_secret,
            &self.endpoint,
        )
        .region(&self.region)
        .client(self.http_client.clone());
        if !credential.security_token.is_empty() {
            builder = builder.sts_token(credential.security_token);
        }
        builder.build().map_err(|error| anyhow!(error))
    }

    /// 向访问统计记录一次给定 HTTP 方法的请求。
    fn record(&self, method: http::Method) {
        let request = http::Request::builder()
            .method(method)
            .body(())
            .expect("valid request");
        objectio::recording::AccessStats::rec_request(self.access_rec.as_deref(), Some(&request));
    }

    /// 在可重试错误上按 `OssRetryer` 退避重试，并响应 `ctx` 取消。
    fn execute<T>(
        &self,
        ctx: &storeapi::Context,
        mut operation: impl FnMut() -> Result<T>,
    ) -> Result<T> {
        let retryer = OssRetryer::default();
        for attempt in 1..=retryer.MaxAttempts() {
            ctx.check()?;
            match operation() {
                Ok(value) => return Ok(value),
                Err(error)
                    if attempt < retryer.MaxAttempts() && retryer.IsErrorRetryable(&error) =>
                {
                    // 分段 sleep，便于中途响应取消。
                    let delay = retryer.RetryDelay(attempt, &error)?;
                    let mut waited = std::time::Duration::ZERO;
                    while waited < delay {
                        ctx.check()?;
                        let step = (delay - waited).min(std::time::Duration::from_millis(50));
                        std::thread::sleep(step);
                        waited += step;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("OSS retry loop always returns")
    }
}

/// 将分片创建请求中的 SSE/存储类型选项转为 SDK `PutObjectOptions`。
fn put_options(
    input: &CreateMultipartUploadInput,
) -> Result<Option<ali_oss_rs::object_common::PutObjectOptions>> {
    let mut builder = ali_oss_rs::object_common::PutObjectOptionsBuilder::new();
    let mut set = false;
    if let Some(value) = &input.server_side_encryption {
        builder = builder.server_side_encryption(
            ali_oss_rs::common::ServerSideEncryptionAlgorithm::try_from(value.as_str())
                .map_err(|e| anyhow!(e))?,
        );
        set = true;
    }
    if let Some(value) = &input.sse_kms_key_id {
        builder = builder.server_side_encryption_key_id(value);
        set = true;
    }
    if let Some(value) = &input.storage_class {
        builder = builder.storage_class(
            ali_oss_rs::common::StorageClass::try_from(value.as_str()).map_err(|e| anyhow!(e))?,
        );
        set = true;
    }
    Ok(set.then(|| builder.build()))
}

impl API for AliyunOssApi {
    fn is_bucket_exist(&self, ctx: &storeapi::Context, bucket: &str) -> Result<bool> {
        ctx.check()?;
        self.record(http::Method::GET);
        match self.execute(ctx, || {
            self.client()?
                .get_bucket_info(bucket)
                .map_err(|e| sdk_error("GetBucketInfo", e))
        }) {
            Ok(_) => Ok(true),
            Err(error) => {
                // NoSuchBucket 视为不存在；其它错误上抛。
                if error_code(&error) == Some("NoSuchBucket") {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }
    }

    fn bucket_location(&self, ctx: &storeapi::Context, bucket: &str) -> Result<String> {
        ctx.check()?;
        self.record(http::Method::GET);
        self.execute(ctx, || {
            self.client()?
                .get_bucket_location(bucket)
                .map_err(|e| sdk_error("GetBucketLocation", e))
        })
    }

    fn head_object(&self, ctx: &storeapi::Context, input: &HeadObjectInput) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::HEAD);
        self.execute(ctx, || {
            self.client()?
                .head_object(&input.bucket, &input.key, None)
                .map(|_| ())
                .map_err(|e| sdk_error("HeadObject", e))
        })
    }

    fn get_object(
        &self,
        ctx: &storeapi::Context,
        input: &GetObjectInput,
    ) -> Result<GetObjectOutput> {
        ctx.check()?;
        let object_len = if input.range.is_some() {
            // The buffer API omits response headers. Fetch the total length so
            // the adapter can preserve Go's Content-Range contract.
            self.record(http::Method::HEAD);
            Some(self.execute(ctx, || {
                self.client()?
                    .head_object(&input.bucket, &input.key, None)
                    .map(|metadata| metadata.content_length)
                    .map_err(|e| sdk_error("HeadObject", e))
            })?)
        } else {
            None
        };
        self.record(http::Method::GET);
        let body = self.execute(ctx, || {
            let options = input.range.as_ref().map(|range| {
                ali_oss_rs::object_common::GetObjectOptionsBuilder::new()
                    .range(range)
                    .build()
            });
            self.client()?
                .get_object_to_buffer(&input.bucket, &input.key, options)
                .map_err(|e| sdk_error("GetObject", e))
        })?;
        let content_range = match object_len {
            Some(object_len) => {
                content_range_for_download(input.range.as_deref(), body.len(), object_len)?
            }
            None => None,
        };
        Ok(GetObjectOutput::from_bytes(body, content_range))
    }

    fn put_object(&self, ctx: &storeapi::Context, input: &PutObjectInput) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::PUT);
        self.execute(ctx, || {
            self.client()?
                .put_object_from_buffer(&input.bucket, &input.key, input.body.clone(), None)
                .map(|_| ())
                .map_err(|e| sdk_error("PutObject", e))
        })
    }

    fn copy_object(&self, ctx: &storeapi::Context, input: &CopyObjectInput) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::PUT);
        self.execute(ctx, || {
            self.client()?
                .copy_object(
                    &input.source_bucket,
                    &input.source_key,
                    &input.bucket,
                    &input.key,
                    None,
                )
                .map(|_| ())
                .map_err(|e| sdk_error("CopyObject", e))
        })
    }

    fn delete_object(&self, ctx: &storeapi::Context, input: &DeleteObjectInput) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::DELETE);
        self.execute(ctx, || {
            self.client()?
                .delete_object(&input.bucket, &input.key, None)
                .map(|_| ())
                .map_err(|e| sdk_error("DeleteObject", e))
        })
    }

    fn delete_objects(&self, ctx: &storeapi::Context, input: &DeleteObjectsInput) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::POST);
        self.execute(ctx, || {
            self.client()?
                .delete_multiple_objects(
                    &input.bucket,
                    ali_oss_rs::object_common::DeleteMultipleObjectsConfig::FromKeys(&input.keys),
                )
                .map(|_| ())
                .map_err(|e| sdk_error("DeleteMultipleObjects", e))
        })
    }

    fn list_objects_v2(
        &self,
        ctx: &storeapi::Context,
        input: &ListObjectsV2Input,
    ) -> Result<ListObjectsV2Output> {
        ctx.check()?;
        self.record(http::Method::GET);
        let output = self.execute(ctx, || {
            let mut options = ali_oss_rs::bucket_common::ListObjectsOptionsBuilder::new()
                .prefix(&input.prefix)
                .max_keys(u32::try_from(input.max_keys.max(1)).unwrap_or(1000));
            if let Some(value) = &input.continuation_token {
                options = options.continuation_token(value);
            }
            if let Some(value) = &input.start_after {
                options = options.start_after(value);
            }
            self.client()?
                .list_objects(&input.bucket, Some(options.build()))
                .map_err(|e| sdk_error("ListObjectsV2", e))
        })?;
        Ok(ListObjectsV2Output {
            next_continuation_token: output.next_continuation_token,
            is_truncated: output.is_truncated,
            contents: output
                .contents
                .into_iter()
                .map(|item| ListedObject {
                    key: item.key,
                    size: i64::try_from(item.size).unwrap_or(i64::MAX),
                })
                .collect(),
        })
    }

    fn initiate_multipart_upload(
        &self,
        ctx: &storeapi::Context,
        input: &CreateMultipartUploadInput,
    ) -> Result<CreateMultipartUploadOutput> {
        ctx.check()?;
        self.record(http::Method::POST);
        let options = put_options(input)?;
        let output = self.execute(ctx, || {
            self.client()?
                .initiate_multipart_uploads(&input.bucket, &input.key, options.clone())
                .map_err(|e| sdk_error("InitiateMultipartUpload", e))
        })?;
        Ok(CreateMultipartUploadOutput {
            bucket: output.bucket,
            key: output.key,
            upload_id: output.upload_id,
        })
    }

    fn upload_part(
        &self,
        ctx: &storeapi::Context,
        input: &UploadPartInput,
    ) -> Result<UploadPartOutput> {
        ctx.check()?;
        self.record(http::Method::PUT);
        let part_number = u32::try_from(input.part_number)
            .map_err(|_| anyhow!("invalid OSS part number {}", input.part_number))?;
        let output = self.execute(ctx, || {
            self.client()?
                .upload_part_from_buffer(
                    &input.bucket,
                    &input.key,
                    input.body.clone(),
                    ali_oss_rs::multipart_common::UploadPartRequest::new(
                        part_number,
                        &input.upload_id,
                    ),
                )
                .map_err(|e| sdk_error("UploadPart", e))
        })?;
        Ok(UploadPartOutput { etag: output.etag })
    }

    fn complete_multipart_upload(
        &self,
        ctx: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
    ) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::POST);
        // 将分片序号转为 SDK 需要的 u32，并收集 (part_number, etag) 对。
        let parts = input
            .parts
            .iter()
            .map(|part| {
                Ok((
                    u32::try_from(part.part_number)
                        .map_err(|_| anyhow!("invalid OSS part number {}", part.part_number))?,
                    part.etag.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.execute(ctx, || {
            self.client()?
                .complete_multipart_uploads(
                    &input.bucket,
                    &input.key,
                    ali_oss_rs::multipart_common::CompleteMultipartUploadRequest {
                        upload_id: input.upload_id.clone(),
                        parts: parts.clone(),
                    },
                    None,
                )
                .map(|_| ())
                .map_err(|e| sdk_error("CompleteMultipartUpload", e))
        })
    }

    fn abort_multipart_upload(
        &self,
        ctx: &storeapi::Context,
        input: &AbortMultipartUploadInput,
    ) -> Result<()> {
        ctx.check()?;
        self.record(http::Method::DELETE);
        self.execute(ctx, || {
            self.client()?
                .abort_multipart_uploads(&input.bucket, &input.key, &input.upload_id)
                .map_err(|e| sdk_error("AbortMultipartUpload", e))
        })
    }

    fn list_parts(
        &self,
        ctx: &storeapi::Context,
        input: &ListPartsInput,
    ) -> Result<Vec<CompletedPart>> {
        ctx.check()?;
        self.record(http::Method::GET);
        let output = self.execute(ctx, || {
            self.client()?
                .list_parts(&input.bucket, &input.key, &input.upload_id, None)
                .map_err(|e| sdk_error("ListParts", e))
        })?;
        Ok(output
            .parts
            .into_iter()
            .map(|part| CompletedPart {
                etag: part.etag,
                part_number: i32::try_from(part.part_number).unwrap_or(i32::MAX),
            })
            .collect())
    }
}
