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

// Placement Bundle：将 PlacementSettings 转为 PD 放置规则组。
//
// Bundle 对应 PD（Placement Driver，集群调度中心）的一组 Rule：
// 描述 Leader/Voter/Learner 等副本角色应落在哪些 label 约束（如 zone/region）上。
// 本模块支持约束语法与「PRIMARY_REGION/REGIONS」糖语法两种输入，
// 并提供 Tidy 合并、按表/分区 Reset 键范围、以及从放置策略构建 Bundle 的入口。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::codec;
use crate::common::*;
use crate::constraint::NewConstraintDirect;
use crate::constraints::{
    AddConstraint, ConstraintsFingerPrint, NewConstraintsDirect, NewConstraintsFromYaml,
};
use crate::errors::{
    ErrInvalidBundleID, ErrInvalidBundleIDFormat, ErrInvalidPlacementOptions,
    ErrInvalidSurvivalPreferenceFormat, Error, wrap,
};
use crate::model;
use crate::pd;
use crate::rule::{NewRule, NewRuleBuilder};
use crate::tablecodec;

/// PD Rule Group 的 Rust 表示：组 ID、优先级索引、是否覆盖及规则列表。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    #[serde(rename = "group_id", default, skip_serializing_if = "String::is_empty")]
    pub ID: String,
    #[serde(rename = "group_index")]
    pub Index: i32,
    #[serde(rename = "group_override")]
    pub Override: bool,
    #[serde(rename = "rules")]
    pub Rules: Vec<pd::Rule>,
}

/// 按对象 ID 创建空 Bundle（组 ID 形如 `TiDB_DDL_{id}`）。
pub fn NewBundle(id: i64) -> Bundle {
    Bundle {
        ID: GroupID(id),
        ..Default::default()
    }
}

/// 将 `Box<Rule>` 向量拆箱为拥有所有权的 `Rule` 向量。
fn boxed_rules(rules: Vec<Box<pd::Rule>>) -> Vec<pd::Rule> {
    rules.into_iter().map(|rule| *rule).collect()
}

