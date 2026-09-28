// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// TiFlash 副本与放置规则（placement rule）管理。
//
// TiFlash 是列存引擎；本模块管理其 Learner 副本在 PD 上的放置规则、
// 同步进度缓存，以及加速调度。包含：
// - `TiFlashReplicaManager` trait 与内存实现 `TiFlashReplicaManagerCtx`；
// - 规则编解码、进度计算、建规则辅助函数；
// - `MockTiFlash` / `mockTiFlashReplicaManagerCtx` 供单测模拟 PD/HTTP。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use crate::{Codec, Error, Result, StoreInfo, StoreMeta, StoresInfo, placement};

/// TiFlash 使用的 PD 放置规则类型别名。
pub type TiFlashRule = placement::pd::Rule;

/// 在 keyspace-aware 模式下，为规则的起止键与 ID 加上 keyspace 前缀。
pub fn encodeRule(codec: Codec, rule: &mut TiFlashRule) {
    if codec.keyspace_aware_rules {
        let prefix = codec.keyspace_id.unwrap_or_default().to_be_bytes();
        rule.StartKey.splice(0..0, prefix);
        rule.EndKey.splice(0..0, prefix);
        rule.ID = encodeRuleID(codec, rule.ID.clone());
    }
}
/// 将规则 ID 编码为 `keyspace-{id}-{ruleID}`（仅 keyspace-aware 时）。
pub fn encodeRuleID(codec: Codec, ruleID: String) -> String {
    if codec.keyspace_aware_rules {
        format!(
            "keyspace-{}-{}",
            codec.keyspace_id.unwrap_or_default(),
            ruleID
        )
    } else {
        ruleID
    }
}

/// TiFlash 副本管理接口：放置规则 CRUD、进度缓存、Store 统计与 Schema 同步。
pub trait TiFlashReplicaManager: Send + Sync {
    /// 确保 TiFlash 规则组配置已写入 PD。
    fn SetTiFlashGroupConfig(&self) -> Result<()>;
    /// 设置单条放置规则；`Count==0` 时通常表示删除。
    fn SetPlacementRule(&self, rule: &TiFlashRule) -> Result<()>;
    /// 批量设置放置规则。
    fn SetPlacementRuleBatch(&self, rules: &[TiFlashRule]) -> Result<()>;
    /// 按表 ID 获取对应放置规则。
    fn GetPlacementRule(&self, table_id: i64) -> Result<TiFlashRule>;
    /// 删除指定 group 下的规则。
    fn DeletePlacementRule(&self, group: &str, rule_id: &str) -> Result<()>;
    /// 列出某 group 下全部规则。
    fn GetGroupRules(&self, group: &str) -> Result<Vec<TiFlashRule>>;
    /// 对表批量触发加速调度（催促 Region 尽快落到 TiFlash）。
    fn PostAccelerateScheduleBatch(&self, table_ids: &[i64]) -> Result<()>;
    /// 从 PD 查询表对应的 Region 数量。
    fn GetRegionCountFromPD(&self, table_id: i64) -> Result<usize>;
    /// 获取集群 Store 状态列表。
    fn GetStoresStat(&self) -> Result<StoresInfo>;
    /// 计算表在 TiFlash 上的同步进度，返回 (全副本进度, 至少一副本进度)。
    fn CalculateTiFlashProgress(
        &self,
        table_id: i64,
        replica_count: u64,
        stores: &HashMap<i64, StoreInfo>,
    ) -> Result<(f64, f64)>;
    /// 更新本地进度缓存。
    fn UpdateTiFlashProgressCache(&self, table_id: i64, progress: f64);
    /// 读取本地进度缓存。
    fn GetTiFlashProgressFromCache(&self, table_id: i64) -> Option<f64>;
    /// 删除单表进度缓存。
    fn DeleteTiFlashProgressFromCache(&self, table_id: i64);
    /// 清空全部进度缓存。
    fn CleanTiFlashProgressCache(&self);
    /// 将表 Schema 同步到 TiFlash Store（默认空实现）。
    fn SyncTiFlashTableSchema(&self, _table_id: i64, _stores: &[StoreInfo]) -> Result<()> {
        Ok(())
    }
    /// 关闭管理器，释放资源（默认空实现）。
    fn Close(&self) {}
}

