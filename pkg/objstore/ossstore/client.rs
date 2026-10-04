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

// 阿里云 OSS（兼容 S3 API）对象存储客户端。
//
// `Client` 在底层 `API` 之上封装带 Bucket/前缀的读写、权限探测、列举与复制；
// 并提供分片上传 Writer（`MultipartWriter`）与并发 Uploader（`MultipartUploader`）。

use std::io::{self, Read};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::{
    API, AbortMultipartUploadInput, CompleteMultipartUploadInput, CompletedPart, CopyObjectInput,
    CreateMultipartUploadInput, CreateMultipartUploadOutput, DeleteObjectInput, DeleteObjectsInput,
    GetObjectInput, HeadObjectInput, ListObjectsV2Input, PutObjectInput, UploadPartInput,
    error_code,
};

/// Head/Get 对象不存在时的错误码（与 S3/OSS 对齐）。
const NO_SUCH_KEY: &str = "NoSuchKey";
/// Go OSS SDK uploader 的默认分片大小：6 MiB。
const DEFAULT_MULTIPART_SIZE: usize = 6 * 1024 * 1024;
/// Go OSS SDK uploader 的默认并发数。
const DEFAULT_MULTIPART_CONCURRENCY: i32 = 3;

/// OSS 前缀客户端：持有 API、桶前缀与备份 S3 选项。
#[derive(Clone)]
pub struct Client {
    svc: Arc<dyn API>,
    presign_svc: Arc<dyn API>,
    bucket_prefix: storeapi::BucketPrefix,
    options: s3like::backuppb::S3,
}

impl Client {
    /// 由具体 API 类型构造 Client。
    pub fn new<T>(
        svc: Arc<T>,
        bucket_prefix: storeapi::BucketPrefix,
        options: s3like::backuppb::S3,
    ) -> Self
    where
        T: API + 'static,
    {
        let presign_svc = svc.clone();
        Self {
            svc,
            presign_svc,
            bucket_prefix,
            options,
        }
    }

    /// 由已装箱的 `dyn API` 构造 Client。
    pub fn from_dyn(
        svc: Arc<dyn API>,
        bucket_prefix: storeapi::BucketPrefix,
        options: s3like::backuppb::S3,
    ) -> Self {
        let presign_svc = svc.clone();
        Self {
            svc,
            presign_svc,
            bucket_prefix,
            options,
        }
    }

    /// 由独立的数据 API 与公网预签名 API 构造 Client。
    pub fn with_presign_api(
        svc: Arc<dyn API>,
        presign_svc: Arc<dyn API>,
        bucket_prefix: storeapi::BucketPrefix,
        options: s3like::backuppb::S3,
    ) -> Self {
        Self {
            svc,
            presign_svc,
            bucket_prefix,
            options,
        }
    }

    /// 探测桶是否存在；不存在则返回 `ErrNoSuchBucket`。
    pub fn CheckBucketExistence(&self, ctx: &storeapi::Context) -> Result<()> {
        if self.svc.is_bucket_exist(ctx, &self.options.Bucket)? {
            Ok(())
        } else {
            Err(anyhow!(s3like::ErrNoSuchBucket))
        }
    }

    /// 探测 ListObjects 权限（最多列 1 个键）。
    pub fn CheckListObjects(&self, ctx: &storeapi::Context) -> Result<()> {
        self.svc.list_objects_v2(
            ctx,
            &ListObjectsV2Input {
                bucket: self.options.Bucket.clone(),
                prefix: self.bucket_prefix.PrefixStr(),
                max_keys: 1,
                ..Default::default()
            },
        )?;
        Ok(())
    }

