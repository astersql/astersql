// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// Region 标签规则（Label Rule）管理。
//
// 定义 `LabelRuleManager` 抽象、基于 PD HTTP 的 `PDLabelManager`，
// 以及内存 mock 实现；并按 Keyspace（键空间，多租户隔离单元）过滤规则。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::{LabelRulePatch, PdHttpClient, Result, label};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// TiKV Codec 上下文：是否按 keyspace 感知过滤标签规则。
pub struct Codec {
    /// 当前 Keyspace ID；`None` 时按 0 处理。
    pub keyspace_id: Option<u32>,
    /// 为 true 时仅返回属于本 keyspace 前缀的规则。
    pub keyspace_aware_rules: bool,
}

/// 标签规则增删改查抽象，供 InfoSyncer 注入 PD 或 mock 实现。
pub trait LabelRuleManager: Send + Sync {
    /// 写入单条标签规则；`None` 视为空操作。
    fn PutLabelRule(&self, rule: Option<&label::Rule>) -> Result<()>;
    /// 批量 patch（删除 + 设置）标签规则。
    fn UpdateLabelRules(&self, patch: Option<&LabelRulePatch>) -> Result<()>;
    /// 获取全部规则，并按 codec 做 keyspace 过滤。
    fn GetAllLabelRules(&self, codec: Codec) -> Result<Vec<label::Rule>>;
    /// 按规则 ID 列表批量查询。
    fn GetLabelRules(&self, rule_ids: &[String]) -> Result<HashMap<String, label::Rule>>;
}

/// 通过 PD HTTP API 操作 Region 标签规则的实现。
pub struct PDLabelManager {
    /// PD HTTP 客户端。
    pub pdHTTPCli: Arc<dyn PdHttpClient>,
}

impl LabelRuleManager for PDLabelManager {
    fn PutLabelRule(&self, rule: Option<&label::Rule>) -> Result<()> {
        match rule {
            Some(rule) => self.pdHTTPCli.set_region_label_rule(rule),
            None => Ok(()),
        }
    }
    fn UpdateLabelRules(&self, patch: Option<&LabelRulePatch>) -> Result<()> {
        match patch {
            Some(patch) => self.pdHTTPCli.patch_region_label_rules(patch),
            None => Ok(()),
        }
    }
    fn GetAllLabelRules(&self, codec: Codec) -> Result<Vec<label::Rule>> {
        Ok(filterRulesByKeyspace(
            self.pdHTTPCli.get_all_region_label_rules()?,
            codec,
        ))
    }
    fn GetLabelRules(&self, rule_ids: &[String]) -> Result<HashMap<String, label::Rule>> {
        Ok(self
            .pdHTTPCli
            .get_region_label_rules_by_ids(rule_ids)?
            .into_iter()
            .map(|rule| (rule.ID.clone(), rule))
            .collect())
    }
}

#[derive(Default)]
/// 内存 mock：以规则 ID → JSON 字节保存标签规则，供单测使用。
pub struct mockLabelManager {
    /// 规则存储：ID 映射到序列化后的 Rule JSON。
    labelRules: RwLock<HashMap<String, Vec<u8>>>,
}

impl LabelRuleManager for mockLabelManager {
    fn PutLabelRule(&self, rule: Option<&label::Rule>) -> Result<()> {
        let Some(rule) = rule else { return Ok(()) };
        self.labelRules
            .write()
            .unwrap()
            .insert(rule.ID.clone(), serde_json::to_vec(rule)?);
        Ok(())
    }
    fn UpdateLabelRules(&self, patch: Option<&LabelRulePatch>) -> Result<()> {
        let Some(patch) = patch else { return Ok(()) };
        let mut rules = self.labelRules.write().unwrap();
        // 先删后写，对齐 PD patch 语义。
        for id in &patch.DeleteRules {
            rules.remove(id);
        }
        for rule in &patch.SetRules {
            rules.insert(rule.ID.clone(), serde_json::to_vec(rule)?);
        }
        Ok(())
    }
    fn GetAllLabelRules(&self, codec: Codec) -> Result<Vec<label::Rule>> {
        let rules = self
            .labelRules
            .read()
            .unwrap()
            .values()
            .map(|value| serde_json::from_slice(value))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(filterRulesByKeyspace(rules, codec))
    }
    fn GetLabelRules(&self, rule_ids: &[String]) -> Result<HashMap<String, label::Rule>> {
        let rules = self.labelRules.read().unwrap();
        let mut result = HashMap::with_capacity(rule_ids.len());
        for id in rule_ids {
            if let Some(value) = rules.get(id) {
                result.insert(id.clone(), serde_json::from_slice(value)?);
            }
        }
        Ok(result)
    }
}

/// 按 Keyspace 前缀过滤规则；未开启 keyspace 感知时原样返回。
pub fn filterRulesByKeyspace(rules: Vec<label::Rule>, codec: Codec) -> Vec<label::Rule> {
    if !codec.keyspace_aware_rules {
        return rules;
    }
    // 规则 ID 形如 `keyspace/<id>/...`，仅保留当前 keyspace。
    let prefix = format!(
        "{}/{}/",
        label::KeyspacePrefix,
        codec.keyspace_id.unwrap_or_default()
    );
    rules
        .into_iter()
        .filter(|rule| rule.ID.starts_with(&prefix))
        .collect()
}
