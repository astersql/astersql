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

// Go OSS SDK 风格 API 的 mock 类型与 mockall 生成的 `MockAPI`。
//
// 请求/结果结构尽量贴合 Go 方法签名（含 `Context`、`OptionFn`），
// 供单元测试用 `EXPECT`/`withf`/`return_once` 配置期望调用。

use std::fmt;
use std::sync::Arc;

/// 简化请求上下文，携带 request_id 便于断言。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// 请求标识。
    pub request_id: String,
}

/// 可选客户端配置（如自定义 endpoint）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// 覆盖默认 endpoint。
    pub endpoint: Option<String>,
}

/// 闭包型选项：对 `Options` 就地修改，对应 Go 的 functional option。
#[derive(Clone)]
pub struct OptionFn(Arc<dyn Fn(&mut Options) + Send + Sync>);

impl OptionFn {
    /// 从闭包装配 `OptionFn`。
    pub fn new(f: impl Fn(&mut Options) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// 将本选项应用到 `options`。
    pub fn apply(&self, options: &mut Options) {
        (self.0)(options);
    }
}

impl Default for OptionFn {
    fn default() -> Self {
        Self::new(|_| {})
    }
}

impl fmt::Debug for OptionFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OptionFn(..)")
    }
}

/// mock 侧可比较的 OSS 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OssError(String);

impl OssError {
    /// 用消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for OssError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OssError {}

/// 批量生成仅含 `key` 字段的请求结构体。
macro_rules! request_with_key {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Debug, Default, PartialEq, Eq)]
            pub struct $name {
                pub key: String,
            }
        )+
    };
}

/// 批量生成空结果结构体（无字段，仅作类型占位）。
macro_rules! empty_result {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Debug, Default, PartialEq, Eq)]
            pub struct $name;
        )+
    };
}

request_with_key!(
    AbortMultipartUploadRequest,
    CompleteMultipartUploadRequest,
    CopyObjectRequest,
    DeleteMultipleObjectsRequest,
    DeleteObjectRequest,
    GetObjectRequest,
    HeadObjectRequest,
    InitiateMultipartUploadRequest,
    ListObjectsV2Request,
    ListPartsRequest,
    UploadPartRequest,
);

/// PutObject 请求：键 + 正文。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PutObjectRequest {
    /// 对象键。
    pub key: String,
    /// 对象内容。
    pub body: Vec<u8>,
}

/// GetObject 结果：内存正文。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GetObjectResult {
    /// 对象内容。
    pub body: Vec<u8>,
}

empty_result!(
    AbortMultipartUploadResult,
    CompleteMultipartUploadResult,
    CopyObjectResult,
    DeleteMultipleObjectsResult,
    DeleteObjectResult,
    HeadObjectResult,
    InitiateMultipartUploadResult,
    ListObjectsV2Result,
    ListPartsResult,
    PutObjectResult,
    UploadPartResult,
);

/// 与 Go OSS SDK Client API 对齐的方法集；测试通过 mockall 生成实现。
pub trait Api: Send + Sync {
    /// 中止分片上传。
    fn AbortMultipartUpload(
        &self,
        ctx: &Context,
        request: &AbortMultipartUploadRequest,
        options: &[OptionFn],
    ) -> Result<AbortMultipartUploadResult, OssError>;
    /// 完成分片上传。
    fn CompleteMultipartUpload(
        &self,
        ctx: &Context,
        request: &CompleteMultipartUploadRequest,
        options: &[OptionFn],
    ) -> Result<CompleteMultipartUploadResult, OssError>;
    /// 拷贝对象。
    fn CopyObject(
        &self,
        ctx: &Context,
        request: &CopyObjectRequest,
        options: &[OptionFn],
    ) -> Result<CopyObjectResult, OssError>;
    /// 批量删除对象。
    fn DeleteMultipleObjects(
        &self,
        ctx: &Context,
        request: &DeleteMultipleObjectsRequest,
        options: &[OptionFn],
    ) -> Result<DeleteMultipleObjectsResult, OssError>;
    /// 删除单个对象。
    fn DeleteObject(
        &self,
        ctx: &Context,
        request: &DeleteObjectRequest,
        options: &[OptionFn],
    ) -> Result<DeleteObjectResult, OssError>;
    /// 下载对象。
    fn GetObject(
        &self,
        ctx: &Context,
        request: &GetObjectRequest,
        options: &[OptionFn],
    ) -> Result<GetObjectResult, OssError>;
    /// Head 对象。
    fn HeadObject(
        &self,
        ctx: &Context,
        request: &HeadObjectRequest,
        options: &[OptionFn],
    ) -> Result<HeadObjectResult, OssError>;
    /// 发起分片上传。
    fn InitiateMultipartUpload(
        &self,
        ctx: &Context,
        request: &InitiateMultipartUploadRequest,
        options: &[OptionFn],
    ) -> Result<InitiateMultipartUploadResult, OssError>;
    /// 判断桶是否存在。
    fn IsBucketExist(
        &self,
        ctx: &Context,
        bucket: &str,
        options: &[OptionFn],
    ) -> Result<bool, OssError>;
    /// 列举对象（V2）。
    fn ListObjectsV2(
        &self,
        ctx: &Context,
        request: &ListObjectsV2Request,
        options: &[OptionFn],
    ) -> Result<ListObjectsV2Result, OssError>;
    /// 列举分片。
    fn ListParts(
        &self,
        ctx: &Context,
        request: &ListPartsRequest,
        options: &[OptionFn],
    ) -> Result<ListPartsResult, OssError>;
    /// 上传对象。
    fn PutObject(
        &self,
        ctx: &Context,
        request: &PutObjectRequest,
        options: &[OptionFn],
    ) -> Result<PutObjectResult, OssError>;
    /// 上传分片。
    fn UploadPart(
        &self,
        ctx: &Context,
        request: &UploadPartRequest,
        options: &[OptionFn],
    ) -> Result<UploadPartResult, OssError>;
}

