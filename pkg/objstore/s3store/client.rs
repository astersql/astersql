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

// 带桶前缀的 S3 高层 Client。
//
// 将逻辑对象名映射为桶内 Key，封装权限探测、读写、列举、复制、
// 预签名与分片上传（同步 Writer / 并行 Uploader）。

use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::backuppb;
use crate::{
    AbortMultipartUploadInput, CompleteMultipartUploadInput, CompletedPart, CopyObjectInput,
    CreateMultipartUploadInput, DeleteObjectInput, DeleteObjectsInput, GetObjectInput,
    HeadBucketInput, HeadObjectInput, ListObjectsV2Input, PutObjectInput, RequestOptions, S3API,
    UploadPartInput, error_code,
};

// HeadObject 不存在时常见错误码（含桶/键缺失）。
const NOT_FOUND: &str = "NotFound";
const NO_SUCH_BUCKET: &str = "NoSuchBucket";
const NO_SUCH_KEY: &str = "NoSuchKey";

#[derive(Clone)]
/// 包装底层 `S3API`，按 `bucket_prefix` 拼装对象键。
pub struct S3Client {
    svc: Arc<dyn S3API>,
    bucket_prefix: storeapi::BucketPrefix,
    options: backuppb::S3,
    s3_compatible: bool,
}

impl S3Client {
    /// 由具体 `S3API` 实现构造 Client；`s3_compatible` 控制是否带 Content-MD5。
    pub fn new<T>(
        svc: Arc<T>,
        bucket_prefix: storeapi::BucketPrefix,
        options: backuppb::S3,
        s3_compatible: bool,
    ) -> Self
    where
        T: S3API + 'static,
    {
        Self {
            svc,
            bucket_prefix,
            options,
            s3_compatible,
        }
    }

    /// 由已装箱的 `dyn S3API` 构造 Client。
    pub fn from_dyn(
        svc: Arc<dyn S3API>,
        bucket_prefix: storeapi::BucketPrefix,
        options: backuppb::S3,
        s3_compatible: bool,
    ) -> Self {
        Self {
            svc,
            bucket_prefix,
            options,
            s3_compatible,
        }
    }

    /// 兼容模式下开启 Content-MD5 校验选项。
    fn request_options(&self) -> RequestOptions {
        RequestOptions {
            content_md5: self.s3_compatible,
        }
    }

    /// 权限探测：HeadBucket。
    pub fn CheckBucketExistence(&self, ctx: &storeapi::Context) -> Result<()> {
        self.svc.head_bucket(
            ctx,
            &HeadBucketInput {
                bucket: self.bucket_prefix.Bucket.clone(),
            },
            RequestOptions::default(),
        )
    }

    /// 权限探测：ListObjectsV2 取 1 条。
    pub fn CheckListObjects(&self, ctx: &storeapi::Context) -> Result<()> {
        self.svc.list_objects_v2(
            ctx,
            &ListObjectsV2Input {
                bucket: self.bucket_prefix.Bucket.clone(),
                prefix: self.bucket_prefix.PrefixStr(),
                max_keys: 1,
                ..Default::default()
            },
            RequestOptions::default(),
        )?;
        Ok(())
    }

    /// 权限探测：Get 探测键；`NoSuchKey` 视为权限正常。
    pub fn CheckGetObject(&self, ctx: &storeapi::Context) -> Result<()> {
        let input = GetObjectInput {
            bucket: self.bucket_prefix.Bucket.clone(),
            key: self
                .bucket_prefix
                .ObjectKey(&storeapi::GenPermCheckObjectKey()),
            range: None,
        };
        match self.svc.get_object(ctx, &input, RequestOptions::default()) {
            Ok(_) => Ok(()),
            Err(error) if error_code(&error) == Some(NO_SUCH_KEY) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// 权限探测：Put 探测对象后立即 Delete。
    pub fn CheckPutAndDeleteObject(&self, ctx: &storeapi::Context) -> Result<()> {
        let key = self
            .bucket_prefix
            .ObjectKey(&storeapi::GenPermCheckObjectKey());
        let put_result = self.svc.put_object(
            ctx,
            &PutObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: key.clone(),
                body: b"check".to_vec(),
                ..Default::default()
            },
            self.request_options(),
        );

        // 对齐 Go defer：无论 Put 成败都尝试 Delete 清理；
        // 仅当 Put 成功时，Delete 结果才成为返回值。Go 只对 NoSuchKey
        // 抑制清理告警，并不会吞掉该错误。
        // Match the Go defer: clean up even if PutObject returned an error, and
        // only let cleanup replace the result when the put itself succeeded.
        let delete_result = self.svc.delete_object(
            ctx,
            &DeleteObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key,
            },
            RequestOptions::default(),
        );
        match put_result {
            Err(error) => Err(error),
            Ok(()) => delete_result,
        }
    }

