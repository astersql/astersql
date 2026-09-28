// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Azure Blob（微软对象存储）后端实现，以及可复用的 `ObjectStorageCore` / 内存存储。
//
// 覆盖认证选择（SAS、Shared Key、Client Secret、默认凭据）、加密选项校验、
// 以及基于 `object_store` crate 的读写、列举、重命名与分范围读取。
// 同时提供测试用 `MemoryStorage` 与路径拼接等辅助函数。

use std::env;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result, anyhow, bail};
use base64::Engine;
use bytes::Bytes;
use futures::TryStreamExt;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::path::Path;
use object_store::{Attribute, Attributes, ObjectStore, ObjectStoreExt, PutOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::runtime::{Builder, Runtime};

use crate::{objectio, storeapi};

/// 下载失败时的最大重试次数（与 Go 常量对齐）。
pub const AZBLOB_RETRY_TIMES: i32 = 5;
/// 分块上传建议块大小（64 MiB）。
pub const AZBLOB_CHUNK_SIZE: usize = 64 * 1024 * 1024;

/// CLI/配置项：自定义 Azure Blob 端点。
pub const AZBLOB_ENDPOINT_OPTION: &str = "azblob.endpoint";
/// CLI/配置项：访问层（Hot/Cool 等 storage class）。
pub const AZBLOB_ACCESS_TIER_OPTION: &str = "azblob.access-tier";
/// CLI/配置项：存储账户名。
pub const AZBLOB_ACCOUNT_NAME_OPTION: &str = "azblob.account-name";
/// CLI/配置项：账户密钥（Shared Key）。
pub const AZBLOB_ACCOUNT_KEY_OPTION: &str = "azblob.account-key";
/// CLI/配置项：SAS（Shared Access Signature，共享访问签名）令牌。
pub const AZBLOB_SAS_TOKEN_OPTION: &str = "azblob.sas-token";
/// CLI/配置项：服务端加密作用域。
pub const AZBLOB_ENCRYPTION_SCOPE_OPTION: &str = "azblob.encryption-scope";
/// CLI/配置项：客户提供密钥（customer-provided key）。
pub const AZBLOB_ENCRYPTION_KEY_OPTION: &str = "azblob.encryption-key";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// 客户提供的加密密钥及其 SHA-256，均以 Base64 存储。
pub struct AzureCustomerKey {
    pub encryption_key: String,
    pub encryption_key_sha256: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// Azure Blob 存储连接与对象前缀等完整配置。
pub struct AzureBlobStorageConfig {
    pub endpoint: String,
    pub bucket: String,
    pub prefix: String,
    pub storage_class: String,
    pub account_name: String,
    pub shared_key: String,
    pub access_sig: String,
    pub encryption_scope: String,
    pub encryption_key: Option<AzureCustomerKey>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// 从命令行/标志解析出的后端选项，可 `apply` 到配置。
pub struct AzblobBackendOptions {
    pub endpoint: String,
    pub account_name: String,
    pub account_key: String,
    pub access_tier: String,
    pub sas_token: String,
    pub encryption_scope: String,
    pub encryption_key: String,
}

impl AzblobBackendOptions {
    /// 把选项合并进 `AzureBlobStorageConfig`；空加密密钥可回退环境变量。
    pub fn apply(&mut self, backend: &mut AzureBlobStorageConfig) -> Result<()> {
        backend.endpoint.clone_from(&self.endpoint);
        backend.storage_class.clone_from(&self.access_tier);
        backend.account_name.clone_from(&self.account_name);
        backend.shared_key.clone_from(&self.account_key);
        backend.access_sig.clone_from(&self.sas_token);
        backend.encryption_scope.clone_from(&self.encryption_scope);

        if self.encryption_key.is_empty() {
            self.encryption_key = env::var("AZURE_ENCRYPTION_KEY").unwrap_or_default();
        }
        if !self.encryption_key.is_empty() {
            let digest = Sha256::digest(self.encryption_key.as_bytes());
            let encoder = base64::engine::general_purpose::STANDARD;
            backend.encryption_key = Some(AzureCustomerKey {
                encryption_key: encoder.encode(self.encryption_key.as_bytes()),
                encryption_key_sha256: encoder.encode(digest),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// 创建客户端时的附加行为：是否回写凭据、是否匿名。
pub struct StorageOptions {
    pub send_credentials: bool,
    pub no_credentials: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 选定的 Azure 认证方式。
pub enum AzureAuth {
    Sas,
    SharedKey,
    ClientSecret,
    Default,
    Anonymous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// `select_azure_client` 的结果：认证类型、账户名与服务 URL。
pub struct AzureClientSelection {
    pub auth: AzureAuth,
    pub account_name: String,
    pub service_url: String,
}

/// 按 endpoint/container/object 拼接对象 URL。
pub fn url_of_object_by_endpoint(endpoint: &str, container: &str, object: &str) -> Result<String> {
    let mut value =
        url::Url::parse(endpoint).with_context(|| format!("{endpoint} isn't a valid url"))?;
    value.set_path(&join_object_path(&[value.path(), container, object]));
    Ok(value.to_string())
}

/// 解析 `finished/total` 进度字符串。
pub fn progress(value: &str) -> Result<(i64, i64)> {
    let (finished, total) = value
        .split_once('/')
        .ok_or_else(|| anyhow!("failed to parse progress {value}"))?;
    let finished = finished
        .parse::<i64>()
        .with_context(|| format!("failed to parse progress {value}"))?;
    let total = total
        .parse::<i64>()
        .with_context(|| format!("failed to parse progress {value}"))?;
    Ok((finished, total))
}

/// 按配置与环境变量优先级选择认证方式与服务 URL。
pub fn select_azure_client(
    options: &mut AzureBlobStorageConfig,
    opts: &StorageOptions,
) -> Result<AzureClientSelection> {
    if options.bucket.is_empty() {
        bail!("bucket(container) cannot be empty to access azure blob storage");
    }

    // 优先 SAS：有账户名与 access_sig 时构造带查询串的服务 URL。
    if !options.account_name.is_empty() && !options.access_sig.is_empty() {
        let service_url = if options.endpoint.is_empty() {
            let token = options.access_sig.trim_start_matches('?');
            format!(
                "https://{}.blob.core.windows.net/?{}",
                options.account_name, token
            )
        } else {
            options.endpoint.clone()
        };
        return Ok(AzureClientSelection {
            auth: AzureAuth::Sas,
            account_name: options.account_name.clone(),
            service_url,
        });
    }

    // 其次 Shared Key：显式账户名+密钥。
    if !options.account_name.is_empty() && !options.shared_key.is_empty() {
        return Ok(AzureClientSelection {
            auth: AzureAuth::SharedKey,
            account_name: options.account_name.clone(),
            service_url: default_service_url(&options.endpoint, &options.account_name),
        });
    }

    let account_name = if options.account_name.is_empty() {
        env::var("AZURE_STORAGE_ACCOUNT").unwrap_or_default()
    } else {
        options.account_name.clone()
    };
    if account_name.is_empty() {
        bail!("account name cannot be empty to access azure blob storage");
    }
    let service_url = default_service_url(&options.endpoint, &account_name);

    let client_id = env::var("AZURE_CLIENT_ID").unwrap_or_default();
    let tenant_id = env::var("AZURE_TENANT_ID").unwrap_or_default();
    let client_secret = env::var("AZURE_CLIENT_SECRET").unwrap_or_default();
    // 再尝试服务主体（Client Secret）环境变量三元组。
    if !client_id.is_empty() && !tenant_id.is_empty() && !client_secret.is_empty() {
        if opts.send_credentials {
            options.account_name.clone_from(&account_name);
        }
        return Ok(AzureClientSelection {
            auth: AzureAuth::ClientSecret,
            account_name,
            service_url,
        });
    }

    if let Ok(shared_key) = env::var("AZURE_STORAGE_KEY") {
        if opts.send_credentials {
            options.account_name.clone_from(&account_name);
            options.shared_key = shared_key;
        }
        return Ok(AzureClientSelection {
            auth: AzureAuth::SharedKey,
            account_name,
            service_url,
        });
    }

    Ok(AzureClientSelection {
        auth: if opts.no_credentials {
            AzureAuth::Anonymous
        } else {
            AzureAuth::Default
        },
        account_name,
        service_url,
    })
}

/// endpoint 为空时使用默认 `*.blob.core.windows.net` 服务地址。
fn default_service_url(endpoint: &str, account: &str) -> String {
    if endpoint.is_empty() {
        format!("https://{account}.blob.core.windows.net")
    } else {
        endpoint.to_owned()
    }
}

/// 根据配置构建真实 Azure Blob 客户端并包装为 `AzureBlobStorage`。
pub fn new_azure_blob_storage(
    mut options: AzureBlobStorageConfig,
    opts: &StorageOptions,
) -> Result<AzureBlobStorage> {
    let selected = select_azure_client(&mut options, opts)?;
    validate_encryption_options(&options)?;

    let mut builder = MicrosoftAzureBuilder::from_env()
        .with_account(selected.account_name.clone())
        .with_container_name(options.bucket.clone());
    if !options.endpoint.is_empty() {
        builder = builder
            .with_endpoint(options.endpoint.clone())
            .with_allow_http(options.endpoint.starts_with("http://"));
    }
    // 按选定认证方式填充 MicrosoftAzureBuilder。
    match selected.auth {
        AzureAuth::Sas => {
            let query = options.access_sig.trim_start_matches('?');
            let pairs = url::form_urlencoded::parse(query.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            builder = builder.with_sas_authorization(pairs);
        }
        AzureAuth::SharedKey => {
            let key = if options.shared_key.is_empty() {
                env::var("AZURE_STORAGE_KEY").unwrap_or_default()
            } else {
                options.shared_key.clone()
            };
            builder = builder.with_access_key(key);
        }
        AzureAuth::ClientSecret => {
            builder = builder.with_client_secret_authorization(
                env::var("AZURE_CLIENT_ID").unwrap_or_default(),
                env::var("AZURE_CLIENT_SECRET").unwrap_or_default(),
                env::var("AZURE_TENANT_ID").unwrap_or_default(),
            );
        }
        AzureAuth::Anonymous => builder = builder.with_skip_signature(true),
        AzureAuth::Default => {}
    }
    if let Some(key) = &options.encryption_key {
        builder = builder.with_encryption_key(key.encryption_key.clone());
    }
    let store = Arc::new(
        builder
            .build()
            .context("failed to create Azure Blob client")?,
    );
    AzureBlobStorage::with_store(options, store, selected.account_name, selected.service_url)
}

/// 校验加密作用域/客户密钥与 access-tier 互斥约束。
fn validate_encryption_options(options: &AzureBlobStorageConfig) -> Result<()> {
    if (!options.encryption_scope.is_empty() || options.encryption_key.is_some())
        && !options.storage_class.is_empty()
    {
        bail!(
            "Set Blob Tier cannot be used with customer-provided key/scope; don't supply access-tier when using encryption"
        );
    }
    if !options.encryption_scope.is_empty() && options.encryption_key.is_some() {
        bail!("select only one of encryption-scope and customer provided key");
    }
    Ok(())
}

#[derive(Clone)]
/// 共享的 `object_store` 客户端与专用 Tokio 运行时，供同步 API 阻塞调用。
pub(crate) struct ObjectStorageCore {
    pub(crate) store: Arc<dyn ObjectStore>,
    pub(crate) runtime: Arc<Runtime>,
}

impl ObjectStorageCore {
    /// 创建双工作线程运行时并持有对象存储句柄。
    pub(crate) fn new(store: Arc<dyn ObjectStore>) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to create object-store runtime")?;
        Ok(Self {
            store,
            runtime: Arc::new(runtime),
        })
    }

    /// 无额外属性地写入对象。
    pub(crate) fn put(&self, path: &Path, data: &[u8]) -> Result<()> {
        self.put_with_attributes(path, data, Attributes::new())
    }

    /// 带对象属性（如 storage class）写入。
    pub(crate) fn put_with_attributes(
        &self,
        path: &Path,
        data: &[u8],
        attributes: Attributes,
    ) -> Result<()> {
        self.runtime.block_on(self.store.put_opts(
            path,
            Bytes::copy_from_slice(data).into(),
            PutOptions {
                attributes,
                ..Default::default()
            },
        ))?;
        Ok(())
    }

    /// Drop an in-flight cloud PUT when its caller's context is cancelled.
    pub(crate) fn put_with_context(
        &self,
        ctx: &objectio::Context,
        path: &Path,
        data: &[u8],
        attributes: Attributes,
    ) -> Result<()> {
        ctx.check()?;
        self.runtime.block_on(async {
            tokio::select! {
                result = self.store.put_opts(path, Bytes::copy_from_slice(data).into(),
                    PutOptions { attributes, ..Default::default() }) => {
                    result.map(|_| ()).map_err(Into::into)
                }
                _ = ctx.wait_cancelled() => Err(anyhow!("operation cancelled")),
            }
        })
    }

    /// 读取整个对象内容。
    pub(crate) fn get(&self, path: &Path) -> Result<Vec<u8>> {
        let result = self.runtime.block_on(self.store.get(path))?;
        Ok(self.runtime.block_on(result.bytes())?.to_vec())
    }

    /// 获取对象元数据（存在性与大小等）。
    pub(crate) fn head(&self, path: &Path) -> Result<object_store::ObjectMeta> {
        Ok(self.runtime.block_on(self.store.head(path))?)
    }

    /// 删除对象；`ignore_missing` 时忽略 NotFound。
    pub(crate) fn delete(&self, path: &Path, ignore_missing: bool) -> Result<()> {
        match self.runtime.block_on(self.store.delete(path)) {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) if ignore_missing => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// 列举指定前缀下的全部对象元数据。
    pub(crate) fn list(&self, prefix: Option<&Path>) -> Result<Vec<object_store::ObjectMeta>> {
        Ok(self
            .runtime
            .block_on(self.store.list(prefix).try_collect::<Vec<_>>())?)
    }

    /// 对象重命名（通常实现为 copy+delete）。
    pub(crate) fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.runtime.block_on(self.store.rename(from, to))?;
        Ok(())
    }

    /// 同后端内复制对象。
    pub(crate) fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        self.runtime.block_on(self.store.copy(from, to))?;
        Ok(())
    }
}

#[derive(Clone)]
/// Azure Blob 的 `storeapi::Storage` 实现。
pub struct AzureBlobStorage {
    options: AzureBlobStorageConfig,
    core: ObjectStorageCore,
    resolved_account_name: String,
    resolved_service_endpoint: String,
}

impl AzureBlobStorage {
    /// 注入已有 `ObjectStore`（测试可用内存实现）构造存储。
    pub fn with_store(
        options: AzureBlobStorageConfig,
        store: Arc<dyn ObjectStore>,
        resolved_account_name: impl Into<String>,
        resolved_service_endpoint: impl Into<String>,
    ) -> Result<Self> {
        validate_encryption_options(&options)?;
        Ok(Self {
            options,
            core: ObjectStorageCore::new(store)?,
            resolved_account_name: resolved_account_name.into(),
            resolved_service_endpoint: resolved_service_endpoint.into(),
        })
    }

    /// 返回配置引用。
    pub fn options(&self) -> &AzureBlobStorageConfig {
        &self.options
    }

    /// 解析后的账户名。
    pub fn resolved_account_name(&self) -> &str {
        &self.resolved_account_name
    }

    /// 解析后的服务端点。
    pub fn resolved_service_endpoint(&self) -> &str {
        &self.resolved_service_endpoint
    }

    /// 从另一 Azure 存储按 CopySpec 复制；同 store 指针时走原生 copy。
    pub fn copy_from(&self, source: &AzureBlobStorage, spec: &storeapi::CopySpec) -> Result<()> {
        let source_path = source.object_path(&spec.From);
        let destination_path = self.object_path(&spec.To);
        // 同一底层 store：服务端 copy；否则读出再写入。
        if Arc::ptr_eq(&self.core.store, &source.core.store) {
            return self.core.copy(&source_path, &destination_path);
        }
        let bytes = source.core.get(&source_path)?;
        self.core.put(&destination_path, &bytes)
    }

    /// 强一致性标记占位（Azure 当前无额外动作）。
    pub fn mark_strong_consistency(&self) {}

    /// 拼上配置 prefix 后的对象键。
    fn object_name(&self, name: &str) -> String {
        join_object_path(&[&self.options.prefix, name])
    }

    /// 转为 `object_store::Path`。
    fn object_path(&self, name: &str) -> Path {
        Path::from(self.object_name(name))
    }

    /// 根据 storage_class 生成上传属性。
    fn put_attributes(&self) -> Attributes {
        storage_class_attributes(&self.options.storage_class)
    }
}

impl storeapi::StrongConsistency for AzureBlobStorage {
    fn MarkStrongConsistency(&self) {
        self.mark_strong_consistency();
    }
}

impl storeapi::Storage for AzureBlobStorage {
    fn WriteFile(&self, ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        ctx.check()?;
        self.core
            .put_with_context(ctx, &self.object_path(name), data, self.put_attributes())
            .with_context(|| format!("failed to write Azure blob {name}"))
    }

    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        ctx.check()?;
        self.core
            .get(&self.object_path(name))
            .with_context(|| format!("failed to read Azure blob {name}"))
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
        self.core.delete(&self.object_path(name), false)
    }

    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        ctx.check()?;
        let object_path = self.object_path(path);
        let total = self.core.head(&object_path)?.size as i64;
        // ReaderOption 的 Start/End 为半开区间，缺省读全文件。
        let start = option.and_then(|value| value.StartOffset).unwrap_or(0);
        let end = option.and_then(|value| value.EndOffset).unwrap_or(total);
        Ok(Box::new(ObjectStoreReader::new(
            self.core.clone(),
            object_path,
            start,
            end,
            total,
            false,
        )?))
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
        let mut prefix = join_object_path(&[&self.options.prefix, &option.SubDir]);
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        prefix.push_str(&option.ObjPrefix);
        let prefix_path = (!prefix.is_empty()).then(|| Path::from(prefix));
        let mut objects = self.core.list(prefix_path.as_ref())?;
        // 对象存储返回顺序不构成接口保证；排序后提供确定的回调顺序。
        // Go 的 Azure 实现不会在客户端应用 WalkOption::StartAfter。
        objects.sort_by(|left, right| left.location.cmp(&right.location));
        for object in objects {
            let name = trim_storage_prefix(object.location.as_ref(), &self.options.prefix);
            callback(&name, object.size as i64)?;
        }
        Ok(())
    }

    fn URI(&self) -> String {
        format!("azure://{}/{}", self.options.bucket, self.options.prefix)
    }

    fn Create(
        &self,
        ctx: &objectio::Context,
        path: &str,
        _option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        ctx.check()?;
        Ok(Box::new(
            ObjectStoreWriter::new(self.core.clone(), self.object_path(path))
                .with_attributes(self.put_attributes()),
        ))
    }

    fn Rename(
        &self,
        ctx: &objectio::Context,
        old_file_name: &str,
        new_file_name: &str,
    ) -> Result<()> {
        ctx.check()?;
        self.core.rename(
            &self.object_path(old_file_name),
            &self.object_path(new_file_name),
        )
    }

    fn PresignFile(
        &self,
        _ctx: &objectio::Context,
        _file_name: &str,
        _expire: Duration,
    ) -> Result<String> {
        bail!("AzureBlobStorage backend does not support PresignFile")
    }

    fn Close(&self) {}
}

/// 基于范围 GET 的可 Seek 对象 Reader。
pub(crate) struct ObjectStoreReader {
    core: ObjectStorageCore,
    path: Path,
    pos: i64,
    end: i64,
    total: i64,
    allow_past_end: bool,
    reader: Option<Cursor<Vec<u8>>>,
}

impl ObjectStoreReader {
    /// 构造半开区间 `[pos, end)` 读取器；`allow_past_end` 控制是否允许越过文件末尾。
    pub(crate) fn new(
        core: ObjectStorageCore,
        path: Path,
        pos: i64,
        end: i64,
        total: i64,
        allow_past_end: bool,
    ) -> Result<Self> {
        if pos < 0 || end < pos {
            bail!("invalid reader range [{pos}, {end})");
        }
        Ok(Self {
            core,
            path,
            pos,
            end: end.min(total),
            total,
            allow_past_end,
            reader: None,
        })
    }

    /// 按当前 pos/end 重新拉取字节范围到本地 Cursor。
    fn reopen(&mut self) -> io::Result<()> {
        if self.pos >= self.end || self.pos >= self.total {
            self.reader = Some(Cursor::new(Vec::new()));
            return Ok(());
        }
        let range = self.pos as u64..self.end.min(self.total) as u64;
        let bytes = self
            .core
            .runtime
            .block_on(self.core.store.get_range(&self.path, range))
            .map_err(io::Error::other)?;
        self.reader = Some(Cursor::new(bytes.to_vec()));
        Ok(())
    }
}

impl Read for ObjectStoreReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let remaining = (self.end - self.pos).max(0) as usize;
        if remaining == 0 || output.is_empty() {
            return Ok(0);
        }
        if self.reader.is_none() {
            self.reopen()?;
        }
        let count_limit = output.len().min(remaining);
        let count = self
            .reader
            .as_mut()
            .expect("reader initialized")
            .read(&mut output[..count_limit])?;
        self.pos += count as i64;
        Ok(count)
    }
}

impl Seek for ObjectStoreReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let real = match position {
            SeekFrom::Start(offset) => i64::try_from(offset)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset out of range"))?,
            SeekFrom::Current(offset) => self.pos.checked_add(offset).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflow")
            })?,
            SeekFrom::End(offset) => {
                if offset > 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SeekFrom::End offset must be non-positive",
                    ));
                }
                self.total.checked_add(offset).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflow")
                })?
            }
        };
        if real < 0 || (!self.allow_past_end && real > self.total) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("seek offset {real} out of range for {} bytes", self.total),
            ));
        }
        // 位置变化则丢弃缓存 Cursor，下次 read 时 reopen 拉新范围。
        if real != self.pos {
            self.pos = real;
            self.reader = None;
        }
        Ok(real as u64)
    }
}