/// 内存版 TiFlash 副本管理上下文：规则表、进度缓存与 Store 列表。
#[derive(Default)]
pub struct TiFlashReplicaManagerCtx {
    /// 规则 ID → 放置规则。
    rules: RwLock<HashMap<String, TiFlashRule>>,
    /// 表 ID → 同步进度缓存。
    tiflashProgressCache: RwLock<HashMap<i64, f64>>,
    /// 已知 Store 列表。
    stores: RwLock<Vec<StoreInfo>>,
}

impl TiFlashReplicaManager for TiFlashReplicaManagerCtx {
    fn SetTiFlashGroupConfig(&self) -> Result<()> {
        Ok(())
    }
    /// Count 为 0 则删除规则，否则插入 / 覆盖。
    fn SetPlacementRule(&self, rule: &TiFlashRule) -> Result<()> {
        if rule.Count == 0 {
            self.rules.write().unwrap().remove(&rule.ID);
        } else {
            self.rules
                .write()
                .unwrap()
                .insert(rule.ID.clone(), rule.clone());
        }
        Ok(())
    }
    fn SetPlacementRuleBatch(&self, rules: &[TiFlashRule]) -> Result<()> {
        for rule in rules {
            self.SetPlacementRule(rule)?;
        }
        Ok(())
    }
    fn GetPlacementRule(&self, table_id: i64) -> Result<TiFlashRule> {
        self.rules
            .read()
            .unwrap()
            .get(&MakeRuleID(table_id))
            .cloned()
            .ok_or_else(|| Error::External("placement rule not found".into()))
    }
    fn DeletePlacementRule(&self, _group: &str, rule_id: &str) -> Result<()> {
        self.rules.write().unwrap().remove(rule_id);
        Ok(())
    }
    fn GetGroupRules(&self, group: &str) -> Result<Vec<TiFlashRule>> {
        Ok(self
            .rules
            .read()
            .unwrap()
            .values()
            .filter(|r| r.GroupID == group)
            .cloned()
            .collect())
    }
    fn PostAccelerateScheduleBatch(&self, _table_ids: &[i64]) -> Result<()> {
        Ok(())
    }
    fn GetRegionCountFromPD(&self, _table_id: i64) -> Result<usize> {
        Ok(1)
    }
    fn GetStoresStat(&self) -> Result<StoresInfo> {
        let stores = self.stores.read().unwrap().clone();
        Ok(StoresInfo {
            Count: stores.len(),
            Stores: stores,
        })
    }
    fn CalculateTiFlashProgress(
        &self,
        table_id: i64,
        replica_count: u64,
        stores: &HashMap<i64, StoreInfo>,
    ) -> Result<(f64, f64)> {
        calculateTiFlashProgress(table_id, replica_count, stores)
    }
    fn UpdateTiFlashProgressCache(&self, table_id: i64, progress: f64) {
        self.tiflashProgressCache
            .write()
            .unwrap()
            .insert(table_id, progress);
    }
    fn GetTiFlashProgressFromCache(&self, table_id: i64) -> Option<f64> {
        self.tiflashProgressCache
            .read()
            .unwrap()
            .get(&table_id)
            .copied()
    }
    fn DeleteTiFlashProgressFromCache(&self, table_id: i64) {
        self.tiflashProgressCache.write().unwrap().remove(&table_id);
    }
    fn CleanTiFlashProgressCache(&self) {
        self.tiflashProgressCache.write().unwrap().clear();
    }
}

impl TiFlashReplicaManagerCtx {
    /// 直接设置单条规则（测试 / 内部入口）。
    pub fn doSetPlacementRule(&self, rule: &TiFlashRule) -> Result<()> {
        self.SetPlacementRule(rule)
    }
    /// 直接批量设置规则（测试 / 内部入口）。
    pub fn doSetPlacementRuleBatch(&self, rules: &[TiFlashRule]) -> Result<()> {
        self.SetPlacementRuleBatch(rules)
    }
}

