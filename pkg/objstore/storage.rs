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

// 对象存储（object store）统一抽象层。
//
// 对应 Go `pkg/objstore/storage.go`：定义可取消的 `Context`、读写/遍历选项，
// 以及 `Storage` trait（删除、读写、Open、WalkDir、URI、Create、Rename、Presign 等）。
// `New` / `NewFromURL` 按后端类型分发到本地、内存、noop，或经 `external_factory`
// 接入云厂商实现。对象存储指按键（路径）存取完整对象的外部存储服务。

use std::any::Any;
use std::io::{Read, Seek};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::hdfs::NewHDFSStorage;
use crate::local::NewLocalStorage;
use crate::memstore::NewMemStorage;
use crate::noop::newNoopStorage;
use crate::parse::{ParseBackend, StorageBackend};

/// TombstoneSize is reported by directory walks for objects deleted during a walk.
/// 遍历目录时若对象在遍历过程中被删除，回调上报的尺寸哨兵值（墓碑）。
pub const TombstoneSize: i64 = -1;

/// A small, cloneable cancellation context matching the checks made by Go storage methods.
/// 可克隆的取消上下文，对齐 Go 存储方法中的 context 取消检查。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
}

impl Context {
    /// 构造未取消的背景上下文。
    pub fn background() -> Self {
        Self::default()
    }

    /// 复用调用方的取消标志，使上层 Go 形状的 context 能贯穿存储创建。
    pub fn from_cancellation_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// 标记为已取消，后续 `check_cancelled` 将返回错误。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// 若已取消则返回 `"context canceled"` 错误。
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(anyhow!("context canceled"))
        } else {
            Ok(())
        }
    }

    /// 等待至多 `duration`，期间若被取消则提前返回错误。
    pub fn wait_timeout(&self, duration: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + duration;
        loop {
            self.check_cancelled()?;
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(10)));
        }
    }
}

/// 目录遍历（WalkDir）选项：子目录、对象前缀、跳过子目录、墓碑与起始键。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WalkOption {
    /// 相对存储前缀的子目录。
    pub sub_dir: String,
    /// 对象键前缀过滤。
    pub obj_prefix: String,
    /// 为真时不递归进入子目录。
    pub skip_sub_dir: bool,
    /// 是否在回调中包含遍历中被删对象的墓碑项。
    pub include_tombstone: bool,
    /// 从该键之后开始列举（对应 ListObjects 的 StartAfter）。
    pub start_after: String,
}

/// 打开对象读取时的字节范围选项。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReaderOption {
    /// 起始偏移（含）；`None` 表示从文件头开始。
    pub start_offset: Option<i64>,
    /// 结束偏移（不含）；`None` 表示读到文件尾。
    pub end_offset: Option<i64>,
}

/// 创建写入器时的选项占位（当前无额外字段，保留与 Go 对齐）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WriterOption;

/// 跨存储复制规格：源对象键与目标对象键。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopySpec {
    /// 源对象名。
    pub from: String,
    /// 目标对象名。
    pub to: String,
}

/// 对象读取器：可读可 Seek，并支持关闭与查询文件大小。
pub trait ObjectReader: Read + Seek + Send {
    fn close(&mut self) -> Result<()>;
    fn get_file_size(&self) -> Result<i64>;
}

/// 对象写入器：按块写入并在 close 时提交。
pub trait ObjectWriter: Send {
    fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<usize>;
    fn close(&mut self, ctx: &Context) -> Result<()>;
}

/// 共享的对象存储引用。
pub type StorageRef = Arc<dyn Storage>;

/// Storage mirrors the object-store surface used by local, memory, noop and lock code.
/// 对象存储统一接口：本地、内存、noop 与锁模块等共用的操作面。
pub trait Storage: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    /// 删除单个对象。
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()>;

    /// 默认实现：顺序删除多个对象。
    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()> {
        for name in names {
            self.DeleteFile(ctx, name)?;
        }
        Ok(())
    }

    /// 一次性写入完整对象内容。
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()>;
    /// 一次性读出完整对象内容。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>>;
    /// 判断对象是否存在。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool>;
    /// 打开流式读取器，可带字节范围。
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn ObjectReader>>;
    /// 遍历目录，对每个对象调用 callback(路径, 大小)。
    fn WalkDir(
        &self,
        ctx: &Context,
        option: Option<&WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;
    /// 返回存储 URI（如 `s3://bucket/prefix/`）。
    fn URI(&self) -> String;
    /// 创建流式写入器。
    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>>;
    /// 重命名对象。
    fn Rename(&self, ctx: &Context, old_name: &str, new_name: &str) -> Result<()>;
    /// 生成带过期时间的预签名访问 URL。
    fn PresignFile(&self, ctx: &Context, name: &str, duration: Duration) -> Result<String>;
    /// 关闭存储并释放资源。
    fn Close(&self);

    /// 默认不支持跨存储复制，返回错误。
    fn CopyFrom(&self, _ctx: &Context, _source: StorageRef, _spec: &CopySpec) -> Result<()> {
        Err(anyhow!("copy is not supported by {}", self.URI()))
    }

    /// 是否提供强一致性读（默认否）。
    fn is_strong_consistent(&self) -> bool {
        false
    }
}

/// 权限检查项：列举、读对象、访问桶。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Permission {
    ListObjects,
    GetObject,
    AccessBuckets,
}

/// 外部（云）后端工厂：由集成模块注入 S3/GCS 等实现。
pub type ExternalFactory =
    Arc<dyn Fn(&Context, &StorageBackend, &Options) -> Result<StorageRef> + Send + Sync + 'static>;

