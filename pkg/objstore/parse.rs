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

// 对象存储后端 URL 解析与配置应用。
//
// 对应 Go `objstore/parse.go`：把 `s3://`/`gcs://`/`azure://`/`local`/`hdfs` 等
// 原始 URL（含 query 参数）解析为 `StorageBackend`，并支持从 `BackendOptions`
// 注入 endpoint、凭证等。凭证中的 `+` 需在 URL 解码前转义，以免被当成空格。

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use url::Url;

/// 阿里云 OSS SDK 提供者标识，写入 S3 兼容后端的 `provider` 字段。
pub const OSSProvider: &str = "oss-sdk";
/// 金山云 KS3 SDK 提供者标识。
pub const KS3SDKProvider: &str = "ks3-sdk";

/// 各云厂商后端可从命令行/配置注入的选项集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackendOptions {
    pub s3: S3BackendOptions,
    pub gcs: GCSBackendOptions,
    pub azblob: AzblobBackendOptions,
}

/// 本地文件系统后端：绝对或相对路径。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Local {
    pub path: String,
}

/// HDFS 后端：保留完整 remote URI。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Hdfs {
    pub remote: String,
}

/// S3 兼容对象存储后端配置（含 OSS/KS3 通过 provider 区分）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct S3 {
    pub bucket: String,
    pub prefix: String,
    pub endpoint: String,
    pub region: String,
    pub storage_class: String,
    pub sse: String,
    pub sse_kms_key_id: String,
    pub acl: String,
    pub access_key: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub force_path_style: bool,
    pub role_arn: String,
    pub external_id: String,
    pub provider: String,
    pub profile: String,
}

/// Google Cloud Storage 后端配置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Gcs {
    pub bucket: String,
    pub prefix: String,
    pub endpoint: String,
    pub storage_class: String,
    pub predefined_acl: String,
    pub credentials_blob: String,
}

/// Azure 客户提供的加密密钥及其 SHA-256（Base64）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AzureCustomerKey {
    pub encryption_key: String,
    pub encryption_key_sha256: String,
}

/// Azure Blob Storage 后端配置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AzureBlobStorage {
    pub bucket: String,
    pub prefix: String,
    pub endpoint: String,
    pub storage_class: String,
    pub account_name: String,
    pub shared_key: String,
    pub access_sig: String,
    pub encryption_scope: String,
    pub encryption_key: Option<AzureCustomerKey>,
}

/// 解析后的统一存储后端枚举。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageBackend {
    Local(Local),
    Hdfs(Hdfs),
    Noop,
    MemStore,
    S3(S3),
    Gcs(Gcs),
    AzureBlobStorage(AzureBlobStorage),
}

impl StorageBackend {
    /// 返回后端种类短名（local/hdfs/s3/gcs/azure 等），用于日志与分支。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Hdfs(_) => "hdfs",
            Self::Noop => "noop",
            Self::MemStore => "memstore",
            Self::S3(_) => "s3",
            Self::Gcs(_) => "gcs",
            Self::AzureBlobStorage(_) => "azure",
        }
    }
}

/// 原始存储 URL 的结构化表示：scheme/host/path/query。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedURL {
    pub scheme: String,
    pub host: String,
    pub path: String,
    query: Vec<(String, String)>,
    original: String,
}

impl ParsedURL {
    /// 还原为 URL 字符串；无 scheme 时仅返回 path。
    pub fn String(&self) -> String {
        if self.scheme.is_empty() {
            return self.path.clone();
        }
        // Preserve an explicit empty authority, as Go url.URL.String does.
        // Missing-bucket errors from ParseBackendFromURL must retain s3:///path.
        let mut value =
            if self.host.is_empty() && !self.original.starts_with(&format!("{}://", self.scheme)) {
                format!("{}:{}", self.scheme, self.path)
            } else {
                format!("{}://{}{}", self.scheme, self.host, self.path)
            };
        if !self.query.is_empty() {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(
                    self.query
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str())),
                )
                .finish();
            value.push('?');
            value.push_str(&query);
        }
        value
    }
}