    /// 探测 GetObject 权限；对象不存在（NoSuchKey）仍视为权限检查通过。
    pub fn CheckGetObject(&self, ctx: &storeapi::Context) -> Result<()> {
        let input = GetObjectInput {
            bucket: self.options.Bucket.clone(),
            key: self
                .bucket_prefix
                .ObjectKey(&storeapi::GenPermCheckObjectKey()),
            range: None,
        };
        match self.svc.get_object(ctx, &input) {
            Ok(mut response) => {
                let _ = response.body.close();
                Ok(())
            }
            Err(error) if error_code(&error) == Some(NO_SUCH_KEY) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// 探测 Put 与 Delete 权限：先写入探测键再删除；删除失败仅打 warn。
    pub fn CheckPutAndDeleteObject(&self, ctx: &storeapi::Context) -> Result<()> {
        let key = self
            .bucket_prefix
            .ObjectKey(&storeapi::GenPermCheckObjectKey());
        let put_result = self.svc.put_object(
            ctx,
            &PutObjectInput {
                bucket: self.options.Bucket.clone(),
                key: key.clone(),
                body: b"check".to_vec(),
            },
        );
        let delete_result = self.svc.delete_object(
            ctx,
            &DeleteObjectInput {
                bucket: self.options.Bucket.clone(),
                key: key.clone(),
            },
        );
        if let Err(error) = &delete_result {
            log::warn!(
                "failed to delete object used for permission check, bucket={}, key={key}: {error:#}",
                self.options.Bucket
            );
        }
        match put_result {
            Err(error) => Err(error),
            Ok(()) => delete_result,
        }
    }

    /// 按可选字节范围读取对象，返回 s3like 风格响应。
    pub fn GetObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        start_offset: i64,
        end_offset: i64,
    ) -> Result<s3like::GetResp> {
        let (is_full_range, range) = storeapi::GetHTTPRange(start_offset, end_offset);
        let output = self.svc.get_object(
            ctx,
            &GetObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
                range: (!range.is_empty()).then_some(range),
            },
        )?;
        Ok(s3like::GetResp {
            Body: output.body,
            IsFullRange: is_full_range,
            ContentLength: output.content_length,
            ContentRange: output.content_range,
        })
    }

    /// 整对象 Put，并记录 OSS PutObject API 调用计数。
    pub fn PutObject(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        s3like::RecordAPICall(s3like::BACKEND_OSS, s3like::API_CALL_PUT_OBJECT);
        self.svc.put_object(
            ctx,
            &PutObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
                body: data.to_vec(),
            },
        )
    }

    /// 删除单个对象。
    pub fn DeleteObject(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        self.svc.delete_object(
            ctx,
            &DeleteObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
            },
        )
    }

    /// 为对象生成预签名 GET URL。
    pub fn PresignObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        expire: Duration,
    ) -> Result<String> {
        self.presign_svc.presign_get_object(
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
            },
        )
    }

    /// 通过 HeadObject 判断对象是否存在；NoSuchKey 视为不存在。
    pub fn IsObjectExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        s3like::RecordAPICall(s3like::BACKEND_OSS, s3like::API_CALL_HEAD_OBJECTS);
        match self.svc.head_object(
            ctx,
            &HeadObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
            },
        ) {
            Ok(()) => Ok(true),
            Err(error) if error_code(&error) == Some(NO_SUCH_KEY) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Head 对象元数据（当前返回空默认响应，仅校验调用成功）。
    pub fn HeadObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<s3like::HeadObjectResp> {
        s3like::RecordAPICall(s3like::BACKEND_OSS, s3like::API_CALL_HEAD_OBJECTS);
        self.svc.head_object(
            ctx,
            &HeadObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
            },
        )?;
        Ok(s3like::HeadObjectResp::default())
    }

    /// ListObjectsV2：支持附加前缀、start_after 与 continuation token。
    pub fn ListObjects(
        &self,
        ctx: &storeapi::Context,
        extra_prefix: &str,
        start_after: &str,
        continuation_token: Option<&str>,
        max_keys: isize,
    ) -> Result<s3like::ListResp> {
        s3like::RecordAPICall(s3like::BACKEND_OSS, s3like::API_CALL_LIST_OBJECTS);
        let output = self.svc.list_objects_v2(
            ctx,
            &ListObjectsV2Input {
                bucket: self.bucket_prefix.Bucket.clone(),
                prefix: self.bucket_prefix.ObjectKey(extra_prefix),
                max_keys: max_keys as i32,
                continuation_token: continuation_token.map(str::to_owned),
                start_after: (!start_after.is_empty())
                    .then(|| self.bucket_prefix.ObjectKey(start_after)),
            },
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

    /// 桶内/跨前缀复制对象。
    pub fn CopyObject(&self, ctx: &storeapi::Context, params: &s3like::CopyInput) -> Result<()> {
        self.svc.copy_object(
            ctx,
            &CopyObjectInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(&params.ToKey),
                source_bucket: params.FromLoc.Bucket.clone(),
                source_key: params.FromLoc.ObjectKey(&params.FromKey),
            },
        )
    }

    /// 发起分片上传并返回流式 `MultipartWriter`（每次 write 上传一片）。
    pub fn MultipartWriter(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Box<dyn objectio::Writer>> {
        let output = self.svc.initiate_multipart_upload(
            ctx,
            &CreateMultipartUploadInput {
                bucket: self.bucket_prefix.Bucket.clone(),
                key: self.bucket_prefix.ObjectKey(name),
                server_side_encryption: non_empty(&self.options.Sse),
                sse_kms_key_id: non_empty(&self.options.SseKmsKeyId),
                storage_class: non_empty(&self.options.StorageClass),
            },
        )?;
        Ok(Box::new(MultipartWriter {
            svc: self.svc.clone(),
            create_output: output,
            complete_parts: Vec::with_capacity(128),
        }))
    }

    /// 构造可读流式并发分片 Uploader。
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
            options: self.options.clone(),
            part_size,
            concurrency,
        })
    }
}