/// 从显式约束类 PlacementSettings（LEADER/FOLLOWER/LEARNER_CONSTRAINTS）构建 Bundle。
pub fn NewBundleFromConstraintsOptions(
    options: Option<&model::PlacementSettings>,
) -> Result<Option<Bundle>, Error> {
    let options =
        options.ok_or_else(|| wrap(ErrInvalidPlacementOptions, "options can not be nil"))?;
    // 不可与 PRIMARY_REGION 等糖语法字段混用。
    if !options.PrimaryRegion.is_empty()
        || !options.Regions.is_empty()
        || !options.Schedule.is_empty()
    {
        return Err(wrap(
            ErrInvalidPlacementOptions,
            format!(
                "should be [LEADER/VOTER/LEARNER/FOLLOWER]_CONSTRAINTS=.. [VOTERS/FOLLOWERS/LEARNERS]=.., mixed other sugar options {}",
                options.String()
            ),
        ));
    }

    let mut rules = Vec::new();
    // Constraints 既可能是 YAML 数组，也可能是字典映射字符串。
    let common_constraints = match NewConstraintsFromYaml(options.Constraints.as_bytes()) {
        Ok(constraints) => constraints,
        Err(_) => {
            let mut builder = NewRuleBuilder();
            let normal_rules = builder
                .SetRole(pd::Voter)
                .SetConstraintStr(options.Constraints.clone())
                .BuildRulesWithDictConstraintsOnly()?;
            rules.extend(boxed_rules(normal_rules));
            Vec::new()
        }
    };
    let need_create_default = rules.is_empty();

    let mut leader_constraints = NewConstraintsFromYaml(options.LeaderConstraints.as_bytes())
        .map_err(|error| {
            wrap(
                &error.to_string(),
                "'LeaderConstraints' should be [constraint1, ...] or any yaml compatible array representation",
            )
        })?;
    for constraint in &common_constraints {
        AddConstraint(&mut leader_constraints, constraint.clone()).map_err(|error| {
            wrap(
                &error.to_string(),
                "LeaderConstraints conflicts with Constraints",
            )
        })?;
    }

    let mut leader_replicas = 1;
    let mut follower_replicas = if options.Followers > 0 {
        options.Followers
    } else {
        2
    };
    // 已有字典约束规则时，缺省角色副本数按「是否显式给出约束」决定。
    if !need_create_default {
        if options.LeaderConstraints.is_empty() {
            leader_replicas = 0;
        }
        if options.FollowerConstraints.is_empty() {
            if options.Followers > 0 {
                return Err(wrap(
                    ErrInvalidPlacementOptions,
                    "specify follower count without specify follower constraints when specify other constraints",
                ));
            }
            follower_replicas = 0;
        }
    }

    if leader_replicas > 0 {
        rules.push(*NewRule(pd::Leader, leader_replicas, leader_constraints));
    }

    if follower_replicas > 0 {
        let mut builder = NewRuleBuilder();
        let mut follower_rules = boxed_rules(
            builder
                .SetRole(pd::Voter)
                .SetReplicasNum(follower_replicas)
                .SetSkipCheckReplicasConsistent(need_create_default && options.Followers == 0)
                .SetConstraintStr(options.FollowerConstraints.clone())
                .BuildRules()
                .map_err(|error| wrap(&error.to_string(), "invalid FollowerConstraints"))?,
        );
        for rule in &mut follower_rules {
            for constraint in &common_constraints {
                AddConstraint(&mut rule.LabelConstraints, constraint.clone()).map_err(|error| {
                    wrap(
                        &error.to_string(),
                        "FollowerConstraints conflicts with Constraints",
                    )
                })?;
            }
        }
        rules.extend(follower_rules);
    }

    let mut builder = NewRuleBuilder();
    let mut learner_rules = boxed_rules(
        builder
            .SetRole(pd::Learner)
            .SetReplicasNum(options.Learners)
            .SetConstraintStr(options.LearnerConstraints.clone())
            .BuildRules()
            .map_err(|error| wrap(&error.to_string(), "invalid LearnerConstraints"))?,
    );
    for rule in &mut learner_rules {
        for constraint in &common_constraints {
            AddConstraint(&mut rule.LabelConstraints, constraint.clone()).map_err(|error| {
                wrap(
                    &error.to_string(),
                    "LearnerConstraints conflicts with Constraints",
                )
            })?;
        }
    }
    rules.extend(learner_rules);

    let labels = newLocationLabelsFromSurvivalPreferences(&options.SurvivalPreferences)?;
    for rule in &mut rules {
        rule.LocationLabels = labels.clone();
    }
    Ok(Some(Bundle {
        Rules: rules,
        ..Default::default()
    }))
}