    /// GetObject，支持 HTTP Range；返回 Body 与范围元数据。
    pub fn GetObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        start_offset: i64,
        end_offset: i64,
    ) -> Result<s3like::GetResp> {
        // 将 [start,end) 转为 HTTP Range 头；全量下载时 range 为空。
        let (is_full_range, range) = storeapi::GetHTTPRange(start_offset, end_offset);
        let output = self.svc.get_object(
            ctx,
            &GetObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
                range: (!range.is_empty()).then_some(range),
            },
            RequestOptions::default(),
        )?;
        Ok(s3like::GetResp {
            Body: output.body,
            IsFullRange: is_full_range,
            ContentLength: output.content_length,
            ContentRange: output.content_range,
        })
    }

    /// PutObject，并记录 API 调用指标。
    pub fn PutObject(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        let input = self.buildPutObjectInput(&self.options, name, data);
        s3like::RecordAPICall(s3like::BACKEND_S3, s3like::API_CALL_PUT_OBJECT);
        self.svc.put_object(ctx, &input, self.request_options())
    }

    /// 按配置组装 PutObject 输入（ACL/SSE/存储类等）。
    pub fn buildPutObjectInput(
        &self,
        options: &backuppb::S3,
        file: &str,
        data: &[u8],
    ) -> PutObjectInput {
        PutObjectInput {
            bucket: options.Bucket.clone(),
            key: self.bucket_prefix.ObjectKey(file),
            body: data.to_vec(),
            acl: non_empty(&options.Acl),
            server_side_encryption: non_empty(&options.Sse),
            sse_kms_key_id: non_empty(&options.SseKmsKeyId),
            storage_class: non_empty(&options.StorageClass),
        }
    }

    /// 删除单个对象。
    pub fn DeleteObject(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        self.svc.delete_object(
            ctx,
            &DeleteObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
            },
            RequestOptions::default(),
        )
    }

    /// 生成预签名 GET URL。
    pub fn PresignObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        expire: Duration,
    ) -> Result<String> {
        self.svc.presign_get_object(
            ctx,
            &GetObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
                range: None,
            },
            expire,
        )
    }

    /// 批量删除；空列表直接成功。
    pub fn DeleteObjects(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()> {
        // 空批量删除视为成功，避免无意义 API 调用。
        if names.is_empty() {
            return Ok(());
        }
        self.svc.delete_objects(
            ctx,
            &DeleteObjectsInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                keys: names
                    .iter()
                    .map(|name| self.bucket_prefix.ObjectKey(name))
                    .collect(),
                quiet: false,
            },
            self.request_options(),
        )
    }

    /// HeadObject：NotFound/NoSuchKey/NoSuchBucket 视为不存在。
    pub fn IsObjectExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        s3like::RecordAPICall(s3like::BACKEND_S3, s3like::API_CALL_HEAD_OBJECTS);
        match self.svc.head_object(
            ctx,
            &HeadObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
            },
            RequestOptions::default(),
        ) {
            Ok(_) => Ok(true),
            Err(error)
                if matches!(
                    error_code(&error),
                    Some(NO_SUCH_BUCKET | NO_SUCH_KEY | NOT_FOUND)
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// HeadObject，返回复制状态等元数据。
    pub fn HeadObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<s3like::HeadObjectResp> {
        s3like::RecordAPICall(s3like::BACKEND_S3, s3like::API_CALL_HEAD_OBJECTS);
        let output = self.svc.head_object(
            ctx,
            &HeadObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
            },
            RequestOptions::default(),
        )?;
        Ok(s3like::HeadObjectResp {
            ReplicationStatus: output.replication_status,
        })
    }

    /// ListObjectsV2，支持额外前缀、StartAfter 与续传 Token。
    pub fn ListObjects(
        &self,
        ctx: &storeapi::Context,
        extra_prefix: &str,
        start_after: &str,
        continuation_token: Option<&str>,
        max_keys: isize,
    ) -> Result<s3like::ListResp> {
        s3like::RecordAPICall(s3like::BACKEND_S3, s3like::API_CALL_LIST_OBJECTS);
        let output = self.svc.list_objects_v2(
            ctx,
            &ListObjectsV2Input {
                bucket: self.bucket_prefix.Bucket.clone(),
                prefix: self.bucket_prefix.ObjectKey(extra_prefix),
                // Go converts int to int32 directly for the SDK field.
                max_keys: max_keys as i32,
                continuation_token: continuation_token.map(str::to_owned),
                start_after: (!start_after.is_empty())
                    .then(|| self.bucket_prefix.ObjectKey(start_after)),
            },
            RequestOptions::default(),
        )?;
        Ok(s3like::ListResp {
            NextContinuationToken: output.next_continuation_token,
            IsTruncated: output.is_truncated,
            Objects: output
                .contents
                .into_iter()
                .map(|object| s3like::Object {
                    Key: object.key,
                    Size: object.size,
                })
                .collect(),
        })
    }

    /// 服务端 CopyObject。
    pub fn CopyObject(&self, ctx: &storeapi::Context, params: &s3like::CopyInput) -> Result<()> {
        let source_key = params.FromLoc.ObjectKey(&params.FromKey);
        self.svc.copy_object(
            ctx,
            &CopyObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                copy_source: join_copy_source(&params.FromLoc.Bucket, &source_key),
                key: self.bucket_prefix.ObjectKey(&params.ToKey),
            },
            RequestOptions::default(),
        )
    }

    /// 创建同步分片上传 Writer（每次 write 上传一个 part）。
    pub fn MultipartWriter(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Box<dyn objectio::Writer>> {
        let input = CreateMultipartUploadInput {
            bucket: self.bucket_prefix.Bucket.clone(),
            key: self.bucket_prefix.ObjectKey(name),
            acl: non_empty(&self.options.Acl),
            server_side_encryption: non_empty(&self.options.Sse),
            sse_kms_key_id: non_empty(&self.options.SseKmsKeyId),
            storage_class: non_empty(&self.options.StorageClass),
        };
        let output = self
            .svc
            .create_multipart_upload(ctx, &input, RequestOptions::default())?;
        Ok(Box::new(MultipartWriter {
            svc: self.svc.clone(),
            create_output: output,
            complete_parts: Vec::with_capacity(128),
            s3_compatible: self.s3_compatible,
        }))
    }

    /// 创建可并行分片的 Uploader。
    pub fn MultipartUploader(
        &self,
        name: &str,
        part_size: i64,
        concurrency: i32,
    ) -> Box<dyn s3like::Uploader> {
        Box::new(MultipartUploader {
            svc: self.svc.clone(),
            bucket: self.bucket_prefix.Bucket.clone(),
            key: self.bucket_prefix.ObjectKey(name),
            part_size,
            concurrency,
            s3_compatible: self.s3_compatible,
        })
    }
}

