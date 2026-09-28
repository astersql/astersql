// Copyright 2026 AsterSQL.

// infosync 公共类型与 PD / Resource Manager 客户端抽象。
//
// 定义 Store / Region 相关数据结构、异构配置值、Region Label 补丁，
// 以及 `PdHttpClient`（PD HTTP API）与 `ResourceManagerClient`（资源组管理）
// 两个 trait。默认方法大多返回 “unsupported”，由具体实现覆盖。

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, label, placement};

/// 更新 Keyspace（键空间）配置的请求参数：目标配置与前置条件。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateKeyspaceConfigParams {
    /// 待写入的配置项；值为 None 表示删除该键。
    #[serde(rename = "config")]
    pub Config: HashMap<String, Option<String>>,
    /// 乐观并发前置条件：键对应的期望现值。
    #[serde(rename = "preconditions", skip_serializing_if = "HashMap::is_empty")]
    pub Preconditions: HashMap<String, Option<String>>,
}

/// PD / TiKV Store（存储节点）元信息。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreMeta {
    /// Store ID。
    #[serde(rename = "id")]
    pub ID: i64,
    /// 节点地址（如 host:port）。
    #[serde(rename = "address")]
    pub Address: String,
    /// 状态服务地址。
    #[serde(rename = "status_address")]
    pub StatusAddress: String,
    /// 状态名（如 Up、Disconnected、Offline）。
    #[serde(rename = "state_name")]
    pub StateName: String,
    /// 节点标签（如 engine=tiflash）。
    #[serde(rename = "labels")]
    pub Labels: HashMap<String, String>,
}

/// 单个 Store 的包装结构，对齐 PD HTTP 返回形状。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreInfo {
    /// Store 元数据。
    #[serde(rename = "store")]
    pub Store: StoreMeta,
}

/// 一组 Store 的聚合信息。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoresInfo {
    /// Store 数量。
    #[serde(rename = "count")]
    pub Count: usize,
    /// Store 列表。
    #[serde(rename = "stores")]
    pub Stores: Vec<StoreInfo>,
}

/// 某 key range 上 Region（TiKV 数据分片）在各 Store 的 peer 分布。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionDistributions {
    /// Region 总数。
    #[serde(rename = "count")]
    pub RegionCount: usize,
    /// Store ID → 该 Store 上 peer 数量。
    #[serde(rename = "store_peer_count")]
    pub StorePeerCount: HashMap<i64, usize>,
}

/// 半开区间 [start_key, end_key) 的字节键范围。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRange {
    /// 起始键（含）。
    pub start_key: Vec<u8>,
    /// 结束键（不含）。
    pub end_key: Vec<u8>,
}

/// 异构 JSON 配置值，对应 PD schedule / scheduler 接口的灵活字段类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<ConfigValue>),
    Object(HashMap<String, ConfigValue>),
    Null,
}

/// PD HTTP 客户端抽象：放置规则、Region Label、调度配置等 API。
///
/// 默认实现一律返回 unsupported，便于只覆盖测试所需方法。
pub trait PdHttpClient: Send + Sync {
    /// 更新指定 Keyspace 的配置（带前置条件）。
    fn update_keyspace_config(
        &self,
        _name: &str,
        _params: &UpdateKeyspaceConfigParams,
    ) -> Result<()> {
        Err(Error::External(
            "UpdateKeyspaceConfig is unsupported".into(),
        ))
    }
    /// 按 group 名获取放置规则 Bundle。
    fn get_placement_rule_bundle(&self, _name: &str) -> Result<placement::Bundle> {
        Err(Error::External(
            "GetPlacementRuleBundleByGroup is unsupported".into(),
        ))
    }
    /// 获取全部放置规则 Bundle。
    fn get_all_placement_rule_bundles(&self) -> Result<Vec<placement::Bundle>> {
        Err(Error::External(
            "GetAllPlacementRuleBundles is unsupported".into(),
        ))
    }
    /// 设置放置规则 Bundle；`partial` 为 true 时部分更新。
    fn set_placement_rule_bundles(
        &self,
        _bundles: &[placement::Bundle],
        _partial: bool,
    ) -> Result<()> {
        Err(Error::External(
            "SetPlacementRuleBundles is unsupported".into(),
        ))
    }
    /// 写入单条 Region Label 规则。
    fn set_region_label_rule(&self, _rule: &label::Rule) -> Result<()> {
        Err(Error::External("SetRegionLabelRule is unsupported".into()))
    }
    /// 批量增删 Region Label 规则。
    fn patch_region_label_rules(&self, _patch: &LabelRulePatch) -> Result<()> {
        Err(Error::External(
            "PatchRegionLabelRules is unsupported".into(),
        ))
    }
    /// 列出全部 Region Label 规则。
    fn get_all_region_label_rules(&self) -> Result<Vec<label::Rule>> {
        Err(Error::External(
            "GetAllRegionLabelRules is unsupported".into(),
        ))
    }
    /// 按 ID 列表查询 Region Label 规则。
    fn get_region_label_rules_by_ids(&self, _ids: &[String]) -> Result<Vec<label::Rule>> {
        Err(Error::External(
            "GetRegionLabelRulesByIDs is unsupported".into(),
        ))
    }
    /// 读取 PD 全局调度配置。
    fn get_schedule_config(&self) -> Result<HashMap<String, ConfigValue>> {
        Err(Error::External("GetScheduleConfig is unsupported".into()))
    }
    /// 写入 PD 全局调度配置。
    fn set_schedule_config(&self, _config: &HashMap<String, ConfigValue>) -> Result<()> {
        Err(Error::External("SetScheduleConfig is unsupported".into()))
    }
    /// 查询指定 key range 内 Region 的复制状态。
    fn get_regions_replicated_state(&self, _range: &KeyRange) -> Result<String> {
        Err(Error::External(
            "GetRegionsReplicatedStateByKeyRange is unsupported".into(),
        ))
    }
    /// 查询指定 key range 在某 engine 上的 Region 分布。
    fn get_region_distribution(
        &self,
        _range: &KeyRange,
        _engine: &str,
    ) -> Result<RegionDistributions> {
        Err(Error::External(
            "GetRegionDistributionByKeyRange is unsupported".into(),
        ))
    }
    /// 读取指定调度器配置。
    fn get_scheduler_config(&self, _name: &str) -> Result<ConfigValue> {
        Err(Error::External("GetSchedulerConfig is unsupported".into()))
    }
    /// 用输入参数创建调度器。
    fn create_scheduler(&self, _name: &str, _input: &HashMap<String, ConfigValue>) -> Result<()> {
        Err(Error::External(
            "CreateSchedulerWithInput is unsupported".into(),
        ))
    }
    /// 取消调度器上的指定 job。
    fn cancel_scheduler_job(&self, _name: &str, _job_id: u64) -> Result<()> {
        Err(Error::External("CancelSchedulerJob is unsupported".into()))
    }
}