/// ParseRawURL preserves plus signs in credentials by escaping them before URL decoding.
///
/// 解析原始 URL；先把 `+` 编成 `%2B`，避免密钥中的加号在 query 解码时变成空格。
pub fn ParseRawURL(raw_url: &str) -> Result<ParsedURL> {
    let escaped = raw_url.replace('+', "%2B");
    // 无冒号时视为纯本地路径，不走 URL 解析器。
    if !escaped.contains(':') {
        return Ok(ParsedURL {
            path: raw_url.to_owned(),
            original: raw_url.to_owned(),
            ..ParsedURL::default()
        });
    }
    let parsed = Url::parse(&escaped)?;
    Ok(ParsedURL {
        scheme: parsed.scheme().to_owned(),
        host: if parsed.host().is_some() {
            parsed[url::Position::BeforeHost..url::Position::AfterPort].to_owned()
        } else {
            String::new()
        },
        path: parsed.path().to_owned(),
        query: parsed
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        original: raw_url.to_owned(),
    })
}

/// 从已解析的 `ParsedURL` 构造后端（raw 为空时用 `url.String()`）。
pub fn ParseBackendFromURL(
    url: &mut ParsedURL,
    options: Option<&BackendOptions>,
) -> Result<StorageBackend> {
    parseBackend(url, "", options)
}

/// 解析原始存储 URL 并应用可选 `BackendOptions`。
pub fn ParseBackend(raw_url: &str, options: Option<&BackendOptions>) -> Result<StorageBackend> {
    if raw_url.is_empty() {
        return Err(anyhow!("empty store is not allowed"));
    }
    let mut url = ParseRawURL(raw_url)?;
    parseBackend(&mut url, raw_url, options)
}

/// 按 scheme 分派到 local/hdfs/s3/gcs/azure 等后端构造逻辑。
fn parseBackend(
    url: &mut ParsedURL,
    raw_url: &str,
    options: Option<&BackendOptions>,
) -> Result<StorageBackend> {
    let effective_raw_url = if raw_url.is_empty() {
        url.String()
    } else {
        raw_url.to_owned()
    };
    match url.scheme.as_str() {
        "" => {
            let absolute = absolute_clean_path(Path::new(&effective_raw_url))?;
            Ok(StorageBackend::Local(Local {
                path: absolute.to_string_lossy().into_owned(),
            }))
        }
        "local" | "file" => Ok(StorageBackend::Local(Local {
            path: url.path.clone(),
        })),
        "hdfs" => Ok(StorageBackend::Hdfs(Hdfs {
            remote: effective_raw_url,
        })),
        "noop" => Ok(StorageBackend::Noop),
        "memstore" => Ok(StorageBackend::MemStore),
        // S3 兼容：host 为 bucket，path 为 prefix；ks3/oss 写入对应 provider。
        "s3" | "ks3" | "oss" => {
            require_bucket(url, &effective_raw_url, "s3")?;
            let mut backend_options =
                options
                    .map(|value| value.s3.clone())
                    .unwrap_or_else(|| S3BackendOptions {
                        force_path_style: true,
                        ..S3BackendOptions::default()
                    });
            ExtractQueryParameters(url, &mut backend_options);
            backend_options.SetForcePathStyle(&effective_raw_url);
            let mut s3 = S3 {
                bucket: url.host.clone(),
                prefix: url.path.trim_matches('/').to_owned(),
                ..S3::default()
            };
            backend_options.Apply(&mut s3)?;
            if url.scheme == "ks3" {
                s3.provider = KS3SDKProvider.to_owned();
            } else if url.scheme == "oss" {
                s3.provider = OSSProvider.to_owned();
            }
            Ok(StorageBackend::S3(s3))
        }
        "gs" | "gcs" => {
            require_bucket(url, &effective_raw_url, "gcs")?;
            let mut backend_options = options.map(|value| value.gcs.clone()).unwrap_or_default();
            ExtractQueryParameters(url, &mut backend_options);
            let mut gcs = Gcs {
                bucket: url.host.clone(),
                prefix: url.path.trim_matches('/').to_owned(),
                ..Gcs::default()
            };
            backend_options.apply(&mut gcs)?;
            Ok(StorageBackend::Gcs(gcs))
        }
        "azure" | "azblob" => {
            require_bucket(url, &effective_raw_url, "azblob")?;
            let mut backend_options = options
                .map(|value| value.azblob.clone())
                .unwrap_or_default();
            ExtractQueryParameters(url, &mut backend_options);
            let mut azure = AzureBlobStorage {
                bucket: url.host.clone(),
                prefix: url.path.trim_matches('/').to_owned(),
                ..AzureBlobStorage::default()
            };
            backend_options.apply(&mut azure)?;
            Ok(StorageBackend::AzureBlobStorage(azure))
        }
        scheme => Err(anyhow!("storage {scheme} not support yet")),
    }
}