/// 空串转为 None，非空克隆为 Some。
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// 拼接 CopySource：`bucket/key`，去掉空路径段。
fn join_copy_source(bucket: &str, key: &str) -> String {
    let mut clean = Vec::new();
    for part in bucket.split('/').chain(key.split('/')) {
        match part {
            "" | "." => {}
            ".." if clean.last().is_some_and(|last| *last != "..") => {
                clean.pop();
            }
            ".." => clean.push(part),
            _ => clean.push(part),
        }
    }
    clean.join("/")
}

/// 强制开启 Content-MD5 请求选项。
pub fn withContentMD5(options: &mut RequestOptions) {
    options.content_md5 = true;
}

/// 同步分片 Writer：Create → 多次 UploadPart → Complete。
struct MultipartWriter {
    svc: Arc<dyn S3API>,
    create_output: crate::CreateMultipartUploadOutput,
    complete_parts: Vec<CompletedPart>,
    s3_compatible: bool,
}

/// 每次 write 上传一个 part；close 时 CompleteMultipartUpload。
impl objectio::Writer for MultipartWriter {
    fn write(&mut self, ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        ctx.check()?;
        // PartNumber 从 1 起递增；上传成功后记入 complete_parts。
        let part_number = i32::try_from(self.complete_parts.len() + 1)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many multipart parts"))?;
        let output = self
            .svc
            .upload_part(
                ctx,
                &UploadPartInput {
                    bucket: self.create_output.bucket.clone(),
                    key: self.create_output.key.clone(),
                    upload_id: self.create_output.upload_id.clone(),
                    part_number,
                    body: data.to_vec(),
                },
                RequestOptions {
                    content_md5: self.s3_compatible,
                },
            )
            .map_err(io::Error::other)?;
        self.complete_parts.push(CompletedPart {
            e_tag: output.e_tag,
            part_number,
        });
        Ok(data.len())
    }

