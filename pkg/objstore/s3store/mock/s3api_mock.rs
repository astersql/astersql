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

// S3API Mock 类型与 trait 定义。
//
// 用 mockall 生成可设置期望的 `MockS3API`，对齐 Go gomock 的 `MockS3API` /
// `NewMockS3API` / `EXPECT` / `ISGOMOCK` 用法；请求入参复用 AWS SDK for Rust 的
// operation Input/Output 类型，便于与真实客户端签名保持一致。

#![allow(non_snake_case)]

use std::{fmt, sync::Arc};

pub use aws_sdk_s3::operation::abort_multipart_upload::{
    AbortMultipartUploadInput, AbortMultipartUploadOutput,
};
pub use aws_sdk_s3::operation::complete_multipart_upload::{
    CompleteMultipartUploadInput, CompleteMultipartUploadOutput,
};
pub use aws_sdk_s3::operation::copy_object::{CopyObjectInput, CopyObjectOutput};
pub use aws_sdk_s3::operation::create_multipart_upload::{
    CreateMultipartUploadInput, CreateMultipartUploadOutput,
};
pub use aws_sdk_s3::operation::delete_object::{DeleteObjectInput, DeleteObjectOutput};
pub use aws_sdk_s3::operation::delete_objects::{DeleteObjectsInput, DeleteObjectsOutput};
pub use aws_sdk_s3::operation::get_object::{GetObjectInput, GetObjectOutput};
pub use aws_sdk_s3::operation::get_object_lock_configuration::{
    GetObjectLockConfigurationInput, GetObjectLockConfigurationOutput,
};
pub use aws_sdk_s3::operation::head_bucket::{HeadBucketInput, HeadBucketOutput};
pub use aws_sdk_s3::operation::head_object::{HeadObjectInput, HeadObjectOutput};
pub use aws_sdk_s3::operation::list_objects::{ListObjectsInput, ListObjectsOutput};
pub use aws_sdk_s3::operation::list_objects_v2::{ListObjectsV2Input, ListObjectsV2Output};
pub use aws_sdk_s3::operation::put_object::{PutObjectInput, PutObjectOutput};
pub use aws_sdk_s3::operation::upload_part::{UploadPartInput, UploadPartOutput};

/// Rust-side request context corresponding to the Go `context.Context` argument.
/// 对应 Go `context.Context` 的请求上下文，此处用 `request_id` 标识一次调用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// 请求标识，便于在期望匹配与日志中关联调用。
    pub request_id: String,
}

impl Context {
    /// 用给定 request_id 构造上下文。
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
        }
    }
}

/// Mutable per-call options exposed to the Go-style option callbacks.
/// 每次调用可变的 S3 选项，供 Go 风格的 option 回调修改。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct S3Options {
    /// 是否强制 path-style 寻址（`bucket.endpoint` vs `endpoint/bucket`）。
    pub force_path_style: bool,
}

/// A clonable option callback, matching Go's variadic `func(*s3.Options)` values.
/// 可克隆的选项回调，对应 Go 可变参数 `func(*s3.Options)`。
#[derive(Clone)]
pub struct OptionFn(Arc<dyn Fn(&mut S3Options) + Send + Sync>);

impl OptionFn {
    /// 用闭包构造选项函数。
    pub fn new(callback: impl Fn(&mut S3Options) + Send + Sync + 'static) -> Self {
        Self(Arc::new(callback))
    }

    /// 将回调应用到目标选项结构。
    pub fn apply(&self, options: &mut S3Options) {
        (self.0)(options);
    }
}

impl fmt::Debug for OptionFn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OptionFn(..)")
    }
}

/// Common error channel used by every method in the Go S3API interface.
/// Go S3API 各方法共用的错误类型，仅携带消息字符串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Error {
    message: String,
}

