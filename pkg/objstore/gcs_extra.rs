// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// GCS 扩展：分片上传 Writer、multipart 结果结构体与默认 HTTP 传输参数。

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result, bail};
use bytes::Bytes;
use object_store::path::Path;
use object_store::{Attributes, MultipartUpload, ObjectStore, PutMultipartOptions};

use crate::azblob::ObjectStorageCore;
use crate::objectio;

/// GCS 分片最小大小（5 MiB）。
pub const GCS_MINIMUM_CHUNK_SIZE: i64 = 5 * 1024 * 1024;
/// GCS 分片最大大小（5 GiB）。
pub const GCS_MAXIMUM_CHUNK_SIZE: i64 = 5 * 1024 * 1024 * 1024;
/// 单次 multipart 允许的最大分片数。
pub const GCS_MAXIMUM_PARTS: usize = 10_000;
/// 默认重试次数。
pub const DEFAULT_RETRY: usize = 3;
/// 预签名 URL 默认有效期（6 小时）。
pub const DEFAULT_SIGNED_URL_EXPIRY: Duration = Duration::from_secs(6 * 60 * 60);

/// GCS multipart 上传 Writer：按分片 `put_part`，关闭时 complete 或 abort。
pub struct GCSWriter {
    context: objectio::Context,
    core: ObjectStorageCore,
    path: Path,
    upload: Option<Box<dyn MultipartUpload>>,
    part_size: i64,
    workers: usize,
    current_part: usize,
    total_size: u64,
    closed: bool,
}

impl GCSWriter {
    /// 无额外属性发起 multipart 上传。
    pub fn new(
        context: objectio::Context,
        store: Arc<dyn ObjectStore>,
        path: impl Into<String>,
        part_size: i64,
        workers: usize,
    ) -> Result<Self> {
        Self::new_with_attributes(context, store, path, part_size, workers, Attributes::new())
    }

    /// 带对象属性发起 multipart；校验分片大小与 worker 数。
    pub fn new_with_attributes(
        context: objectio::Context,
        store: Arc<dyn ObjectStore>,
        path: impl Into<String>,
        part_size: i64,
        workers: usize,
        attributes: Attributes,
    ) -> Result<Self> {
        if !(GCS_MINIMUM_CHUNK_SIZE..=GCS_MAXIMUM_CHUNK_SIZE).contains(&part_size) {
            bail!(
                "invalid chunk size: {part_size}. Chunk size must be between {GCS_MINIMUM_CHUNK_SIZE} and {GCS_MAXIMUM_CHUNK_SIZE}"
            );
        }
        if workers == 0 {
            bail!("parallel worker count must be positive");
        }
        let core = ObjectStorageCore::new(store)?;
        let path = Path::from(path.into());
        let upload = core
            .runtime
            .block_on(core.store.put_multipart_opts(
                &path,
                PutMultipartOptions {
                    attributes,
                    ..Default::default()
                },
            ))
            .context("failed to initiate GCS multipart upload")?;
        Ok(Self {
            context,
            core,
            path,
            upload: Some(upload),
            part_size,
            workers,
            current_part: 1,
            total_size: 0,
            closed: false,
        })
    }

    /// 配置的分片大小。
    pub fn part_size(&self) -> i64 {
        self.part_size
    }

    /// 配置的并行 worker 数。
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// 已上传总字节数。
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// 目标对象路径。
    pub fn object_path(&self) -> &Path {
        &self.path
    }

    /// 上传一个分片并递增 part 编号。
    fn upload_part(&mut self, data: &[u8]) -> Result<()> {
        if self.current_part > GCS_MAXIMUM_PARTS {
            bail!("exceed maximum parts {GCS_MAXIMUM_PARTS}");
        }
        self.context.check()?;
        let upload = self
            .upload
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("multipart upload is closed"))?;
        let future = upload.put_part(Bytes::copy_from_slice(data).into());
        self.core
            .runtime
            .block_on(future)
            .with_context(|| format!("failed to upload part {}", self.current_part))?;
        self.current_part += 1;
        self.total_size += data.len() as u64;
        Ok(())
    }

    /// 完成或中止上传：无分片时 abort；有分片则 complete，失败再 abort。
    fn finish(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.context.check()?;
        let Some(mut upload) = self.upload.take() else {
            self.closed = true;
            return Ok(());
        };

        if self.current_part == 1 {
            // Go returns success without finalizing an upload that has no parts.
            // Abort explicitly so the mature backend cannot leak an incomplete MPU.
            // Go 无分片时直接成功；此处显式 abort，避免后端残留未完成 multipart。
            self.core.runtime.block_on(upload.abort())?;
            self.closed = true;
            return Ok(());
        }

        if let Err(error) = self.core.runtime.block_on(upload.complete()) {
            let cancel_error = self.core.runtime.block_on(upload.abort()).err();
            return match cancel_error {
                Some(cancel_error) => Err(anyhow::anyhow!(
                    "failed to finalize multipart upload: {error}; failed to cancel multipart upload: {cancel_error}"
                )),
                None => Err(error).context("failed to finalize multipart upload"),
            };
        }
        self.closed = true;
        Ok(())
    }
}

impl objectio::Writer for GCSWriter {
    fn write(&mut self, ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        ctx.check()?;
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "writer is closed",
            ));
        }
        self.upload_part(data).map_err(io::Error::other)?;
        Ok(data.len())
    }

    fn close(&mut self, ctx: &objectio::Context) -> io::Result<()> {
        ctx.check()?;
        self.finish().map_err(io::Error::other)
    }
}

impl Drop for GCSWriter {
    fn drop(&mut self) {
        // 未正常关闭时 abort，防止泄漏未完成上传。
        if self.closed {
            return;
        }
        if let Some(mut upload) = self.upload.take() {
            let _ = self.core.runtime.block_on(upload.abort());
        }
    }
}

/// 发起 multipart 的响应摘要（桶、键、upload_id）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitiateMultipartUploadResult {
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
}

/// 单个已上传分片的编号与 ETag。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Part {
    pub part_number: usize,
    pub etag: String,
}

/// 完成 multipart 时提交的分片列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompleteMultipartUpload {
    pub parts: Vec<Part>,
}

impl CompleteMultipartUpload {
    /// 按 part_number 排序，满足完成上传接口要求。
    pub fn sort_parts(&mut self) {
        self.parts.sort_by_key(|part| part.part_number);
    }
}

/// HTTP 传输超时与连接池参数（对齐 Go transport 默认值意图）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportConfig {
    pub connect_timeout: Duration,
    pub keep_alive: Duration,
    pub max_idle_connections: usize,
    pub idle_timeout: Duration,
    pub tls_handshake_timeout: Duration,
    pub expect_continue_timeout: Duration,
    pub max_idle_connections_per_host: usize,
}

/// 构造默认传输配置；每主机空闲连接数约为 CPU 并行度 + 1。
pub fn create_transport() -> TransportConfig {
    TransportConfig {
        connect_timeout: Duration::from_secs(30),
        keep_alive: Duration::from_secs(30),
        max_idle_connections: 100,
        idle_timeout: Duration::from_secs(90),
        tls_handshake_timeout: Duration::from_secs(10),
        expect_continue_timeout: Duration::from_secs(1),
        max_idle_connections_per_host: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            + 1,
    }
}
