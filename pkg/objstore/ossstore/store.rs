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

// 阿里云 OSS 对象存储后端构造与区域/内网端点解析。
//
// 对应 Go `ossstore/store.go`：把 `backuppb.S3` 配置转为可读写的 `s3like::Storage`。
// 凭证优先用显式 AccessKey；否则走 reqsign 默认链（环境变量/配置文件/ECS RAM 角色/
// OIDC/AssumeRole）。同 Region 的 ECS 可选用内网 endpoint 降低延迟与流量费。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};

use crate::{
    API, AliyunOssApi, Client, CredentialRefresher, CredentialsProvider,
    ReqsignCredentialsProvider, StaticCredentialsProvider,
};

/// 未指定 Region 时的默认地域（杭州）。
pub const DEFAULT_REGION: &str = "cn-hangzhou";
/// ECS RAM 角色凭证提供者名称片段，用于判断是否查询实例元数据。
pub const ECS_RAM_ROLE_PROVIDER_NAME: &str = "ecs_ram_role";
/// ECS 元数据中读取实例所在 Region 的 URL。
pub const REGION_ID_META_URL: &str = "http://100.100.100.200/latest/meta-data/region-id";

/// 包装 `s3like::Storage` 与可选的后台凭证刷新器。
pub struct OSSStore {
    /// 对外暴露的对象存储实现。
    pub Storage: s3like::Storage,
    /// 动态凭证场景下的周期性刷新器；静态密钥时为 `None`。
    credential_refresher: Option<Arc<CredentialRefresher>>,
}

impl OSSStore {
    /// 关闭底层存储并停止凭证刷新后台任务。
    pub fn Close(&self) {
        self.Storage.Close();
        if let Some(refresher) = &self.credential_refresher {
            refresher.close();
        }
    }
}

/// 拷贝并清洗后端配置：禁止把 OSS 凭证下发给 TiKV，并清空敏感字段。
pub fn prepare_backend(
    backend: &mut s3like::backuppb::S3,
    send_credentials: bool,
) -> Result<s3like::backuppb::S3> {
    let copy = backend.clone();
    if send_credentials {
        return Err(anyhow!("sending OSS credentials to TiKV is not supported"));
    }
    // 避免凭证残留在可能外传的配置中。
    backend.AccessKey.clear();
    backend.SecretAccessKey.clear();
    backend.SessionToken.clear();
    Ok(copy)
}

/// 按 Region 与是否内网拼出默认 OSS endpoint。
pub fn endpoint_for_region(region: &str, internal: bool) -> String {
    if internal {
        format!("https://oss-{region}-internal.aliyuncs.com")
    } else {
        format!("https://oss-{region}.aliyuncs.com")
    }
}

/// 去掉 Location API 返回值里可能带的 `oss-` 前缀，得到标准 Region ID。
pub fn trim_oss_region_id(region: &str) -> String {
    region.strip_prefix("oss-").unwrap_or(region).to_owned()
}

/// 仅当 ECS 与 Bucket 同 Region 且 ECS Region 非空时才走内网 endpoint。
pub fn can_use_internal_endpoint(ecs_region_id: &str, bucket_region_id: &str) -> bool {
    !ecs_region_id.is_empty() && ecs_region_id == bucket_region_id
}

/// 若凭证来自 ECS RAM 角色，则从实例元数据读取本机 Region；否则返回空串。
fn metadata_region(provider_name: &str) -> Result<String> {
    if !provider_name.contains(ECS_RAM_ROLE_PROVIDER_NAME) {
        return Ok(String::new());
    }
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(30))
        .build()?
        .get(REGION_ID_META_URL)
        .send()?
        .error_for_status()?
        .text()
        .map(|value| value.trim().to_owned())
        .context("failed to get region ID from ECS metadata service")
}

/// 按凭证、endpoint/Region、访问统计构造 `AliyunOssApi`。
fn build_api(
    credentials: Arc<dyn CredentialsProvider>,
    qs: &s3like::backuppb::S3,
    region: &str,
    internal: bool,
    access_rec: Option<Arc<objectio::recording::AccessStats>>,
) -> Result<Arc<AliyunOssApi>> {
    let endpoint = if qs.Endpoint.is_empty() {
        endpoint_for_region(region, internal)
    } else {
        qs.Endpoint.clone()
    };
    Ok(Arc::new(AliyunOssApi::new(
        credentials,
        endpoint,
        region.to_owned(),
        access_rec,
    )?))
}

