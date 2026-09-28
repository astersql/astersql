// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// TiCDC（Change Data Capture，变更数据捕获）工具：枚举 etcd 中的 changefeed，
// 并按安全时间戳（safe-ts）筛选与 DDL/升级不兼容的订阅。
//
// 对应 Go `util/cdcutil`。Changefeed 表示一条 CDC 订阅任务；checkpoint-ts 是其
// 已确认同步到的时间戳。etcd 键路径存在旧版（无集群/命名空间）与新版两种布局。

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use path_clean::PathClean;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Prefix of CDC information in etcd.
/// etcd 中 CDC 信息键的公共前缀。
pub const CDCPrefix: &str = "/tidb/cdc/";
/// Path fragment identifying changefeed information keys.
/// 标识 changefeed info 键的路径片段。
pub const ChangefeedPath: &str = "/changefeed/info/";
/// Legacy CDC changefeed information prefix.
/// 旧版（≤ v6.1）changefeed info 前缀。
pub const CDCPrefixV61: &str = "/tidb/cdc/changefeed/info/";

/// 哨兵时间戳：表示“无效/已完成/可忽略”的 checkpoint。
const INVALID_TS: u64 = u64::MAX;

/// TiCDC 集群名合法性校验（字母数字与连字符分段）。
static CLUSTER_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").expect("the TiCDC cluster regex is valid")
});

/// etcd 键布局版本：旧版扁平路径 vs 带 cluster/namespace 的新版。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyVersion {
    Legacy,
    Namespaced,
}

/// 一次扫描得到的 changefeed 元数据（id、集群、命名空间与键版本）。
#[derive(Clone, Debug)]
struct Changefeed {
    id: String,
    cluster: String,
    namespace: String,
    key_version: KeyVersion,
}

impl Changefeed {
    /// 构造该 changefeed 的 info 键路径。
    fn info_key(&self) -> String {
        match self.key_version {
            KeyVersion::Legacy => join_path(&[CDCPrefix, "changefeed", "info", &self.id]),
            KeyVersion::Namespaced => join_path(&[
                CDCPrefix,
                &self.cluster,
                &self.namespace,
                "changefeed",
                "info",
                &self.id,
            ]),
        }
    }

    /// 构造该 changefeed 的 status 键路径（含 checkpoint-ts）。
    fn status_key(&self) -> String {
        match self.key_version {
            KeyVersion::Legacy => join_path(&[CDCPrefix, "changefeed", "status", &self.id]),
            KeyVersion::Namespaced => join_path(&[
                CDCPrefix,
                &self.cluster,
                &self.namespace,
                "changefeed",
                "status",
                &self.id,
            ]),
        }
    }
}

/// 用 `/` 拼接路径片段并做路径清理（去冗余分隔符等）。
fn join_path(parts: &[&str]) -> String {
    PathBuf::from(parts.join("/"))
        .clean()
        .to_string_lossy()
        .into_owned()
}

/// 在字节切片中按分隔符切成前后两段；未找到则返回 `None`。
fn cut<'a>(value: &'a [u8], separator: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    value
        .windows(separator.len())
        .position(|window| window == separator)
        .map(|position| (&value[..position], &value[position + separator.len()..]))
}

/// One etcd key/value item used by the CDC inspection logic.
/// CDC 巡检使用的单条 etcd 键值对。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KvPair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// Options needed from the etcd range API.
/// etcd range 查询选项：是否前缀扫描、是否只取键。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GetOptions {
    pub prefix: bool,
    pub keys_only: bool,
}

/// Minimal KV boundary shared by the real etcd adapter and focused tests.
/// 真实 etcd 适配器与测试共用的最小 KV 客户端抽象。
#[async_trait]
pub trait KvClient: Send + Sync {
    async fn get(&self, key: &str, options: GetOptions) -> Result<Vec<KvPair>, CdcError>;
}

/// Errors returned while enumerating and inspecting TiCDC changefeeds.
/// 枚举与检查 TiCDC changefeed 过程中的错误。
#[derive(Debug, thiserror::Error)]
pub enum CdcError {
    #[error("etcd request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid base64 in etcd response: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("invalid TiCDC JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to check changefeed {changefeed:?}: {source}")]
    CheckChangefeed {
        changefeed: String,
        #[source]
        source: Box<CdcError>,
    },
}

/// An etcd v3 JSON-gateway client suitable for TiDB deployments exposing the
/// standard `/v3/kv/range` endpoint.
/// 面向 TiDB 暴露的 etcd v3 JSON 网关（`/v3/kv/range`）的 HTTP 客户端。
#[derive(Clone, Debug)]
pub struct EtcdHttpClient {
    endpoint: String,
    client: reqwest::Client,
}

impl EtcdHttpClient {
    /// 以给定 endpoint 构造客户端；会去掉末尾 `/`。
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }
}