/// Region Label 规则的批量补丁：先删后设。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelRulePatch {
    /// 待删除的规则 ID 列表。
    #[serde(rename = "deletes")]
    pub DeleteRules: Vec<String>,
    /// 待写入 / 覆盖的规则列表。
    #[serde(rename = "sets")]
    pub SetRules: Vec<label::Rule>,
}

/// 令牌桶限速参数（RU：Request Unit，请求资源计量单位）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenLimitSettings {
    /// 令牌填充速率。
    pub FillRate: i64,
    /// 突发上限；-1 表示不限制突发。
    pub BurstLimit: i64,
}
/// 资源组定义：名称、RU 限速与优先级。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceGroup {
    /// 资源组名称。
    pub Name: String,
    /// RU 令牌桶设置。
    pub RUSettings: TokenLimitSettings,
    /// 调度优先级。
    pub Priority: u32,
}
/// 资源组变更事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    /// 新增或更新。
    Put,
    /// 删除。
    Delete,
}
/// 资源组变更事件：类型 + 受影响的资源组快照。
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceGroupEvent {
    /// 事件类型。
    pub event_type: EventType,
    /// 资源组内容。
    pub group: ResourceGroup,
}

/// 资源管理客户端：资源组 CRUD、watch 与令牌桶相关接口。
pub trait ResourceManagerClient: Send + Sync {
    /// 列出全部资源组。
    fn list_resource_groups(&self) -> Vec<ResourceGroup>;
    /// 按名称获取资源组。
    fn get_resource_group(&self, name: &str) -> Result<ResourceGroup>;
    /// 新增资源组。
    fn add_resource_group(&self, group: ResourceGroup) -> Result<String>;
    /// 修改资源组。
    fn modify_resource_group(&self, group: ResourceGroup) -> Result<String>;
    /// 删除资源组。
    fn delete_resource_group(&self, name: &str) -> Result<String>;
    /// 按 etcd key 前缀订阅资源组变更事件。
    fn watch(&self, key: &[u8]) -> Option<Receiver<Vec<ResourceGroupEvent>>>;
    /// 申请令牌桶（占位，默认空列表）。
    fn AcquireTokenBuckets(&self) -> Vec<()> {
        Vec::new()
    }
    /// 从指定 revision 起 watch 资源组列表变更（占位）。
    fn WatchResourceGroup(&self, _revision: i64) -> Option<Receiver<Vec<ResourceGroup>>> {
        None
    }
    /// 加载资源组列表及对应 revision（占位）。
    fn LoadResourceGroups(&self) -> (Vec<ResourceGroup>, i64) {
        (Vec::new(), 0)
    }
    /// Go 风格大写 Watch，默认转调 `watch`。
    fn Watch(&self, key: &[u8]) -> Option<Receiver<Vec<ResourceGroupEvent>>> {
        self.watch(key)
    }
}

/// 构造某 keyspace 下资源组设置在 etcd 中的路径前缀字节。
pub fn group_settings_path_prefix(keyspace_id: u32) -> Vec<u8> {
    if keyspace_id == u32::MAX {
        b"resource_group/settings".to_vec()
    } else {
        format!("resource_group/keyspace/settings/{keyspace_id}").into_bytes()
    }
}