/// 空字符串转为 None，非空则 Some（用于可选 SSE/存储类字段）。
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

impl s3like::PrefixClient for Client {
    fn CheckBucketExistence(&self, ctx: &storeapi::Context) -> Result<()> {
        Client::CheckBucketExistence(self, ctx)
    }
    fn CheckListObjects(&self, ctx: &storeapi::Context) -> Result<()> {
        Client::CheckListObjects(self, ctx)
    }
    fn CheckGetObject(&self, ctx: &storeapi::Context) -> Result<()> {
        Client::CheckGetObject(self, ctx)
    }
    fn CheckPutAndDeleteObject(&self, ctx: &storeapi::Context) -> Result<()> {
        Client::CheckPutAndDeleteObject(self, ctx)
    }
    fn GetObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        start_offset: i64,
        end_offset: i64,
    ) -> Result<Option<s3like::GetResp>> {
        Client::GetObject(self, ctx, name, start_offset, end_offset).map(Some)
    }
    fn PutObject(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        Client::PutObject(self, ctx, name, data)
    }
    fn DeleteObject(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        Client::DeleteObject(self, ctx, name)
    }
    fn DeleteObjects(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()> {
        Client::DeleteObjects(self, ctx, names)
    }
    fn PresignObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        expire: Duration,
    ) -> Result<String> {
        Client::PresignObject(self, ctx, name, expire)
    }
    fn HeadObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Option<s3like::HeadObjectResp>> {
        Client::HeadObject(self, ctx, name).map(Some)
    }
    fn IsObjectExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        Client::IsObjectExists(self, ctx, name)
    }
    fn ListObjects(
        &self,
        ctx: &storeapi::Context,
        extra_prefix: &str,
        start_after: &str,
        continuation_token: Option<&str>,
        max_keys: isize,
    ) -> Result<Option<s3like::ListResp>> {
        Client::ListObjects(
            self,
            ctx,
            extra_prefix,
            start_after,
            continuation_token,
            max_keys,
        )
        .map(Some)
    }
    fn CopyObject(&self, ctx: &storeapi::Context, params: &s3like::CopyInput) -> Result<()> {
        Client::CopyObject(self, ctx, params)
    }
    fn MultipartWriter(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Option<Box<dyn objectio::Writer>>> {
        Client::MultipartWriter(self, ctx, name).map(Some)
    }
    fn MultipartUploader(
        &self,
        name: &str,
        part_size: i64,
        concurrency: i32,
    ) -> Option<Box<dyn s3like::Uploader>> {
        Some(Client::MultipartUploader(
            self,
            name,
            part_size,
            concurrency,
        ))
    }
}

/// 流式分片写入：每次 `write` 上传一片，`close` 时 CompleteMultipartUpload。
struct MultipartWriter {
    svc: Arc<dyn API>,
    create_output: CreateMultipartUploadOutput,
    complete_parts: Vec<CompletedPart>,
}

impl objectio::Writer for MultipartWriter {
    fn write(&mut self, ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        let part_number = self.complete_parts.len().wrapping_add(1) as i32;
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
            )
            .map_err(io::Error::other)?;
        self.complete_parts.push(CompletedPart {
            etag: output.etag,
            part_number,
        });
        Ok(data.len())
    }

    fn close(&mut self, ctx: &objectio::Context) -> io::Result<()> {
        self.svc
            .complete_multipart_upload(
                ctx,
                &CompleteMultipartUploadInput {
                    bucket: self.create_output.bucket.clone(),
                    key: self.create_output.key.clone(),
                    upload_id: self.create_output.upload_id.clone(),
                    parts: self.complete_parts.clone(),
                },
            )
            .map_err(io::Error::other)?;
        Ok(())
    }
}