/// 从糖语法 PlacementSettings（PRIMARY_REGION / REGIONS / SCHEDULE）构建 Bundle。
pub fn NewBundleFromSugarOptions(
    options: Option<&model::PlacementSettings>,
) -> Result<Option<Bundle>, Error> {
    let options =
        options.ok_or_else(|| wrap(ErrInvalidPlacementOptions, "options can not be nil"))?;
    if !options.LeaderConstraints.is_empty()
        || !options.LearnerConstraints.is_empty()
        || !options.FollowerConstraints.is_empty()
        || !options.Constraints.is_empty()
        || options.Learners > 0
    {
        return Err(wrap(
            ErrInvalidPlacementOptions,
            format!(
                "should be PRIMARY_REGION=.. REGIONS=.. FOLLOWERS=.. SCHEDULE=.., mixed other constraints into options {}",
                options.String()
            ),
        ));
    }

    let primary_region = options.PrimaryRegion.trim().to_owned();
    let mut regions = if options.Regions.trim().is_empty() {
        Vec::new()
    } else {
        options
            .Regions
            .trim()
            .split(',')
            .map(|region| region.trim().to_owned())
            .collect::<Vec<_>>()
    };
    let followers = if options.Followers == 0 {
        2
    } else {
        options.Followers
    };
    let location_labels = newLocationLabelsFromSurvivalPreferences(&options.SurvivalPreferences)?;
    let mut rules = Vec::new();

    // 未指定地域时生成无约束的 voter 规则（副本数 = followers + 1，含 leader）。
    if primary_region.is_empty() && regions.is_empty() {
        let mut rule = *NewRule(pd::Voter, followers + 1, NewConstraintsDirect(Vec::new()));
        rule.LocationLabels = location_labels;
        return Ok(Some(Bundle {
            Rules: vec![rule],
            ..Default::default()
        }));
    }

    regions.sort();
    let primary_index = regions.binary_search(&primary_region).map_err(|_| {
        wrap(
            ErrInvalidPlacementOptions,
            "primary region must be included in regions",
        )
    })?;
    // even：各 region 尽量均分；majority_in_primary：主 region 保证多数副本。
    let primary_count = match options.Schedule.to_lowercase().as_str() {
        "" | "even" => (followers + 1).div_ceil(regions.len() as u64),
        "majority_in_primary" => (followers + 1) / 2 + 1,
        schedule => {
            return Err(wrap(
                ErrInvalidPlacementOptions,
                format!("unsupported schedule {schedule}"),
            ));
        }
    };

    let primary_constraint = || {
        NewConstraintsDirect(vec![NewConstraintDirect(
            "region",
            pd::In,
            vec![primary_region.clone()],
        )])
    };
    rules.push(*NewRule(pd::Leader, 1, primary_constraint()));
    if primary_count > 1 {
        rules.push(*NewRule(pd::Voter, primary_count - 1, primary_constraint()));
    }
    let remaining = followers + 1 - primary_count;
    if remaining > 0 {
        regions.remove(primary_index);
        let constraints = if regions.is_empty() {
            Vec::new()
        } else {
            vec![NewConstraintDirect("region", pd::In, regions)]
        };
        rules.push(*NewRule(pd::Voter, remaining, constraints));
    }
    for rule in &mut rules {
        rule.LocationLabels = location_labels.clone();
    }
    Ok(Some(Bundle {
        Rules: rules,
        ..Default::default()
    }))
}

/// 按字段形态选择糖语法或约束语法路径构建 Bundle（未 Tidy）。
pub fn newBundleFromOptions(
    options: Option<&model::PlacementSettings>,
) -> Result<Option<Bundle>, Error> {
    let options =
        options.ok_or_else(|| wrap(ErrInvalidPlacementOptions, "options can not be nil"))?;
    if options.Followers > 8 {
        return Err(wrap(
            ErrInvalidPlacementOptions,
            format!(
                "followers should be less than or equal to 8: {}",
                options.Followers
            ),
        ));
    }
    let sugar = options.LeaderConstraints.is_empty()
        && options.LearnerConstraints.is_empty()
        && options.FollowerConstraints.is_empty()
        && options.Constraints.is_empty()
        && options.Learners == 0;
    if sugar {
        NewBundleFromSugarOptions(Some(options))
    } else {
        NewBundleFromConstraintsOptions(Some(options))
    }
}

/// 解析 SurvivalPreferences YAML 数组为 LocationLabels。
pub fn newLocationLabelsFromSurvivalPreferences(value: &str) -> Result<Vec<String>, Error> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    serde_yaml::from_str(value).map_err(|_| Error::new(ErrInvalidSurvivalPreferenceFormat))
}

/// 公开入口：从 PlacementSettings 构建并 Tidy 规范化 Bundle。
pub fn NewBundleFromOptions(
    options: Option<&model::PlacementSettings>,
) -> Result<Option<Bundle>, Error> {
    let mut bundle = match newBundleFromOptions(options)? {
        Some(bundle) => bundle,
        None => return Ok(None),
    };
    bundle.Tidy()?;
    Ok(Some(bundle))
}

