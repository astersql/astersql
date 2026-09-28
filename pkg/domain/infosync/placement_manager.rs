// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// 放置策略（Placement）Bundle 管理。
//
// 定义 `PlacementManager` 抽象、PD HTTP 实现与内存 mock；
// 并校验 Bundle 内 Leader 规则的 key range 不得重叠。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{Error, PdHttpClient, Result, placement};

/// 放置规则 Bundle 的读写抽象。
pub trait PlacementManager: Send + Sync {
    /// 按名称获取单个 Bundle。
    fn GetRuleBundle(&self, name: &str) -> Result<placement::Bundle>;
    /// 获取全部 Bundle。
    fn GetAllRuleBundles(&self) -> Result<Vec<placement::Bundle>>;
    /// 批量写入 Bundle（空切片为 no-op）。
    fn PutRuleBundles(&self, bundles: &[placement::Bundle]) -> Result<()>;
}

/// 通过 PD HTTP API 管理放置规则 Bundle。
pub struct PDPlacementManager {
    /// PD HTTP 客户端。
    pub pdHTTPCli: Arc<dyn PdHttpClient>,
}
impl PlacementManager for PDPlacementManager {
    fn GetRuleBundle(&self, name: &str) -> Result<placement::Bundle> {
        let mut bundle = self.pdHTTPCli.get_placement_rule_bundle(name)?;
        // PD 响应可能不带回 ID，用请求名补齐。
        bundle.ID = name.to_owned();
        Ok(bundle)
    }
    fn GetAllRuleBundles(&self) -> Result<Vec<placement::Bundle>> {
        self.pdHTTPCli.get_all_placement_rule_bundles()
    }
    fn PutRuleBundles(&self, bundles: &[placement::Bundle]) -> Result<()> {
        if bundles.is_empty() {
            return Ok(());
        }
        self.pdHTTPCli.set_placement_rule_bundles(bundles, true)
    }
}

#[derive(Default)]
/// 内存 mock：用 HashMap 保存 Bundle，空 Bundle 表示删除。
pub struct mockPlacementManager {
    /// Bundle ID → Bundle 内容。
    bundles: Mutex<HashMap<String, placement::Bundle>>,
}
impl PlacementManager for mockPlacementManager {
    fn GetRuleBundle(&self, name: &str) -> Result<placement::Bundle> {
        Ok(self
            .bundles
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_else(|| placement::Bundle {
                ID: name.into(),
                ..Default::default()
            }))
    }
    fn GetAllRuleBundles(&self) -> Result<Vec<placement::Bundle>> {
        Ok(self.bundles.lock().unwrap().values().cloned().collect())
    }
    fn PutRuleBundles(&self, bundles: &[placement::Bundle]) -> Result<()> {
        let mut stored = self.bundles.lock().unwrap();
        for bundle in bundles {
            // 空 Bundle 表示删除该 ID 的规则集合。
            if bundle.IsEmpty() {
                stored.remove(&bundle.ID);
            } else {
                stored.insert(bundle.ID.clone(), bundle.clone());
            }
        }
        checkBundles(&stored)
    }
}

/// 校验单个 Bundle：Leader 角色规则的 key range 不得重叠。
pub fn CheckBundle(bundle: &placement::Bundle) -> Result<()> {
    let mut ranges = Vec::<(String, String)>::new();
    for rule in &bundle.Rules {
        if rule.Role == placement::pd::Leader {
            // Override 表示覆盖此前同组规则的 range 累积。
            if rule.Override {
                ranges.clear();
            }
            ranges.push((rule.StartKeyHex.clone(), rule.EndKeyHex.clone()));
        }
    }
    ranges.sort();
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(Error::External(format!(
                "ERROR 8243 (HY000): \"[PD:placement:ErrBuildRuleList]build rule list failed, multiple leader replicas for range {{{}, {}}}",
                pair[0].0, pair[1].1
            )));
        }
    }
    Ok(())
}

/// 对 map 中全部 Bundle 逐一调用 `CheckBundle`。
pub fn checkBundles(bundles: &HashMap<String, placement::Bundle>) -> Result<()> {
    for bundle in bundles.values() {
        CheckBundle(bundle)?;
    }
    Ok(())
}