/// 统计无滞后的 TiFlash peer 数与覆盖的 Region 数。
///
/// 从 Store Label `table-{id}-regions`（逗号分隔 Region ID）收集；
/// 仅统计状态为 Up / Disconnected 的 Store。
pub fn getTiFlashPeerWithoutLagCount(
    table_id: i64,
    stores: &HashMap<i64, StoreInfo>,
) -> Result<(usize, usize)> {
    let mut regions = HashSet::new();
    let mut peers = 0;
    for store in stores.values() {
        if matches!(store.Store.StateName.as_str(), "Up" | "Disconnected") {
            // The collector boundary supplies comma-separated region IDs in the testable
            // `regions` label. Production collectors can adapt HTTP output to this shape.
            if let Some(value) = store.Store.Labels.get(&format!("table-{table_id}-regions")) {
                for region in value.split(',').filter(|r| !r.is_empty()) {
                    peers += 1;
                    regions.insert(region.to_owned());
                }
            }
        }
    }
    Ok((peers, regions.len()))
}

/// 计算 TiFlash 同步进度。
///
/// 返回 `(full, one)`：`full` 为相对期望副本数的完成度，`one` 为至少一副本覆盖度。
/// `replica_count==0` 时视为已完成；无 Region 时返回 (0,0)。
pub fn calculateTiFlashProgress(
    table_id: i64,
    replica_count: u64,
    stores: &HashMap<i64, StoreInfo>,
) -> Result<(f64, f64)> {
    if replica_count == 0 {
        return Ok((1.0, 1.0));
    }
    let (peer_count, region_count) = getTiFlashPeerWithoutLagCount(table_id, stores)?;
    if region_count == 0 {
        return Ok((0.0, 0.0));
    }
    let one = (peer_count as f64 / region_count as f64).min(1.0);
    let full = (peer_count as f64 / (region_count as f64 * replica_count as f64)).min(1.0);
    Ok((full, one))
}

/// 构造表 ID 对应的键前缀 `t` + memcomparable big-endian table_id。
fn table_prefix(id: i64) -> Vec<u8> {
    let mut value = b"t".to_vec();
    value.extend_from_slice(&((id as u64) ^ (1_u64 << 63)).to_be_bytes());
    value
}

/// 构造表记录前缀 `t[table_id]_r`，对齐 `tablecodec.GenTableRecordPrefix`。
fn table_record_prefix(id: i64) -> Vec<u8> {
    let mut value = table_prefix(id);
    value.extend_from_slice(b"_r");
    value
}

/// 创建 TiFlash 基础放置规则模板（Learner、engine=tiflash、默认 Count=2）。
pub fn makeBaseRule() -> TiFlashRule {
    TiFlashRule {
        GroupID: placement::TiFlashRuleGroupID.into(),
        Index: placement::RuleIndexTiFlash,
        Role: placement::pd::Learner,
        Count: 2,
        LabelConstraints: vec![placement::pd::LabelConstraint {
            Key: "engine".into(),
            Op: placement::pd::In,
            Values: vec!["tiflash".into()],
        }],
        ..Default::default()
    }
}

/// 为表创建完整放置规则：键范围为 [table_id, table_id+1)，并设置副本数与 location labels。
pub fn MakeNewRule(id: i64, count: u64, locationLabels: Vec<String>) -> TiFlashRule {
    let mut rule = makeBaseRule();
    rule.ID = MakeRuleID(id);
    rule.StartKey = table_record_prefix(id);
    rule.EndKey = table_prefix(id + 1);
    rule.Count = count as i32;
    rule.LocationLabels = locationLabels;
    rule
}

/// 表放置规则 ID 格式：`table-{id}-r`。
pub fn MakeRuleID(id: i64) -> String {
    format!("table-{id}-r")
}

/// Mock 中单表的同步状态：已有 Region 列表与是否已加速调度。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct mockTiFlashTableInfo {
    /// 已同步的 Region ID 列表。
    pub Regions: Vec<i32>,
    /// 是否已触发加速调度。
    pub Accel: bool,
}