impl S3Error {
    /// 用消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for S3Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for S3Error {}

/// Rust equivalent of the Go `S3API` method set used by s3store.
/// 对应 Go `S3API` 方法集：涵盖对象读写、Head、列举、分片上传与对象锁查询等。
pub trait S3Api {
    /// 中止进行中的分片上传（multipart upload）。
    fn AbortMultipartUpload(
        &self,
        ctx: &Context,
        input: &AbortMultipartUploadInput,
        options: &[OptionFn],
    ) -> Result<AbortMultipartUploadOutput, S3Error>;
    /// 完成分片上传，合并各 part。
    fn CompleteMultipartUpload(
        &self,
        ctx: &Context,
        input: &CompleteMultipartUploadInput,
        options: &[OptionFn],
    ) -> Result<CompleteMultipartUploadOutput, S3Error>;
    /// 服务端对象复制。
    fn CopyObject(
        &self,
        ctx: &Context,
        input: &CopyObjectInput,
        options: &[OptionFn],
    ) -> Result<CopyObjectOutput, S3Error>;
    /// 发起分片上传，返回 upload id。
    fn CreateMultipartUpload(
        &self,
        ctx: &Context,
        input: &CreateMultipartUploadInput,
        options: &[OptionFn],
    ) -> Result<CreateMultipartUploadOutput, S3Error>;
    /// 删除单个对象。
    fn DeleteObject(
        &self,
        ctx: &Context,
        input: &DeleteObjectInput,
        options: &[OptionFn],
    ) -> Result<DeleteObjectOutput, S3Error>;
    /// 批量删除对象。
    fn DeleteObjects(
        &self,
        ctx: &Context,
        input: &DeleteObjectsInput,
        options: &[OptionFn],
    ) -> Result<DeleteObjectsOutput, S3Error>;
    /// 读取对象内容与元数据。
    fn GetObject(
        &self,
        ctx: &Context,
        input: &GetObjectInput,
        options: &[OptionFn],
    ) -> Result<GetObjectOutput, S3Error>;
    /// 查询桶的对象锁（Object Lock）配置。
    fn GetObjectLockConfiguration(
        &self,
        ctx: &Context,
        input: &GetObjectLockConfigurationInput,
        options: &[OptionFn],
    ) -> Result<GetObjectLockConfigurationOutput, S3Error>;
    /// 探测桶是否存在/可访问（不返回对象体）。
    fn HeadBucket(
        &self,
        ctx: &Context,
        input: &HeadBucketInput,
        options: &[OptionFn],
    ) -> Result<HeadBucketOutput, S3Error>;
    /// 探测对象元数据（不返回对象体）。
    fn HeadObject(
        &self,
        ctx: &Context,
        input: &HeadObjectInput,
        options: &[OptionFn],
    ) -> Result<HeadObjectOutput, S3Error>;
    /// 列举对象（ListObjects v1）。
    fn ListObjects(
        &self,
        ctx: &Context,
        input: &ListObjectsInput,
        options: &[OptionFn],
    ) -> Result<ListObjectsOutput, S3Error>;
    /// 列举对象（ListObjectsV2）。
    fn ListObjectsV2(
        &self,
        ctx: &Context,
        input: &ListObjectsV2Input,
        options: &[OptionFn],
    ) -> Result<ListObjectsV2Output, S3Error>;
    /// 上传完整对象。
    fn PutObject(
        &self,
        ctx: &Context,
        input: &PutObjectInput,
        options: &[OptionFn],
    ) -> Result<PutObjectOutput, S3Error>;
    /// 上传分片的一个 part。
    fn UploadPart(
        &self,
        ctx: &Context,
        input: &UploadPartInput,
        options: &[OptionFn],
    ) -> Result<UploadPartOutput, S3Error>;
}

// mockall 宏：生成可设置期望（EXPECT）并实现 `S3Api` 的 `MockS3API`。
mockall::mock! {
    pub S3API {}

    impl S3Api for S3API {
        fn AbortMultipartUpload(&self, ctx: &Context, input: &AbortMultipartUploadInput, options: &[OptionFn]) -> Result<AbortMultipartUploadOutput, S3Error>;
        fn CompleteMultipartUpload(&self, ctx: &Context, input: &CompleteMultipartUploadInput, options: &[OptionFn]) -> Result<CompleteMultipartUploadOutput, S3Error>;
        fn CopyObject(&self, ctx: &Context, input: &CopyObjectInput, options: &[OptionFn]) -> Result<CopyObjectOutput, S3Error>;
        fn CreateMultipartUpload(&self, ctx: &Context, input: &CreateMultipartUploadInput, options: &[OptionFn]) -> Result<CreateMultipartUploadOutput, S3Error>;
        fn DeleteObject(&self, ctx: &Context, input: &DeleteObjectInput, options: &[OptionFn]) -> Result<DeleteObjectOutput, S3Error>;
        fn DeleteObjects(&self, ctx: &Context, input: &DeleteObjectsInput, options: &[OptionFn]) -> Result<DeleteObjectsOutput, S3Error>;
        fn GetObject(&self, ctx: &Context, input: &GetObjectInput, options: &[OptionFn]) -> Result<GetObjectOutput, S3Error>;
        fn GetObjectLockConfiguration(&self, ctx: &Context, input: &GetObjectLockConfigurationInput, options: &[OptionFn]) -> Result<GetObjectLockConfigurationOutput, S3Error>;
        fn HeadBucket(&self, ctx: &Context, input: &HeadBucketInput, options: &[OptionFn]) -> Result<HeadBucketOutput, S3Error>;
        fn HeadObject(&self, ctx: &Context, input: &HeadObjectInput, options: &[OptionFn]) -> Result<HeadObjectOutput, S3Error>;
        fn ListObjects(&self, ctx: &Context, input: &ListObjectsInput, options: &[OptionFn]) -> Result<ListObjectsOutput, S3Error>;
        fn ListObjectsV2(&self, ctx: &Context, input: &ListObjectsV2Input, options: &[OptionFn]) -> Result<ListObjectsV2Output, S3Error>;
        fn PutObject(&self, ctx: &Context, input: &PutObjectInput, options: &[OptionFn]) -> Result<PutObjectOutput, S3Error>;
        fn UploadPart(&self, ctx: &Context, input: &UploadPartInput, options: &[OptionFn]) -> Result<UploadPartOutput, S3Error>;
    }
}

/// GoMock exposes a separate recorder; mockall keeps the same role on the mock value.
/// GoMock 有独立 recorder；mockall 把期望录制器挂在 mock 自身上，此别名保持命名兼容。
pub type MockS3APIMockRecorder = MockS3API;

/// Creates a fresh mock instance, corresponding to Go's `NewMockS3API`.
/// 创建新的 mock 实例，对应 Go 的 `NewMockS3API`。
pub fn NewMockS3API() -> MockS3API {
    MockS3API::new()
}

impl MockS3API {
    /// Returns the expectation recorder used to declare arguments and results.
    /// 返回期望录制器，用于声明参数匹配与返回值。
    pub fn EXPECT(&mut self) -> &mut MockS3APIMockRecorder {
        self
    }

    /// Marker retained for compatibility with generated GoMock mocks.
    /// 兼容 GoMock 生成代码的标记方法，无实际逻辑。
    pub fn ISGOMOCK(&self) {}
}
