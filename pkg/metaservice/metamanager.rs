// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Meta Service 元信息管理：解析 keyspace 配置中的 meta service 分组。
//
// Keyspace 是多租户隔离的键空间；每个 keyspace 可绑定独立的 meta service group
//（一组提供元数据服务的地址），也可回退到全局 PD（Placement Driver，集群调度中心）地址。
// 配置了独立 group 时，必须启用 keyspace 级 GC（垃圾回收），避免与统一 GC 冲突。

use super::{Context, PdClient, get_pd_addrs};
use std::collections::HashMap;

/// 未配置独立 group 时使用的全局分组 ID（字符串 `"0"`）。
pub const GLOBAL_GROUP_ID: &str = "0";
/// keyspace 配置中存放 meta service group ID 的键名。
pub const GROUP_ID_KEY: &str = "meta_service_group_id";
/// keyspace 配置中存放 meta service 地址列表（逗号分隔）的键名。
pub const GROUP_ADDRS_KEY: &str = "meta_service_group_addrs";
/// keyspace 配置中 GC 管理模式的键名。
pub const GC_MANAGEMENT_TYPE_KEY: &str = "gc_management_type";
/// keyspace 级 GC：按 keyspace 独立回收过期版本。
pub const GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL: &str = "keyspace_level";
/// 统一 GC：集群共享同一套 GC 调度。
pub const GC_MANAGEMENT_TYPE_UNIFIED: &str = "unified";

/// Meta Service 相关错误，覆盖配置校验、PD 客户端与 URL 解析失败等场景。
#[derive(Debug, thiserror::Error)]
pub enum MetaServiceError {
    #[error("it is unexpected for the keyspace to have a group ID but no group addresses")]
    GroupNotMatch,
    #[error("GetGroup: keyspace meta is nil")]
    NilKeyspaceMeta,
    #[error(
        "invalid meta service group id: it must contain at least one letter and only contain letters, digits, '-' or '_'"
    )]
    InvalidGroupId,
    #[error(
        "keyspace {name:?} configured meta service group {group_id:?}: meta service group requires keyspace-level GC"
    )]
    KeyspaceLevelGcRequired { name: String, group_id: String },
    #[error("PD client not found")]
    PdClientNotFound,
    #[error("no usable PD client URL found in PD members")]
    NoUsablePdUrl,
    #[error("{0}")]
    ServiceUrl(String),
    #[error("invalid URL prefix")]
    InvalidUrlPrefix,
    #[error("invalid URL format, expect host:port")]
    InvalidUrlFormat,
    #[error("parse client url from pd members {url:?}: {source}")]
    PdMemberUrl {
        url: String,
        source: Box<MetaServiceError>,
    },
    #[error("keyspace meta not found for keyspace {0:?}")]
    MissingKeyspaceMeta(String),
    #[error("etcd request failed: {0}")]
    Etcd(#[from] etcd_client::Error),
    #[error("PD request failed: {0}")]
    Pd(String),
    #[error("request context cancelled")]
    Cancelled,
}

/// Keyspace 元数据：名称与配置键值（含 group ID、地址、GC 类型等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyspaceMeta {
    /// keyspace 名称。
    pub name: String,
    /// 配置项字典，键名见本模块常量。
    pub config: HashMap<String, String>,
}

/// 对外返回的 meta service 信息：PD 地址列表与解析出的分组。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Info {
    /// 当前可用的 PD client 地址列表。
    pub pd_addrs: Vec<String>,
    /// 解析得到的 meta service 分组。
    pub group: Group,
}

/// Meta service 分组：分组 ID 与对应服务地址。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Group {
    /// 分组标识；全局回退时为 [`GLOBAL_GROUP_ID`]。
    pub group_id: String,
    /// 该分组的服务地址列表。
    pub addrs: Vec<String>,
}