impl objectio::Reader for ObjectStoreReader {
    fn close(&mut self) -> io::Result<()> {
        self.reader = None;
        Ok(())
    }

    fn file_size(&self) -> io::Result<i64> {
        Ok(self.total)
    }
}

/// 内存聚合后一次性 put 的对象 Writer。
pub(crate) struct ObjectStoreWriter {
    core: ObjectStorageCore,
    path: Path,
    data: Vec<u8>,
    closed: bool,
    attributes: Attributes,
}

impl ObjectStoreWriter {
    /// 创建未关闭的空缓冲 Writer。
    pub(crate) fn new(core: ObjectStorageCore, path: Path) -> Self {
        Self {
            core,
            path,
            data: Vec::new(),
            closed: false,
            attributes: Attributes::new(),
        }
    }

    /// 设置关闭时 put 使用的对象属性。
    pub(crate) fn with_attributes(mut self, attributes: Attributes) -> Self {
        self.attributes = attributes;
        self
    }
}

impl objectio::Writer for ObjectStoreWriter {
    fn write(&mut self, ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        ctx.check()?;
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "writer is closed",
            ));
        }
        self.data.extend_from_slice(data);
        Ok(data.len())
    }

    fn close(&mut self, ctx: &objectio::Context) -> io::Result<()> {
        ctx.check()?;
        if self.closed {
            return Ok(());
        }
        self.core
            .put_with_attributes(&self.path, &self.data, self.attributes.clone())
            .map_err(io::Error::other)?;
        self.closed = true;
        Ok(())
    }
}

