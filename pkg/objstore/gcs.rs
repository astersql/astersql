// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Google Cloud Storage（GCS）对象存储后端：配置、访问统计与 `storeapi::Storage` 实现。

use std::error::Error as StdError;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result, bail};
use object_store::ObjectStore;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path;
use object_store::signer::Signer;
use serde::{Deserialize, Serialize};

use crate::azblob::{
    ObjectStorageCore, ObjectStoreReader, ObjectStoreWriter, StorageOptions, is_not_found,
    join_object_path, storage_class_attributes, trim_storage_prefix,
};
use crate::gcs_extra::{GCS_MINIMUM_CHUNK_SIZE, GCSWriter};
use crate::{objectio, storeapi};

/// 旗标名：GCS API endpoint（可指向模拟器或私有端点）。
pub const GCS_ENDPOINT_OPTION: &str = "gcs.endpoint";
/// 旗标名：存储类（如 STANDARD / NEARLINE）。
pub const GCS_STORAGE_CLASS_OPTION: &str = "gcs.storage-class";
/// 旗标名：预定义 ACL。
pub const GCS_PREDEFINED_ACL_OPTION: &str = "gcs.predefined-acl";
/// 旗标名：服务账号凭据 JSON 文件路径。
pub const GCS_CREDENTIALS_FILE_OPTION: &str = "gcs.credentials-file";
/// 与 Go 侧一致的客户端句柄数量语义（Rust 侧由 HTTP 池复用替代）。
pub const GCS_CLIENT_COUNT: usize = 16;

/// 运行时 GCS 配置（桶、前缀、凭据内容等）。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GCSConfig {
    pub endpoint: String,
    pub bucket: String,
    pub prefix: String,
    pub storage_class: String,
    pub predefined_acl: String,
    pub credentials_blob: String,
}

/// 从旗标/文件加载的后端选项，可 `apply` 到 [`GCSConfig`]。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GCSBackendOptions {
    pub endpoint: String,
    pub storage_class: String,
    pub predefined_acl: String,
    pub credentials_file: String,
}

impl GCSBackendOptions {
    /// 将选项写入配置；若指定凭据文件则读入 `credentials_blob`。
    pub fn apply(&self, config: &mut GCSConfig) -> Result<()> {
        config.endpoint.clone_from(&self.endpoint);
        config.storage_class.clone_from(&self.storage_class);
        config.predefined_acl.clone_from(&self.predefined_acl);
        if !self.credentials_file.is_empty() {
            config.credentials_blob = fs::read_to_string(&self.credentials_file)
                .with_context(|| format!("failed to read {}", self.credentials_file))?;
        }
        Ok(())
    }

    /// 从 [`crate::flags::FlagSet`] 解析 GCS 四项旗标。
    pub fn parse_from_flags(&mut self, flags: &crate::flags::FlagSet) -> Result<()> {
        self.endpoint = flags.get(GCS_ENDPOINT_OPTION)?;
        self.storage_class = flags.get(GCS_STORAGE_CLASS_OPTION)?;
        self.predefined_acl = flags.get(GCS_PREDEFINED_ACL_OPTION)?;
        self.credentials_file = flags.get(GCS_CREDENTIALS_FILE_OPTION)?;
        Ok(())
    }
}

/// 读写请求次数与字节数的原子计数器，用于测试与指标。
#[derive(Default)]
pub struct AccessRecorder {
    read_requests: AtomicU64,
    write_requests: AtomicU64,
    read_bytes: AtomicU64,
    write_bytes: AtomicU64,
}

impl AccessRecorder {
    /// 记一次读请求并累加字节。
    fn record_read(&self, bytes: usize) {
        self.read_requests.fetch_add(1, Ordering::Relaxed);
        self.read_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 记一次写请求并累加字节。
    fn record_write(&self, bytes: usize) {
        self.write_requests.fetch_add(1, Ordering::Relaxed);
        self.write_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 仅累加读字节（同一 Open 上的后续 `read` 调用）。
    fn record_read_bytes(&self, bytes: usize) {
        self.read_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 返回 (读次数, 写次数, 读字节, 写字节) 快照。
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.read_requests.load(Ordering::Relaxed),
            self.write_requests.load(Ordering::Relaxed),
            self.read_bytes.load(Ordering::Relaxed),
            self.write_bytes.load(Ordering::Relaxed),
        )
    }
}

/// 包装 Reader：首次读计请求，后续只加字节。
struct RecordingReader {
    inner: Box<dyn objectio::Reader>,
    recorder: Arc<AccessRecorder>,
    requested: bool,
    position: u64,
    total: u64,
}

impl Read for RecordingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(output)?;
        if !self.requested {
            self.requested = true;
            self.recorder.record_read(count);
        } else {
            self.recorder.record_read_bytes(count);
        }
        self.position = self.position.saturating_add(count as u64);
        Ok(count)
    }
}