/// etcd range 请求体（键与 range_end 均为 base64）。
#[derive(Serialize)]
struct RangeRequest {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    range_end: Option<String>,
    keys_only: bool,
}

/// etcd range 响应中的 KV 列表。
#[derive(Deserialize)]
struct RangeResponse {
    #[serde(default)]
    kvs: Vec<GatewayKv>,
}

/// 网关返回的单条 KV（仍为 base64 字符串）。
#[derive(Deserialize)]
struct GatewayKv {
    key: String,
    #[serde(default)]
    value: String,
}

/// 计算前缀扫描的 `range_end`：对前缀做“字节进位”得到上界。
fn prefix_range_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    for index in (0..end.len()).rev() {
        if end[index] < u8::MAX {
            end[index] += 1;
            end.truncate(index + 1);
            return end;
        }
    }
    vec![0]
}

#[async_trait]
impl KvClient for EtcdHttpClient {
    async fn get(&self, key: &str, options: GetOptions) -> Result<Vec<KvPair>, CdcError> {
        // 按 etcd JSON API 约定：key/range_end 使用 base64 编码。
        let request = RangeRequest {
            key: BASE64.encode(key.as_bytes()),
            range_end: options
                .prefix
                .then(|| BASE64.encode(prefix_range_end(key.as_bytes()))),
            keys_only: options.keys_only,
        };
        let response: RangeResponse = self
            .client
            .post(format!("{}/v3/kv/range", self.endpoint))
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        response
            .kvs
            .into_iter()
            .map(|kv| {
                Ok(KvPair {
                    key: BASE64.decode(kv.key)?,
                    value: if options.keys_only {
                        Vec::new()
                    } else {
                        BASE64.decode(kv.value)?
                    },
                })
            })
            .collect()
    }
}

/// 基于任意 `KvClient` 的 CDC 巡检逻辑封装。
struct CheckCDCClient<'a, C> {
    cli: &'a C,
}

impl<C: KvClient> CheckCDCClient<'_, C> {
    /// 从前缀扫描结果解析出合法 changefeed 列表（忽略元数据与备份键）。
    async fn load_changefeeds(&self, out: &mut Vec<Changefeed>) -> Result<(), CdcError> {
        let response = self
            .cli
            .get(
                CDCPrefix,
                GetOptions {
                    prefix: true,
                    keys_only: true,
                },
            )
            .await?;

        for kv in response {
            if kv.key.len() < CDCPrefix.len() - 1 {
                continue;
            }
            // 去掉公共前缀后，按 `/changefeed/info/` 切出集群/命名空间与 id。
            let key = &kv.key[CDCPrefix.len() - 1..];
            let Some((cluster_and_namespace, changefeed_id)) = cut(key, ChangefeedPath.as_bytes())
            else {
                continue;
            };

            if cluster_and_namespace.is_empty() {
                // 旧版布局：无 cluster/namespace。
                out.push(Changefeed {
                    id: String::from_utf8_lossy(changefeed_id).into_owned(),
                    cluster: String::new(),
                    namespace: String::new(),
                    key_version: KeyVersion::Legacy,
                });
                continue;
            }

            let Some(without_leading_slash) = cluster_and_namespace.strip_prefix(b"/") else {
                continue;
            };
            let Some((cluster_id, namespace)) = cut(without_leading_slash, b"/") else {
                // This includes TiCDC's temporary __backup__ migration key.
                // 亦覆盖仅含 cluster id、缺少 namespace 的噪声键。
                continue;
            };
            let cluster = String::from_utf8_lossy(cluster_id);
            if !CLUSTER_NAME_RE.is_match(&cluster) {
                continue;
            }

            out.push(Changefeed {
                id: String::from_utf8_lossy(changefeed_id).into_owned(),
                cluster: cluster.into_owned(),
                namespace: String::from_utf8_lossy(namespace).into_owned(),
                key_version: KeyVersion::Namespaced,
            });
        }
        Ok(())
    }

    /// 从 status 键读取 checkpoint-ts；缺失或空值视为 0。
    async fn fetch_checkpoint_ts_from_status(&self, cf: &Changefeed) -> Result<u64, CdcError> {
        let response = self
            .cli
            .get(&cf.status_key(), GetOptions::default())
            .await?;
        let Some(value) = response.first().map(|kv| kv.value.as_slice()) else {
            return Ok(0);
        };
        if value.is_empty() {
            return Ok(0);
        }
        Ok(serde_json::from_slice::<ChangefeedStatusView>(value)?.checkpoint)
    }

    /// 计算有效 checkpoint：finished 返回哨兵；活跃状态取 status 与 start-ts 的较大值。
    async fn checkpoint_ts_for(&self, cf: &Changefeed) -> Result<u64, CdcError> {
        let response = self.cli.get(&cf.info_key(), GetOptions::default()).await?;
        let Some(value) = response.first().map(|kv| kv.value.as_slice()) else {
            // The changefeed was removed after the prefix scan.
            return Ok(INVALID_TS);
        };
        let info: ChangefeedInfoView = serde_json::from_slice(value)?;
        match info.state.as_str() {
            "finished" => Ok(INVALID_TS),
            "failed" | "running" | "warning" | "normal" | "stopped" | "error" => {
                let checkpoint = self.fetch_checkpoint_ts_from_status(cf).await?;
                // 有效进度取 checkpoint 与 start-ts 的最大值（与 Go 一致）。
                Ok(checkpoint.max(info.start))
            }
            _ => {
                log::warn!(
                    "ignoring invalid changefeed {:?} with state {:?}",
                    cf,
                    info.state
                );
                Ok(INVALID_TS)
            }
        }
    }

    /// 收集有效 checkpoint 小于 `safe_ts` 的不兼容 changefeed 集合。
    async fn get_incompatible(&self, safe_ts: u64) -> Result<CDCNameSet, CdcError> {
        let mut changefeeds = Vec::new();
        self.load_changefeeds(&mut changefeeds).await?;

        let mut names = CDCNameSet::default();
        for cf in changefeeds {
            let checkpoint =
                self.checkpoint_ts_for(&cf)
                    .await
                    .map_err(|source| CdcError::CheckChangefeed {
                        changefeed: format!("{cf:?}"),
                        source: Box::new(source),
                    })?;
            if checkpoint < safe_ts {
                log::info!(
                    "found incompatible changefeed {:?}: checkpoint-ts={}, safe-ts={}",
                    cf,
                    checkpoint,
                    safe_ts
                );
                names.save(cf);
            }
        }
        Ok(names)
    }
}