/// 从 `Read` 读入后按 part_size 切分，多线程并发 UploadPart，最后 Complete。
struct MultipartUploader {
    svc: Arc<dyn API>,
    bucket: String,
    key: String,
    options: s3like::backuppb::S3,
    part_size: i64,
    concurrency: i32,
}

impl s3like::Uploader for MultipartUploader {
    fn Upload(&self, ctx: &storeapi::Context, reader: &mut dyn Read) -> Result<()> {
        ctx.check()?;
        let part_size = if self.part_size <= 0 {
            DEFAULT_MULTIPART_SIZE
        } else {
            usize::try_from(self.part_size)
                .map_err(|_| anyhow!("multipart part size is too large"))?
        };
        let concurrency = if self.concurrency <= 0 {
            DEFAULT_MULTIPART_CONCURRENCY
        } else {
            self.concurrency
        };
        let create = self.svc.initiate_multipart_upload(
            ctx,
            &CreateMultipartUploadInput {
                bucket: self.bucket.clone(),
                key: self.key.clone(),
                server_side_encryption: non_empty(&self.options.Sse),
                sse_kms_key_id: non_empty(&self.options.SseKmsKeyId),
                storage_class: non_empty(&self.options.StorageClass),
            },
        )?;

        let completed = Mutex::new(Vec::<CompletedPart>::new());
        let failure = Mutex::new(None::<anyhow::Error>);
        let workers = usize::try_from(concurrency).unwrap_or(1).max(1);
        let (sender, receiver) = mpsc::sync_channel::<(i32, Vec<u8>)>(workers);
        let receiver = Mutex::new(receiver);
        // 与 Go SDK 一样，以并发数为界流水读取并上传，避免先缓存整个对象。
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let receiver = &receiver;
                let failure = &failure;
                let completed = &completed;
                let create = &create;
                scope.spawn(move || {
                    loop {
                        if failure.lock().unwrap().is_some() {
                            return;
                        }
                        let Ok((part_number, body)) = receiver.lock().unwrap().recv() else {
                            return;
                        };
                        match self.svc.upload_part(
                            ctx,
                            &UploadPartInput {
                                bucket: create.bucket.clone(),
                                key: create.key.clone(),
                                upload_id: create.upload_id.clone(),
                                part_number,
                                body,
                            },
                        ) {
                            Ok(output) => completed.lock().unwrap().push(CompletedPart {
                                etag: output.etag,
                                part_number,
                            }),
                            Err(error) => *failure.lock().unwrap() = Some(error),
                        }
                    }
                });
            }

            let mut part_number = 0_i32;
            'produce: while failure.lock().unwrap().is_none() {
                let mut data = vec![0; part_size];
                let mut filled = 0;
                while filled < data.len() {
                    match reader.read(&mut data[filled..]) {
                        Ok(0) => break,
                        Ok(read) => filled += read,
                        Err(error) => {
                            *failure.lock().unwrap() = Some(error.into());
                            break;
                        }
                    }
                }
                if filled == 0 || failure.lock().unwrap().is_some() {
                    break;
                }
                data.truncate(filled);
                part_number = part_number.saturating_add(1);
                let mut pending = (part_number, data);
                loop {
                    match sender.try_send(pending) {
                        Ok(()) => break,
                        Err(mpsc::TrySendError::Full(value)) => {
                            if failure.lock().unwrap().is_some() {
                                break 'produce;
                            }
                            pending = value;
                            std::thread::yield_now();
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => break 'produce,
                    }
                }
            }
            drop(sender);
        });

        if let Some(error) = failure.into_inner().unwrap() {
            let _ = self.svc.abort_multipart_upload(
                ctx,
                &AbortMultipartUploadInput {
                    bucket: create.bucket,
                    key: create.key,
                    upload_id: create.upload_id,
                },
            );
            return Err(error);
        }
        let mut completed = completed.into_inner().unwrap();
        completed.sort_by_key(|part| part.part_number);
        let complete_result = self.svc.complete_multipart_upload(
            ctx,
            &CompleteMultipartUploadInput {
                bucket: create.bucket.clone(),
                key: create.key.clone(),
                upload_id: create.upload_id.clone(),
                parts: completed,
            },
        );
        if complete_result.is_err() {
            let _ = self.svc.abort_multipart_upload(
                ctx,
                &AbortMultipartUploadInput {
                    bucket: create.bucket,
                    key: create.key,
                    upload_id: create.upload_id,
                },
            );
        }
        complete_result
    }
}
