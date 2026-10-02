// Copyright 2026 PingCAP, Inc.
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

// S3 对象存储后端的构造与凭证装载。
//
// 对应 Go `s3store/store.go`：根据 `backuppb::S3` 配置选择静态密钥、阿里云 RAM
// 元数据或 AWS 默认凭证链，创建 SDK 客户端，探测桶区域（bucket region），
// 校验权限，并可选查询对象锁（Object Lock）是否开启，最终包装为 `s3like::Storage`。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_types::region::Region;
use serde::Deserialize;
use tokio::runtime::Runtime;

use crate::backuppb;
use crate::{AwsS3Api, GetObjectLockConfigurationInput, RequestOptions, S3API, S3Client};

/// 未指定 region 时使用的 AWS 默认区域。
pub const DEFAULT_REGION: &str = "us-east-1";
/// 阿里云 OSS/S3 兼容 endpoint 域名片段，用于识别走阿里云元数据凭证。
pub const DOMAIN_ALIYUN: &str = "aliyuncs.com";
/// 阿里云 ECS RAM 角色凭证的元数据 URL 前缀。
const ALIYUN_METADATA: &str = "http://100.100.100.200/latest/meta-data/ram/security-credentials/";

/// 凭证来源：静态密钥、阿里云元数据，或 AWS 默认凭证链。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialSource {
    /// AccessKey + SecretAccessKey 已在配置中给出。
    Static,
    /// Endpoint 指向阿里云，从 ECS 元数据拉取 RAM 临时凭证。
    AliyunMetadata,
    /// 交给 AWS SDK 默认链（环境变量、共享配置、IMDS 等）。
    DefaultChain,
}

#[derive(Clone, Copy)]
enum ClientPurpose {
    Storage,
    RegionProbe,
}

/// 按配置字段判断应使用的凭证来源。
pub fn credential_source(options: &backuppb::S3) -> CredentialSource {
    if !options.AccessKey.is_empty() && !options.SecretAccessKey.is_empty() {
        CredentialSource::Static
    } else if options.Endpoint.contains(DOMAIN_ALIYUN) {
        CredentialSource::AliyunMetadata
    } else {
        CredentialSource::DefaultChain
    }
}

