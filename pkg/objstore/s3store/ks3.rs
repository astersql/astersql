// Copyright 2023 PingCAP, Inc.
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

// 金山云 KS3（Kingsoft Standard Storage Service）对象存储后端。
//
// KS3 兼容 S3 协议但限制 RoleARN、不支持 Presign；本模块在通用 S3 Store 之上
// 做区域/凭证校验与权限探测，并实现 `storeapi::Storage`。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::backuppb;
use crate::{
    DeleteObjectInput, GetObjectInput, HeadBucketInput, ListObjectsInput, PutObjectInput,
    RequestOptions, S3API, error_code,
};

/// 通过顺序读跳过的最大偏移（64KiB）；更大偏移应改用 Range 请求。
pub const MAX_SKIP_OFFSET_BY_READ: i64 = 1 << 16;
/// 读对象体失败时的最大重试次数。
pub const MAX_ERROR_RETRIES: usize = 3;

/// KS3 存储封装：内部复用通用 S3 `Storage`，并保留本地 S3 配置副本。
pub struct KS3Storage {
    inner: s3like::Storage,
    options: backuppb::S3,
}

/// 校验 KS3 区域与 RoleARN 限制后，构造可用的 `KS3Storage`，并把解析出的凭证写回 `backend`。
pub fn NewKS3Storage(
    ctx: &storeapi::Context,
    backend: &mut backuppb::S3,
    options: &storeapi::Options,
) -> Result<KS3Storage> {
    if backend.Region.is_empty() {
        return Err(anyhow!("ks3 region is empty"));
    }
    if !backend.RoleArn.is_empty() {
        return Err(anyhow!(
            "ks3 does not support role arn, arn: {}",
            backend.RoleArn
        ));
    }
    // 强制标记为 KS3 SDK Provider，走通用 S3 构造路径。
    let mut local = backend.clone();
    local.Provider = s3like::KS3SDKProvider.to_owned();
    let inner = crate::NewS3Storage(ctx, &mut local, options)?;
    // 将解析后的凭证/区域/对象锁状态回写到调用方配置。
    backend.AccessKey = local.AccessKey.clone();
    backend.SecretAccessKey = local.SecretAccessKey.clone();
    backend.SessionToken = local.SessionToken.clone();
    backend.Region = local.Region.clone();
    backend.ObjectLockEnabled = local.ObjectLockEnabled;
    Ok(KS3Storage {
        inner,
        options: local,
    })
}

/// 测试用构造：直接注入 Mock `S3API`，跳过真实网络与凭证解析。
pub fn NewKS3StorageForTest<T>(
    svc: Arc<T>,
    options: &backuppb::S3,
    access_rec: Option<Arc<objectio::recording::AccessStats>>,
) -> KS3Storage
where
    T: S3API + 'static,
{
    KS3Storage {
        inner: crate::NewS3StorageForTest(svc, options, access_rec),
        options: options.clone(),
    }
}

/// 权限探测：HeadBucket 检查桶是否可访问。
pub fn s3BucketExistenceCheckKS3(
    ctx: &storeapi::Context,
    svc: &dyn S3API,
    options: &backuppb::S3,
) -> Result<()> {
    svc.head_bucket(
        ctx,
        &HeadBucketInput {
            bucket: options.Bucket.clone(),
        },
        RequestOptions::default(),
    )
}

/// 权限探测：ListObjects v1 仅取 1 条，验证列举权限。
pub fn listObjectsCheckKS3(
    ctx: &storeapi::Context,
    svc: &dyn S3API,
    options: &backuppb::S3,
) -> Result<()> {
    svc.list_objects(
        ctx,
        &ListObjectsInput {
            bucket: options.Bucket.clone(),
            prefix: options.Prefix.clone(),
            max_keys: 1,
            ..Default::default()
        },
        RequestOptions::default(),
    )?;
    Ok(())
}

/// 权限探测：GetObject 不存在的 key；`NoSuchKey` 视为权限正常。
pub fn getObjectCheckKS3(
    ctx: &storeapi::Context,
    svc: &dyn S3API,
    options: &backuppb::S3,
) -> Result<()> {
    match svc.get_object(
        ctx,
        &GetObjectInput {
            bucket: options.Bucket.clone(),
            key: "not-exists".to_owned(),
            range: None,
        },
        RequestOptions::default(),
    ) {
        Ok(_) => Ok(()),
        Err(error) if error_code(&error) == Some("NoSuchKey") => Ok(()),
        Err(error) if error_code(&error).is_some() => Err(error),
        // Go returns nil when the KS3 SDK error does not implement awserr.Error.
        Err(_) => Ok(()),
    }
}