impl Seek for RecordingReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let old_position = self.position;
        let new_position = self.inner.seek(position)?;
        self.position = new_position;
        if new_position != old_position && new_position < self.total {
            // Go closes the current range reader on an effective seek and the
            // next read creates a new HTTP range request. A seek past EOF uses
            // an empty reader and therefore must not create a request.
            self.requested = false;
        }
        Ok(new_position)
    }
}

impl objectio::Reader for RecordingReader {
    fn close(&mut self) -> io::Result<()> {
        self.inner.close()
    }

    fn file_size(&self) -> io::Result<i64> {
        self.inner.file_size()
    }
}

/// 包装 Writer：`close` 时按累计字节记一次写请求。
struct RecordingWriter {
    inner: Box<dyn objectio::Writer>,
    recorder: Arc<AccessRecorder>,
    bytes: usize,
    recorded: bool,
}

impl objectio::Writer for RecordingWriter {
    fn write(&mut self, ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(ctx, data)?;
        self.bytes += count;
        Ok(count)
    }

    fn close(&mut self, ctx: &objectio::Context) -> io::Result<()> {
        self.inner.close(ctx)?;
        if !self.recorded {
            self.recorder.record_write(self.bytes);
            self.recorded = true;
        }
        Ok(())
    }
}

/// 预签名闭包类型：给定对象路径与过期时间，返回 GET URL。
type Presigner = dyn Fn(&Path, Duration) -> Result<String> + Send + Sync;

/// GCS 存储实现：配置 + 共享 `ObjectStorageCore` + 可选统计与预签名。
pub struct GCSStorage {
    config: GCSConfig,
    core: ObjectStorageCore,
    access_recorder: Option<Arc<AccessRecorder>>,
    presigner: Option<Arc<Presigner>>,
    client_count: usize,
}

impl GCSStorage {
    /// 用已有 `ObjectStore` 构造（测试常用内存/本地实现注入）。
    pub fn with_store(
        config: GCSConfig,
        store: Arc<dyn ObjectStore>,
        access_recorder: Option<Arc<AccessRecorder>>,
    ) -> Result<Self> {
        if config.bucket.is_empty() {
            bail!("bucket cannot be empty to access GCS storage");
        }
        Ok(Self {
            config,
            core: ObjectStorageCore::new(store)?,
            access_recorder,
            presigner: None,
            client_count: GCS_CLIENT_COUNT,
        })
    }

    /// 返回当前配置引用。
    pub fn options(&self) -> &GCSConfig {
        &self.config
    }

    /// 返回语义上的客户端数量（对齐 Go）。
    pub fn client_count(&self) -> usize {
        self.client_count
    }

    /// 重置客户端计数；HTTP 池由 object_store 持有，无需真正重建句柄。
    pub fn reset(&mut self) -> Result<()> {
        // object_store owns a production HTTP pool and refreshes credentials
        // internally. Keeping the same pool is the Rust equivalent of rebuilding
        // Go's 16 handles without discarding in-flight-safe shared state.
        // object_store 持有生产 HTTP 池并内部刷新凭据；保留池等价于 Go 重建 16 句柄。
        self.client_count = GCS_CLIENT_COUNT;
        Ok(())
    }

    /// 从另一 GCS 存储拷贝对象；同 store 走服务端 copy，否则 get+put。
    pub fn copy_from(&self, source: &GCSStorage, spec: &storeapi::CopySpec) -> Result<()> {
        let from = source.object_path(&spec.From);
        let to = self.object_path(&spec.To);
        if Arc::ptr_eq(&self.core.store, &source.core.store) {
            return self.core.copy(&from, &to);
        }
        let bytes = source.core.get(&from)?;
        self.core.put(&to, &bytes)
    }

