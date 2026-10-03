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

// 对象存储统一接口与辅助类型。
//
// 定义 `Storage` trait（读写、遍历、预签名等）、分片上传 `Uploader`、服务端复制
// `Copier`，以及路径前缀 `Prefix` / `BucketPrefix`、权限探测常量与 HTTP Range 工具。
// 具体后端（本地、S3、GCS 等）实现本模块抽象，供备份恢复（BR）与 Lightning 等使用。

#![allow(non_snake_case, non_upper_case_globals)]

use std::io::{Read, Seek};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
pub use objectio::Context;
use objectio::recording::AccessStats;
use uuid::Uuid;

/// 对应 Go 的 Permission 字符串类型，描述创建存储时需要探测的权限。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Permission {
    AccessBuckets,
    ListObjects,
    GetObject,
    PutObject,
    PutAndDeleteObject,
}

impl Permission {
    /// 返回与 Go Permission 字符串字面量一致的权限名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccessBuckets => "AccessBucket",
            Self::ListObjects => "ListObjects",
            Self::GetObject => "GetObject",
            Self::PutObject => "PutObject",
            Self::PutAndDeleteObject => "PutAndDeleteObject",
        }
    }
}

/// 标记实现具备强一致性的存储；对应 Go 的 marker interface。
pub trait StrongConsistency {
    fn MarkStrongConsistency(&self);
}

/// 对应 Go `aws.Retryer`；同时提供通用重试配置与可选 SDK 分类器。
///
/// `RetryConfig` 承载最大次数和退避参数；自定义分类器可检查结构化响应/错误，
/// 并通过 `RetryAction::retryable_error_with_explicit_delay` 指定逐错误延迟。
pub trait Retryer: Send + Sync {
    fn retry_config(&self) -> aws_smithy_types::retry::RetryConfig;

    fn retry_classifier(
        &self,
    ) -> Option<aws_smithy_runtime_api::client::retries::classifiers::SharedRetryClassifier> {
        None
    }
}

/// 桶级访问权限，替代旧的 skip-check-path 行为。
pub const AccessBuckets: Permission = Permission::AccessBuckets;
/// 列举对象权限。
pub const ListObjects: Permission = Permission::ListObjects;
/// 读取对象权限。
pub const GetObject: Permission = Permission::GetObject;
/// 写入对象权限。
pub const PutObject: Permission = Permission::PutObject;
/// 组合写入与删除权限；云厂商通常不能单独探测 DeleteObject。
pub const PutAndDeleteObject: Permission = Permission::PutAndDeleteObject;

/// 对应 Storage.WalkDir 的遍历选项。
#[derive(Clone, Debug, Default)]
pub struct WalkOption {
    /// 相对存储基目录的子目录，例如 base/<SubDir>。
    pub SubDir: String,
    /// 是否跳过下级目录；Go 当前仅本地存储支持，默认递归遍历。
    pub SkipSubDir: bool,
    /// 由后端用于前缀检索，可避免扫描大量无关对象。
    pub ObjPrefix: String,
    /// 每页对象数；零值让云存储采用通常为 1000 的默认上限。
    pub ListCount: i64,
    /// 是否把遍历期间已删除的对象以 TombstoneSize 回调给调用方。
    pub IncludeTombstone: bool,
    /// 从该 key 之后开始遍历；S3-like、GCS 和本地存储支持。
    pub StartAfter: String,
}

/// 聚合 Go io.Reader、io.Seeker 和 io.Closer 的接口形状。
pub trait ReadSeekCloser: Read + Seek {
    fn close(&mut self) -> Result<()>;
}

/// 对应 Go 的分片上传器。
pub trait Uploader {
    /// 上传单个文件分片；ctx 保留取消与超时传播语义。
    fn UploadPart(&mut self, ctx: &Context, data: &[u8]) -> Result<()>;

    /// 提交已上传的全部分片，使对象最终可见。
    fn CompleteUpload(&mut self, ctx: &Context) -> Result<()>;
}