/// 权限探测：Put 后立即 Delete；Put 失败优先返回，否则返回 Delete 的结果。
pub fn putAndDeleteObjectCheckKS3(
    ctx: &storeapi::Context,
    svc: &dyn S3API,
    options: &backuppb::S3,
) -> Result<()> {
    let file = storeapi::GenPermCheckObjectKey();
    let input = buildPutObjectInputKS3(options, &file, b"check");
    let put_result = svc.put_object(ctx, &input, RequestOptions::default());
    // 对齐 Go defer：无论 Put 成败都尝试清理探测对象。
    let delete_result = svc.delete_object(
        ctx,
        &DeleteObjectInput {
            bucket: options.Bucket.clone(),
            key: format!("{}{}", options.Prefix, file),
        },
        RequestOptions::default(),
    );
    match put_result {
        Err(error) => Err(error),
        Ok(()) => delete_result,
    }
}

/// 按 KS3 配置组装 PutObject 输入（前缀 + ACL/SSE/存储类）。
pub fn buildPutObjectInputKS3(options: &backuppb::S3, file: &str, data: &[u8]) -> PutObjectInput {
    PutObjectInput {
        bucket: options.Bucket.clone(),
        key: format!("{}{}", options.Prefix, file),
        body: data.to_vec(),
        acl: non_empty(&options.Acl),
        server_side_encryption: non_empty(&options.Sse),
        sse_kms_key_id: non_empty(&options.SseKmsKeyId),
        storage_class: non_empty(&options.StorageClass),
    }
}

/// 空串转为 `None`，非空则克隆为 `Some`。
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// 将 i64 包装为 `Option`（对齐 Go 指针语义辅助）。
pub fn int64p(value: i64) -> Option<i64> {
    Some(value)
}
/// 将 bool 包装为 `Option`（对齐 Go 指针语义辅助）。
pub fn boolP(value: bool) -> Option<bool> {
    Some(value)
}

/// 判断是否为「对象已存在」类错误（含历史拼写 `ObjectAlreayExists`）。
pub fn maybeObjectAlreadyExists(error: &anyhow::Error) -> bool {
    matches!(
        error_code(error),
        Some("ObjectAlreayExists" | "ObjectAlreadyExists")
    )
}

impl KS3Storage {
    /// 返回构造时保存的 S3 配置副本。
    pub fn GetOptions(&self) -> &backuppb::S3 {
        &self.options
    }

    /// 服务端 Copy：从另一 KS3 存储按 `CopySpec` 复制对象。
    pub fn CopyFrom(
        &self,
        ctx: &storeapi::Context,
        source: &KS3Storage,
        spec: &storeapi::CopySpec,
    ) -> Result<()> {
        loop {
            match self.inner.CopyFrom(ctx, &source.inner, spec) {
                Ok(()) => return Ok(()),
                Err(error) if maybeObjectAlreadyExists(&error) => {
                    // KS3 refuses to overwrite an existing destination whereas AWS S3 does.
                    // Match Go by removing the target and retrying the server-side copy.
                    self.inner
                        .DeleteFile(ctx, &spec.To)
                        .context("during deleting an exist object for making place for copy")?;
                }
                // The Go KS3 SDK branch returns nil for other structured service errors.
                Err(error) if error_code(&error).is_some() => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
}

impl storeapi::StrongConsistency for KS3Storage {
    fn MarkStrongConsistency(&self) {}
}

/// 将读写/列举/分片等委托给内部通用 S3 Store；Presign 明确不支持。
impl storeapi::Storage for KS3Storage {
    fn WriteFile(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        self.inner.WriteFile(ctx, name, data)
    }
    fn ReadFile(&self, ctx: &storeapi::Context, name: &str) -> Result<Vec<u8>> {
        self.inner.ReadFile(ctx, name)
    }
    fn FileExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        self.inner.FileExists(ctx, name)
    }
    fn DeleteFile(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        self.inner.DeleteFile(ctx, name)
    }
    fn Open(
        &self,
        ctx: &storeapi::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        self.inner.Open(ctx.clone(), path, option)
    }
    fn DeleteFiles(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()> {
        self.inner.DeleteFiles(ctx, names)
    }
    fn WalkDir(
        &self,
        ctx: &storeapi::Context,
        option: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        self.inner
            .WalkDir(ctx, option, |name, size| callback(name, size))
    }
    fn URI(&self) -> String {
        format!("ks3://{}/{}", self.options.Bucket, self.options.Prefix)
    }
    fn Create(
        &self,
        ctx: &storeapi::Context,
        path: &str,
        option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        self.inner.Create(ctx.clone(), path, option)
    }
    fn Rename(&self, ctx: &storeapi::Context, old_name: &str, new_name: &str) -> Result<()> {
        self.inner.Rename(ctx, old_name, new_name)
    }
    fn PresignFile(
        &self,
        _ctx: &storeapi::Context,
        _name: &str,
        _expire: Duration,
    ) -> Result<String> {
        Err(anyhow!("KS3 backend does not support PresignFile"))
    }
    fn Close(&self) {
        self.inner.Close()
    }
}