/// 模拟 TiFlash / PD 侧行为的测试替身。
#[derive(Default)]
pub struct MockTiFlash {
    /// 当前规则组 Index。
    groupIndex: Mutex<i32>,
    /// 表 ID → 同步状态。
    pub SyncStatus: RwLock<HashMap<i64, mockTiFlashTableInfo>>,
    /// Store ID → Store 元信息。
    pub StoreInfo: RwLock<HashMap<u64, StoreMeta>>,
    /// 全局 TiFlash 放置规则表。
    pub GlobalTiFlashPlacementRules: RwLock<HashMap<String, TiFlashRule>>,
    /// 是否启用 PD 侧写入（关闭时 SetPlacementRule 直接成功但不落库）。
    pub PdEnabled: RwLock<bool>,
    /// 是否模拟网络错误。
    pub NetworkError: RwLock<bool>,
}

/// 创建默认启用 PD 的 MockTiFlash。
pub fn NewMockTiFlash() -> Arc<MockTiFlash> {
    Arc::new(MockTiFlash {
        PdEnabled: RwLock::new(true),
        ..Default::default()
    })
}

impl MockTiFlash {
    /// 启动 Mock HTTP 服务占位（当前为空实现）。
    fn setUpMockTiFlashHTTPServer(&self) {}
    /// 处理设置放置规则：更新全局规则表，并解析表 ID 写入 SyncStatus。
    pub fn HandleSetPlacementRule(&self, rule: &TiFlashRule) -> Result<()> {
        *self.groupIndex.lock().unwrap() = placement::RuleIndexTiFlash;
        if !*self.PdEnabled.read().unwrap() {
            return Ok(());
        }
        if rule.Count == 0 {
            self.GlobalTiFlashPlacementRules
                .write()
                .unwrap()
                .remove(&rule.ID);
        } else {
            self.GlobalTiFlashPlacementRules
                .write()
                .unwrap()
                .insert(rule.ID.clone(), rule.clone());
        }
        // 从 `table-{id}-r` 解析表 ID，失败则报错。
        let id = rule
            .ID
            .strip_prefix("table-")
            .and_then(|v| v.strip_suffix("-r"))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Error::External("Can't parse rule".into()))?;
        self.SyncStatus
            .write()
            .unwrap()
            .entry(id)
            .and_modify(|s| s.Regions = vec![1])
            .or_insert(mockTiFlashTableInfo {
                Regions: vec![1],
                Accel: false,
            });
        Ok(())
    }
    /// 批量处理设置放置规则。
    pub fn HandleSetPlacementRuleBatch(&self, rules: &[TiFlashRule]) -> Result<()> {
        for rule in rules {
            self.HandleSetPlacementRule(rule)?;
        }
        Ok(())
    }
    /// 重置表同步状态：available 为 true 时插入默认 Region，否则删除。
    pub fn ResetSyncStatus(&self, table_id: i64, available: bool) {
        if available {
            self.SyncStatus
                .write()
                .unwrap()
                .entry(table_id)
                .and_modify(|status| status.Regions = vec![1])
                .or_insert(mockTiFlashTableInfo {
                    Regions: vec![1],
                    Accel: false,
                });
        } else {
            self.SyncStatus.write().unwrap().remove(&table_id);
        }
    }
    /// 删除全局规则表中的指定规则。
    pub fn HandleDeletePlacementRule(&self, _group: &str, rule_id: &str) {
        self.GlobalTiFlashPlacementRules
            .write()
            .unwrap()
            .remove(rule_id);
    }
    /// 返回全部规则；Go mock 为兼容测试替身而忽略 group 参数。
    pub fn HandleGetGroupRules(&self, _group: &str) -> Vec<TiFlashRule> {
        self.GlobalTiFlashPlacementRules
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }
    /// 标记表已加速调度（Accel=true）。
    pub fn HandlePostAccelerateSchedule(&self, table_id: i64) {
        self.SyncStatus
            .write()
            .unwrap()
            .entry(table_id)
            .and_modify(|s| s.Accel = true)
            .or_insert(mockTiFlashTableInfo {
                Regions: Vec::new(),
                Accel: true,
            });
    }
    /// Mock：固定返回 1 个 Region 记录统计。
    pub fn HandleGetPDRegionRecordStats(&self, _table_id: i64) -> usize {
        1
    }
    /// 向 Mock 注册一个 engine=tiflash 且状态 Up 的 Store。
    pub fn AddStore(&self, store_id: u64, address: String) {
        self.StoreInfo.write().unwrap().insert(
            store_id,
            StoreMeta {
                ID: store_id as i64,
                Address: address,
                StateName: "Up".into(),
                Labels: HashMap::from([("engine".into(), "tiflash".into())]),
                ..Default::default()
            },
        );
    }
    /// 返回已注册 Store；若为空则提供默认 127.0.0.1:3930 的 TiFlash Store。
    pub fn HandleGetStoresStat(&self) -> StoresInfo {
        let stores: Vec<_> = self
            .StoreInfo
            .read()
            .unwrap()
            .values()
            .cloned()
            .map(|Store| StoreInfo { Store })
            .collect();
        if stores.is_empty() {
            let Store = StoreMeta {
                ID: 1,
                Address: "127.0.0.1:3930".into(),
                StateName: "Up".into(),
                Labels: HashMap::from([("engine".into(), "tiflash".into())]),
                ..Default::default()
            };
            StoresInfo {
                Count: 1,
                Stores: vec![StoreInfo { Store }],
            }
        } else {
            StoresInfo {
                Count: stores.len(),
                Stores: stores,
            }
        }
    }
    /// 设置规则组 Index。
    pub fn SetRuleGroupIndex(&self, index: i32) {
        *self.groupIndex.lock().unwrap() = index;
    }
    /// 读取规则组 Index。
    pub fn GetRuleGroupIndex(&self) -> i32 {
        *self.groupIndex.lock().unwrap()
    }
    /// 检查全局规则表中是否存在与入参完全相等的规则。
    pub fn CheckPlacementRule(&self, rule: &TiFlashRule) -> bool {
        self.GlobalTiFlashPlacementRules
            .read()
            .unwrap()
            .values()
            .any(|stored| {
                isRuleMatch(
                    rule.clone(),
                    stored.StartKey.clone(),
                    stored.EndKey.clone(),
                    stored.Count,
                    stored.LocationLabels.clone(),
                )
            })
    }
    /// 按名称获取放置规则。
    pub fn GetPlacementRule(&self, name: &str) -> Option<TiFlashRule> {
        self.GlobalTiFlashPlacementRules
            .read()
            .unwrap()
            .get(name)
            .cloned()
    }
    /// 清空全部放置规则。
    pub fn CleanPlacementRules(&self) {
        self.GlobalTiFlashPlacementRules.write().unwrap().clear();
    }
    /// 当前放置规则数量。
    pub fn PlacementRulesLen(&self) -> usize {
        self.GlobalTiFlashPlacementRules.read().unwrap().len()
    }
    /// 获取表同步状态快照。
    pub fn GetTableSyncStatus(&self, table_id: i64) -> Option<mockTiFlashTableInfo> {
        self.SyncStatus.read().unwrap().get(&table_id).cloned()
    }
    /// 开关 PD 写入路径。
    pub fn PdSwitch(&self, enabled: bool) {
        *self.PdEnabled.write().unwrap() = enabled;
    }
    /// 设置是否模拟网络错误。
    pub fn SetNetworkError(&self, value: bool) {
        *self.NetworkError.write().unwrap() = value;
    }
}

