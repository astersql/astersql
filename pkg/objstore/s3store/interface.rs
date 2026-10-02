// Copyright 2025 PingCAP, Inc.
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

// S3 API 请求/响应类型、错误码辅助与 AWS SDK 实现。
//
// 定义可 Mock 的 `S3API` trait，以及基于 `aws_sdk_s3` 的 `AwsS3Api`，
// 供高层 Client 在同步上下文中通过 Tokio Runtime 发起对象存储调用。

use std::fmt;
use std::future::Future;
use std::io::{self, Cursor, Read};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use aws_sdk_s3::primitives::ByteStream;
use base64::Engine;
use md5::{Digest, Md5};
use tokio::runtime::Runtime;

pub(crate) fn run_cancellable<F: Future>(
    runtime: &Runtime,
    ctx: &storeapi::Context,
    future: F,
) -> Result<F::Output> {
    ctx.check()?;
    runtime.block_on(async {
        tokio::select! {
            result = future => Ok(result),
            _ = ctx.wait_cancelled() => Err(anyhow!("operation cancelled")),
        }
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 单次请求选项；`content_md5` 控制是否附加 Content-MD5/校验和。
pub struct RequestOptions {
    pub content_md5: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// HeadBucket 输入。
pub struct HeadBucketInput {
    pub bucket: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ListObjects v1 输入；`marker` 对应 Go SDK 的分页游标。
pub struct ListObjectsInput {
    pub bucket: String,
    pub prefix: String,
    pub max_keys: i32,
    pub marker: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ListObjects v1 输出（下一页 Marker、截断标志、对象列表）。
pub struct ListObjectsOutput {
    pub next_marker: Option<String>,
    pub is_truncated: bool,
    pub contents: Vec<ListedObject>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ListObjectsV2 输入（前缀、分页、StartAfter）。
pub struct ListObjectsV2Input {
    pub bucket: String,
    pub prefix: String,
    pub max_keys: i32,
    pub continuation_token: Option<String>,
    pub start_after: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列举结果中的单个对象（键与大小）。
pub struct ListedObject {
    pub key: String,
    pub size: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ListObjectsV2 输出（续传 Token、截断标志、对象列表）。
pub struct ListObjectsV2Output {
    pub next_continuation_token: Option<String>,
    pub is_truncated: bool,
    pub contents: Vec<ListedObject>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// GetObject 输入，可选 HTTP Range。
pub struct GetObjectInput {
    pub bucket: String,
    pub key: String,
    pub range: Option<String>,
}

/// GetObject 输出：可读 Body 与长度/Range 元数据。
pub struct GetObjectOutput {
    pub body: Box<dyn prefetch::reader::ReadCloser>,
    pub content_length: Option<i64>,
    pub content_range: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// PutObject 输入，含可选 ACL/SSE/存储类。
pub struct PutObjectInput {
    pub bucket: String,
    pub key: String,
    pub body: Vec<u8>,
    pub acl: Option<String>,
    pub server_side_encryption: Option<String>,
    pub sse_kms_key_id: Option<String>,
    pub storage_class: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// DeleteObject 输入。
pub struct DeleteObjectInput {
    pub bucket: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 批量 DeleteObjects 输入。
pub struct DeleteObjectsInput {
    pub bucket: String,
    pub keys: Vec<String>,
    pub quiet: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// HeadObject 输入。
pub struct HeadObjectInput {
    pub bucket: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// HeadObject 输出（含复制状态）。
pub struct HeadObjectOutput {
    pub replication_status: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// CopyObject 输入（目标桶、CopySource、目标键）。
pub struct CopyObjectInput {
    pub bucket: String,
    pub copy_source: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 创建分片上传输入。
pub struct CreateMultipartUploadInput {
    pub bucket: String,
    pub key: String,
    pub acl: Option<String>,
    pub server_side_encryption: Option<String>,
    pub sse_kms_key_id: Option<String>,
    pub storage_class: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 创建分片上传输出（含 upload_id）。
pub struct CreateMultipartUploadOutput {
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 上传单个分片输入。
pub struct UploadPartInput {
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
    pub part_number: i32,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 上传分片输出（ETag）。
pub struct UploadPartOutput {
    pub e_tag: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Complete 时提交的已完成分片（ETag + PartNumber）。
pub struct CompletedPart {
    pub e_tag: Option<String>,
    pub part_number: i32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 完成分片上传输入。
pub struct CompleteMultipartUploadInput {
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
    pub parts: Vec<CompletedPart>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 中止分片上传输入。
pub struct AbortMultipartUploadInput {
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 查询桶对象锁配置输入。
pub struct GetObjectLockConfigurationInput {
    pub bucket: String,
}

#[derive(Debug)]
/// 带 S3 错误码的结构化错误，便于 `error_code` 抽取。
pub struct S3Error {
    code: String,
    message: String,
}

/// 格式为 `code: message`。
impl fmt::Display for S3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for S3Error {}

/// 构造带错误码的 `S3Error` 并包装为 anyhow。
pub fn api_error(code: impl Into<String>, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(S3Error {
        code: code.into(),
        message: message.into(),
    })
}

/// 从错误链中提取 `S3Error` 的 code。
pub fn error_code(error: &anyhow::Error) -> Option<&str> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<S3Error>()
            .map(|error| error.code.as_str())
    })
}

/// 内存中的可读可关闭 Body，供测试与 SDK 响应物化。
pub struct MemoryBody {
    inner: Cursor<Vec<u8>>,
    closed: bool,
}

impl MemoryBody {
    /// 由字节缓冲构造未关闭的 MemoryBody。
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(data),
            closed: false,
        }
    }
}

/// 关闭后继续读返回 BrokenPipe。
impl Read for MemoryBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "body is closed"));
        }
        self.inner.read(buffer)
    }
}

/// close 仅置标志，不释放已读缓冲。
impl prefetch::reader::ReadCloser for MemoryBody {
    fn close(&mut self) -> io::Result<()> {
        self.closed = true;
        Ok(())
    }
}

/// 保留 AWS SDK ByteStream 的惰性读取语义，避免 GetObject 预先物化整个对象。
pub struct AwsBody {
    body: ByteStream,
    runtime: Arc<Runtime>,
    current: Cursor<Vec<u8>>,
    closed: bool,
}

impl AwsBody {
    /// 由 SDK 响应流和执行该流的 Tokio Runtime 构造。
    pub fn new(body: ByteStream, runtime: Arc<Runtime>) -> Self {
        Self {
            body,
            runtime,
            current: Cursor::new(Vec::new()),
            closed: false,
        }
    }
}

impl Read for AwsBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "body is closed"));
        }
        if buffer.is_empty() {
            return Ok(0);
        }

        loop {
            let read = self.current.read(buffer)?;
            if read != 0 {
                return Ok(read);
            }
            match self.runtime.block_on(self.body.try_next()) {
                Ok(Some(bytes)) => self.current = Cursor::new(bytes.to_vec()),
                Ok(None) => return Ok(0),
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }
}

impl prefetch::reader::ReadCloser for AwsBody {
    fn close(&mut self) -> io::Result<()> {
        self.closed = true;
        self.body = ByteStream::from_static(&[]);
        self.current = Cursor::new(Vec::new());
        Ok(())
    }
}

/// 可注入的 S3 操作抽象；默认实现返回「未实现」错误。
pub trait S3API: Send + Sync {
    fn unsupported<T>(&self, operation: &str) -> Result<T>
    where
        Self: Sized,
    {
        Err(anyhow!("S3 operation {operation} is not implemented"))
    }

    fn head_bucket(
        &self,
        _: &storeapi::Context,
        _: &HeadBucketInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!("S3 operation HeadBucket is not implemented"))
    }
    fn list_objects(
        &self,
        _: &storeapi::Context,
        _: &ListObjectsInput,
        _: RequestOptions,
    ) -> Result<ListObjectsOutput> {
        Err(anyhow!("S3 operation ListObjects is not implemented"))
    }
    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        _: &ListObjectsV2Input,
        _: RequestOptions,
    ) -> Result<ListObjectsV2Output> {
        Err(anyhow!("S3 operation ListObjectsV2 is not implemented"))
    }
    fn get_object(
        &self,
        _: &storeapi::Context,
        _: &GetObjectInput,
        _: RequestOptions,
    ) -> Result<GetObjectOutput> {
        Err(anyhow!("S3 operation GetObject is not implemented"))
    }
    fn put_object(
        &self,
        _: &storeapi::Context,
        _: &PutObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!("S3 operation PutObject is not implemented"))
    }
    fn delete_object(
        &self,
        _: &storeapi::Context,
        _: &DeleteObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!("S3 operation DeleteObject is not implemented"))
    }
    fn delete_objects(
        &self,
        _: &storeapi::Context,
        _: &DeleteObjectsInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!("S3 operation DeleteObjects is not implemented"))
    }
    fn head_object(
        &self,
        _: &storeapi::Context,
        _: &HeadObjectInput,
        _: RequestOptions,
    ) -> Result<HeadObjectOutput> {
        Err(anyhow!("S3 operation HeadObject is not implemented"))
    }
    fn copy_object(
        &self,
        _: &storeapi::Context,
        _: &CopyObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!("S3 operation CopyObject is not implemented"))
    }
    fn create_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &CreateMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<CreateMultipartUploadOutput> {
        Err(anyhow!(
            "S3 operation CreateMultipartUpload is not implemented"
        ))
    }
    fn upload_part(
        &self,
        _: &storeapi::Context,
        _: &UploadPartInput,
        _: RequestOptions,
    ) -> Result<UploadPartOutput> {
        Err(anyhow!("S3 operation UploadPart is not implemented"))
    }
    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &CompleteMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!(
            "S3 operation CompleteMultipartUpload is not implemented"
        ))
    }
    fn abort_multipart_upload(
        &self,
        _: &storeapi::Context,
        _: &AbortMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        Err(anyhow!(
            "S3 operation AbortMultipartUpload is not implemented"
        ))
    }
    fn get_object_lock_configuration(
        &self,
        _: &storeapi::Context,
        _: &GetObjectLockConfigurationInput,
        _: RequestOptions,
    ) -> Result<bool> {
        Err(anyhow!(
            "S3 operation GetObjectLockConfiguration is not implemented"
        ))
    }
    fn presign_get_object(
        &self,
        _: &storeapi::Context,
        _: &GetObjectInput,
        _: Duration,
    ) -> Result<String> {
        Err(anyhow!("PresignObject requires concrete S3 client"))
    }
    fn bucket_region(&self, _: &storeapi::Context, _: &str) -> Result<String> {
        Err(anyhow!("S3 operation HeadBucket is not implemented"))
    }
}

#[derive(Clone)]
/// 基于 aws_sdk_s3 + Tokio Runtime 的真实 S3 实现。
pub struct AwsS3Api {
    client: aws_sdk_s3::Client,
    runtime: Arc<Runtime>,
    access_rec: Option<Arc<objectio::recording::AccessStats>>,
}

impl AwsS3Api {
    /// 由 SDK Client、Runtime 与可选访问统计构造。
    pub fn new(
        client: aws_sdk_s3::Client,
        runtime: Arc<Runtime>,
        access_rec: Option<Arc<objectio::recording::AccessStats>>,
    ) -> Self {
        Self {
            client,
            runtime,
            access_rec,
        }
    }

    /// Cancel an in-flight SDK request when the caller's context expires.
    fn run<F: Future>(&self, ctx: &storeapi::Context, future: F) -> Result<F::Output> {
        run_cancellable(&self.runtime, ctx, future)
    }

    /// 将一次 GET 类请求记入访问统计。
    fn record_get(&self) {
        objectio::recording::AccessStats::rec_request(
            self.access_rec.as_deref(),
            Some(
                &http::Request::builder()
                    .method(http::Method::GET)
                    .body(())
                    .expect("valid request"),
            ),
        );
    }

    /// 将一次 PUT/写类请求记入访问统计。
    fn record_put(&self) {
        objectio::recording::AccessStats::rec_request(
            self.access_rec.as_deref(),
            Some(
                &http::Request::builder()
                    .method(http::Method::PUT)
                    .body(())
                    .expect("valid request"),
            ),
        );
    }
}

/// 从 SDK 错误消息中识别常见错误码，否则用 operation 名作为 code。
fn sdk_error(operation: &str, error: impl fmt::Display) -> anyhow::Error {
    let message = error.to_string();
    let code = [
        "NoSuchBucket",
        "NoSuchKey",
        "NotFound",
        "AccessDenied",
        "BucketAlreadyExists",
    ]
    .into_iter()
    .find(|code| message.contains(code))
    .unwrap_or(operation);
    api_error(code, message)
}

/// 计算 Body 的 Base64(MD5)，用于 Content-MD5 头。
fn content_md5(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(Md5::digest(data))
}

/// 若 Option 有值则应用到 builder，否则原样返回。
fn optional<T>(value: &Option<String>, builder: T, apply: impl FnOnce(T, String) -> T) -> T {
    match value {
        Some(value) => apply(builder, value.clone()),
        None => builder,
    }
}

/// 将各 S3 操作转发到 aws_sdk_s3，并统一错误码/统计。
impl S3API for AwsS3Api {
    fn head_bucket(
        &self,
        ctx: &storeapi::Context,
        input: &HeadBucketInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.record_get();
        self.run(ctx, self.client.head_bucket().bucket(&input.bucket).send())?
            .map(|_| ())
            .map_err(|error| sdk_error("HeadBucket", error))
    }

    fn list_objects(
        &self,
        ctx: &storeapi::Context,
        input: &ListObjectsInput,
        _: RequestOptions,
    ) -> Result<ListObjectsOutput> {
        self.record_get();
        let mut request = self
            .client
            .list_objects()
            .bucket(&input.bucket)
            .prefix(&input.prefix)
            .max_keys(input.max_keys);
        request = optional(&input.marker, request, |request, value| {
            request.marker(value)
        });
        let output = self
            .run(ctx, request.send())?
            .map_err(|error| sdk_error("ListObjects", error))?;
        Ok(ListObjectsOutput {
            next_marker: output.next_marker().map(str::to_owned),
            is_truncated: output.is_truncated().unwrap_or(false),
            contents: output
                .contents()
                .iter()
                .map(|object| ListedObject {
                    key: object.key().unwrap_or_default().to_owned(),
                    size: object.size().unwrap_or_default(),
                })
                .collect(),
        })
    }

    fn list_objects_v2(
        &self,
        ctx: &storeapi::Context,
        input: &ListObjectsV2Input,
        _: RequestOptions,
    ) -> Result<ListObjectsV2Output> {
        self.record_get();
        let mut request = self
            .client
            .list_objects_v2()
            .bucket(&input.bucket)
            .prefix(&input.prefix)
            .max_keys(input.max_keys);
        request = optional(&input.continuation_token, request, |request, value| {
            request.continuation_token(value)
        });
        request = optional(&input.start_after, request, |request, value| {
            request.start_after(value)
        });
        let output = self
            .run(ctx, request.send())?
            .map_err(|error| sdk_error("ListObjectsV2", error))?;
        Ok(ListObjectsV2Output {
            next_continuation_token: output.next_continuation_token().map(str::to_owned),
            is_truncated: output.is_truncated().unwrap_or(false),
            contents: output
                .contents()
                .iter()
                .map(|object| ListedObject {
                    key: object.key().unwrap_or_default().to_owned(),
                    size: object.size().unwrap_or_default(),
                })
                .collect(),
        })
    }

    fn get_object(
        &self,
        ctx: &storeapi::Context,
        input: &GetObjectInput,
        _: RequestOptions,
    ) -> Result<GetObjectOutput> {
        self.record_get();
        let mut request = self
            .client
            .get_object()
            .bucket(&input.bucket)
            .key(&input.key);
        request = optional(&input.range, request, |request, value| request.range(value));
        let output = self
            .run(ctx, request.send())?
            .map_err(|error| sdk_error("GetObject", error))?;
        let content_length = output.content_length();
        let content_range = output.content_range().map(str::to_owned);
        Ok(GetObjectOutput {
            body: Box::new(AwsBody::new(output.body, self.runtime.clone())),
            content_length,
            content_range,
        })
    }

    fn put_object(
        &self,
        ctx: &storeapi::Context,
        input: &PutObjectInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.record_put();
        let mut request = self
            .client
            .put_object()
            .bucket(&input.bucket)
            .key(&input.key)
            .body(ByteStream::from(input.body.clone()));
        request = optional(&input.acl, request, |request, value| {
            request.acl(aws_sdk_s3::types::ObjectCannedAcl::from(value.as_str()))
        });
        request = optional(&input.server_side_encryption, request, |request, value| {
            request.server_side_encryption(aws_sdk_s3::types::ServerSideEncryption::from(
                value.as_str(),
            ))
        });
        request = optional(&input.sse_kms_key_id, request, |request, value| {
            request.ssekms_key_id(value)
        });
        request = optional(&input.storage_class, request, |request, value| {
            request.storage_class(aws_sdk_s3::types::StorageClass::from(value.as_str()))
        });
        // 兼容模式：为 PutObject Body 附加 Content-MD5。
        if options.content_md5 {
            request = request.content_md5(content_md5(&input.body));
        }
        self.run(ctx, request.send())?
            .map(|_| ())
            .map_err(|error| sdk_error("PutObject", error))
    }

    fn delete_object(
        &self,
        ctx: &storeapi::Context,
        input: &DeleteObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.record_put();
        self.run(
            ctx,
            self.client
                .delete_object()
                .bucket(&input.bucket)
                .key(&input.key)
                .send(),
        )?
        .map(|_| ())
        .map_err(|error| sdk_error("DeleteObject", error))
    }

    fn delete_objects(
        &self,
        ctx: &storeapi::Context,
        input: &DeleteObjectsInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.record_put();
        let objects = input
            .keys
            .iter()
            .map(|key| {
                aws_sdk_s3::types::ObjectIdentifier::builder()
                    .key(key)
                    .build()
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| anyhow!(error))?;
        let delete = aws_sdk_s3::types::Delete::builder()
            .set_objects(Some(objects))
            .quiet(input.quiet)
            .build()
            .map_err(|error| anyhow!(error))?;
        let mut request = self
            .client
            .delete_objects()
            .bucket(&input.bucket)
            .delete(delete);
        // Rust SDK 无法直接对 DeleteObjects XML 体设 Content-MD5；
        // 选用显式 checksum 算法以走兼容端点要求的校验路径。
        // The Rust SDK does not expose DeleteObjects' serialized XML body for
        // setting Content-MD5 directly. Selecting an explicit payload checksum
        // keeps compatible endpoints on a required-checksum request path.
        if options.content_md5 {
            request = request.checksum_algorithm(aws_sdk_s3::types::ChecksumAlgorithm::Crc32);
        }
        self.run(ctx, request.send())?
            .map(|_| ())
            .map_err(|error| sdk_error("DeleteObjects", error))
    }

    fn head_object(
        &self,
        ctx: &storeapi::Context,
        input: &HeadObjectInput,
        _: RequestOptions,
    ) -> Result<HeadObjectOutput> {
        self.record_get();
        let output = self
            .run(
                ctx,
                self.client
                    .head_object()
                    .bucket(&input.bucket)
                    .key(&input.key)
                    .send(),
            )?
            .map_err(|error| sdk_error("HeadObject", error))?;
        Ok(HeadObjectOutput {
            replication_status: output
                .replication_status()
                .map(|value| value.as_str().to_owned())
                .unwrap_or_default(),
        })
    }

    fn copy_object(
        &self,
        ctx: &storeapi::Context,
        input: &CopyObjectInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.record_put();
        self.run(
            ctx,
            self.client
                .copy_object()
                .bucket(&input.bucket)
                .copy_source(&input.copy_source)
                .key(&input.key)
                .send(),
        )?
        .map(|_| ())
        .map_err(|error| sdk_error("CopyObject", error))
    }

    fn create_multipart_upload(
        &self,
        ctx: &storeapi::Context,
        input: &CreateMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<CreateMultipartUploadOutput> {
        self.record_put();
        let mut request = self
            .client
            .create_multipart_upload()
            .bucket(&input.bucket)
            .key(&input.key);
        request = optional(&input.acl, request, |request, value| {
            request.acl(aws_sdk_s3::types::ObjectCannedAcl::from(value.as_str()))
        });
        request = optional(&input.server_side_encryption, request, |request, value| {
            request.server_side_encryption(aws_sdk_s3::types::ServerSideEncryption::from(
                value.as_str(),
            ))
        });
        request = optional(&input.sse_kms_key_id, request, |request, value| {
            request.ssekms_key_id(value)
        });
        request = optional(&input.storage_class, request, |request, value| {
            request.storage_class(aws_sdk_s3::types::StorageClass::from(value.as_str()))
        });
        let output = self
            .run(ctx, request.send())?
            .map_err(|error| sdk_error("CreateMultipartUpload", error))?;
        Ok(CreateMultipartUploadOutput {
            bucket: output.bucket().unwrap_or(&input.bucket).to_owned(),
            key: output.key().unwrap_or(&input.key).to_owned(),
            upload_id: output.upload_id().unwrap_or_default().to_owned(),
        })
    }

    fn upload_part(
        &self,
        ctx: &storeapi::Context,
        input: &UploadPartInput,
        options: RequestOptions,
    ) -> Result<UploadPartOutput> {
        self.record_put();
        let mut request = self
            .client
            .upload_part()
            .bucket(&input.bucket)
            .key(&input.key)
            .upload_id(&input.upload_id)
            .part_number(input.part_number)
            .content_length(input.body.len() as i64)
            .body(ByteStream::from(input.body.clone()));
        // 兼容模式：为 UploadPart Body 附加 Content-MD5。
        if options.content_md5 {
            request = request.content_md5(content_md5(&input.body));
        }
        let output = self
            .run(ctx, request.send())?
            .map_err(|error| sdk_error("UploadPart", error))?;
        Ok(UploadPartOutput {
            e_tag: output.e_tag().map(str::to_owned),
        })
    }

    fn complete_multipart_upload(
        &self,
        ctx: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.record_put();
        let parts = input
            .parts
            .iter()
            .map(|part| {
                aws_sdk_s3::types::CompletedPart::builder()
                    .set_e_tag(part.e_tag.clone())
                    .part_number(part.part_number)
                    .build()
            })
            .collect();
        let upload = aws_sdk_s3::types::CompletedMultipartUpload::builder()
            .set_parts(Some(parts))
            .build();
        self.run(
            ctx,
            self.client
                .complete_multipart_upload()
                .bucket(&input.bucket)
                .key(&input.key)
                .upload_id(&input.upload_id)
                .multipart_upload(upload)
                .send(),
        )?
        .map(|_| ())
        .map_err(|error| sdk_error("CompleteMultipartUpload", error))
    }

    fn abort_multipart_upload(
        &self,
        ctx: &storeapi::Context,
        input: &AbortMultipartUploadInput,
        _: RequestOptions,
    ) -> Result<()> {
        self.record_put();
        self.run(
            ctx,
            self.client
                .abort_multipart_upload()
                .bucket(&input.bucket)
                .key(&input.key)
                .upload_id(&input.upload_id)
                .send(),
        )?
        .map(|_| ())
        .map_err(|error| sdk_error("AbortMultipartUpload", error))
    }

    fn get_object_lock_configuration(
        &self,
        ctx: &storeapi::Context,
        input: &GetObjectLockConfigurationInput,
        _: RequestOptions,
    ) -> Result<bool> {
        self.record_get();
        let output = self
            .run(
                ctx,
                self.client
                    .get_object_lock_configuration()
                    .bucket(&input.bucket)
                    .send(),
            )?
            .map_err(|error| sdk_error("GetObjectLockConfiguration", error))?;
        Ok(output
            .object_lock_configuration()
            .and_then(|config| config.object_lock_enabled())
            == Some(&aws_sdk_s3::types::ObjectLockEnabled::Enabled))
    }

    fn presign_get_object(
        &self,
        ctx: &storeapi::Context,
        input: &GetObjectInput,
        expire: Duration,
    ) -> Result<String> {
        ctx.check()?;
        let config = aws_sdk_s3::presigning::PresigningConfig::expires_in(expire)?;
        let output = self
            .runtime
            .block_on(
                self.client
                    .get_object()
                    .bucket(&input.bucket)
                    .key(&input.key)
                    .presigned(config),
            )
            .map_err(|error| sdk_error("PresignGetObject", error))?;
        Ok(output.uri().to_string())
    }

    fn bucket_region(&self, ctx: &storeapi::Context, bucket: &str) -> Result<String> {
        self.record_get();
        match self.run(ctx, self.client.head_bucket().bucket(bucket).send())? {
            Ok(output) => Ok(output.bucket_region().unwrap_or_default().to_owned()),
            Err(error) => {
                // S3 returns the bucket's region in the response header even
                // when the configured region caused an expected HTTP redirect.
                let detected_region = error
                    .raw_response()
                    .and_then(|response| response.headers().get("x-amz-bucket-region"))
                    .filter(|region| !region.is_empty())
                    .map(str::to_owned);
                let diagnostic = sdk_error("HeadBucket", &error);
                let _ = crate::newBucketRegionDetectionRetryer()
                    .IsErrorRetryable(&anyhow::Error::new(error));
                if let Some(region) = detected_region {
                    return Ok(region);
                }
                Err(diagnostic)
            }
        }
    }
}