impl Bundle {
    /// 序列化为 JSON 字符串（与 Go Bundle.String 对齐）。
    pub fn String(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// 丢弃 Count<=0 的规则，按约束指纹分组并合并同角色规则，必要时合并可转换的 Leader。
    pub fn Tidy(&mut self) -> Result<(), Error> {
        let mut useful = Vec::with_capacity(self.Rules.len());
        for mut rule in self.Rules.drain(..) {
            if rule.Count <= 0 {
                continue;
            }
            rule.ID = useful.len().to_string();
            useful.push(rule);
        }

        let mut groups: HashMap<String, ConstraintsGroup> = HashMap::new();
        for rule in useful {
            let key = ConstraintsFingerPrint(&rule.LabelConstraints);
            groups.entry(key).or_default().rules.push(rule);
        }
        for group in groups.values_mut() {
            group.MergeRulesByRole();
        }
        transformableLeaderConstraint(&mut groups)?;

        self.Rules = groups.into_values().flat_map(|group| group.rules).collect();
        self.Rules.sort_by(|left, right| left.ID.cmp(&right.ID));
        Ok(())
    }

    /// 将 Bundle 重建为全局/meta 等特殊键范围策略，并重写各 Rule 的起止 key。
    pub fn RebuildForRange(&mut self, range_name: &str, policy_name: &str) -> &mut Self {
        match range_name {
            KeyRangeGlobal => {
                self.ID = TiDBBundleRangePrefixForGlobal.to_owned();
                self.Index = RuleIndexKeyRangeForGlobal;
            }
            KeyRangeMeta => {
                self.ID = TiDBBundleRangePrefixForMeta.to_owned();
                self.Index = RuleIndexKeyRangeForMeta;
            }
            _ => {}
        }
        let (start_key, end_key) = GetRangeStartAndEndKeyHex(&self.ID);
        self.Override = true;
        for (index, rule) in self.Rules.iter_mut().enumerate() {
            rule.ID = format!("{}_rule_{index}", policy_name.to_lowercase());
            rule.GroupID = self.ID.clone();
            rule.StartKeyHex = start_key.clone();
            rule.EndKeyHex = end_key.clone();
            rule.Index = index as i32;
        }
        self
    }

    /// 按表/分区物理 ID 列表复制规则：首个 ID 为表级，其余为分区级，并编码 key 前缀。
    pub fn Reset(&mut self, rule_index: i32, new_ids: &[i64]) -> &mut Self {
        assert!(!new_ids.is_empty(), "new IDs must not be empty");
        let table_rules = self
            .Rules
            .iter()
            .filter(|rule| rule.Index == RuleIndexTable)
            .cloned()
            .collect::<Vec<_>>();
        // 若尚无表级规则，则用整组规则作为模板。
        let basic_rules = if table_rules.is_empty() {
            self.Rules.clone()
        } else {
            table_rules
        };

        self.ID = GroupID(new_ids[0]);
        self.Index = rule_index;
        self.Override = true;
        self.Rules.clear();
        self.Rules.reserve(basic_rules.len() * new_ids.len());
        for (id_index, new_id) in new_ids.iter().copied().enumerate() {
            let rule_id = if rule_index == RuleIndexPartition {
                format!("partition_rule_{new_id}")
            } else if id_index == 0 {
                format!("table_rule_{new_id}")
            } else {
                format!("partition_rule_{new_id}")
            };
            let start_key = encode_table_prefix(new_id);
            let end_key = encode_table_prefix(new_id + 1);
            for (rule_index_in_id, rule) in basic_rules.iter().enumerate() {
                let mut rule = rule.clone();
                rule.ID = format!("{rule_id}_{rule_index_in_id}");
                rule.GroupID = self.ID.clone();
                rule.StartKeyHex = start_key.clone();
                rule.EndKeyHex = end_key.clone();
                rule.Index = if id_index == 0 {
                    RuleIndexTable
                } else {
                    RuleIndexPartition
                };
                self.Rules.push(rule);
            }
        }
        self
    }

    /// 深拷贝 Bundle。
    pub fn Clone(&self) -> Self {
        Clone::clone(self)
    }

    /// 判断是否为空 Bundle（无规则且默认索引/覆盖标志）。
    pub fn IsEmpty(&self) -> bool {
        self.Rules.is_empty() && self.Index == 0 && !self.Override
    }

    /// 从 `TiDB_DDL_{id}` 形式的组 ID 解析对象 ID。
    pub fn ObjectID(&self) -> Result<i64, Error> {
        let value = self
            .ID
            .strip_prefix(BundleIDPrefix)
            .ok_or_else(|| Error::new(ErrInvalidBundleIDFormat))?;
        let id = value
            .parse::<i64>()
            .map_err(|error| wrap(ErrInvalidBundleID, error))?;
        if id <= 0 {
            return Err(wrap(
                ErrInvalidBundleID,
                format!("{} doesn't include an id", self.ID),
            ));
        }
        Ok(id)
    }

    /// 查找唯一 Leader 规则上指定 DC label 的取值，用于展示主可用区。
    pub fn GetLeaderDC(&self, dc_label_key: &str) -> (String, bool) {
        let value = self
            .Rules
            .iter()
            .find(|rule| isValidLeaderRule(rule, dc_label_key))
            .and_then(|rule| rule.LabelConstraints.first())
            .and_then(|constraint| constraint.Values.first())
            .cloned();
        match value {
            Some(value) => (value, true),
            None => (String::new(), false),
        }
    }
}

/// 相同 LabelConstraints 指纹下的规则分组，供 Tidy 合并。
#[derive(Default)]
struct ConstraintsGroup {
    rules: Vec<pd::Rule>,
    can_became_leader: bool,
    is_leader_group: bool,
}

impl ConstraintsGroup {
    /// 合并同 Role 规则的 Count，并标记组是否含 Leader。
    fn MergeRulesByRole(&mut self) {
        let mut merged: Vec<pd::Rule> = Vec::new();
        for rule in self.rules.drain(..) {
            if rule.Role == pd::Leader || rule.Role == pd::Voter {
                self.can_became_leader = true;
            }
            if rule.Role == pd::Leader {
                self.is_leader_group = true;
            }
            if let Some(existing) = merged.iter_mut().find(|item| item.Role == rule.Role) {
                existing.Count += rule.Count;
                if existing.ID > rule.ID {
                    existing.ID = rule.ID;
                }
            } else {
                merged.push(rule);
            }
        }
        self.rules = merged;
    }