    /// 强一致标记占位（GCS 列表/读语义由后端保证，此处无操作）。
    pub fn mark_strong_consistency(&self) {}

    /// 配置前缀与逻辑名拼接为对象键。
    fn object_name(&self, name: &str) -> String {
        join_object_path(&[&self.config.prefix, name])
    }

    /// 逻辑名转为 `object_store::Path`。
    fn object_path(&self, name: &str) -> Path {
        Path::from(self.object_name(name))
    }

    /// 写入时附带的存储类属性。
    fn put_attributes(&self) -> object_store::Attributes {
        storage_class_attributes(&self.config.storage_class)
    }
}

/// 按配置与 [`StorageOptions`] 创建真实 GCS 客户端与预签名器。
pub fn new_gcs_storage(mut config: GCSConfig, opts: &StorageOptions) -> Result<GCSStorage> {
    if config.bucket.is_empty() {
        bail!("bucket cannot be empty to access GCS storage");
    }
    if opts.send_credentials && config.credentials_blob.is_empty() {
        bail!("gcs.credentials-file is required when send-credentials-to-tikv is true");
    }

    let mut builder = GoogleCloudStorageBuilder::from_env().with_bucket_name(config.bucket.clone());
    if !config.endpoint.is_empty() {
        builder = builder.with_base_url(&config.endpoint);
    }
    if !config.credentials_blob.is_empty() {
        builder = builder.with_service_account_key(config.credentials_blob.clone());
    } else if opts.no_credentials {
        builder = builder.with_skip_signature(true);
    }
    let concrete = Arc::new(builder.build().context("failed to create GCS client")?);
    // 不向 TiKV 发送凭据时清除内存中的 blob。
    if !opts.send_credentials {
        config.credentials_blob.clear();
    }
    let mut storage = GCSStorage::with_store(config, concrete.clone(), None)?;
    let runtime = storage.core.runtime.clone();
    storage.presigner = Some(Arc::new(move |path, expire| {
        let url = runtime.block_on(concrete.signed_url(http::Method::GET, path, expire))?;
        Ok(url.to_string())
    }));
    Ok(storage)
}

impl storeapi::StrongConsistency for GCSStorage {
    fn MarkStrongConsistency(&self) {
        self.mark_strong_consistency();
    }
}

impl storeapi::Storage for GCSStorage {
    fn AccessRequestSnapshot(&self) -> Option<(u64, u64)> {
        self.access_recorder.as_ref().map(|recorder| {
            let (gets, puts, _, _) = recorder.snapshot();
            (gets, puts)
        })
    }