    fn close(&mut self, ctx: &objectio::Context) -> io::Result<()> {
        ctx.check()?;
        self.svc
            .complete_multipart_upload(
                ctx,
                &CompleteMultipartUploadInput {
                    bucket: self.create_output.bucket.clone(),
                    key: self.create_output.key.clone(),
                    upload_id: self.create_output.upload_id.clone(),
                    parts: self.complete_parts.clone(),
                },
                RequestOptions::default(),
            )
            .map_err(io::Error::other)?;
        Ok(())
    }
}

/// 并行分片 Uploader：按 part_size 切块，concurrency 控制并行度。
struct MultipartUploader {
    svc: Arc<dyn S3API>,
    bucket: String,
    key: String,
    part_size: i64,
    concurrency: i32,
    s3_compatible: bool,
}

/// 读尽 reader 后：单块走 Put，多块走并行分片上传。
impl s3like::Uploader for MultipartUploader {
    fn Upload(&self, ctx: &storeapi::Context, reader: &mut dyn Read) -> Result<()> {
        if self.part_size < 0 || self.concurrency <= 0 {
            return Err(anyhow!(
                "multipart part size must be non-negative and concurrency must be positive"
            ));
        }
        // part_size<=0 时回退到全局 HardcodedChunkSize。
        let chunk_size = if self.part_size > 0 {
            usize::try_from(self.part_size)
                .map_err(|_| anyhow!("multipart part size overflows usize"))?
        } else {
            unsafe { s3like::HardcodedChunkSize }
        };
        if chunk_size == 0 {
            return Err(anyhow!("multipart part size must not be zero"));
        }

        // 先在内存中按块读尽；单块直接 Put，多块走 multipart。
        let mut chunks = Vec::new();
        loop {
            ctx.check()?;
            let mut chunk = vec![0_u8; chunk_size];
            let mut filled = 0;
            while filled < chunk_size {
                let count = reader.read(&mut chunk[filled..])?;
                if count == 0 {
                    break;
                }
                filled += count;
            }
            chunk.truncate(filled);
            if chunk.is_empty() {
                break;
            }
            chunks.push(chunk);
            if filled < chunk_size {
                break;
            }
        }

        // 不足两块时无需 multipart，直接 PutObject。
        if chunks.len() <= 1 {
            return self.svc.put_object(
                ctx,
                &PutObjectInput {
                    bucket: self.bucket.clone(),
                    key: self.key.clone(),
                    body: chunks.pop().unwrap_or_default(),
                    ..Default::default()
                },
                RequestOptions {
                    content_md5: self.s3_compatible,
                },
            );
        }

        let created = self.svc.create_multipart_upload(
            ctx,
            &CreateMultipartUploadInput {
                bucket: self.bucket.clone(),
                key: self.key.clone(),
                ..Default::default()
            },
            RequestOptions::default(),
        )?;
        // 按 concurrency 分批并行 UploadPart，失败则 Abort。
        let parallelism = usize::try_from(self.concurrency).unwrap_or(1).max(1);
        let mut completed = Vec::with_capacity(chunks.len());
        let upload_result: Result<()> = (|| {
            for (batch_index, batch) in chunks.chunks(parallelism).enumerate() {
                let batch_parts = std::thread::scope(|scope| {
                    let mut handles = Vec::with_capacity(batch.len());
                    for (offset, data) in batch.iter().enumerate() {
                        let part_number = i32::try_from(batch_index * parallelism + offset + 1)
                            .map_err(|_| anyhow!("too many multipart parts"))?;
                        let input = UploadPartInput {
                            bucket: created.bucket.clone(),
                            key: created.key.clone(),
                            upload_id: created.upload_id.clone(),
                            part_number,
                            body: data.clone(),
                        };
                        handles.push(scope.spawn(move || {
                            self.svc
                                .upload_part(
                                    ctx,
                                    &input,
                                    RequestOptions {
                                        content_md5: self.s3_compatible,
                                    },
                                )
                                .map(|output| CompletedPart {
                                    e_tag: output.e_tag,
                                    part_number,
                                })
                        }));
                    }
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle
                                .join()
                                .map_err(|_| anyhow!("multipart worker panicked"))?
                        })
                        .collect::<Result<Vec<_>>>()
                })?;
                completed.extend(batch_parts);
            }
            Ok(())
        })();
        // 任一 part 失败：中止 multipart，避免残留未完成上传。
        if let Err(error) = upload_result {
            let _ = self.svc.abort_multipart_upload(
                ctx,
                &AbortMultipartUploadInput {
                    bucket: created.bucket,
                    key: created.key,
                    upload_id: created.upload_id,
                },
                RequestOptions::default(),
            );
            return Err(error);
        }
        // Complete 要求 part 按 PartNumber 有序。
        completed.sort_by_key(|part| part.part_number);
        self.svc.complete_multipart_upload(
            ctx,
            &CompleteMultipartUploadInput {
                bucket: created.bucket,
                key: created.key,
                upload_id: created.upload_id,
                parts: completed,
            },
            RequestOptions { content_md5: false },
        )
    }
}