/// Match Go `filepath.Abs`: make a path absolute and clean `.`/`..` without
/// resolving symlinks or requiring the target to exist.
fn absolute_clean_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                clean.push(component.as_os_str());
            }
        }
    }
    Ok(clean)
}

/// 对象存储 URL 必须带 bucket（host 非空）。
fn require_bucket(url: &ParsedURL, raw_url: &str, provider: &str) -> Result<()> {
    if url.host.is_empty() {
        Err(anyhow!(
            "please specify the bucket for {provider} in {}",
            parser_ast::misc::redact_url(raw_url)
        ))
    } else {
        Ok(())
    }
}

/// 后端选项从 URL query 键值填充的接口。
pub trait QueryParameterOptions {
    fn set_query_parameter(&mut self, key: &str, value: &str);
}

/// 把 URL query 写入 options，并清空 query（避免残留进最终 URL）。
pub fn ExtractQueryParameters<T: QueryParameterOptions>(url: &mut ParsedURL, options: &mut T) {
    for (key, value) in &url.query {
        options.set_query_parameter(&NormalizeQueryParameterKey(key), value);
    }
    url.query.clear();
}

/// 规范化 query 键：下划线转连字符并小写，兼容 `force_path_style` 与 `force-path-style`。
pub fn NormalizeQueryParameterKey(key: &str) -> String {
    key.replace('_', "-").to_lowercase()
}

/// 可注入到 S3 兼容后端的配置项（endpoint、SSE、凭证、path-style 等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct S3BackendOptions {
    pub endpoint: String,
    pub region: String,
    pub storage_class: String,
    pub sse: String,
    pub sse_kms_key_id: String,
    pub acl: String,
    pub access_key: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub provider: String,
    pub force_path_style: bool,
    pub use_accelerate_endpoint: bool,
    pub role_arn: String,
    pub external_id: String,
    pub profile: String,
    pub object_lock_enabled: bool,
}

impl S3BackendOptions {
    /// 校验 endpoint/凭证配对后，把选项拷贝到 `S3` 结构。
    pub fn Apply(&self, s3: &mut S3) -> Result<()> {
        if !self.endpoint.is_empty() {
            if !self.endpoint.contains(':') {
                return Err(anyhow!("scheme not found in endpoint"));
            }
            let endpoint = Url::parse(&self.endpoint)?;
            // Go's net/url keeps `http:12345` as an opaque URL with no host,
            // while url::Url normalizes it as an HTTP URL. Require the `//`
            // authority marker so both implementations reject it alike.
            // 要求 `://` 权威部分，使 Rust 与 Go 同样拒绝无 host 的 endpoint。
            if !self.endpoint.contains("://") || endpoint.host_str().is_none() {
                return Err(anyhow!("host not found in endpoint"));
            }
        }
        if self.profile.is_empty() {
            if self.access_key.is_empty() && !self.secret_access_key.is_empty() {
                return Err(anyhow!("access_key not found"));
            }
            if !self.access_key.is_empty() && self.secret_access_key.is_empty() {
                return Err(anyhow!("secret_access_key not found"));
            }
        }
        s3.endpoint = self.endpoint.trim_end_matches('/').to_owned();
        s3.region = self.region.clone();
        s3.storage_class = self.storage_class.clone();
        s3.sse = self.sse.clone();
        s3.sse_kms_key_id = self.sse_kms_key_id.clone();
        s3.acl = self.acl.clone();
        s3.access_key = self.access_key.clone();
        s3.secret_access_key = self.secret_access_key.clone();
        s3.session_token = self.session_token.clone();
        s3.force_path_style = self.force_path_style;
        s3.role_arn = self.role_arn.clone();
        s3.external_id = self.external_id.clone();
        s3.provider = self.provider.clone();
        s3.profile = self.profile.clone();
        Ok(())
    }