    fn WriteFile(&self, ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        ctx.check()?;
        self.core
            .put_with_context(ctx, &self.object_path(name), data, self.put_attributes())?;
        if let Some(recorder) = &self.access_recorder {
            recorder.record_write(data.len());
        }
        Ok(())
    }

    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        ctx.check()?;
        let bytes = self
            .core
            .get(&self.object_path(name))
            .with_context(|| format!("failed to read GCS object {name}"))?;
        if let Some(recorder) = &self.access_recorder {
            recorder.record_read(bytes.len());
        }
        Ok(bytes)
    }

    fn FileExists(&self, ctx: &objectio::Context, name: &str) -> Result<bool> {
        ctx.check()?;
        match self.core.head(&self.object_path(name)) {
            Ok(_) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn DeleteFile(&self, ctx: &objectio::Context, name: &str) -> Result<()> {
        ctx.check()?;
        self.core.delete(&self.object_path(name), true)
    }

    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        ctx.check()?;
        let object = self.object_path(path);
        let total = self.core.head(&object)?.size as i64;
        let start = option.and_then(|value| value.StartOffset).unwrap_or(0);
        let end = option
            .and_then(|value| value.EndOffset)
            .map(|value| value.min(total))
            .unwrap_or(total);
        let reader: Box<dyn objectio::Reader> = Box::new(ObjectStoreReader::new(
            self.core.clone(),
            object,
            start,
            end,
            total,
            true,
        )?);
        // Open 时先记一次读请求（字节 0），后续 read 再累加字节。
        if let Some(recorder) = &self.access_recorder {
            recorder.record_read(0);
            Ok(Box::new(RecordingReader {
                inner: reader,
                recorder: recorder.clone(),
                requested: false,
                position: start as u64,
                total: total as u64,
            }))
        } else {
            Ok(reader)
        }
    }

    fn DeleteFiles(&self, ctx: &objectio::Context, names: &[String]) -> Result<()> {
        for name in names {
            self.DeleteFile(ctx, name)?;
        }
        Ok(())
    }

    fn WalkDir(
        &self,
        ctx: &objectio::Context,
        option: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        ctx.check()?;
        let option = option.cloned().unwrap_or_default();
        // 前缀 = 配置 prefix + SubDir + ObjPrefix，并保证目录斜杠。
        let mut prefix = join_object_path(&[&self.config.prefix, &option.SubDir]);
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        prefix.push_str(&option.ObjPrefix);
        let prefix = (!prefix.is_empty()).then(|| Path::from(prefix));
        let mut objects = self.core.list(prefix.as_ref())?;
        objects.sort_by(|left, right| left.location.cmp(&right.location));
        for object in objects {
            let name = trim_storage_prefix(object.location.as_ref(), &self.config.prefix);
            if !option.StartAfter.is_empty() && name <= option.StartAfter {
                continue;
            }
            callback(&name, object.size as i64)?;
        }
        Ok(())
    }

    fn URI(&self) -> String {
        format!("gcs://{}/{}", self.config.bucket, self.config.prefix)
    }

    fn Create(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        ctx.check()?;
        let writer: Box<dyn objectio::Writer> =
            // 并发 > 1 走分片 multipart + 缓冲写；否则单对象 put 风格 Writer。
            if let Some(option) = option.filter(|option| option.Concurrency > 1) {
                let part_size = option.PartSize.max(GCS_MINIMUM_CHUNK_SIZE);
                let writer = GCSWriter::new_with_attributes(
                    ctx.clone(),
                    self.core.store.clone(),
                    self.object_name(path),
                    part_size,
                    option.Concurrency as usize,
                    self.put_attributes(),
                )?;
                Box::new(objectio::new_buffered_writer(
                    Box::new(writer),
                    part_size as usize,
                    objectio::CompressType::NoCompression,
                    None,
                ))
            } else {
                Box::new(
                    ObjectStoreWriter::new(self.core.clone(), self.object_path(path))
                        .with_attributes(self.put_attributes()),
                )
            };
        if let Some(recorder) = &self.access_recorder {
            Ok(Box::new(RecordingWriter {
                inner: writer,
                recorder: recorder.clone(),
                bytes: 0,
                recorded: false,
            }))
        } else {
            Ok(writer)
        }
    }

    fn Rename(
        &self,
        ctx: &objectio::Context,
        old_file_name: &str,
        new_file_name: &str,
    ) -> Result<()> {
        ctx.check()?;
        // Keep Go's ReadFile -> WriteFile -> DeleteFile ordering. Besides the
        // same partial-failure behavior, this preserves access recording and
        // reapplies the configured storage class to the destination object.
        let data = self.ReadFile(ctx, old_file_name)?;
        self.WriteFile(ctx, new_file_name, &data)?;
        self.DeleteFile(ctx, old_file_name)
    }

    fn PresignFile(
        &self,
        ctx: &objectio::Context,
        file_name: &str,
        expire: Duration,
    ) -> Result<String> {
        ctx.check()?;
        let presigner = self
            .presigner
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("GCS signer is unavailable"))?;
        presigner(&self.object_path(file_name), expire)
    }

    fn Close(&self) {}
}

/// 判断错误是否值得重试（网络断开、HTTP/2 GOAWAY、401 等瞬时故障）。
pub fn should_retry(error: &(dyn StdError + 'static)) -> bool {
    if let Some(error) = error.downcast_ref::<io::Error>()
        && matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::TimedOut
        )
    {
        return true;
    }
    let message = error.to_string();
    [
        "http2: client connection force closed via ClientConn.Close",
        "broken pipe",
        "http2: client connection lost",
        "http2: server sent GOAWAY",
        "internal HTTP2 error",
        "status code 401",
    ]
    .iter()
    .any(|retryable| message.contains(retryable))
}