    /// 将非 Learner 角色合并为一个 Voter（保留 Learner 单独列出）。
    fn MergeTransformableRoles(&mut self) {
        if self.rules.len() <= 1 {
            return;
        }
        let mut merged: Option<pd::Rule> = None;
        let mut learners = Vec::new();
        for rule in self.rules.drain(..) {
            if rule.Role == pd::Learner {
                learners.push(rule);
            } else if let Some(existing) = &mut merged {
                existing.Count += rule.Count;
                if existing.ID > rule.ID {
                    existing.ID = rule.ID;
                }
            } else {
                merged = Some(rule);
            }
        }
        if let Some(mut rule) = merged {
            rule.Role = pd::Voter;
            learners.push(rule);
        }
        self.rules = learners;
    }
}

/// 若全局仅一组可竞选 Leader，则对该 Leader 组执行可转换角色合并。
fn transformableLeaderConstraint(
    groups: &mut HashMap<String, ConstraintsGroup>,
) -> Result<(), Error> {
    let mut leader_key = None;
    let mut can_become_leader_count = 0;
    for (key, group) in groups.iter() {
        if group.is_leader_group {
            if leader_key.is_some() {
                return Err(Error::new(ErrInvalidPlacementOptions));
            }
            leader_key = Some(key.clone());
        }
        if group.can_became_leader {
            can_become_leader_count += 1;
        }
    }
    if can_become_leader_count == 1 {
        if let Some(key) = leader_key {
            groups
                .get_mut(&key)
                .expect("leader group came from this map")
                .MergeTransformableRoles();
        }
    }
    Ok(())
}

/// 将表物理 ID 编码为 hex 形式的表前缀起止键。
fn encode_table_prefix(table_id: i64) -> String {
    let prefix = tablecodec::GenTablePrefix(table_id);
    hex::encode(codec::EncodeBytes(Vec::new(), prefix.as_ref()))
}

/// 按 Bundle 范围 ID 返回起止 key 的 hex；目前仅 meta 范围有非空结果。
pub fn GetRangeStartAndEndKeyHex(range_bundle_id: &str) -> (String, String) {
    if range_bundle_id == TiDBBundleRangePrefixForMeta {
        (
            hex::encode(codec::EncodeBytes(Vec::new(), metaPrefix)),
            encode_table_prefix(0),
        )
    } else {
        (String::new(), String::new())
    }
}

/// 判断是否为「Count=1 且含指定 DC label 单值 In 约束」的合法 Leader 规则。
fn isValidLeaderRule(rule: &pd::Rule, dc_label_key: &str) -> bool {
    rule.Role == pd::Leader
        && rule.Count == 1
        && rule.LabelConstraints.iter().any(|constraint| {
            constraint.Op == pd::In
                && constraint.Key == dc_label_key
                && constraint.Values.len() == 1
        })
}

/// 按策略 ID 查询放置策略元信息。
pub trait PolicyGetter {
    fn GetPolicy(&self, policy_id: i64) -> Result<model::PolicyInfo, Error>;
}

/// 根据表级 PlacementPolicyRef 构建 Bundle，并为表及各分区 Reset 键范围。
pub fn NewTableBundle<G: PolicyGetter + ?Sized>(
    getter: &G,
    table_info: &model::TableInfo,
) -> Result<Option<Bundle>, Error> {
    let mut bundle = match newBundleFromPolicy(
        getter,
        table_info
            .PlacementPolicyRef
            .as_ref()
            .map(|policy| policy.ID),
    )? {
        Some(bundle) => bundle,
        None => return Ok(None),
    };
    let mut ids = vec![table_info.ID];
    if let Some(partition) = &table_info.Partition {
        ids.extend(partition.Definitions.iter().map(|definition| definition.ID));
    }
    bundle.Reset(RuleIndexTable, &ids);
    Ok(Some(bundle))
}

/// 根据单个分区定义上的 PlacementPolicyRef 构建分区级 Bundle。
pub fn NewPartitionBundle<G: PolicyGetter + ?Sized>(
    getter: &G,
    definition: &model::PartitionDefinition,
) -> Result<Option<Bundle>, Error> {
    let mut bundle = match newBundleFromPolicy(
        getter,
        definition
            .PlacementPolicyRef
            .as_ref()
            .map(|policy| policy.ID),
    )? {
        Some(bundle) => bundle,
        None => return Ok(None),
    };
    bundle.Reset(RuleIndexPartition, &[definition.ID]);
    Ok(Some(bundle))
}

/// 批量为分区列表构建 Bundle（跳过无策略的分区）。
pub fn NewPartitionListBundles<G: PolicyGetter + ?Sized>(
    getter: &G,
    definitions: &[model::PartitionDefinition],
) -> Result<Vec<Bundle>, Error> {
    let mut bundles = Vec::new();
    for definition in definitions {
        if let Some(bundle) = NewPartitionBundle(getter, definition)? {
            bundles.push(bundle);
        }
    }
    Ok(bundles)
}

/// 构建表级 Bundle 加上各分区独立 Bundle 的完整列表。
pub fn NewFullTableBundles<G: PolicyGetter + ?Sized>(
    getter: &G,
    table_info: &model::TableInfo,
) -> Result<Vec<Bundle>, Error> {
    let mut bundles = Vec::new();
    if let Some(bundle) = NewTableBundle(getter, table_info)? {
        bundles.push(bundle);
    }
    if let Some(partition) = &table_info.Partition {
        bundles.extend(NewPartitionListBundles(getter, &partition.Definitions)?);
    }
    Ok(bundles)
}

/// 通过 PolicyGetter 取策略并转为 Bundle；无 policy_id 时返回 None。
fn newBundleFromPolicy<G: PolicyGetter + ?Sized>(
    getter: &G,
    policy_id: Option<i64>,
) -> Result<Option<Bundle>, Error> {
    let Some(policy_id) = policy_id else {
        return Ok(None);
    };
    let policy = getter.GetPolicy(policy_id)?;
    NewBundleFromOptions(Some(&policy.PlacementSettings))
}