/// 将 S3Client 适配到 s3like::PrefixClient。
impl s3like::PrefixClient for S3Client {
    fn CheckBucketExistence(&self, ctx: &storeapi::Context) -> Result<()> {
        S3Client::CheckBucketExistence(self, ctx)
    }
    fn CheckListObjects(&self, ctx: &storeapi::Context) -> Result<()> {
        S3Client::CheckListObjects(self, ctx)
    }
    fn CheckGetObject(&self, ctx: &storeapi::Context) -> Result<()> {
        S3Client::CheckGetObject(self, ctx)
    }
    fn CheckPutAndDeleteObject(&self, ctx: &storeapi::Context) -> Result<()> {
        S3Client::CheckPutAndDeleteObject(self, ctx)
    }
    fn GetObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        start_offset: i64,
        end_offset: i64,
    ) -> Result<Option<s3like::GetResp>> {
        S3Client::GetObject(self, ctx, name, start_offset, end_offset).map(Some)
    }
    fn PutObject(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        S3Client::PutObject(self, ctx, name, data)
    }
    fn DeleteObject(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        S3Client::DeleteObject(self, ctx, name)
    }
    fn DeleteObjects(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()> {
        S3Client::DeleteObjects(self, ctx, names)
    }
    fn HeadObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Option<s3like::HeadObjectResp>> {
        S3Client::HeadObject(self, ctx, name).map(Some)
    }
    fn IsObjectExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        S3Client::IsObjectExists(self, ctx, name)
    }
    fn ListObjects(
        &self,
        ctx: &storeapi::Context,
        extra_prefix: &str,
        start_after: &str,
        token: Option<&str>,
        max_keys: isize,
    ) -> Result<Option<s3like::ListResp>> {
        S3Client::ListObjects(self, ctx, extra_prefix, start_after, token, max_keys).map(Some)
    }
    fn CopyObject(&self, ctx: &storeapi::Context, params: &s3like::CopyInput) -> Result<()> {
        S3Client::CopyObject(self, ctx, params)
    }
    fn MultipartWriter(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Option<Box<dyn objectio::Writer>>> {
        S3Client::MultipartWriter(self, ctx, name).map(Some)
    }
    fn MultipartUploader(
        &self,
        name: &str,
        part_size: i64,
        concurrency: i32,
    ) -> Option<Box<dyn s3like::Uploader>> {
        Some(S3Client::MultipartUploader(
            self,
            name,
            part_size,
            concurrency,
        ))
    }
    fn PresignObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        expire: Duration,
    ) -> Result<String> {
        S3Client::PresignObject(self, ctx, name, expire)
    }
}