/// 构造存储时的选项：是否回传凭证、HTTP 客户端、权限检查与外部工厂。
#[derive(Clone, Default)]
pub struct Options {
    /// 是否在返回的 backend 中保留/回填凭证字段。
    pub send_credentials: bool,
    /// 可选的自定义 HTTP 客户端配置。
    pub http_client: Option<HttpClient>,
    /// 创建后需要校验的权限列表。
    pub check_permissions: Vec<Permission>,
    /// 是否在创建 S3 存储时探测 Object Lock，供外部后端工厂使用。
    pub check_s3_object_lock_options: bool,
    /// Cloud implementations are supplied by their own production modules at integration time.
    /// 云实现由各自生产模块在集成时注入。
    pub external_factory: Option<ExternalFactory>,
}

/// Create is the compatibility constructor retaining the Go sendCreds option.
/// 兼容 Go `Create`：仅暴露 sendCreds 布尔参数的构造入口。
pub fn Create(
    ctx: &Context,
    backend: &StorageBackend,
    send_credentials: bool,
) -> Result<StorageRef> {
    New(
        ctx,
        backend,
        Some(&Options {
            send_credentials,
            ..Options::default()
        }),
    )
}

/// 使用默认 Options 构造存储。
pub fn NewWithDefaultOpt(ctx: &Context, backend: &StorageBackend) -> Result<StorageRef> {
    New(ctx, backend, None)
}

/// 从 URI 字符串解析后端并构造存储；`memstore://` 走内存实现捷径。
pub fn NewFromURL(ctx: &Context, uri: &str) -> Result<StorageRef> {
    if uri.is_empty() {
        return Err(anyhow!("empty store is not allowed"));
    }
    if uri.starts_with("memstore://") {
        return Ok(Arc::new(NewMemStorage()));
    }
    let backend = ParseBackend(uri, None)?;
    NewWithDefaultOpt(ctx, &backend)
}

/// New keeps Go's backend dispatch. Network backends are delegated to their real provider modules.
/// 按后端类型分发：本地/noop/内存本地构造，其余委托 `external_factory`。
pub fn New(
    ctx: &Context,
    backend: &StorageBackend,
    options: Option<&Options>,
) -> Result<StorageRef> {
    let default_options = Options::default();
    let options = options.unwrap_or(&default_options);
    match backend {
        StorageBackend::Local(local) => Ok(Arc::new(NewLocalStorage(&local.path)?)),
        StorageBackend::Hdfs(hdfs) => Ok(Arc::new(NewHDFSStorage(hdfs.remote.clone()))),
        StorageBackend::Noop => Ok(Arc::new(newNoopStorage())),
        StorageBackend::MemStore => Ok(Arc::new(NewMemStorage())),
        _ => options
            .external_factory
            .as_ref()
            .ok_or_else(|| anyhow!("storage {} is not supported yet", backend.kind()))?(
            ctx, backend, options,
        ),
    }
}

/// HTTP 传输层连接池相关参数。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HttpTransport {
    /// 每主机最大连接数；0 通常表示不额外限制。
    pub max_connections_per_host: usize,
    /// 全局最大空闲连接数。
    pub max_idle_connections: usize,
    /// 每主机最大空闲连接数。
    pub max_idle_connections_per_host: usize,
    /// 是否禁用 HTTP keep-alive。
    pub disable_keep_alives: bool,
}

/// 包装传输配置的 HTTP 客户端描述。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HttpClient {
    pub transport: HttpTransport,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self {
            max_connections_per_host: 0,
            max_idle_connections: 100,
            max_idle_connections_per_host: 2,
            disable_keep_alives: false,
        }
    }
}

/// 克隆默认 HTTP 传输配置；第二个返回值表示是否成功（始终为 true）。
pub fn CloneDefaultHTTPTransport() -> (HttpTransport, bool) {
    (HttpTransport::default(), true)
}

/// 按并发度构造默认 HTTP 客户端（空闲连接数与每主机空闲数等于 concurrency）。
pub fn GetDefaultHTTPClient(concurrency: usize) -> HttpClient {
    HttpClient {
        transport: HttpTransport {
            max_idle_connections: concurrency,
            max_idle_connections_per_host: concurrency,
            ..HttpTransport::default()
        },
    }
}

/// ReadDataInRange reads exactly p.len() bytes from [start, start + p.len()).
/// 从对象的 `[start, start+p.len())` 精确读取 `p.len()` 字节到缓冲区。
pub fn ReadDataInRange(
    ctx: &Context,
    storage: StorageRef,
    name: &str,
    start: i64,
    p: &mut [u8],
) -> Result<usize> {
    if start < 0 {
        return Err(anyhow!("invalid negative start offset: {start}"));
    }
    let length = i64::try_from(p.len())
        .map_err(|_| anyhow!("range calculation overflow: start={start}, len={}", p.len()))?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| anyhow!("range calculation overflow: start={start}, len={}", p.len()))?;
    let mut reader = storage.Open(
        ctx,
        name,
        Some(&ReaderOption {
            start_offset: Some(start),
            end_offset: Some(end),
        }),
    )?;
    let read_result = reader.read_exact(p).map(|()| p.len()).map_err(Into::into);
    // 关闭失败只记录，不覆盖已成功的读取结果。
    if let Err(error) = reader.close() {
        eprintln!("failed to close reader: {error:#}");
    }
    read_result
}
