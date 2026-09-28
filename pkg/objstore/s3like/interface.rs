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

// S3 兼容对象存储客户端抽象接口。
//
// 定义 BR（Backup & Restore，备份恢复）外部存储对 S3/OSS/KS3 等后端共用的
// `PrefixClient` trait 与请求/响应类型。具体 SDK 适配在各自 store 包中实现，
// 本模块只约定对象读写、列举、复制、分片上传与权限探测的能力边界。

#![allow(non_snake_case, non_upper_case_globals)]

use std::io::Read;
use std::time::Duration;

use anyhow::{Result, anyhow};

pub use prefetch::reader::ReadCloser;

/// 阿里云 OSS SDK 提供商标识。
pub const OSSProvider: &str = "oss-sdk";
/// 金山云 KS3 SDK 提供商标识。
pub const KS3SDKProvider: &str = "ks3-sdk";
/// 桶不存在时的错误文案常量。
pub const ErrNoSuchBucket: &str = "no such bucket";

/// GetObject 响应：对象正文流与可选的范围元数据。
pub struct GetResp {
    /// 可读可关闭的对象正文（对应 Go 的 io.ReadCloser）。
    pub Body: Box<dyn ReadCloser>,
    /// 是否返回了完整对象（start/end 均为 0 时通常为 true）。
    pub IsFullRange: bool,
    /// 完整对象长度；仅全量读取时有值。
    pub ContentLength: Option<i64>,
    /// HTTP Content-Range 字符串，部分读取时用于解析起止与总大小。
    pub ContentRange: Option<String>,
}

/// HeadObject 响应：目前主要携带跨区域复制状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeadObjectResp {
    /// 对象复制/同步状态（如 COMPLETE、PENDING、FAILED）。
    pub ReplicationStatus: String,
}

/// 列举结果中的单个对象条目。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Object {
    /// 对象键（含前缀路径）。
    pub Key: String,
    /// 对象字节大小。
    pub Size: i64,
}

/// ListObjects 分页响应。
#[derive(Default)]
pub struct ListResp {
    /// 下一页 continuation token；无更多页时为 None。
    pub NextContinuationToken: Option<String>,
    /// 结果是否被截断（尚有后续页）。
    pub IsTruncated: bool,
    /// 本页对象列表。
    pub Objects: Vec<Object>,
}

/// 服务端 CopyObject 入参：源桶前缀、源键与目标键。
#[derive(Clone, Debug)]
pub struct CopyInput {
    /// 源对象所在桶与前缀定位。
    pub FromLoc: storeapi::BucketPrefix,
    /// 源对象键。
    pub FromKey: String,
    /// 目标对象键。
    pub ToKey: String,
}

/// 分片上传执行器：从可读流持续上传直至 EOF。
pub trait Uploader: Send + Sync {
    /// 将 `reader` 内容上传到已绑定的对象键。
    fn Upload(&self, ctx: &storeapi::Context, reader: &mut dyn Read) -> Result<()>;
}