/// 根据 backend 与 store Options 构造可用的 `s3like::Storage`。
///
/// 流程：校验 ctx → 装载 SDK 配置与 API → 按需回填/清除凭证 → 探测并校正 region →
/// 规范化前缀 → 权限检查 → 可选 Object Lock 探测 → 返回存储门面。
pub fn NewS3Storage(
    ctx: &storeapi::Context,
    backend: &mut backuppb::S3,
    options: &storeapi::Options,
) -> Result<s3like::Storage> {
    ctx.check()?;
    let mut query = backend.clone();
    // 配置为空时回退到默认区域，避免 SDK 无法解析。
    let configured_region = if query.Region.is_empty() {
        DEFAULT_REGION.to_owned()
    } else {
        query.Region.clone()
    };
    let runtime = Arc::new(Runtime::new().context("create S3 async runtime")?);
    let mut sdk_config = load_sdk_config(&runtime, &query, &configured_region, options)?;
    let mut api = build_api(
        &runtime,
        &sdk_config,
        &query,
        options,
        ClientPurpose::Storage,
    );

    // SendCredentials=false 时从 backend 抹掉密钥，避免序列化外泄；
    // 否则若配置未带密钥，尝试从已解析的 provider 回填到 backend。
    if !options.SendCredentials {
        backend.AccessKey.clear();
        backend.SecretAccessKey.clear();
        backend.SessionToken.clear();
    } else if query.AccessKey.is_empty() || query.SecretAccessKey.is_empty() {
        if let Some(provider) = sdk_config.credentials_provider() {
            if let Ok(credentials) = runtime.block_on(provider.provide_credentials()) {
                backend.AccessKey = credentials.access_key_id().to_owned();
                backend.SecretAccessKey = credentials.secret_access_key().to_owned();
                backend.SessionToken = credentials.session_token().unwrap_or_default().to_owned();
            }
        }
    }

    // 官方 AWS S3 通过 API 探测真实 region；其他 provider 信任配置中的 Region。
    let official_s3 =
        (query.Provider.is_empty() || query.Provider == "aws") && !is_gcs_s3_compatible(&query);
    let mut detected_region = if official_s3 {
        // The region probe uses the same credentials and transport, but its
        // retry policy and classifiers are independent of S3Retryer.
        build_api(
            &runtime,
            &sdk_config,
            &query,
            options,
            ClientPurpose::RegionProbe,
        )
        .bucket_region(ctx, &query.Bucket)
        .with_context(|| format!("failed to get region of bucket {}", query.Bucket))?
    } else {
        query.Region.clone()
    };
    if official_s3 && detected_region.is_empty() {
        detected_region = DEFAULT_REGION.to_owned();
    }
    // 配置 region 与探测结果不一致：已显式配置则报错；否则写回并必要时重建客户端。
    if query.Region != detected_region {
        if !query.Region.is_empty() {
            return Err(anyhow!(
                "s3 bucket and region are not matched, bucket={}, input region={}, real region={}",
                query.Bucket,
                query.Region,
                detected_region
            ));
        }
        query.Region = detected_region.clone();
        backend.Region = detected_region.clone();
        if detected_region != DEFAULT_REGION {
            sdk_config = load_sdk_config(&runtime, &query, &detected_region, options)?;
            api = build_api(
                &runtime,
                &sdk_config,
                &query,
                options,
                ClientPurpose::Storage,
            );
        }
    }

    query.Prefix = storeapi::NewPrefix(&query.Prefix).String();
    let bucket_prefix = storeapi::NewBucketPrefix(&query.Bucket, &query.Prefix);
    let client = S3Client::new(
        api.clone(),
        bucket_prefix.clone(),
        query.clone(),
        !official_s3,
    );
    s3like::CheckPermissions(ctx, &client, &options.CheckPermissions)
        .context("check S3 permissions")?;

    if options.CheckS3ObjectLockOptions {
        backend.ObjectLockEnabled = IsObjectLockEnabled(api, &query);
    }
    Ok(s3like::NewStorage(
        client,
        bucket_prefix,
        query,
        options.AccessRecording.clone(),
    ))
}

/// 由 SDK 全局配置构建带 endpoint / path-style 的 `AwsS3Api`。
fn build_api(
    runtime: &Arc<Runtime>,
    config: &aws_types::SdkConfig,
    options: &backuppb::S3,
    store_options: &storeapi::Options,
    purpose: ClientPurpose,
) -> Arc<AwsS3Api> {
    let mut builder =
        aws_sdk_s3::config::Builder::from(config).force_path_style(options.ForcePathStyle);
    if let Some(http_client) = http_client_for_options(store_options) {
        builder.set_http_client(Some(http_client));
    }
    if matches!(purpose, ClientPurpose::RegionProbe) {
        builder = builder.retry_config(
            <crate::S3StandardRetryer as storeapi::Retryer>::retry_config(
                &crate::S3StandardRetryer,
            ),
        );
    } else if let Some(classifier) = retry_classifier_for_options(store_options) {
        builder.push_retry_classifier(classifier);
    }
    // As in Go, the endpoint is S3-local rather than global so AssumeRole STS
    // continues to use its normal endpoint.
    // 与 Go 一致：endpoint 仅作用于 S3，避免影响 AssumeRole 的 STS 端点。
    if !options.Endpoint.is_empty() {
        builder = builder.endpoint_url(options.Endpoint.clone());
    }
    if is_gcs_s3_compatible(options) {
        crate::gcs_s3_signer::configure_gcs_signer(&mut builder);
    }
    let client = aws_sdk_s3::Client::from_conf(builder.build());
    Arc::new(AwsS3Api::new(
        client,
        runtime.clone(),
        store_options.AccessRecording.clone(),
    ))
}

