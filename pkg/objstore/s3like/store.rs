// Copyright 2020 PingCAP, Inc.
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

// S3-like 对象存储高层 Storage 实现。
//
// 基于 `PrefixClient` 提供读写、列举、分片上传、复制与预签名等能力，
// 并处理 HTTP/2 中断重试、Content-Range 解析与后端 Flag 配置。

#![allow(non_snake_case, non_upper_case_globals)]

use std::io::Read;
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use regex::Regex;

use crate::backuppb;
use crate::pflag;
use crate::{
    AsyncWriter, CopyInput, IsDeadlineExceedError, IsHTTP2ConnAborted, PrefixClient, ReadCloser,
    RecordRetryableError, S3ObjectReader,
};

/// 分片上传默认块大小（5MiB），与 AWS 最小 multipart part 常见实践对齐。
pub static mut HardcodedChunkSize: usize = 5 * 1024 * 1024;
/// 命令行/URL 查询参数名：Access Key。
pub const S3AccessKey: &str = "access-key";
/// 命令行/URL 查询参数名：Secret Access Key。
pub const S3SecretAccessKey: &str = "secret-access-key";
/// 命令行/URL 查询参数名：AssumeRole 的 Role ARN。
pub const S3RoleARN: &str = "role-arn";
/// 命令行/URL 查询参数名：AssumeRole 的 External ID。
pub const S3ExternalID: &str = "external-id";

// 以下为 pflag 选项名，对应 br/lightning 等工具的 S3 后端参数。
const s3EndpointOption: &str = "s3.endpoint";
const s3RegionOption: &str = "s3.region";
const s3StorageClassOption: &str = "s3.storage-class";
const s3SseOption: &str = "s3.sse";
const s3SseKmsKeyIDOption: &str = "s3.sse-kms-key-id";
const s3ACLOption: &str = "s3.acl";
const s3ProviderOption: &str = "s3.provider";
const s3RoleARNOption: &str = "s3.role-arn";
const s3ExternalIDOption: &str = "s3.external-id";
const s3ProfileOption: &str = "s3.profile";
/// 读对象体失败时的最大重试次数。
pub(crate) const MAX_ERROR_RETRIES: usize = 3;
/// 通过顺序读跳过的最大偏移；更大偏移应使用 Range Get。
pub(crate) const MAX_SKIP_OFFSET_BY_READ: i64 = 1 << 16;
// AWS 官方域名片段，用于判断是否走 virtual-host 风格。
const domainAWS: &str = "amazonaws.com";
// S3 DeleteObjects 单次最多删除的对象数上限。
const s3DeleteObjectsLimit: usize = 1000;

/// 写缓冲默认大小（5MiB），可通过 WriterOption.PartSize 覆盖。
pub static mut WriteBufferSize: usize = 5 * 1024 * 1024;

#[derive(Clone)]
/// 基于 PrefixClient 的 S3-like 对象存储实现。
///
/// `bucketPrefix` 将逻辑路径映射到桶内对象键；`accessRec` 可选记录读写流量。
pub struct Storage {
    s3Cli: Arc<dyn PrefixClient>,
    bucketPrefix: storeapi::BucketPrefix,
    options: backuppb::S3,
    pub(crate) accessRec: Option<Arc<objectio::recording::AccessStats>>,
}

/// 由底层客户端、桶前缀、S3 配置与可选访问统计构造 `Storage`。
pub fn NewStorage<C>(
    s3Cli: C,
    bucketPrefix: storeapi::BucketPrefix,
    options: backuppb::S3,
    accessRec: Option<Arc<objectio::recording::AccessStats>>,
) -> Storage
where
    C: PrefixClient + 'static,
{
    Storage {
        s3Cli: Arc::new(s3Cli),
        bucketPrefix,
        options,
        accessRec,
    }
}

/// 提供桶前缀，供服务端 Copy 时解析源对象位置。
pub trait BucketPrefixProvider {
    fn GetBucketPrefix(&self) -> storeapi::BucketPrefix;
}

impl BucketPrefixProvider for Storage {
    fn GetBucketPrefix(&self) -> storeapi::BucketPrefix {
        self.bucketPrefix.clone()
    }
}

impl Storage {
    /// 标记强一致性语义（S3 侧当前为空实现，保留接口对齐）。
    pub fn MarkStrongConsistency(&self) {}