    /// 按厂商/加速域名/显式参数决定是否使用 path-style 寻址。
    pub fn SetForcePathStyle(&mut self, raw_url: &str) {
        let explicit = raw_url.contains("force-path-style") || raw_url.contains("force_path_style");
        // AWS 虚拟主机风格：未显式指定时关闭 force-path-style。
        let aws_virtual_host = !explicit
            && (self.provider == "aws"
                || self.endpoint.contains("amazonaws.com")
                || !self.role_arn.is_empty());
        if matches!(self.provider.as_str(), "alibaba" | "netease" | "tencent")
            || self.use_accelerate_endpoint
            || aws_virtual_host
        {
            self.force_path_style = false;
        }
    }
}

impl QueryParameterOptions for S3BackendOptions {
    fn set_query_parameter(&mut self, key: &str, value: &str) {
        match key {
            "endpoint" => self.endpoint = value.to_owned(),
            "region" => self.region = value.to_owned(),
            "storage-class" => self.storage_class = value.to_owned(),
            "sse" => self.sse = value.to_owned(),
            "sse-kms-key-id" => self.sse_kms_key_id = value.to_owned(),
            "acl" => self.acl = value.to_owned(),
            "access-key" => self.access_key = value.to_owned(),
            "secret-access-key" => self.secret_access_key = value.to_owned(),
            "session-token" => self.session_token = value.to_owned(),
            "provider" => self.provider = value.to_owned(),
            "role-arn" => self.role_arn = value.to_owned(),
            "external-id" => self.external_id = value.to_owned(),
            "profile" => self.profile = value.to_owned(),
            "force-path-style" => {
                if let Some(parsed) = parse_go_bool(value) {
                    self.force_path_style = parsed;
                }
            }
            "use-accelerate-endpoint" => {
                if let Some(parsed) = parse_go_bool(value) {
                    self.use_accelerate_endpoint = parsed;
                }
            }
            "object-lock-enabled" => {
                if let Some(parsed) = parse_go_bool(value) {
                    self.object_lock_enabled = parsed;
                }
            }
            _ => {}
        }
    }
}

/// 解析 Go `strconv.ParseBool` 接受的真值/假值字面量；非法则返回 `None`。
fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

/// GCS 可注入选项：endpoint、存储类、预定义 ACL、凭证文件路径。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GCSBackendOptions {
    pub endpoint: String,
    pub storage_class: String,
    pub predefined_acl: String,
    pub credentials_file: String,
}

impl GCSBackendOptions {
    /// 应用选项；若指定凭证文件则读入 `credentials_blob`。
    fn apply(&self, gcs: &mut Gcs) -> Result<()> {
        gcs.endpoint = self.endpoint.clone();
        gcs.storage_class = self.storage_class.clone();
        gcs.predefined_acl = self.predefined_acl.clone();
        if !self.credentials_file.is_empty() {
            gcs.credentials_blob = std::fs::read_to_string(&self.credentials_file)?;
        }
        Ok(())
    }
}