/// 创建对象 writer 时的并发数和分片大小配置。
#[derive(Clone, Debug, Default)]
pub struct WriterOption {
    /// 分片上传并发数。
    pub Concurrency: i32,
    /// 单个分片字节大小。
    pub PartSize: i64,
}

/// 打开对象 reader 时的范围和预取配置。
#[derive(Clone, Debug, Default)]
pub struct ReaderOption {
    /// 包含在读取范围内的起始偏移；None 对应 Go nil。
    pub StartOffset: Option<i64>,
    /// 不包含在读取范围内的结束偏移；None 对应 Go nil。
    pub EndOffset: Option<i64>,
    /// 正数时切换到预取 reader，非正数沿用普通 reader。
    pub PrefetchSize: i32,
}

/// 为支持服务端复制的存储提供扩展能力。
pub trait Copier {
    /// 按 CopySpec 将源存储对象复制到当前外部存储。
    fn CopyFrom(&self, ctx: &Context, external: &dyn Storage, spec: CopySpec) -> Result<()>;
}

/// 描述服务端复制的源对象名和目标对象名。
#[derive(Clone, Debug, Default)]
pub struct CopySpec {
    /// 源对象相对路径。
    pub From: String,
    /// 目标对象相对路径。
    pub To: String,
}

/// 对应 Go 的 Storage 接口，抽象本地文件系统和各类云对象存储。
pub trait Storage: Send + Sync {
    /// Request counters for a dedicated recording handle, when supported.
    fn AccessRequestSnapshot(&self) -> Option<(u64, u64)> {
        None
    }

    /// 原子写入完整文件，语义类似 os.WriteFile。
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()>;

    /// 一次性读取完整文件。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>>;

    /// 查询相对路径指向的文件是否存在。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool>;

    /// 删除单个文件。
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()>;

    /// 按相对路径及可选半开区间打开 reader。
    /// 部分 Go 实现会把传入 ctx 保存为 reader 的内部上下文。
    fn Open(
        &self,
        ctx: &Context,
        path: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>>;

    /// 批量删除文件，具体是否原子仍由后端决定。
    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()>;

    /// 遍历目录下的普通文件，并把可用于 Open 的路径与字节大小传给回调。
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: Option<&WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;

    /// 返回存储基路径的 URI。
    fn URI(&self) -> String;

    /// 创建覆盖式文件 writer；各后端对 WriterOption 的支持见其 Create 实现。
    fn Create(
        &self,
        ctx: &Context,
        path: &str,
        option: Option<&WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>>;

    /// 把旧相对路径重命名为新相对路径。
    fn Rename(&self, ctx: &Context, oldFileName: &str, newFileName: &str) -> Result<()>;

    /// 生成有时效的共享地址；不支持的 Azure/HDFS 等后端应返回错误。
    fn PresignFile(&self, ctx: &Context, fileName: &str, expire: Duration) -> Result<String>;

    /// 释放存储持有的连接、句柄等资源。
    fn Close(&self);
}

/// Thread-safe shared object store used by parallel encode/sort workers.
pub type StorageRef = Arc<dyn Storage>;

/// 传给各后端 New 函数的通用选项。
#[derive(Default)]
pub struct Options {
    /// 是否把凭据传给下游；外部密钥管理场景应设为 false。
    pub SendCredentials: bool,
    /// 明确表示 BR 没有拿到任何云凭据。
    pub NoCredentials: bool,
    /// S3/Azure/GCS 使用的 HTTP 基础客户端；本地存储可忽略。
    pub HTTPClient: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
    /// New 中需要主动探测的权限列表。
    pub CheckPermissions: Vec<Permission>,
    /// 创建后的 S3 storage 使用的重试器；None 时使用默认重试器。
    pub S3Retryer: Option<Arc<dyn Retryer>>,
    /// 是否检查 S3 bucket 的 ObjectLock 并把结果发送给 TiKV。
    pub CheckS3ObjectLockOptions: bool,
    /// 以读写文件大小近似记录流量，不统计协议和重试产生的额外流量。
    pub AccessRecording: Option<Arc<AccessStats>>,
}

/// 对应对象存储“目录”前缀；非空值不以 `/` 开头且始终以 `/` 结尾。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Prefix(pub String);

/// 对应 Go 的 NewPrefix，去掉两端斜杠后规范化为零个或一个尾斜杠。
pub fn NewPrefix(prefix: &str) -> Prefix {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        Prefix(String::new())
    } else {
        Prefix(format!("{}/", trimmed))
    }
}

impl Prefix {
    /// 对应 Go 的私有 join；Prefix 已规范化，因此可以直接拼接。
    fn join(&self, other: &Prefix) -> Prefix {
        Prefix(format!("{}{}", self.0, other.0))
    }