/// 判断规则的键范围、副本数与 location labels 是否与期望一致。
pub fn isRuleMatch(
    rule: TiFlashRule,
    startKey: Vec<u8>,
    endKey: Vec<u8>,
    count: i32,
    labels: Vec<String>,
) -> bool {
    rule.StartKey == startKey
        && rule.EndKey == endKey
        && rule.Count == count
        && rule.LocationLabels == labels
        && rule.Role == placement::pd::Learner
        && rule.LabelConstraints.iter().any(|constraint| {
            constraint.Key == "engine"
                && constraint.Op == placement::pd::In
                && constraint.Values == ["tiflash"]
        })
}

/// Mock TiFlash 错误类型，携带可读消息。
#[derive(Debug)]
pub struct MockTiFlashError(pub String);
impl MockTiFlashError {
    /// 返回错误消息字符串。
    pub fn Error(&self) -> String {
        self.0.clone()
    }
}
impl std::fmt::Display for MockTiFlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for MockTiFlashError {}

/// 包装可选 `MockTiFlash` 的副本管理器，将 trait 调用转发给 Mock。
pub struct mockTiFlashReplicaManagerCtx {
    /// 可选的 Mock TiFlash 实例。
    tiflash: RwLock<Option<Arc<MockTiFlash>>>,
    /// 本地进度缓存。
    tiflashProgressCache: RwLock<HashMap<i64, f64>>,
}
impl Default for mockTiFlashReplicaManagerCtx {
    fn default() -> Self {
        Self {
            tiflash: RwLock::new(None),
            tiflashProgressCache: RwLock::new(HashMap::new()),
        }
    }
}
impl mockTiFlashReplicaManagerCtx {
    /// 注入 MockTiFlash 实例。
    pub fn SetMockTiFlash(&self, tiflash: Arc<MockTiFlash>) {
        *self.tiflash.write().unwrap() = Some(tiflash);
    }
    /// 取出当前 MockTiFlash（若有）。
    pub fn GetMockTiFlash(&self) -> Option<Arc<MockTiFlash>> {
        self.tiflash.read().unwrap().clone()
    }
}
impl TiFlashReplicaManager for mockTiFlashReplicaManagerCtx {
    fn SetTiFlashGroupConfig(&self) -> Result<()> {
        if let Some(t) = self.tiflash.read().unwrap().as_ref() {
            t.SetRuleGroupIndex(placement::RuleIndexTiFlash);
        }
        Ok(())
    }
    fn SetPlacementRule(&self, rule: &TiFlashRule) -> Result<()> {
        match self.tiflash.read().unwrap().as_ref() {
            Some(t) => t.HandleSetPlacementRule(rule),
            None => Ok(()),
        }
    }
    fn SetPlacementRuleBatch(&self, rules: &[TiFlashRule]) -> Result<()> {
        match self.tiflash.read().unwrap().as_ref() {
            Some(t) => t.HandleSetPlacementRuleBatch(rules),
            None => Ok(()),
        }
    }
    fn GetPlacementRule(&self, table_id: i64) -> Result<TiFlashRule> {
        self.tiflash
            .read()
            .unwrap()
            .as_ref()
            .and_then(|t| t.GetPlacementRule(&MakeRuleID(table_id)))
            .ok_or_else(|| Error::External("not implemented".into()))
    }
    fn DeletePlacementRule(&self, group: &str, rule_id: &str) -> Result<()> {
        if let Some(t) = self.tiflash.read().unwrap().as_ref() {
            t.HandleDeletePlacementRule(group, rule_id);
        }
        Ok(())
    }
    fn GetGroupRules(&self, group: &str) -> Result<Vec<TiFlashRule>> {
        Ok(self
            .tiflash
            .read()
            .unwrap()
            .as_ref()
            .map(|t| t.HandleGetGroupRules(group))
            .unwrap_or_default())
    }
    fn PostAccelerateScheduleBatch(&self, ids: &[i64]) -> Result<()> {
        if let Some(t) = self.tiflash.read().unwrap().as_ref() {
            for id in ids {
                t.HandlePostAccelerateSchedule(*id);
            }
        }
        Ok(())
    }
    fn GetRegionCountFromPD(&self, _id: i64) -> Result<usize> {
        Ok(if self.tiflash.read().unwrap().is_some() {
            1
        } else {
            0
        })
    }
    fn GetStoresStat(&self) -> Result<StoresInfo> {
        self.tiflash
            .read()
            .unwrap()
            .as_ref()
            .map(|t| t.HandleGetStoresStat())
            .ok_or_else(|| Error::External("MockTiFlash is not accessible".into()))
    }
    fn CalculateTiFlashProgress(
        &self,
        id: i64,
        count: u64,
        stores: &HashMap<i64, StoreInfo>,
    ) -> Result<(f64, f64)> {
        calculateTiFlashProgress(id, count, stores)
    }
    fn UpdateTiFlashProgressCache(&self, id: i64, value: f64) {
        self.tiflashProgressCache.write().unwrap().insert(id, value);
    }
    fn GetTiFlashProgressFromCache(&self, id: i64) -> Option<f64> {
        self.tiflashProgressCache.read().unwrap().get(&id).copied()
    }
    fn DeleteTiFlashProgressFromCache(&self, id: i64) {
        self.tiflashProgressCache.write().unwrap().remove(&id);
    }
    fn CleanTiFlashProgressCache(&self) {
        self.tiflashProgressCache.write().unwrap().clear();
    }
}