impl QueryParameterOptions for GCSBackendOptions {
    fn set_query_parameter(&mut self, key: &str, value: &str) {
        match key {
            "endpoint" => self.endpoint = value.to_owned(),
            "storage-class" => self.storage_class = value.to_owned(),
            "predefined-acl" => self.predefined_acl = value.to_owned(),
            "credentials-file" => self.credentials_file = value.to_owned(),
            _ => {}
        }
    }
}

/// Azure Blob 可注入选项：账号、密钥、SAS、加密范围/密钥等。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
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
    /// 应用选项；加密密钥可来自字段或环境变量 `AZURE_ENCRYPTION_KEY`。
    fn apply(&self, azure: &mut AzureBlobStorage) -> Result<()> {
        azure.endpoint = self.endpoint.clone();
        azure.storage_class = self.access_tier.clone();
        azure.account_name = self.account_name.clone();
        azure.shared_key = self.account_key.clone();
        azure.access_sig = self.sas_token.clone();
        azure.encryption_scope = self.encryption_scope.clone();
        let key = if self.encryption_key.is_empty() {
            std::env::var("AZURE_ENCRYPTION_KEY").unwrap_or_default()
        } else {
            self.encryption_key.clone()
        };
        if !key.is_empty() {
            // Azure 要求同时提供密钥原文（Base64）与其 SHA-256。
            let digest = Sha256::digest(key.as_bytes());
            azure.encryption_key = Some(AzureCustomerKey {
                encryption_key: base64::engine::general_purpose::STANDARD.encode(key.as_bytes()),
                encryption_key_sha256: base64::engine::general_purpose::STANDARD.encode(digest),
            });
        }
        Ok(())
    }
}

impl QueryParameterOptions for AzblobBackendOptions {
    fn set_query_parameter(&mut self, key: &str, value: &str) {
        match key {
            "endpoint" => self.endpoint = value.to_owned(),
            "account-name" => self.account_name = value.to_owned(),
            "account-key" => self.account_key = value.to_owned(),
            "access-tier" => self.access_tier = value.to_owned(),
            "sas-token" => self.sas_token = value.to_owned(),
            "encryption-scope" => self.encryption_scope = value.to_owned(),
            "encryption-key" => self.encryption_key = value.to_owned(),
            _ => {}
        }
    }
}

/// 把后端格式化为不含敏感 query 的规范 URL（仅 scheme/bucket/prefix）。
pub fn FormatBackendURL(backend: &StorageBackend) -> String {
    match backend {
        StorageBackend::Local(local) => format_backend_url("local", "", &local.path),
        StorageBackend::Noop => "noop:///".to_owned(),
        StorageBackend::MemStore => "memstore://".to_owned(),
        StorageBackend::Hdfs(hdfs) => hdfs.remote.clone(),
        StorageBackend::S3(s3) => format_backend_url("s3", &s3.bucket, &s3.prefix),
        StorageBackend::Gcs(gcs) => format_backend_url("gcs", &gcs.bucket, &gcs.prefix),
        StorageBackend::AzureBlobStorage(azure) => {
            format_backend_url("azure", &azure.bucket, &azure.prefix)
        }
    }
}

/// 拼装 `scheme://host/path`，并对 path 做 URL 编码。
fn format_backend_url(scheme: &str, host: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        let mut url = Url::parse(&format!("{scheme}://{host}/"))
            .expect("backend scheme and host must form a URL");
        url.set_path(path);
        url.to_string()
    }
}

/// 判断路径字符串是否指向本地存储。
pub fn IsLocalPath(path: &str) -> Result<bool> {
    Ok(IsLocal(&ParseRawURL(path)?))
}

/// 判断已解析 URL 是否为 local/file/无 scheme。
pub fn IsLocal(url: &ParsedURL) -> bool {
    matches!(url.scheme.as_str(), "" | "local" | "file")
}

/// 判断是否为 S3 兼容 scheme（含 OSS）。
pub fn IsS3Like(url: &ParsedURL) -> bool {
    matches!(url.scheme.as_str(), "s3" | "oss")
}