#[derive(Clone)]
/// 进程内内存对象存储，供单元测试与批处理演示。
pub struct MemoryStorage {
    core: ObjectStorageCore,
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self {
            core: ObjectStorageCore::new(Arc::new(object_store::memory::InMemory::new()))
                .expect("in-memory object store runtime"),
        }
    }
}

impl storeapi::Storage for MemoryStorage {
    fn WriteFile(&self, ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        ctx.check()?;
        self.core
            .put_with_context(ctx, &Path::from(name), data, Attributes::new())
    }
    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        ctx.check()?;
        self.core.get(&Path::from(name))
    }
    fn FileExists(&self, ctx: &objectio::Context, name: &str) -> Result<bool> {
        ctx.check()?;
        match self.core.head(&Path::from(name)) {
            Ok(_) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn DeleteFile(&self, ctx: &objectio::Context, name: &str) -> Result<()> {
        ctx.check()?;
        self.core.delete(&Path::from(name), true)
    }
    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        ctx.check()?;
        let path = Path::from(path);
        let total = self.core.head(&path)?.size as i64;
        Ok(Box::new(ObjectStoreReader::new(
            self.core.clone(),
            path,
            option.and_then(|value| value.StartOffset).unwrap_or(0),
            option.and_then(|value| value.EndOffset).unwrap_or(total),
            total,
            false,
        )?))
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
        let prefix = join_object_path(&[&option.SubDir, &option.ObjPrefix]);
        let prefix = (!prefix.is_empty()).then(|| Path::from(prefix));
        let mut objects = self.core.list(prefix.as_ref())?;
        objects.sort_by(|left, right| left.location.cmp(&right.location));
        for object in objects {
            let name = object.location.to_string();
            if !option.StartAfter.is_empty() && name <= option.StartAfter {
                continue;
            }
            callback(&name, object.size as i64)?;
        }
        Ok(())
    }
    fn URI(&self) -> String {
        "memory://".to_owned()
    }
    fn Create(
        &self,
        ctx: &objectio::Context,
        path: &str,
        _option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        ctx.check()?;
        Ok(Box::new(ObjectStoreWriter::new(
            self.core.clone(),
            Path::from(path),
        )))
    }
    fn Rename(&self, ctx: &objectio::Context, old: &str, new: &str) -> Result<()> {
        ctx.check()?;
        self.core.rename(&Path::from(old), &Path::from(new))
    }
    fn PresignFile(
        &self,
        _ctx: &objectio::Context,
        file: &str,
        _expire: Duration,
    ) -> Result<String> {
        Ok(format!("memory://{file}"))
    }
    fn Close(&self) {}
}

/// 类似路径规范化地拼接对象键片段（处理 `.`/`..`/多余斜杠）。
pub(crate) fn join_object_path(parts: &[&str]) -> String {
    let mut components = Vec::new();
    for part in parts {
        for component in part.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    components.pop();
                }
                value => components.push(value),
            }
        }
    }
    components.join("/")
}

/// 去掉存储 prefix，得到对外可见的相对对象名。
pub(crate) fn trim_storage_prefix(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix)
        .unwrap_or(path)
        .trim_start_matches('/')
        .to_owned()
}

/// 非空 storage class 时写入 `Attribute::StorageClass`。
pub(crate) fn storage_class_attributes(storage_class: &str) -> Attributes {
    let mut attributes = Attributes::new();
    if !storage_class.is_empty() {
        attributes.insert(Attribute::StorageClass, storage_class.to_owned().into());
    }
    attributes
}

/// 判断错误链中是否为对象不存在。
pub(crate) fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<object_store::Error>()
        .is_some_and(|error| matches!(error, object_store::Error::NotFound { .. }))
}

/// 获取 Mutex；毒化时恢复内层数据，避免测试中 panic。
pub(crate) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