// mockall 生成 `MockAPI`，实现上方 `Api` 全部方法。
mockall::mock! {
    pub API {}

    impl Api for API {
        fn AbortMultipartUpload(&self, ctx: &Context, request: &AbortMultipartUploadRequest, options: &[OptionFn]) -> Result<AbortMultipartUploadResult, OssError>;
        fn CompleteMultipartUpload(&self, ctx: &Context, request: &CompleteMultipartUploadRequest, options: &[OptionFn]) -> Result<CompleteMultipartUploadResult, OssError>;
        fn CopyObject(&self, ctx: &Context, request: &CopyObjectRequest, options: &[OptionFn]) -> Result<CopyObjectResult, OssError>;
        fn DeleteMultipleObjects(&self, ctx: &Context, request: &DeleteMultipleObjectsRequest, options: &[OptionFn]) -> Result<DeleteMultipleObjectsResult, OssError>;
        fn DeleteObject(&self, ctx: &Context, request: &DeleteObjectRequest, options: &[OptionFn]) -> Result<DeleteObjectResult, OssError>;
        fn GetObject(&self, ctx: &Context, request: &GetObjectRequest, options: &[OptionFn]) -> Result<GetObjectResult, OssError>;
        fn HeadObject(&self, ctx: &Context, request: &HeadObjectRequest, options: &[OptionFn]) -> Result<HeadObjectResult, OssError>;
        fn InitiateMultipartUpload(&self, ctx: &Context, request: &InitiateMultipartUploadRequest, options: &[OptionFn]) -> Result<InitiateMultipartUploadResult, OssError>;
        fn IsBucketExist(&self, ctx: &Context, bucket: &str, options: &[OptionFn]) -> Result<bool, OssError>;
        fn ListObjectsV2(&self, ctx: &Context, request: &ListObjectsV2Request, options: &[OptionFn]) -> Result<ListObjectsV2Result, OssError>;
        fn ListParts(&self, ctx: &Context, request: &ListPartsRequest, options: &[OptionFn]) -> Result<ListPartsResult, OssError>;
        fn PutObject(&self, ctx: &Context, request: &PutObjectRequest, options: &[OptionFn]) -> Result<PutObjectResult, OssError>;
        fn UploadPart(&self, ctx: &Context, request: &UploadPartRequest, options: &[OptionFn]) -> Result<UploadPartResult, OssError>;
    }
}

/// GoMock 的独立 recorder 在 mockall 中由 mock 自身承担；保留公开类型名。
pub type MockAPIMockRecorder = MockAPI;

/// 创建空的 `MockAPI`（对应 Go `NewMockAPI`）。
pub fn NewMockAPI() -> MockAPI {
    MockAPI::new()
}

impl MockAPI {
    /// 返回自身可变引用，兼容 Go mockgen 的 `EXPECT()` 链式风格。
    pub fn EXPECT(&mut self) -> &mut MockAPIMockRecorder {
        self
    }

    /// Go mock 兼容空操作标记。
    pub fn ISGOMOCK(&self) {}
}