    /// 返回构造时的 S3 后端配置。
    pub fn GetOptions(&self) -> &backuppb::S3 {
        &self.options
    }

    /// 服务端 Copy：从另一 Storage 按 CopySpec 复制对象。
    pub fn CopyFrom(
        &self,
        ctx: &storeapi::Context,
        inStore: &dyn BucketPrefixProvider,
        spec: &storeapi::CopySpec,
    ) -> Result<()> {
        self.s3Cli.CopyObject(
            ctx,
            &CopyInput {
                FromLoc: inStore.GetBucketPrefix(),
                FromKey: spec.From.clone(),
                ToKey: spec.To.clone(),
            },
        )
    }

    /// 返回当前桶前缀副本。
    pub fn GetBucketPrefix(&self) -> storeapi::BucketPrefix {
        self.bucketPrefix.clone()
    }

    /// 整文件 Put，并记录写入字节数。
    pub fn WriteFile(&self, ctx: &storeapi::Context, file: &str, data: &[u8]) -> Result<()> {
        self.s3Cli.PutObject(ctx, file, data)?;
        objectio::recording::AccessStats::rec_write(self.accessRec.as_deref(), data.len());
        Ok(())
    }

    /// 整文件 Get；遇 HTTP/2 连接中断时短暂休眠后重试。
    pub fn ReadFile(&self, ctx: &storeapi::Context, file: &str) -> Result<Vec<u8>> {
        // HTTP/2 连接中断时最多再试 5 次，间隔 10ms。
        let mut remainRetry = 5;
        loop {
            match self.doReadFile(ctx, file) {
                Ok(data) => {
                    objectio::recording::AccessStats::rec_read(
                        self.accessRec.as_deref(),
                        data.len(),
                    );
                    return Ok(data);
                }
                Err(err) if IsHTTP2ConnAborted(&err) && remainRetry > 0 => {
                    thread::sleep(Duration::from_millis(10));
                    remainRetry -= 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// 实际读对象体：GetObject 后读尽 Body，可重试非取消/超时错误。
    fn doReadFile(&self, ctx: &storeapi::Context, file: &str) -> Result<Vec<u8>> {
        let mut lastReadErr = anyhow!("response body was not read");
        // 读 Body 失败且非取消/超时则记录可重试错误并继续。
        for retryCnt in 0..MAX_ERROR_RETRIES {
            let mut result = self
                .s3Cli
                .GetObject(ctx, file, 0, 0)
                .map_err(|err| {
                    anyhow!(
                        "failed to read s3 file, file info: input.bucket='{}', input.key='{}': {err}",
                        self.options.Bucket,
                        self.bucketPrefix.ObjectKey(file),
                    )
                })?
                .expect("PrefixClient.GetObject returned nil without error");
            let mut data = Vec::new();
            let readResult = result.Body.read_to_end(&mut data);
            let _ = result.Body.close();
            match readResult {
                Ok(_) if fail::eval("read-s3-body-failed", |_| ()).is_none() => return Ok(data),
                Ok(_) => lastReadErr = anyhow!("read: connection reset by peer"),
                Err(err) => lastReadErr = err.into(),
            }
            if IsDeadlineExceedError(&lastReadErr) || isCancelError(&lastReadErr) {
                return Err(anyhow!(
                    "failed to read body from get object result, file info: input.bucket='{}', input.key='{}', retryCnt='{}': {}",
                    self.options.Bucket,
                    self.bucketPrefix.ObjectKey(file),
                    retryCnt,
                    lastReadErr,
                ));
            }
            RecordRetryableError(&lastReadErr.to_string());
        }
        Err(anyhow!(
            "failed to read body from get object result (retry too much), file info: input.bucket='{}', input.key='{}': {}",
            self.options.Bucket,
            self.bucketPrefix.ObjectKey(file),
            lastReadErr,
        ))
    }

    /// 删除单个对象。
    pub fn DeleteFile(&self, ctx: &storeapi::Context, file: &str) -> Result<()> {
        self.s3Cli.DeleteObject(ctx, file)
    }

    /// 批量删除，按 S3 上限分批调用 DeleteObjects。
    pub fn DeleteFiles(&self, ctx: &storeapi::Context, files: &[String]) -> Result<()> {
        // S3 批量删除上限为 1000，超限则分批。
        for batch in files.chunks(s3DeleteObjectsLimit) {
            self.s3Cli.DeleteObjects(ctx, batch)?;
        }
        Ok(())
    }

    /// 通过 Head/存在性检查判断对象是否存在。
    pub fn FileExists(&self, ctx: &storeapi::Context, file: &str) -> Result<bool> {
        self.s3Cli.IsObjectExists(ctx, file)
    }

    /// 根据跨区域复制状态判断对象是否已同步完成。
    pub fn FileSynced(&self, ctx: &storeapi::Context, file: &str) -> Result<bool> {
        // 跨区域复制状态：COMPLETE(D)/REPLICA 视为已同步，PENDING 未完成。
        match self
            .s3Cli
            .HeadObject(ctx, file)?
            .expect("PrefixClient.HeadObject returned nil without error")
            .ReplicationStatus
            .as_str()
        {
            "COMPLETE" | "COMPLETED" | "REPLICA" => Ok(true),
            "PENDING" => Ok(false),
            "FAILED" => Err(anyhow!("upstream replication status for {file} is FAILED")),
            "" => Err(anyhow!("upstream replication status for {file} is empty")),
            status => Err(anyhow!(
                "upstream replication status for {file} is {status:?}"
            )),
        }
    }

    /// 列举前缀下对象并回调；跳过零大小「目录」占位键。
    pub fn WalkDir<F>(
        &self,
        ctx: &storeapi::Context,
        opt: Option<&storeapi::WalkOption>,
        mut callback: F,
    ) -> Result<()>
    where
        F: FnMut(&str, i64) -> Result<()>,
    {
        let defaultOpt = storeapi::WalkOption::default();
        let opt = opt.unwrap_or(&defaultOpt);
        let prefix = storeapi::NewPrefix(&opt.SubDir).ObjectKey(&opt.ObjPrefix);
        let maxKeys = if opt.ListCount > 0 {
            opt.ListCount as isize
        } else {
            1000
        };
        let mut initialStartAfter = opt.StartAfter.clone();
        // 分页列举：优先用 StartAfter，之后改用 ContinuationToken。
        let mut continuationToken = None;
        let cliPrefix = self.bucketPrefix.PrefixStr();
        loop {
            let response = self
                .s3Cli
                .ListObjects(
                    ctx,
                    &prefix,
                    &initialStartAfter,
                    continuationToken.as_deref(),
                    maxKeys,
                )?
                .expect("PrefixClient.ListObjects returned nil without error");
            for object in response.Objects {
                let trimmed = object.Key.strip_prefix(&cliPrefix).unwrap_or(&object.Key);
                let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
                if object.Size <= 0 && trimmed.ends_with('/') {
                    continue;
                }
                callback(trimmed, object.Size)?;
            }
            continuationToken = response.NextContinuationToken;
            initialStartAfter.clear();
            if !response.IsTruncated {
                break;
            }
        }
        Ok(())
    }

    /// 返回 `s3://bucket/prefix` 形式的 URI。
    pub fn URI(&self) -> String {
        format!(
            "s3://{}/{}",
            self.options.Bucket,
            self.bucketPrefix.PrefixStr()
        )
    }

    /// 打开可读对象，可选 Range 与预取缓冲。
    pub fn Open(
        &self,
        ctx: storeapi::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        let start = option.and_then(|value| value.StartOffset).unwrap_or(0);
        let end = option.and_then(|value| value.EndOffset).unwrap_or(0);
        let prefetchSize = option
            .map(|value| value.PrefetchSize.max(0) as usize)
            .unwrap_or(0);
        let (mut reader, range) = self.open(&ctx, path, start, end)?;
        // 预取：用独立缓冲读取器包装底层 ReadCloser。
        if prefetchSize > 0 {
            reader = prefetch::reader::NewReader(reader, range.RangeSize(), prefetchSize);
        }
        Ok(Box::new(S3ObjectReader::new(
            Arc::new(self.clone()),
            path.to_owned(),
            reader,
            range,
            ctx,
            prefetchSize,
        )))
    }

    /// 底层 GetObject 并解析 Content-Range / Content-Length 为 RangeInfo。
    pub(crate) fn open(
        &self,
        ctx: &storeapi::Context,
        path: &str,
        startOffset: i64,
        endOffset: i64,
    ) -> Result<(Box<dyn ReadCloser>, RangeInfo)> {
        // 全文下载用 ContentLength 构造 Range；部分下载解析 Content-Range。
        let result = self
            .s3Cli
            .GetObject(ctx, path, startOffset, endOffset)?
            .expect("PrefixClient.GetObject returned nil without error");
        let range = if result.IsFullRange {
            let objectSize = result.ContentLength.ok_or_else(|| {
                anyhow!("open file '{path}' failed. The S3 object has no content length")
            })?;
            if objectSize == 0 {
                RangeInfo {
                    Start: 0,
                    End: 0,
                    Size: 0,
                }
            } else {
                RangeInfo {
                    Start: 0,
                    End: objectSize - 1,
                    Size: objectSize,
                }
            }
        } else {
            ParseRangeInfo(result.ContentRange.as_deref())?
        };
        if startOffset != range.Start || (endOffset != 0 && endOffset != range.End.wrapping_add(1))
        {
            return Err(anyhow!(
                "open file '{}' failed, expected range: [{},{}), got: {}",
                path,
                startOffset,
                endOffset,
                result.ContentRange.as_deref().unwrap_or("<empty>"),
            ));
        }
        Ok((result.Body, range))
    }

    /// 创建写对象：并发≤1 用同步分片 Writer，否则用异步 Uploader + 缓冲。
    pub fn Create(
        &self,
        ctx: storeapi::Context,
        name: &str,
        option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        // Concurrency<=1：同步 MultipartWriter；否则异步分片 + 缓冲写出。
        let writer: Box<dyn objectio::Writer> = match option {
            None => self
                .s3Cli
                .MultipartWriter(&ctx, name)?
                .expect("PrefixClient.MultipartWriter returned nil without error"),
            Some(option) if option.Concurrency <= 1 => self
                .s3Cli
                .MultipartWriter(&ctx, name)?
                .expect("PrefixClient.MultipartWriter returned nil without error"),
            Some(option) => Box::new(AsyncWriter::new(
                ctx,
                self.s3Cli
                    .MultipartUploader(name, option.PartSize, option.Concurrency)
                    .expect("PrefixClient.MultipartUploader returned nil"),
            )?),
        };
        let bufSize = option
            .filter(|value| value.PartSize > 0)
            .map(|value| value.PartSize as usize)
            .unwrap_or(unsafe { WriteBufferSize });
        Ok(Box::new(objectio::NewBufferedWriter(
            writer,
            bufSize,
            objectio::CompressType::NoCompression,
            self.accessRec.clone(),
        )))
    }

    /// 通过读-写-删模拟 Rename（S3 无原子重命名）。
    pub fn Rename(
        &self,
        ctx: &storeapi::Context,
        oldFileName: &str,
        newFileName: &str,
    ) -> Result<()> {
        let content = self.ReadFile(ctx, oldFileName)?;
        self.WriteFile(ctx, newFileName, &content)?;
        self.DeleteFile(ctx, oldFileName)
    }

    /// 生成带过期时间的预签名下载 URL。
    pub fn PresignFile(
        &self,
        ctx: &storeapi::Context,
        fileName: &str,
        expire: Duration,
    ) -> Result<String> {
        if expire.is_zero() {
            return Err(anyhow!("presign expiration must be positive"));
        }
        self.s3Cli.PresignObject(ctx, fileName, expire)
    }

    /// 关闭存储（当前无额外资源需释放）。
    pub fn Close(&self) {}
}

impl storeapi::StrongConsistency for Storage {
    fn MarkStrongConsistency(&self) {
        Storage::MarkStrongConsistency(self)
    }
}

/// 将本模块 Storage 适配到统一对象存储 trait。
impl storeapi::Storage for Storage {
    fn WriteFile(&self, ctx: &storeapi::Context, name: &str, data: &[u8]) -> Result<()> {
        Storage::WriteFile(self, ctx, name, data)
    }
    fn ReadFile(&self, ctx: &storeapi::Context, name: &str) -> Result<Vec<u8>> {
        Storage::ReadFile(self, ctx, name)
    }
    fn FileExists(&self, ctx: &storeapi::Context, name: &str) -> Result<bool> {
        Storage::FileExists(self, ctx, name)
    }
    fn DeleteFile(&self, ctx: &storeapi::Context, name: &str) -> Result<()> {
        Storage::DeleteFile(self, ctx, name)
    }
    fn Open(
        &self,
        ctx: &storeapi::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        Storage::Open(self, ctx.clone(), path, option)
    }
    fn DeleteFiles(&self, ctx: &storeapi::Context, names: &[String]) -> Result<()> {
        Storage::DeleteFiles(self, ctx, names)
    }
    fn WalkDir(
        &self,
        ctx: &storeapi::Context,
        opt: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        Storage::WalkDir(self, ctx, opt, |name, size| callback(name, size))
    }
    fn URI(&self) -> String {
        Storage::URI(self)
    }
    fn Create(
        &self,
        ctx: &storeapi::Context,
        path: &str,
        option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        Storage::Create(self, ctx.clone(), path, option)
    }
    fn Rename(&self, ctx: &storeapi::Context, oldFileName: &str, newFileName: &str) -> Result<()> {
        Storage::Rename(self, ctx, oldFileName, newFileName)
    }
    fn PresignFile(
        &self,
        ctx: &storeapi::Context,
        fileName: &str,
        expire: Duration,
    ) -> Result<String> {
        Storage::PresignFile(self, ctx, fileName, expire)
    }
    fn Close(&self) {
        Storage::Close(self)
    }
}

#[derive(Clone, Default)]
/// 从 Flag/URL 解析出的 S3 后端选项，可 Apply 到 backuppb::S3。
pub struct S3BackendOptions {
    pub Endpoint: String,
    pub Region: String,
    pub StorageClass: String,
    pub Sse: String,
    pub SseKmsKeyID: String,
    pub ACL: String,
    pub AccessKey: String,
    pub SecretAccessKey: String,
    pub SessionToken: String,
    pub Provider: String,
    pub ForcePathStyle: bool,
    pub UseAccelerateEndpoint: bool,
    pub RoleARN: String,
    pub ExternalID: String,
    pub Profile: String,
    pub ObjectLockEnabled: bool,
}

impl S3BackendOptions {
    /// 校验 endpoint/凭证完整性后写入 protobuf 配置。
    pub fn Apply(&self, s3: &mut backuppb::S3) -> Result<()> {
        if !self.Endpoint.is_empty() {
            let (scheme, remainder) = self.Endpoint.split_once(':').unwrap_or(("", ""));
            if scheme.is_empty()
                || !scheme.as_bytes()[0].is_ascii_alphabetic()
                || !scheme
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
            {
                return Err(anyhow!("scheme not found in endpoint"));
            }
            if !remainder.starts_with("//") {
                return Err(anyhow!("host not found in endpoint"));
            }
            let parsed = url::Url::parse(&self.Endpoint)?;
            if parsed.host_str().is_none() {
                return Err(anyhow!("host not found in endpoint"));
            }
        }
        if self.Profile.is_empty() {
            if self.AccessKey.is_empty() && !self.SecretAccessKey.is_empty() {
                return Err(anyhow!("access_key not found"));
            }
            if !self.AccessKey.is_empty() && self.SecretAccessKey.is_empty() {
                return Err(anyhow!("secret_access_key not found"));
            }
        }
        s3.Endpoint = trimOneSlash(&self.Endpoint);
        s3.Region = self.Region.clone();
        s3.StorageClass = self.StorageClass.clone();
        s3.Sse = self.Sse.clone();
        s3.SseKmsKeyId = self.SseKmsKeyID.clone();
        s3.Acl = self.ACL.clone();
        s3.AccessKey = self.AccessKey.clone();
        s3.SecretAccessKey = self.SecretAccessKey.clone();
        s3.SessionToken = self.SessionToken.clone();
        s3.ForcePathStyle = self.ForcePathStyle;
        s3.RoleArn = self.RoleARN.clone();
        s3.ExternalId = self.ExternalID.clone();
        s3.Provider = self.Provider.clone();
        s3.Profile = self.Profile.clone();
        Ok(())
    }

    /// 按厂商与加速端点决定是否关闭 path-style（改用 virtual-host）。
    pub fn SetForcePathStyle(&mut self, rawURL: &str) {
        if matches!(self.Provider.as_str(), "alibaba" | "netease" | "tencent")
            || self.UseAccelerateEndpoint
            || useVirtualHostStyleForAWSS3(self, rawURL)
        {
            self.ForcePathStyle = false;
        }
    }

    /// 从 pflag 集合填充本结构字段。
    pub fn ParseFromFlags(&mut self, flags: &pflag::FlagSet) -> Result<()> {
        self.Endpoint = trimOneSlash(&flags.GetString(s3EndpointOption)?);
        self.Region = flags.GetString(s3RegionOption)?;
        self.Sse = flags.GetString(s3SseOption)?;
        self.SseKmsKeyID = flags.GetString(s3SseKmsKeyIDOption)?;
        self.ACL = flags.GetString(s3ACLOption)?;
        self.StorageClass = flags.GetString(s3StorageClassOption)?;
        self.ForcePathStyle = true;
        self.Provider = flags.GetString(s3ProviderOption)?;
        self.RoleARN = flags.GetString(s3RoleARNOption)?;
        self.ExternalID = flags.GetString(s3ExternalIDOption)?;
        self.Profile = flags.GetString(s3ProfileOption)?;
        Ok(())
    }
}

/// 去掉 endpoint 末尾多余的单个 `/`。
fn trimOneSlash(value: &str) -> String {
    value.strip_suffix('/').unwrap_or(value).to_owned()
}

/// 判断 AWS S3 是否应使用 virtual-host 风格寻址。
fn useVirtualHostStyleForAWSS3(options: &S3BackendOptions, rawURL: &str) -> bool {
    if rawURL.is_empty()
        || rawURL.contains("force-path-style")
        || rawURL.contains("force_path_style")
    {
        return false;
    }
    options.Provider == "aws" || options.Endpoint.contains(domainAWS) || !options.RoleARN.is_empty()
}

/// 向 FlagSet 注册实验性/正式 S3 后端参数。
pub fn DefineS3Flags(flags: &mut pflag::FlagSet) {
    flags.String(s3EndpointOption, "", "(experimental) Set the S3 endpoint URL, please specify the http or https scheme explicitly");
    flags.String(
        s3RegionOption,
        "",
        "(experimental) Set the S3 region, e.g. us-east-1",
    );
    flags.String(
        s3StorageClassOption,
        "",
        "(experimental) Set the S3 storage class, e.g. STANDARD",
    );
    flags.String(
        s3SseOption,
        "",
        "Set S3 server-side encryption, e.g. aws:kms",
    );
    flags.String(
        s3SseKmsKeyIDOption,
        "",
        "KMS CMK key id to use with S3 server-side encryption. Leave empty to use S3 owned key.",
    );
    flags.String(
        s3ACLOption,
        "",
        "(experimental) Set the S3 canned ACLs, e.g. authenticated-read",
    );
    flags.String(
        s3ProviderOption,
        "",
        "(experimental) Set the S3 provider, e.g. aws, alibaba, ceph",
    );
    flags.String(
        s3RoleARNOption,
        "",
        "(experimental) Set the ARN of the IAM role to assume when accessing AWS S3",
    );
    flags.String(
        s3ExternalIDOption,
        "",
        "(experimental) Set the external ID when assuming the role to access AWS S3",
    );
    flags.String(s3ProfileOption, "", "(experimental) Set the AWS profile to use for AWS S3 authentication. Command line options take precedence over profile settings");
}

// Content-Range：bytes <start>-<end>/<size>
static contentRangeRegex: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"bytes (\d+)-(\d+)/(\d+)$").expect("valid Content-Range regex"));

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// HTTP Content-Range 解析结果：闭区间 [Start, End] 与对象总 Size。
pub struct RangeInfo {
    pub Start: i64,
    pub End: i64,
    pub Size: i64,
}

impl RangeInfo {
    /// 当前 Range 覆盖的字节数。
    pub fn RangeSize(&self) -> i64 {
        self.End.wrapping_add(1).wrapping_sub(self.Start)
    }
}

/// 解析形如 `bytes start-end/total` 的 Content-Range 头。
pub fn ParseRangeInfo(info: Option<&str>) -> Result<RangeInfo> {
    let info = info
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("ContentRange is empty"))?;
    let captures = contentRangeRegex
        .captures(info)
        .ok_or_else(|| anyhow!("invalid content range: '{info}'"))?;
    let parse = |index: usize, kind: &str| -> Result<i64> {
        let value = captures.get(index).expect("capture exists").as_str();
        value.parse::<i64>().map_err(|err| {
            anyhow!("invalid {kind} value '{value}' in ContentRange '{info}': {err}")
        })
    };
    Ok(RangeInfo {
        Start: parse(1, "start offset")?,
        End: parse(2, "end offset")?,
        Size: parse(3, "size size")?,
    })
}

/// 上下文已被取消（context canceled）。
fn isCancelError(err: &anyhow::Error) -> bool {
    err.to_string().contains("context canceled")
}