/// Creates a real Aliyun OSS-backed s3like storage. Credential resolution uses
/// reqsign's default chain (environment/profile/files/URI/ECS/OIDC/AssumeRole),
/// while object operations use ali-oss-rs.
///
/// 创建真实阿里云 OSS 后端的 `s3like` 存储。凭证经 reqsign 默认链解析，
/// 对象操作走 ali-oss-rs；会探测 Bucket 真实 Region 并在同区 ECS 上启用内网。
pub fn NewOSSStorage(
    ctx: &storeapi::Context,
    backend: &mut s3like::backuppb::S3,
    opts: &storeapi::Options,
) -> Result<OSSStore> {
    let mut qs = prepare_backend(backend, opts.SendCredentials)?;
    if qs.ForcePathStyle {
        log::warn!("force-path-style is not supported on OSS");
    }

    // 显式 AK/SK 用静态提供者；否则走 reqsign 链并启动后台刷新。
    let (credentials, credential_refresher): (
        Arc<dyn CredentialsProvider>,
        Option<Arc<CredentialRefresher>>,
    ) = if !qs.AccessKey.is_empty() && !qs.SecretAccessKey.is_empty() {
        (
            Arc::new(StaticCredentialsProvider::new(
                qs.AccessKey.clone(),
                qs.SecretAccessKey.clone(),
                qs.SessionToken.clone(),
            )),
            None,
        )
    } else {
        let provider: Arc<dyn CredentialsProvider> = Arc::new(
            ReqsignCredentialsProvider::new(&qs.RoleArn, &qs.ExternalId)
                .context("failed to configure Aliyun default credential provider")?,
        );
        let refresher = Arc::new(CredentialRefresher::new(provider));
        refresher
            .refresh_once()
            .context("failed to get initial OSS credentials")?;
        (refresher.clone(), Some(refresher))
    };

    let credential = credentials.get_credentials()?;
    let ecs_region_id = metadata_region(&credential.provider_name)?;
    let input_region = if qs.Region.is_empty() {
        DEFAULT_REGION
    } else {
        &qs.Region
    };
    // 先用公网 API 探测 Bucket 真实所在 Region，并校验用户输入。
    let location_api = build_api(
        credentials.clone(),
        &qs,
        input_region,
        false,
        opts.AccessRecording.clone(),
    )?;
    let detected_region = trim_oss_region_id(
        &location_api
            .bucket_location(ctx, &qs.Bucket)
            .with_context(|| format!("failed to get location of bucket {}", qs.Bucket))?,
    );
    if !qs.Region.is_empty() && detected_region != qs.Region {
        return Err(anyhow!(
            "bucket and region are not matched, bucket={}, input region={}, real region={}",
            qs.Bucket,
            qs.Region,
            detected_region
        ));
    }

    let use_internal_endpoint = can_use_internal_endpoint(&ecs_region_id, &detected_region);
    log::info!(
        "succeed to get bucket region: bucketRegion={detected_region}, ecsRegion={ecs_region_id}, useInternalEndpoint={use_internal_endpoint}"
    );
    qs.Prefix = storeapi::NewPrefix(&qs.Prefix).String();
    let bucket_prefix = storeapi::NewBucketPrefix(&qs.Bucket, &qs.Prefix);
    let api = build_api(
        credentials.clone(),
        &qs,
        &detected_region,
        use_internal_endpoint,
        opts.AccessRecording.clone(),
    )?;
    // 预签名 URL 可能在阿里云 VPC 外消费，因此始终使用公网 endpoint；显式
    // 自定义 endpoint 仍由 build_api 保留。
    let presign_api = build_api(
        credentials,
        &qs,
        &detected_region,
        false,
        opts.AccessRecording.clone(),
    )?;
    let client = Client::with_presign_api(api, presign_api, bucket_prefix.clone(), qs.clone());
    s3like::CheckPermissions(ctx, &client, &opts.CheckPermissions)
        .map_err(|error| anyhow!("check permission failed due to {error}"))?;

    if let Some(refresher) = &credential_refresher {
        refresher
            .start_refresh()
            .context("failed to start OSS credential refresher")?;
    }
    Ok(OSSStore {
        Storage: s3like::NewStorage(client, bucket_prefix, qs, opts.AccessRecording.clone()),
        credential_refresher,
    })
}

/// 测试用：用注入的 `API` 实现直接构造 `s3like::Storage`，跳过真实联网探测。
pub fn new_oss_storage_for_test(
    svc: Arc<dyn API>,
    options: s3like::backuppb::S3,
    access_rec: Option<Arc<objectio::recording::AccessStats>>,
) -> s3like::Storage {
    let bucket_prefix = storeapi::NewBucketPrefix(&options.Bucket, &options.Prefix);
    let client = Client::from_dyn(svc, bucket_prefix.clone(), options.clone());
    s3like::NewStorage(client, bucket_prefix, options, access_rec)
}

/// Go 风格命名别名：去掉 Region 字符串前缀 `oss-`。
pub fn trimOSSRegionID(region: String) -> String {
    trim_oss_region_id(&region)
}

/// Go 风格命名别名：判断是否可使用内网 endpoint。
pub fn canUseInternalEndpoint(ecs_region_id: &str, bucket_region_id: &str) -> bool {
    can_use_internal_endpoint(ecs_region_id, bucket_region_id)
}