/// 可查询 PD 地址的服务客户端抽象（裸地址与带 HTTP 前缀地址）。
pub trait ServiceClient {
    /// 返回不带协议前缀的 PD 地址（`host:port`）。
    fn get_pd_addrs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError>;
    /// Return service URLs while preserving Unix-family schemes.
    fn get_pd_service_urls(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError> {
        self.get_pd_http_addrs(ctx)
    }
    /// Compatibility alias retained for existing Rust callers.
    fn get_pd_http_addrs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError>;
}

/// 校验 group ID：须含至少一个字母，且仅允许字母、数字、`-`、`_`。
pub fn validate_group_id(group_id: &str) -> Result<(), MetaServiceError> {
    // 字符集与「至少一个字母」两条规则同时满足才合法。
    let only_allowed = group_id
        .bytes()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_');
    let has_letter = group_id.bytes().any(|ch| ch.is_ascii_alphabetic());
    if !only_allowed || !has_letter {
        return Err(MetaServiceError::InvalidGroupId);
    }
    Ok(())
}

/// 判断 keyspace 是否配置为 keyspace 级 GC。
fn is_keyspace_level_gc(keyspace_meta: &KeyspaceMeta) -> bool {
    keyspace_meta
        .config
        .get(GC_MANAGEMENT_TYPE_KEY)
        .is_some_and(|value| value == GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL)
}

/// 从 keyspace 配置解析 meta service 分组；无独立配置时回退到全局 PD 地址。
pub fn get_group(
    keyspace_meta: Option<&KeyspaceMeta>,
    pd_addrs: &[String],
) -> Result<Group, MetaServiceError> {
    let keyspace_meta = keyspace_meta.ok_or(MetaServiceError::NilKeyspaceMeta)?;
    // 配置了 GROUP_ID_KEY 则走独立分组路径，并强制 keyspace 级 GC。
    if let Some(group_id) = keyspace_meta.config.get(GROUP_ID_KEY) {
        validate_group_id(group_id)?;
        if !is_keyspace_level_gc(keyspace_meta) {
            return Err(MetaServiceError::KeyspaceLevelGcRequired {
                name: keyspace_meta.name.clone(),
                group_id: group_id.clone(),
            });
        }
        // 地址为逗号分隔；trim 后过滤空段，空列表视为配置不完整。
        let addrs = keyspace_meta
            .config
            .get(GROUP_ADDRS_KEY)
            .ok_or(MetaServiceError::GroupNotMatch)?
            .split(',')
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if addrs.is_empty() {
            return Err(MetaServiceError::GroupNotMatch);
        }
        let group = Group {
            group_id: group_id.clone(),
            addrs,
        };
        log::info!("get keyspace meta service group info: {group:?}");
        return Ok(group);
    }

    // 未配置独立 group：使用全局 ID 与传入的 PD 地址。
    let group = Group {
        group_id: GLOBAL_GROUP_ID.to_owned(),
        addrs: pd_addrs.to_vec(),
    };
    log::info!("get default keyspace meta service group info: {group:?}");
    Ok(group)
}

/// 组装 [`Info`]：有 keyspace 时走 [`get_group`]，否则直接使用全局分组。
pub fn get_info(
    keyspace_meta: Option<&KeyspaceMeta>,
    pd_addrs: &[String],
) -> Result<Info, MetaServiceError> {
    let group = match keyspace_meta {
        Some(keyspace_meta) => get_group(Some(keyspace_meta), pd_addrs)?,
        None => Group {
            group_id: GLOBAL_GROUP_ID.to_owned(),
            addrs: pd_addrs.to_vec(),
        },
    };
    let info = Info {
        pd_addrs: pd_addrs.to_vec(),
        group,
    };
    log::info!("return meta service group info: {info:?}");
    Ok(info)
}

impl Info {
    /// 返回分组内服务地址切片。
    pub fn group_addrs(&self) -> &[String] {
        &self.group.addrs
    }

    /// Go 命名风格别名，语义同 [`Self::group_addrs`]。
    #[allow(non_snake_case)]
    pub fn GroupAddrs(&self) -> &[String] {
        self.group_addrs()
    }
}

/// 先从 PD 客户端拉取地址，再结合 keyspace 元数据组装 [`Info`]。
pub fn fetch_info(
    ctx: &Context,
    pd_client: &dyn PdClient,
    keyspace_meta: Option<&KeyspaceMeta>,
) -> Result<Info, MetaServiceError> {
    let pd_addrs = get_pd_addrs(ctx, pd_client, false)?;
    get_info(keyspace_meta, &pd_addrs)
}

/// 一次性返回 [`Info`] 与分组地址副本，便于调用方同时使用两者。
pub fn get_info_and_group_addrs(
    ctx: &Context,
    pd_client: &dyn PdClient,
    keyspace_meta: Option<&KeyspaceMeta>,
) -> Result<(Info, Vec<String>), MetaServiceError> {
    let info = fetch_info(ctx, pd_client, keyspace_meta)?;
    let group_addrs = info.group_addrs().to_vec();
    Ok((info, group_addrs))
}

/// Go 风格导出常量，等同 [`GLOBAL_GROUP_ID`]。
#[allow(non_upper_case_globals)]
pub const GlobalGroupID: &str = GLOBAL_GROUP_ID;
/// Go 风格导出常量，等同 [`GROUP_ID_KEY`]。
#[allow(non_upper_case_globals)]
pub const GroupIDKey: &str = GROUP_ID_KEY;
/// Go 风格导出常量，等同 [`GROUP_ADDRS_KEY`]。
#[allow(non_upper_case_globals)]
pub const GroupAddrsKey: &str = GROUP_ADDRS_KEY;

/// Go 风格导出函数，转发至 [`get_group`]。
#[allow(non_snake_case)]
pub fn GetGroup(
    keyspace_meta: Option<&KeyspaceMeta>,
    pd_addrs: &[String],
) -> Result<Group, MetaServiceError> {
    get_group(keyspace_meta, pd_addrs)
}

/// Go 风格导出函数，转发至 [`get_info`]。
#[allow(non_snake_case)]
pub fn GetInfo(
    keyspace_meta: Option<&KeyspaceMeta>,
    pd_addrs: &[String],
) -> Result<Info, MetaServiceError> {
    get_info(keyspace_meta, pd_addrs)
}

/// Go 风格导出函数，转发至 [`fetch_info`]。
#[allow(non_snake_case)]
pub fn FetchInfo(
    ctx: &Context,
    pd_client: &dyn PdClient,
    keyspace_meta: Option<&KeyspaceMeta>,
) -> Result<Info, MetaServiceError> {
    fetch_info(ctx, pd_client, keyspace_meta)
}

/// Go 风格导出函数，转发至 [`get_info_and_group_addrs`]。
#[allow(non_snake_case)]
pub fn GetInfoAndGroupAddrs(
    ctx: &Context,
    pd_client: &dyn PdClient,
    keyspace_meta: Option<&KeyspaceMeta>,
) -> Result<(Info, Vec<String>), MetaServiceError> {
    get_info_and_group_addrs(ctx, pd_client, keyspace_meta)
}

#[cfg(test)]
#[path = "metamanager_test.rs"]
mod metamanager_test;