/// 带固定桶前缀的 S3 兼容客户端能力集合。
///
/// 除 CRUD 外，还包含权限探测、分片 Writer/Uploader 与预签名 URL（默认不支持）。
pub trait PrefixClient: Send + Sync {
    /// 探测桶是否存在且可访问。
    fn CheckBucketExistence(&self, ctx: &storeapi::Context) -> Result<()>;
    /// 探测 ListObjects 权限。
    fn CheckListObjects(&self, ctx: &storeapi::Context) -> Result<()>;
    /// 探测 GetObject 权限。
    fn CheckGetObject(&self, ctx: &storeapi::Context) -> Result<()>;
    /// 探测 Put/Delete 权限。
    fn CheckPutAndDeleteObject(&self, ctx: &storeapi::Context) -> Result<()>;
    /// 按字节范围读取对象；`endOffset == 0` 常表示读到末尾。
    fn GetObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        startOffset: i64,
        endOffset: i64,
    ) -> Result<Option<GetResp>>;
    /// 整对象上传。
    fn PutObject(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()>;
    /// 删除单个对象。
    fn DeleteObject(&self, ctx: &storeapi::Context, name: &str) -> Result<()>;
    /// 批量删除对象。
    fn DeleteObjects(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()>;
    /// 仅取对象元数据（Head）。
    fn HeadObject(&self, ctx: &storeapi::Context, name: &str) -> Result<Option<HeadObjectResp>>;
    /// 判断对象是否存在。
    fn IsObjectExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool>;
    /// 按前缀分页列举对象。
    fn ListObjects(
        &self,
        ctx: &storeapi::Context,
        extraPrefix: &str,
        startAfter: &str,
        continuationToken: Option<&str>,
        maxKeys: isize,
    ) -> Result<Option<ListResp>>;
    /// 服务端复制对象。
    fn CopyObject(&self, ctx: &storeapi::Context, params: &CopyInput) -> Result<()>;
    /// 创建分片上传 Writer（同步写入路径）。
    fn MultipartWriter(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Option<Box<dyn objectio::Writer>>>;
    /// 创建带分片大小与并发度的 Uploader（异步上传路径）。
    fn MultipartUploader(
        &self,
        name: &str,
        partSize: i64,
        concurrency: i32,
    ) -> Option<Box<dyn Uploader>>;

    /// 生成预签名下载 URL；S3 兼容层默认返回不支持错误。
    fn PresignObject(
        &self,
        _ctx: &storeapi::Context,
        _name: &str,
        _expire: Duration,
    ) -> Result<String> {
        Err(anyhow!(
            "S3-compatible storage does not support PresignFile"
        ))
    }
}

/// 为 `Box<T>` 转发 `PrefixClient`，便于以 trait 对象持有客户端。
impl<T: PrefixClient + ?Sized> PrefixClient for Box<T> {
    fn CheckBucketExistence(&self, ctx: &storeapi::Context) -> Result<()> {
        (**self).CheckBucketExistence(ctx)
    }
    fn CheckListObjects(&self, ctx: &storeapi::Context) -> Result<()> {
        (**self).CheckListObjects(ctx)
    }
    fn CheckGetObject(&self, ctx: &storeapi::Context) -> Result<()> {
        (**self).CheckGetObject(ctx)
    }
    fn CheckPutAndDeleteObject(&self, ctx: &storeapi::Context) -> Result<()> {
        (**self).CheckPutAndDeleteObject(ctx)
    }
    fn GetObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        startOffset: i64,
        endOffset: i64,
    ) -> Result<Option<GetResp>> {
        (**self).GetObject(ctx, name, startOffset, endOffset)
    }
    fn PutObject(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        (**self).PutObject(ctx, name, data)
    }
    fn DeleteObject(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        (**self).DeleteObject(ctx, name)
    }
    fn DeleteObjects(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()> {
        (**self).DeleteObjects(ctx, names)
    }
    fn HeadObject(&self, ctx: &storeapi::Context, name: &str) -> Result<Option<HeadObjectResp>> {
        (**self).HeadObject(ctx, name)
    }
    fn IsObjectExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        (**self).IsObjectExists(ctx, name)
    }
    fn ListObjects(
        &self,
        ctx: &storeapi::Context,
        extraPrefix: &str,
        startAfter: &str,
        continuationToken: Option<&str>,
        maxKeys: isize,
    ) -> Result<Option<ListResp>> {
        (**self).ListObjects(ctx, extraPrefix, startAfter, continuationToken, maxKeys)
    }
    fn CopyObject(&self, ctx: &storeapi::Context, params: &CopyInput) -> Result<()> {
        (**self).CopyObject(ctx, params)
    }
    fn MultipartWriter(
        &self,
        ctx: &storeapi::Context,
        name: &str,
    ) -> Result<Option<Box<dyn objectio::Writer>>> {
        (**self).MultipartWriter(ctx, name)
    }
    fn MultipartUploader(
        &self,
        name: &str,
        partSize: i64,
        concurrency: i32,
    ) -> Option<Box<dyn Uploader>> {
        (**self).MultipartUploader(name, partSize, concurrency)
    }
    fn PresignObject(
        &self,
        ctx: &storeapi::Context,
        name: &str,
        expire: Duration,
    ) -> Result<String> {
        (**self).PresignObject(ctx, name, expire)
    }
}