/// 装载 AWS SDK 配置：region、profile/静态凭证，以及可选的 AssumeRole。
fn load_sdk_config(
    runtime: &Runtime,
    options: &backuppb::S3,
    region: &str,
    store_options: &storeapi::Options,
) -> Result<aws_types::SdkConfig> {
    let mut loader =
        aws_config::defaults(BehaviorVersion::latest()).region(Region::new(region.to_owned()));
    loader = loader.retry_config(retry_config_for_options(store_options));
    if let Some(http_client) = http_client_for_options(store_options) {
        loader = loader.http_client(http_client);
    }
    // profile 优先；否则尝试 autoNewCred 注入静态/阿里云凭证；都没有则用默认链。
    if !options.Profile.is_empty() {
        loader = loader.profile_name(&options.Profile);
    } else if let Some(credentials) = autoNewCred(options)? {
        loader = loader.credentials_provider(credentials);
    }
    let mut config = runtime.block_on(loader.load());
    // RoleArn 非空时用 STS AssumeRole 包装凭证，并可附带 ExternalId。
    if !options.RoleArn.is_empty() {
        let mut builder =
            aws_config::sts::AssumeRoleProvider::builder(&options.RoleArn).configure(&config);
        if !options.ExternalId.is_empty() {
            builder = builder.external_id(&options.ExternalId);
        }
        let provider = runtime.block_on(builder.build());
        config = config
            .into_builder()
            .credentials_provider(SharedCredentialsProvider::new(provider))
            .build();
    }
    Ok(config)
}

/// 选择调用方提供的 AWS 重试配置；未提供时使用 TiDB 的 20 次/32 秒上限策略。
pub fn retry_config_for_options(
    options: &storeapi::Options,
) -> aws_sdk_s3::config::retry::RetryConfig {
    options
        .S3Retryer
        .as_ref()
        .map(|retryer| retryer.retry_config())
        .unwrap_or_else(|| {
            <crate::S3StandardRetryer as storeapi::Retryer>::retry_config(&crate::S3StandardRetryer)
        })
}

/// 克隆调用方提供的共享 HTTP client，供配置加载器和 S3 service 共用连接池。
pub fn http_client_for_options(
    options: &storeapi::Options,
) -> Option<aws_sdk_s3::config::SharedHttpClient> {
    options.HTTPClient.clone()
}

/// 选择调用方分类器；默认使用 TiDB 的 EC2 元数据错误禁重试分类器。
pub fn retry_classifier_for_options(
    options: &storeapi::Options,
) -> Option<storeapi::aws_smithy_runtime_api::client::retries::classifiers::SharedRetryClassifier> {
    options
        .S3Retryer
        .as_ref()
        .and_then(|retryer| retryer.retry_classifier())
        .or_else(|| {
            <crate::S3StandardRetryer as storeapi::Retryer>::retry_classifier(
                &crate::S3StandardRetryer,
            )
        })
}

/// 查询桶是否启用对象锁；API 失败时视为未启用（返回 false）。
pub fn IsObjectLockEnabled<T>(svc: Arc<T>, options: &backuppb::S3) -> bool
where
    T: S3API + ?Sized,
{
    svc.get_object_lock_configuration(
        &storeapi::Context::default(),
        &GetObjectLockConfigurationInput {
            bucket: options.Bucket.clone(),
        },
        RequestOptions::default(),
    )
    .unwrap_or(false)
}

/// 测试用构造：注入已有 `S3API` 实现，跳过真实网络与凭证装载。
pub fn NewS3StorageForTest<T>(
    svc: Arc<T>,
    options: &backuppb::S3,
    access_rec: Option<Arc<objectio::recording::AccessStats>>,
) -> s3like::Storage
where
    T: S3API + 'static,
{
    let bucket_prefix = storeapi::NewBucketPrefix(&options.Bucket, &options.Prefix);
    s3like::NewStorage(
        S3Client::new(svc, bucket_prefix.clone(), options.clone(), false),
        bucket_prefix,
        options.clone(),
        access_rec,
    )
}

/// 按 `credential_source` 自动生成 SDK `Credentials`；默认链返回 `None`。
pub fn autoNewCred(options: &backuppb::S3) -> Result<Option<Credentials>> {
    match credential_source(options) {
        CredentialSource::Static => Ok(Some(Credentials::new(
            options.AccessKey.clone(),
            options.SecretAccessKey.clone(),
            (!options.SessionToken.is_empty()).then(|| options.SessionToken.clone()),
            None,
            "tidb-s3-static",
        ))),
        CredentialSource::AliyunMetadata => createOssRAMCred(),
        CredentialSource::DefaultChain => Ok(None),
    }
}