/// changefeed info JSON 的精简视图（state / start-ts）。
#[derive(Deserialize)]
struct ChangefeedInfoView {
    state: String,
    #[serde(rename = "start-ts", default)]
    start: u64,
}

/// changefeed status JSON 的精简视图（checkpoint-ts）。
#[derive(Deserialize)]
struct ChangefeedStatusView {
    #[serde(rename = "checkpoint-ts", default)]
    checkpoint: u64,
}

/// CDC changefeeds grouped by `cluster/namespace`.
/// 按 `cluster/namespace` 分组的 changefeed 名称集合。
#[derive(Debug, Default, Eq, PartialEq)]
pub struct CDCNameSet {
    changefeeds: HashMap<String, Vec<String>>,
}

impl CDCNameSet {
    /// 将单个 changefeed 归入对应命名空间桶；旧版命名空间记为 `<nil>`。
    fn save(&mut self, cf: Changefeed) {
        let namespace = match cf.key_version {
            KeyVersion::Legacy => "<nil>".to_owned(),
            KeyVersion::Namespaced => join_path(&[&cf.cluster, &cf.namespace]),
        };
        self.changefeeds.entry(namespace).or_default().push(cf.id);
    }

    /// An empty set means no matching changefeed exists.
    /// 空集合表示没有匹配的 changefeed。
    pub fn Empty(&self) -> bool {
        self.changefeeds.is_empty()
    }

    /// Converts the set to the user-facing form used by the Go implementation.
    /// 转为与 Go 一致的面向用户提示文案。
    pub fn MessageToUser(&self) -> String {
        let mut message = String::from("found CDC changefeed(s): ");
        for (cluster_id, changefeed_ids) in &self.changefeeds {
            let _ = write!(
                message,
                "cluster/namespace: {cluster_id} changefeed(s): [{}], ",
                changefeed_ids.join(" ")
            );
        }
        message
    }

    /// Flattens names using the same path form as Go's test export helper.
    /// 展平为与 Go 测试导出相同的路径形式，并按字典序排序。
    pub fn changefeed_names(&self) -> Vec<String> {
        let mut names = self
            .changefeeds
            .iter()
            .flat_map(|(namespace, changefeeds)| {
                changefeeds
                    .iter()
                    .map(|changefeed| join_path(&[namespace, changefeed]))
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

/// Gets all non-finished CDC changefeeds.
/// 获取所有未 finished 的 CDC changefeed（以哨兵 safe-ts 过滤）。
pub async fn GetRunningChangefeeds<C: KvClient>(cli: &C) -> Result<CDCNameSet, CdcError> {
    CheckCDCClient { cli }.get_incompatible(INVALID_TS).await
}

/// Gets CDC changefeeds whose effective checkpoint is older than `safe_ts`.
/// 获取有效 checkpoint 早于 `safe_ts` 的不兼容 changefeed。
pub async fn GetIncompatibleChangefeedsWithSafeTS<C: KvClient>(
    cli: &C,
    safe_ts: u64,
) -> Result<CDCNameSet, CdcError> {
    CheckCDCClient { cli }.get_incompatible(safe_ts).await
}