    /// 把任意字符串先规范化成 Prefix，再连接到当前前缀。
    pub fn JoinStr(&self, value: &str) -> Prefix {
        self.join(&NewPrefix(value))
    }

    /// 把对象名连接到当前前缀。
    pub fn ObjectKey(&self, name: &str) -> String {
        // 沿用 Go 的既有行为：name 若以 `/` 开头，结果可能包含双斜杠。
        format!("{}{}", self.0, name)
    }

    /// 转成相对 bucket 的 URL path；空前缀也返回合法路径 `/`。
    pub fn ToPath(&self) -> String {
        format!("/{}", self.0)
    }

    /// 对应 Go fmt.Stringer，返回底层规范化字符串。
    pub fn String(&self) -> String {
        self.0.clone()
    }

    /// Borrow the normalized prefix without allocating.
    /// 借用规范化前缀字符串，不额外分配。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 表示 bucket 名及其内部规范化前缀。
#[derive(Clone, Debug, Default)]
pub struct BucketPrefix {
    /// 桶（bucket）名称。
    pub Bucket: String,
    /// 桶内规范化对象前缀。
    pub Prefix: Prefix,
}

/// 构造 BucketPrefix，并立即规范化 prefix。
pub fn NewBucketPrefix(bucket: &str, prefix: &str) -> BucketPrefix {
    BucketPrefix {
        Bucket: bucket.to_owned(),
        Prefix: NewPrefix(prefix),
    }
}

impl BucketPrefix {
    /// 复用 Prefix 的对象 key 拼接规则。
    pub fn ObjectKey(&self, name: &str) -> String {
        self.Prefix.ObjectKey(name)
    }

    /// 返回规范化后的前缀字符串。
    pub fn PrefixStr(&self) -> String {
        self.Prefix.String()
    }
}

/// 对应 Go 的 GetHTTPRange，生成 HTTP Range 请求头值。
/// `full=true` 仅表示 start/end 都为零、应请求完整对象。
pub fn GetHTTPRange(startOffset: i64, endOffset: i64) -> (bool, String) {
    if endOffset > startOffset {
        // HTTP Range 两端都包含，因此把 Go 的排他 endOffset 减一。
        (false, format!("bytes={}-{}", startOffset, endOffset - 1))
    } else if startOffset == 0 {
        // 完整读取不发送 Range，使空对象也能像本地空文件一样正常打开。
        (true, String::new())
    } else {
        // 没有有效结束位置时，从 startOffset 一直读取到对象末尾。
        (false, format!("bytes={}-", startOffset))
    }
}

/// 生成权限探测使用的唯一对象 key，避免并发检查互相覆盖。
pub fn GenPermCheckObjectKey() -> String {
    format!("perm-check/{}", Uuid::new_v4())
}

/// Shared per-object S3/GCS/OSS multipart limit.
pub const MaxUploadParts: usize = 10_000;
/// Typed sentinel retained through IO/anyhow wrappers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExceedMaxUploadParts;
impl std::fmt::Display for ExceedMaxUploadParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("data exceeds the object store's per-object multipart upload part limit")
    }
}
impl std::error::Error for ExceedMaxUploadParts {}
pub const ErrExceedMaxUploadParts: ExceedMaxUploadParts = ExceedMaxUploadParts;