/// 阿里云 RAM 元数据返回的临时凭证 JSON 字段。
#[derive(Deserialize)]
#[allow(non_snake_case)]
struct AliyunRamCredential {
    AccessKeyId: String,
    AccessKeySecret: String,
    SecurityToken: String,
    Code: String,
}

/// 从阿里云 ECS 元数据服务拉取当前 RAM 角色的临时凭证；失败返回 `Ok(None)`。
pub fn createOssRAMCred() -> Result<Option<Credentials>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    // 先取角色名，再拼 URL 拉凭证；任一 HTTP 失败都降级为无凭证。
    let role = match client.get(ALIYUN_METADATA).send() {
        Ok(response) if response.status().is_success() => response.text()?.trim().to_owned(),
        Ok(_) | Err(_) => return Ok(None),
    };
    if role.is_empty() {
        return Ok(None);
    }
    let credential: AliyunRamCredential =
        match client.get(format!("{ALIYUN_METADATA}{role}")).send() {
            Ok(response) if response.status().is_success() => response.json()?,
            Ok(_) | Err(_) => return Ok(None),
        };
    if credential.Code != "Success" {
        return Ok(None);
    }
    Ok(Some(Credentials::new(
        credential.AccessKeyId,
        credential.AccessKeySecret,
        Some(credential.SecurityToken),
        None,
        "aliyun-ram-metadata",
    )))
}

/// Detect GCS by explicit provider or the XML API endpoint, independently of
/// the AWS provider setting. An opaque/relative URL has no hostname in Go.
pub fn is_gcs_s3_compatible(options: &backuppb::S3) -> bool {
    // Go's EqualFold also accepts the Unicode long s (ſ), whose uppercase is S.
    if options.Provider.to_uppercase() == "GCS" {
        return true;
    }
    let endpoint = &options.Endpoint;
    let (parse_endpoint, authority) = if let Some(rest) = endpoint.strip_prefix("//") {
        (format!("http:{endpoint}"), rest)
    } else if let Some((scheme, rest)) = endpoint.split_once(':') {
        if !scheme
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            || !scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        {
            return false;
        }
        let Some(authority) = rest.strip_prefix("//") else {
            return false;
        };
        (endpoint.clone(), authority)
    } else {
        return false;
    };
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    // WHATWG URL parsing can manufacture a host from extra slashes or map
    // fullwidth ASCII using IDNA. Go's url.Parse/Hostname does neither.
    if authority.is_empty() || authority.contains('\\') {
        return false;
    }
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let (hostname, port) = host_port
        .split_once(':')
        .map_or((host_port, None), |(host, port)| (host, Some(port)));
    if port.is_some_and(|value| !value.bytes().all(|b| b.is_ascii_digit())) {
        return false;
    }
    // Go accepts any numeric port here; WHATWG parsing limits it to u16.
    // Validate the URL with the port removed, retaining the original hostname.
    let validation_endpoint = if port.is_some() {
        let start = parse_endpoint.find("//").unwrap() + 2;
        let end = start + authority.len();
        format!(
            "{}{}{}{}",
            &parse_endpoint[..start],
            &authority[..authority.len() - host_port.len()],
            hostname,
            &parse_endpoint[end..]
        )
    } else {
        parse_endpoint
    };
    let path_and_authority = endpoint.split('?').next().unwrap_or_default();
    let fragment = endpoint
        .split_once('#')
        .map_or("", |(_, fragment)| fragment);
    if endpoint.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || !valid_url_escapes(path_and_authority)
        || !valid_url_escapes(fragment)
        || reqwest::Url::parse(&validation_endpoint).is_err()
    {
        return false;
    }
    let host = hostname.to_lowercase();
    host == "storage.googleapis.com" || host.ends_with(".storage.googleapis.com")
}

fn valid_url_escapes(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1..index + 3]
                    .iter()
                    .all(u8::is_ascii_hexdigit)
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}
